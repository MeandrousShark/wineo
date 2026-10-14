use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::time::Duration;

use rustix::process::Signal;

use crate::cancel::Token;
use crate::config::Arch;
use crate::error::{Error, IoContext, Result};
use crate::installer::Output;
use crate::paths::{self, Paths};
use crate::process::{Ended, Escalation};
use crate::runner::Runner;

const CHILD_POLL: Duration = Duration::from_millis(25);
const UNCONFIRMED: &str = "the prefix processes could not be confirmed stopped";
const CANCEL_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum PrefixEvent {
    Started { dir: PathBuf },
    Finished,
}

pub fn is_prefix(dir: &Path) -> bool {
    dir.join("system.reg").is_file()
}

/// True when `dir` holds a wine prefix directly, or a Proton one in `pfx/`.
/// Used where the runner isn't known, such as removing a broken environment.
pub fn holds_prefix(dir: &Path) -> bool {
    wine_prefix_in(dir).is_some()
}

/// The wine prefix inside a configured prefix directory, worked out from the
/// layout rather than from the runner, so a verb that only reads still works
/// when the configured runner has gone missing.
pub fn wine_prefix_in(dir: &Path) -> Option<PathBuf> {
    let proton = dir.join("pfx");
    if is_prefix(dir) {
        Some(dir.to_owned())
    } else if is_prefix(&proton) {
        Some(proton)
    } else {
        None
    }
}

/// Reads the arch wine records in the registry header, e.g. `#arch=win64`.
pub fn detect_arch(dir: &Path) -> Result<Option<Arch>> {
    let path = dir.join("system.reg");
    let file = File::open(&path).context(|| format!("failed to read {}", path.display()))?;
    for line in BufReader::new(file).lines().take(10) {
        let line = line.context(|| format!("failed to read {}", path.display()))?;
        if let Some(arch) = line.strip_prefix("#arch=") {
            return Ok(arch.trim().parse().ok());
        }
    }
    Ok(None)
}

/// What a configured prefix dir needs before wine can use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPlan {
    /// It already holds a prefix of the runner's layout.
    Ready,
    /// It is missing or empty, and safe to build a prefix in.
    Build,
}

/// Decides whether a prefix can be built in `dir` without hiding a real one
/// that just isn't reachable, like one on an unmounted drive. Changes nothing
/// on disk. An empty dir outside `prefix_root` can't be told apart from a
/// mount point, so it is refused with `EmptyPrefix` unless `empty_confirmed`.
/// So is a missing dir outside `prefix_root` whose parent is empty.
pub fn build_plan(
    paths: &Paths,
    prefix_root: &Path,
    runner: &Runner,
    dir: &Path,
    empty_confirmed: bool,
) -> Result<BuildPlan> {
    if is_prefix(&runner.wine_prefix(dir)) {
        return Ok(BuildPlan::Ready);
    }
    if holds_prefix(dir) {
        let (found, needed) = if runner.is_proton() {
            ("Wine", "Proton")
        } else {
            ("Proton", "Wine")
        };
        return Err(Error::PrefixLayout {
            path: dir.to_owned(),
            runner: runner.name().to_owned(),
            found,
            needed,
        });
    }
    paths.check_prefix(dir)?;

    let under_root = paths::resolve(dir)
        .strip_prefix(paths::resolve(prefix_root))
        .is_ok_and(|rest| !rest.as_os_str().is_empty());
    match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(Error::NotAPrefix(dir.to_owned()));
            }
            if !under_root && !empty_confirmed {
                return Err(Error::EmptyPrefix(dir.to_owned()));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // A dangling symlink points at something unreachable, and
            // building through it would hide whatever was there.
            if dir.symlink_metadata().is_ok() {
                return Err(Error::MissingPrefix(dir.to_owned()));
            }
            if let Some(parent) = dir.parent()
                && !under_root
            {
                if !parent.exists() {
                    return Err(Error::PrefixParentMissing {
                        prefix: dir.to_owned(),
                        parent: parent.to_owned(),
                    });
                }
                if !empty_confirmed && is_empty_dir(parent) {
                    return Err(Error::EmptyPrefixParent {
                        prefix: dir.to_owned(),
                        parent: parent.to_owned(),
                    });
                }
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotADirectory => {
            return Err(Error::NotAPrefix(dir.to_owned()));
        }
        Err(err) => return Err(err).context(|| format!("failed to read {}", dir.display())),
    }
    Ok(BuildPlan::Build)
}

fn is_empty_dir(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

/// Makes sure `dir` can hold a new prefix. Returns true if this call created it.
pub fn prepare_dir(dir: &Path) -> Result<bool> {
    match fs::read_dir(dir) {
        Ok(mut entries) => match entries.next() {
            Some(_) => Err(Error::NotAPrefix(dir.to_owned())),
            None => Ok(false),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent)
                    .context(|| format!("failed to create {}", parent.display()))?;
            }
            match fs::create_dir(dir) {
                Ok(()) => Ok(true),
                // Created by someone else in the meantime, so never ours to clean up.
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => prepare_dir(dir),
                Err(err) => Err(err).context(|| format!("failed to create {}", dir.display())),
            }
        }
        Err(err) => Err(err).context(|| format!("failed to read {}", dir.display())),
    }
}

/// Initializes a prefix, allowing a caller to cancel and follow the child.
pub fn create(
    runner: &Runner,
    dir: &Path,
    arch: Arch,
    log: &Path,
    token: &Token,
    output: Output<'_>,
    mut events: impl FnMut(PrefixEvent),
) -> Result<()> {
    if token.is_canceled() {
        return Err(Error::Canceled("prefix creation"));
    }
    let error_log = match output {
        Output::Inherit => log,
        Output::Log(path) => path,
    };
    let mut started = false;
    let result = (|| {
        if let Output::Inherit = output {
            File::create(log).context(|| format!("failed to create {}", log.display()))?;
        }
        // umu creates the prefix when it is handed an empty program, and waits for
        // it. Proton prefixes are always win64, so WINEARCH means nothing there.
        let mut command = runner.command(dir, &[])?;
        if runner.is_proton() {
            command.arg("");
        } else {
            command
                .env("WINEARCH", arch.as_str())
                .args(["wineboot", "--init"]);
        }
        command.stdin(Stdio::null()).process_group(0);
        if let Output::Log(path) = output {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .context(|| format!("failed to create {}", parent.display()))?;
            }
            let file =
                File::create(path).context(|| format!("failed to create {}", path.display()))?;
            let stderr = file
                .try_clone()
                .context(|| format!("failed to open {}", path.display()))?;
            command.stdout(file).stderr(stderr);
        }
        events(PrefixEvent::Started {
            dir: dir.to_owned(),
        });
        started = true;
        let child = command
            .spawn()
            .context(|| "failed to run wineboot".to_owned())?;
        crate::process::quiet_core_dumps(&child);
        let status = wait_for_child(child, token, || Ok(()))?;
        if token.is_canceled() {
            return Err(Error::Canceled("prefix creation"));
        }

        // A missing or failed prefix would make another wine command create a
        // fresh one, so only write the override once wineboot has initialized it.
        // Its own error is folded into the check below since wine is still
        // running either way and must be allowed to shut down. umu makes the
        // prefix and then fails to run the empty program, so its exit code says
        // nothing and the prefix itself is checked instead.
        let initialized = runner.is_proton() || status.success();
        let menu_builder_disabled = if initialized {
            match disable_menu_builder(runner, dir, log, token, output) {
                Ok(()) => true,
                Err(err @ Error::Canceled(_)) => return Err(err),
                Err(_) => false,
            }
        } else {
            false
        };

        // Wine only writes the registry files once wineserver shuts down.
        runner.wait_idle_cancellable(dir, token)?;
        if token.is_canceled() {
            return Err(Error::Canceled("prefix creation"));
        }

        if menu_builder_disabled && is_prefix(&runner.wine_prefix(dir)) {
            Ok(())
        } else {
            Err(Error::PrefixCreation {
                reason: last_wine_message(error_log),
                log: error_log.to_owned(),
            })
        }
    })();
    if started {
        // Best effort, and before the caller cleans up: wineserver would refill
        // the directory otherwise.
        if matches!(result, Err(Error::Canceled(_))) {
            let _ = runner.stop_server(dir, token);
        }
        events(PrefixEvent::Finished);
    }
    result
}

/// Waits for `child`, which leads its own group. A cancel asks the group to
/// stop and then runs `on_cancel`, which makes a waiter's target stop too.
pub(crate) fn wait_for_child(
    mut child: Child,
    token: &Token,
    mut on_cancel: impl FnMut() -> Result<()>,
) -> Result<ExitStatus> {
    let mut escalation = Escalation::new(CANCEL_GRACE, None);
    let mut canceled = false;
    let stopped =
        crate::process::stop_group(&child, Signal::TERM, &mut escalation, token, None, |step| {
            if step.is_none() || std::mem::replace(&mut canceled, true) {
                return Ok(());
            }
            on_cancel()
        })
        .context(|| "failed to stop the prefix process group".to_owned());
    let failed = match stopped {
        Ok(Ok(ended)) => Ok(ended),
        Ok(Err((err, ended))) => Err((err, ended == Ended::Stopped)),
        Err(err) => Err((err, false)),
    };
    let ended = match failed {
        Ok(ended) => ended,
        Err((err, settled)) => {
            let reaped = crate::process::kill_and_reap(&mut child);
            return Err(crate::process::failed_stop(
                err,
                settled && reaped,
                UNCONFIRMED,
            ));
        }
    };
    let group = child.id();
    let status = child
        .wait()
        .context(|| "failed to wait for wineboot".to_owned())?;
    match ended {
        Ended::Exited => Ok(status),
        Ended::Stopped => Err(Error::Canceled("prefix creation")),
        // Without `/proc`, wait for the group number itself to go, which needs
        // none, as this always did.
        Ended::Unconfirmed => loop {
            match crate::process::group_exists(group) {
                Some(false) => return Err(Error::Canceled("prefix creation")),
                Some(true) => token.wait_timeout(CHILD_POLL),
                None => {
                    return Err(Error::Unsettled(
                        "the canceled prefix process group could not be inspected".to_owned(),
                    ));
                }
            }
        },
    }
}

/// Writes `HKCU\Software\Wine\DllOverrides\winemenubuilder.exe=""`, so wine
/// run outside wineo doesn't build menu entries in this prefix either.
/// Shared by a freshly created prefix and one `new` adopts as is.
pub fn disable_menu_builder(
    runner: &Runner,
    dir: &Path,
    log: &Path,
    token: &Token,
    output: Output<'_>,
) -> Result<()> {
    if let Output::Inherit = output {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .context(|| format!("failed to open {}", log.display()))?;
    }
    let mut command = runner.command(dir, &[])?;
    command.args([
        "reg",
        "add",
        r"HKCU\Software\Wine\DllOverrides",
        "/v",
        "winemenubuilder.exe",
        "/t",
        "REG_SZ",
        "/d",
        "",
        "/f",
    ]);
    command.stdin(Stdio::null()).process_group(0);
    if let Output::Log(path) = output {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .context(|| format!("failed to create {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context(|| format!("failed to open {}", path.display()))?;
        let stderr = file
            .try_clone()
            .context(|| format!("failed to open {}", path.display()))?;
        command.stdout(file).stderr(stderr);
    }
    let child = command
        .spawn()
        .context(|| "failed to run wine reg add".to_owned())?;
    crate::process::quiet_core_dumps(&child);
    let status = wait_for_child(child, token, || runner.kill(dir, None).map(|_| ()))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::MenuBuilderOverride {
            log: log.to_owned(),
        })
    }
}

/// Fatal loader errors are the lines wine prefixes with `wine: `, as opposed to
/// the thread-tagged fixme and err noise.
fn last_wine_message(log: &Path) -> Option<String> {
    let text = fs::read_to_string(log).ok()?;
    text.lines()
        .rev()
        .find_map(|line| line.strip_prefix("wine: "))
        .map(|message| message.trim().trim_end_matches('.').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_arch_from_registry_header() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_prefix(dir.path()));
        fs::write(
            dir.path().join("system.reg"),
            "WINE REGISTRY Version 2\n;; All keys relative to REGISTRY\\\\Machine\n\n#arch=win64\n",
        )
        .unwrap();
        assert!(is_prefix(dir.path()));
        assert_eq!(detect_arch(dir.path()).unwrap(), Some(Arch::Win64));

        fs::write(dir.path().join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        assert_eq!(detect_arch(dir.path()).unwrap(), None);
    }

    #[test]
    fn prepare_dir_creates_missing_and_accepts_empty() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("a/b");
        assert!(prepare_dir(&missing).unwrap());
        assert!(missing.is_dir());
        assert!(!prepare_dir(&missing).unwrap());
    }

    #[test]
    fn prepare_dir_refuses_non_empty_dirs_and_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file"), "").unwrap();
        assert!(matches!(
            prepare_dir(root.path()),
            Err(Error::NotAPrefix(_))
        ));
        assert!(prepare_dir(&root.path().join("file")).is_err());
    }

    struct Layout {
        _temp: tempfile::TempDir,
        paths: Paths,
        root: PathBuf,
        outside: PathBuf,
    }

    fn layout() -> Layout {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let paths = Paths::from_lookup(|key| match key {
            "HOME" => Some(home.clone().into()),
            _ => None,
        })
        .unwrap();
        let outside = temp.path().join("mnt");
        fs::create_dir_all(&outside).unwrap();
        Layout {
            root: paths.default_prefix_root(),
            _temp: temp,
            paths,
            outside,
        }
    }

    fn plan(layout: &Layout, runner: &Runner, dir: &Path, confirmed: bool) -> Result<BuildPlan> {
        build_plan(&layout.paths, &layout.root, runner, dir, confirmed)
    }

    fn fake_prefix(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    }

    #[test]
    fn build_plan_finds_a_prefix_of_the_runners_layout_ready() {
        let l = layout();
        let wine = l.outside.join("wine");
        fake_prefix(&wine);
        let proton = l.outside.join("proton");
        fake_prefix(&proton.join("pfx"));
        for confirmed in [false, true] {
            assert_eq!(
                plan(&l, &Runner::fake("wine"), &wine, confirmed).unwrap(),
                BuildPlan::Ready
            );
            assert_eq!(
                plan(
                    &l,
                    &Runner::fake_proton("GE-Proton10-4"),
                    &proton,
                    confirmed
                )
                .unwrap(),
                BuildPlan::Ready
            );
        }
    }

    #[test]
    fn build_plan_refuses_a_prefix_of_the_other_layout() {
        let l = layout();
        let wine = l.root.join("wine");
        fake_prefix(&wine);
        let proton = l.root.join("proton");
        fake_prefix(&proton.join("pfx"));

        let err = plan(&l, &Runner::fake_proton("GE-Proton10-4"), &wine, true).unwrap_err();
        assert!(matches!(err, Error::PrefixLayout { .. }), "{err}");
        assert_eq!(
            err.to_string(),
            format!(
                "{} holds a Wine prefix, but runner GE-Proton10-4 needs a Proton one",
                wine.display()
            )
        );
        let err = plan(&l, &Runner::fake("wine"), &proton, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{} holds a Proton prefix, but runner wine needs a Wine one",
                proton.display()
            )
        );
    }

    #[test]
    fn build_plan_builds_a_missing_dir_whose_parent_holds_something() {
        let l = layout();
        fs::create_dir_all(l.outside.join("other")).unwrap();
        let dir = l.outside.join("palworld");
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
            BuildPlan::Build
        );
        assert!(!dir.exists());
    }

    #[test]
    fn build_plan_needs_confirmation_for_an_empty_parent_outside_the_prefix_root() {
        let l = layout();
        let games = l.outside.join("games");
        fs::create_dir_all(&games).unwrap();
        let dir = games.join("palworld");
        let err = plan(&l, &Runner::fake("wine"), &dir, false).unwrap_err();
        assert!(matches!(err, Error::EmptyPrefixParent { .. }), "{err}");
        assert_eq!(
            err.to_string(),
            format!(
                "prefix {} is missing, and {} is empty",
                dir.display(),
                games.display()
            )
        );
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &dir, true).unwrap(),
            BuildPlan::Build
        );
    }

    #[test]
    fn build_plan_builds_under_an_empty_prefix_root() {
        let l = layout();
        fs::create_dir_all(&l.root).unwrap();
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &l.root.join("palworld"), false).unwrap(),
            BuildPlan::Build
        );
    }

    #[test]
    fn build_plan_builds_under_a_missing_prefix_root() {
        let l = layout();
        assert!(!l.root.exists());
        for dir in [l.root.join("palworld"), l.root.join("games/palworld")] {
            assert_eq!(
                plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
                BuildPlan::Build
            );
        }
        assert!(!l.root.exists());
    }

    #[test]
    fn build_plan_refuses_a_missing_parent_outside_the_prefix_root() {
        let l = layout();
        let dir = l.outside.join("games/prefixes/palworld");
        let err = plan(&l, &Runner::fake("wine"), &dir, true).unwrap_err();
        assert!(matches!(err, Error::PrefixParentMissing { .. }), "{err}");
        assert_eq!(
            err.to_string(),
            format!(
                "prefix {} is missing, and so is {}",
                dir.display(),
                l.outside.join("games/prefixes").display()
            )
        );
    }

    #[test]
    fn build_plan_builds_in_an_empty_dir_under_the_prefix_root() {
        let l = layout();
        let dir = l.root.join("palworld");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
            BuildPlan::Build
        );
    }

    #[test]
    fn build_plan_needs_confirmation_for_an_empty_dir_outside_the_prefix_root() {
        let l = layout();
        let dir = l.outside.join("game-ssd");
        fs::create_dir_all(&dir).unwrap();
        let err = plan(&l, &Runner::fake("wine"), &dir, false).unwrap_err();
        assert!(matches!(err, Error::EmptyPrefix(_)), "{err}");
        assert_eq!(
            err.to_string(),
            format!("prefix {} is empty", dir.display())
        );
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &dir, true).unwrap(),
            BuildPlan::Build
        );

        // The root itself isn't under the root.
        fs::create_dir_all(&l.root).unwrap();
        assert!(matches!(
            plan(&l, &Runner::fake("wine"), &l.root, false),
            Err(Error::EmptyPrefix(_))
        ));
    }

    #[test]
    fn build_plan_compares_the_prefix_root_resolved() {
        let l = layout();
        let real = l.outside.join("real-root");
        fs::create_dir_all(real.join("palworld")).unwrap();
        fs::create_dir_all(l.root.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &l.root).unwrap();
        for dir in [real.join("palworld"), l.root.join("palworld")] {
            assert_eq!(
                plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
                BuildPlan::Build
            );
        }
    }

    #[test]
    fn build_plan_refuses_non_empty_dirs_and_files() {
        let l = layout();
        let dir = l.root.join("stuff");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), "keep me").unwrap();
        let file = l.root.join("file");
        fs::write(&file, "").unwrap();
        for path in [&dir, &file] {
            assert!(
                matches!(
                    plan(&l, &Runner::fake("wine"), path, true),
                    Err(Error::NotAPrefix(_))
                ),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn build_plan_refuses_a_dangling_symlink() {
        let l = layout();
        fs::create_dir_all(&l.root).unwrap();
        let dir = l.root.join("palworld");
        std::os::unix::fs::symlink(l.outside.join("unmounted/palworld"), &dir).unwrap();
        assert!(matches!(
            plan(&l, &Runner::fake("wine"), &dir, true),
            Err(Error::MissingPrefix(_))
        ));
    }

    #[test]
    fn build_plan_keeps_portable_prefixes_direct_children() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let folder = temp.path().join("usb");
        let paths = Paths::select(
            |key| match key {
                "HOME" => Some(home.clone().into()),
                _ => None,
            },
            Some(&folder),
        )
        .unwrap();
        let root = paths.default_prefix_root();
        let runner = Runner::fake("wine");
        assert_eq!(
            build_plan(&paths, &root, &runner, &root.join("palworld"), false).unwrap(),
            BuildPlan::Build
        );
        assert!(matches!(
            build_plan(&paths, &root, &runner, &root.join("games/palworld"), false),
            Err(Error::PrefixOutsidePortable(_))
        ));
    }

    #[test]
    fn finds_last_loader_message() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("create.log");
        fs::write(
            &log,
            "0054:fixme:ole:thing\nwine: first\n0060:err:x:y\nwine: WINEARCH is set to 'win32' but this is not supported in wow64 mode.\n",
        )
        .unwrap();
        assert_eq!(
            last_wine_message(&log).as_deref(),
            Some("WINEARCH is set to 'win32' but this is not supported in wow64 mode")
        );
        fs::write(&log, "0054:fixme:ole:thing\n").unwrap();
        assert_eq!(last_wine_message(&log), None);
    }
}
