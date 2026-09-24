//! Desktop entries wineo writes for targets that ask for one.
//!
//! An entry runs `wineo start`, never wine directly, so a launch from the
//! desktop's menu carries everything the environment does: gamemode, DXVK, and
//! umu all apply exactly as they do from a terminal. That is also why the
//! `Exec` line needs no quoting, since environment and target names are limited
//! to letters, digits, `-`, and `_`.
//!
//! Entries are a reconciled setting rather than something a verb creates, so a
//! hand-edited config needs no command afterwards. The `X-Wineo-*` keys below
//! are how an entry is recognized as wineo's, which means no separate manifest
//! and means an entry whose target has gone can still be found and removed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::{GlobalConfig, Target};
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::launch;
use crate::paths::Paths;
use crate::programs;
use crate::runner::Runner;
use crate::winpath;

const ENVIRONMENT_KEY: &str = "X-Wineo-Environment";
const TARGET_KEY: &str = "X-Wineo-Target";
/// Prefixed so wineo's entries never collide with anything else installed, and
/// so a stray one is recognizable before its contents are read.
const FILE_PREFIX: &str = "wineo-";

/// What reconciliation did, for the caller to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Wrote {
        target: String,
        icon: bool,
    },
    Updated {
        target: String,
    },
    Removed {
        target: String,
    },
    /// A target asked for an entry that cannot be made. Reported rather than
    /// returned as an error, because one broken target must not stop a launch
    /// of a different one. `wineo set` refuses it up front instead.
    Skipped {
        target: String,
        reason: String,
    },
}

/// An entry on disk that wineo wrote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub environment: String,
    pub target: String,
    pub name: String,
    pub path: PathBuf,
}

/// Brings the entries for `environment` in line with its config. Returns what
/// changed, and nothing at all when the entries already match.
///
/// `runner` is only needed to pull an icon out of the prefix, so a missing one
/// costs the icon rather than the entry.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: &Path,
) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    let mut wanted = BTreeSet::new();

    for (target_name, target) in &environment.config.targets {
        if !target.desktop {
            continue;
        }
        wanted.insert(target_name.clone());
        match write(
            paths,
            global,
            environment,
            runner,
            prefix,
            target_name,
            target,
        ) {
            Ok(Some(change)) => changes.push(change),
            Ok(None) => {}
            Err(reason) => changes.push(Change::Skipped {
                target: target_name.clone(),
                reason: reason.to_string(),
            }),
        }
    }

    // Anything left over is a target that stopped asking for an entry, or one
    // that was removed from the config entirely.
    for entry in owned_by(paths, &environment.name)? {
        if !wanted.contains(&entry.target) {
            remove_file(&entry.path)?;
            remove_icons(paths, &environment.name, &entry.target)?;
            changes.push(Change::Removed {
                target: entry.target,
            });
        }
    }
    Ok(changes)
}

/// Removes every entry belonging to `environment`, for `wineo rm`. Returns the
/// targets whose entries went.
pub fn remove_all(paths: &Paths, environment: &str) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for entry in owned_by(paths, environment)? {
        remove_file(&entry.path)?;
        remove_icons(paths, environment, &entry.target)?;
        removed.push(entry.target);
    }
    Ok(removed)
}

/// Every entry wineo has written, whatever environment it belongs to.
pub fn list(paths: &Paths) -> Result<Vec<Entry>> {
    let dir = paths.applications_dir();
    let mut entries = Vec::new();
    for found in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = found.path();
        let name = found.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".desktop") {
            continue;
        }
        if let Some(entry) = read_entry(&path) {
            entries.push(entry);
        }
    }
    entries.sort();
    Ok(entries)
}

fn owned_by(paths: &Paths, environment: &str) -> Result<Vec<Entry>> {
    Ok(list(paths)?
        .into_iter()
        .filter(|entry| entry.environment == environment)
        .collect())
}

/// Parses back the keys wineo wrote. Anything without them is not wineo's,
/// whatever it is called.
fn read_entry(path: &Path) -> Option<Entry> {
    let text = fs::read_to_string(path).ok()?;
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::to_owned)
    };
    Some(Entry {
        environment: value(ENVIRONMENT_KEY)?,
        target: value(TARGET_KEY)?,
        name: value("Name").unwrap_or_default(),
        path: path.to_owned(),
    })
}

fn write(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: &Path,
    target_name: &str,
    target: &Target,
) -> Result<Option<Change>> {
    check(paths, prefix, &target.exe)?;

    let path = entry_path(paths, &environment.name, target_name);
    let existing = fs::read_to_string(&path).ok();
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target_name.to_owned());
    let gaming = runner.is_some_and(Runner::is_proton)
        || launch::gamemode(global, environment, Some(target));

    let rendered = |icon: bool| {
        render(&Desired {
            environment: &environment.name,
            target: target_name,
            name: &name,
            exe: &target.exe,
            gaming,
            icon,
        })
    };

    // An icon that is already there is what makes a reconcile on every launch
    // cheap: the text below then matches and nothing runs.
    let had_icon = has_icon(paths, &environment.name, target_name);
    if existing.as_deref() == Some(rendered(had_icon).as_str()) {
        return Ok(None);
    }

    // Only attempted when the entry is being written anyway, so a target with
    // no icon to find does not pay for wine starting up on every launch.
    let icon = had_icon
        || runner.is_some_and(|runner| {
            extract_icon(
                paths,
                runner,
                prefix,
                &environment.name,
                target_name,
                &target.exe,
            )
            .is_some()
        });
    write_atomic(&path, &rendered(icon))?;
    Ok(Some(match existing {
        Some(_) => Change::Updated {
            target: target_name.to_owned(),
        },
        None => Change::Wrote {
            target: target_name.to_owned(),
            icon,
        },
    }))
}

struct Desired<'a> {
    environment: &'a str,
    target: &'a str,
    name: &'a str,
    exe: &'a str,
    gaming: bool,
    icon: bool,
}

fn render(desired: &Desired) -> String {
    let Desired {
        environment,
        target,
        name,
        exe,
        gaming,
        icon,
    } = desired;
    // `Software` is not a registered XDG category, so a registered one leads
    // and keeps the entry out of a catch-all menu group.
    let categories = if *gaming {
        "Game;"
    } else {
        "Utility;Software;"
    };
    let mut text = String::from("[Desktop Entry]\n");
    text.push_str("Type=Application\n");
    text.push_str(&format!("Name={}\n", clean(name)));
    text.push_str(&format!(
        "Comment=Runs in the wineo environment {environment}\n"
    ));
    text.push_str(&format!(
        "Exec=wineo start {environment} --target {target}\n"
    ));
    if *icon {
        text.push_str(&format!("Icon={}\n", icon_name(environment, target)));
    }
    text.push_str("Terminal=false\n");
    text.push_str(&format!("Categories={categories}\n"));
    if let Some(class) = wm_class(exe) {
        // Without this the launcher shows a second, generic icon for the
        // running window instead of matching it to this entry.
        text.push_str(&format!("StartupWMClass={class}\n"));
    }
    text.push_str(&format!("{ENVIRONMENT_KEY}={environment}\n"));
    text.push_str(&format!("{TARGET_KEY}={target}\n"));
    text
}

/// A desktop entry value is a single line, so anything that would break out of
/// one goes. Names come from shortcuts, which can hold whatever an installer put there.
fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|char| !char.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// What wine reports as the window class, which is the executable's file name.
fn wm_class(exe: &str) -> Option<String> {
    let file = exe.rsplit(['\\', '/']).next()?.trim();
    if file.is_empty() {
        return None;
    }
    Some(if file.contains('.') {
        file.to_owned()
    } else {
        // A bare name like `notepad` is run as `notepad.exe`.
        format!("{file}.exe")
    })
}

fn entry_path(paths: &Paths, environment: &str, target: &str) -> PathBuf {
    paths
        .applications_dir()
        .join(format!("{FILE_PREFIX}{environment}-{target}.desktop"))
}

/// Named after the environment and target rather than the executable, so the
/// entry's `Icon` line is known before the icon has been pulled out.
fn icon_name(environment: &str, target: &str) -> String {
    format!("{FILE_PREFIX}{environment}-{target}")
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
    }
}

/// The icon theme directories wineo may have put an icon in. Which sizes exist
/// depends on what the executable carried, so they are discovered, not assumed.
fn icon_files(paths: &Paths, environment: &str, target: &str) -> Vec<PathBuf> {
    let hicolor = paths.icons_dir().join("hicolor");
    let name = format!("{}.png", icon_name(environment, target));
    let mut found = Vec::new();
    for size in fs::read_dir(&hicolor).into_iter().flatten().flatten() {
        let candidate = size.path().join("apps").join(&name);
        if candidate.is_file() {
            found.push(candidate);
        }
    }
    found.sort();
    found
}

fn has_icon(paths: &Paths, environment: &str, target: &str) -> bool {
    !icon_files(paths, environment, target).is_empty()
}

fn remove_icons(paths: &Paths, environment: &str, target: &str) -> Result<()> {
    for path in icon_files(paths, environment, target) {
        remove_file(&path)?;
    }
    Ok(())
}

/// Refuses an executable that is not really there, so a typo never becomes an
/// entry that fails the moment it is clicked. A bare name like `notepad` is
/// left alone, since wine is the one that looks those up. `prefix` is the
/// configured directory, and the wine prefix inside it is worked out from the
/// layout rather than the runner, which may be gone.
pub fn check(paths: &Paths, prefix: &Path, exe: &str) -> Result<()> {
    let wine_prefix = crate::prefix::wine_prefix_in(prefix).unwrap_or_else(|| prefix.to_owned());
    let found = if winpath::is_windows_path(exe) {
        winpath::to_unix(&wine_prefix, exe).is_some_and(|unix| unix.is_file())
    } else if exe.contains('/') {
        paths.config_path(exe).is_ok_and(|unix| unix.is_file())
    } else {
        true
    };
    if found {
        Ok(())
    } else {
        Err(Error::ExeNotFound(exe.to_owned()))
    }
}

/// Pulls the executable's icon out with wine's own menu builder, the one 0001
/// otherwise keeps switched off.
///
/// Wine only processes a `.lnk` that sits in a Start Menu or on a Desktop, and
/// writes both an icon theme and a `.desktop` of its own. So this finds the
/// shortcut already pointing at the executable, points `XDG_DATA_HOME` at a
/// temporary directory, and keeps only the icons: wineo writes its own entry,
/// and wine's never reaches the real applications directory.
fn extract_icon(
    paths: &Paths,
    runner: &Runner,
    prefix: &Path,
    environment: &str,
    target: &str,
    exe: &str,
) -> Option<()> {
    let wine_prefix = runner.wine_prefix(prefix);
    let shortcut = shortcut_for(&wine_prefix, exe)?;
    let scratch = Scratch::new(paths).ok()?;

    let status = runner
        .command(prefix, None)
        .ok()?
        .args(["winemenubuilder.exe", "-a", "-r", &shortcut])
        // Set outright rather than merged: this command exists only to run the
        // menu builder, so an inherited value that disables it has nothing to say.
        .env("WINEDLLOVERRIDES", "winemenubuilder.exe=b")
        .env("XDG_DATA_HOME", scratch.path())
        .env("XDG_CONFIG_HOME", scratch.path().join("config"))
        .env("WINEDEBUG", "-all")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }

    // Wine picks the icon's file name itself, from a hash it does not report,
    // so it is read back out of the entry wine wrote beside it.
    let written = wine_icon_name(&scratch.path().join("applications"))?;
    let mut copied = 0;
    let hicolor = scratch.path().join("icons/hicolor");
    for size in fs::read_dir(&hicolor).into_iter().flatten().flatten() {
        let from = size.path().join("apps").join(format!("{written}.png"));
        if !from.is_file() {
            continue;
        }
        let to = paths
            .icons_dir()
            .join("hicolor")
            .join(size.file_name())
            .join("apps")
            .join(format!("{}.png", icon_name(environment, target)));
        if to
            .parent()
            .is_some_and(|dir| fs::create_dir_all(dir).is_err())
        {
            continue;
        }
        if fs::copy(&from, &to).is_ok() {
            copied += 1;
        }
    }
    (copied > 0).then_some(())
}

/// A directory for wine to write an icon theme into, under wineo's own state so
/// nothing depends on the system temp dir, and removed however this call ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(paths: &Paths) -> Result<Self> {
        let dir = paths
            .icon_scratch_dir()
            .join(std::process::id().to_string());
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).context(|| format!("failed to create {}", dir.display()))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Reads the shortcut icon, ignoring file associations Wine may refresh too.
fn wine_icon_name(dir: &Path) -> Option<String> {
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        for found in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = found.path();
            if found.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "desktop")
                && let Ok(text) = fs::read_to_string(&path)
                && !text.lines().any(|line| line.starts_with("MimeType="))
                && let Some(icon) = text
                    .lines()
                    .find_map(|line| line.strip_prefix("Icon="))
                    .map(str::trim)
                && !icon.is_empty()
            {
                return Some(icon.to_owned());
            }
        }
    }
    None
}

/// A Start Menu shortcut already pointing at `exe`, which is the only thing
/// wine's menu builder will take an icon from.
fn shortcut_for(wine_prefix: &Path, exe: &str) -> Option<String> {
    if !winpath::is_windows_path(exe) {
        return None;
    }
    let windows = exe.to_ascii_lowercase();
    let listing = programs::scan(wine_prefix).ok()?;
    listing
        .programs
        .into_iter()
        .find(|program| program.exe.to_ascii_lowercase() == windows)
        .and_then(|program| program.lnk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::symlink;

    /// An XDG root, an environment config, and a prefix holding its executable.
    struct Fixture {
        _root: tempfile::TempDir,
        paths: Paths,
        prefix: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().to_owned();
            let paths = Paths::from_lookup(move |key| {
                (key == "HOME").then(|| OsString::from(home.as_os_str()))
            })
            .unwrap();
            let prefix = root.path().join("prefix");
            fs::create_dir_all(prefix.join("dosdevices")).unwrap();
            fs::create_dir_all(prefix.join("drive_c/Hades")).unwrap();
            fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
            symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
            fs::write(prefix.join("drive_c/Hades/Hades.exe"), b"").unwrap();
            Self {
                _root: root,
                paths,
                prefix,
            }
        }

        fn environment(&self, targets: &str) -> Environment {
            let path = self.paths.environment_config("hades");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = format!(
                "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n{targets}",
                self.prefix.display()
            );
            fs::write(&path, text).unwrap();
            Environment::load(&self.paths, "hades").unwrap()
        }

        /// Reconciles with no runner, so nothing tries to start wine for an icon.
        fn reconcile(&self, environment: &Environment) -> Vec<Change> {
            reconcile(
                &self.paths,
                &GlobalConfig::default(),
                environment,
                None,
                &self.prefix,
            )
            .unwrap()
        }

        fn entry(&self, target: &str) -> Option<String> {
            fs::read_to_string(entry_path(&self.paths, "hades", target)).ok()
        }
    }

    const ASKS: &str = "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n";

    #[test]
    fn writes_an_entry_for_a_target_that_asks() {
        let fixture = Fixture::new();
        let environment = fixture.environment(ASKS);
        assert_eq!(
            fixture.reconcile(&environment),
            [Change::Wrote {
                target: "hades".to_owned(),
                icon: false,
            }]
        );
        assert_eq!(
            fixture.entry("hades").unwrap(),
            concat!(
                "[Desktop Entry]\n",
                "Type=Application\n",
                "Name=hades\n",
                "Comment=Runs in the wineo environment hades\n",
                "Exec=wineo start hades --target hades\n",
                "Terminal=false\n",
                "Categories=Utility;Software;\n",
                "StartupWMClass=Hades.exe\n",
                "X-Wineo-Environment=hades\n",
                "X-Wineo-Target=hades\n",
            )
        );
    }

    // Reconciliation runs on every launch, so an entry that already matches has
    // to cost nothing and rewrite nothing.
    #[test]
    fn leaves_a_matching_entry_alone() {
        let fixture = Fixture::new();
        let environment = fixture.environment(ASKS);
        assert_eq!(fixture.reconcile(&environment).len(), 1);
        assert!(fixture.reconcile(&environment).is_empty());
        assert!(fixture.reconcile(&environment).is_empty());
    }

    #[test]
    fn a_target_with_no_setting_gets_no_entry() {
        let fixture = Fixture::new();
        let environment = fixture.environment("\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\n");
        assert!(fixture.reconcile(&environment).is_empty());
        assert_eq!(fixture.entry("hades"), None);
    }

    #[test]
    fn clearing_the_setting_takes_the_entry_away() {
        let fixture = Fixture::new();
        let asking = fixture.environment(ASKS);
        fixture.reconcile(&asking);
        assert!(fixture.entry("hades").is_some());

        let no_longer = fixture.environment("\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\n");
        assert_eq!(
            fixture.reconcile(&no_longer),
            [Change::Removed {
                target: "hades".to_owned()
            }]
        );
        assert_eq!(fixture.entry("hades"), None);
    }

    #[test]
    fn removing_the_target_takes_the_entry_away() {
        let fixture = Fixture::new();
        let asking = fixture.environment(ASKS);
        fixture.reconcile(&asking);
        let gone = fixture.environment("");
        assert_eq!(
            fixture.reconcile(&gone),
            [Change::Removed {
                target: "hades".to_owned()
            }]
        );
        assert_eq!(fixture.entry("hades"), None);
    }

    #[test]
    fn the_name_field_titles_the_entry() {
        let fixture = Fixture::new();
        let environment = fixture.environment(
            "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\nname = \"Hades\"\n",
        );
        fixture.reconcile(&environment);
        assert!(fixture.entry("hades").unwrap().contains("\nName=Hades\n"));

        // Changing it rewrites the entry rather than adding a second one.
        let renamed = fixture.environment(
            "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\nname = \"Hades II\"\n",
        );
        assert_eq!(
            fixture.reconcile(&renamed),
            [Change::Updated {
                target: "hades".to_owned()
            }]
        );
        assert!(
            fixture
                .entry("hades")
                .unwrap()
                .contains("\nName=Hades II\n")
        );
    }

    #[test]
    fn gamemode_makes_it_a_game() {
        let fixture = Fixture::new();
        let environment = fixture.environment(
            "gamemode = true\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n",
        );
        fixture.reconcile(&environment);
        assert!(
            fixture
                .entry("hades")
                .unwrap()
                .contains("\nCategories=Game;\n"),
            "{}",
            fixture.entry("hades").unwrap()
        );
    }

    // One target with a bad exe must not stop the others, since reconciliation
    // runs on every launch and a launch of `good` has nothing to do with it.
    #[test]
    fn skips_a_target_whose_exe_is_not_there() {
        let fixture = Fixture::new();
        let environment = fixture.environment(concat!(
            "\n[targets.typo]\nexe = 'C:\\Hades\\Typo.exe'\ndesktop = true\n",
            "\n[targets.good]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n",
        ));
        let changes = fixture.reconcile(&environment);
        assert_eq!(
            changes,
            [
                Change::Wrote {
                    target: "good".to_owned(),
                    icon: false,
                },
                Change::Skipped {
                    target: "typo".to_owned(),
                    reason: r"`C:\Hades\Typo.exe` does not exist".to_owned(),
                },
            ],
            "{changes:?}"
        );
        assert!(fixture.entry("good").is_some());
        assert_eq!(fixture.entry("typo"), None);
    }

    #[test]
    fn check_refuses_an_exe_that_is_not_there() {
        let fixture = Fixture::new();
        assert!(check(&fixture.paths, &fixture.prefix, r"C:\Hades\Hades.exe").is_ok());
        assert!(check(&fixture.paths, &fixture.prefix, "notepad").is_ok());
        let err = check(&fixture.paths, &fixture.prefix, r"C:\Hades\Typo.exe").unwrap_err();
        assert!(matches!(err, Error::ExeNotFound(_)), "{err}");
    }

    // A bare name is wine's to look up, so there is nothing to check it against.
    #[test]
    fn accepts_a_bare_program_name() {
        let fixture = Fixture::new();
        let environment = fixture.environment("\n[targets.pad]\nexe = 'notepad'\ndesktop = true\n");
        assert_eq!(fixture.reconcile(&environment).len(), 1);
        assert!(
            fixture
                .entry("pad")
                .unwrap()
                .contains("\nStartupWMClass=notepad.exe\n")
        );
    }

    #[test]
    fn lists_only_entries_wineo_wrote() {
        let fixture = Fixture::new();
        let environment = fixture.environment(ASKS);
        fixture.reconcile(&environment);

        let dir = fixture.paths.applications_dir();
        // Named like one of wineo's, but without the keys that make it wineo's.
        fs::write(
            dir.join("wineo-someone-elses.desktop"),
            "[Desktop Entry]\nName=Not ours\n",
        )
        .unwrap();
        fs::write(
            dir.join("firefox.desktop"),
            "[Desktop Entry]\nName=Firefox\n",
        )
        .unwrap();

        let listed = list(&fixture.paths).unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0].environment, "hades");
        assert_eq!(listed[0].target, "hades");
        assert_eq!(listed[0].name, "hades");
    }

    #[test]
    fn ownership_keys_identify_a_renamed_entry() {
        let fixture = Fixture::new();
        let environment = fixture.environment(ASKS);
        fixture.reconcile(&environment);
        let renamed = fixture.paths.applications_dir().join("renamed.desktop");
        fs::rename(entry_path(&fixture.paths, "hades", "hades"), &renamed).unwrap();

        let listed = list(&fixture.paths).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, renamed);
        assert_eq!(remove_all(&fixture.paths, "hades").unwrap(), ["hades"]);
        assert!(!renamed.exists());
    }

    #[test]
    fn remove_all_clears_one_environment() {
        let fixture = Fixture::new();
        let environment = fixture.environment(
            "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n\n[targets.pad]\nexe = 'notepad'\ndesktop = true\n",
        );
        fixture.reconcile(&environment);
        assert_eq!(list(&fixture.paths).unwrap().len(), 2);

        let removed = remove_all(&fixture.paths, "hades").unwrap();
        assert_eq!(removed, ["hades", "pad"]);
        assert!(list(&fixture.paths).unwrap().is_empty());
        // A second pass has nothing left to do.
        assert!(remove_all(&fixture.paths, "hades").unwrap().is_empty());
    }

    #[test]
    fn entry_values_stay_on_one_line() {
        assert_eq!(clean("Hades\nInjected=x"), "HadesInjected=x");
        assert_eq!(clean("  Hades  "), "Hades");
    }

    #[test]
    fn shortcut_icon_ignores_file_association_entries() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("wine-extension-vbs.desktop"),
            "[Desktop Entry]\nMimeType=text/vbscript;\nIcon=wscript\n",
        )
        .unwrap();
        assert_eq!(wine_icon_name(root.path()), None);

        let menu = root.path().join("wine/Programs");
        fs::create_dir_all(&menu).unwrap();
        fs::write(
            menu.join("Game.desktop"),
            "[Desktop Entry]\nName=Game\nIcon=game\n",
        )
        .unwrap();
        assert_eq!(wine_icon_name(root.path()).as_deref(), Some("game"));
    }

    #[test]
    fn window_class_comes_from_the_executable() {
        assert_eq!(wm_class(r"C:\Hades\Hades.exe").unwrap(), "Hades.exe");
        assert_eq!(wm_class("/games/hades/Hades.exe").unwrap(), "Hades.exe");
        assert_eq!(wm_class("notepad").unwrap(), "notepad.exe");
        assert_eq!(wm_class(""), None);
    }

    #[test]
    fn icons_go_when_the_entry_does() {
        let fixture = Fixture::new();
        let environment = fixture.environment(ASKS);
        fixture.reconcile(&environment);

        // Stand in for what wine's menu builder would have written.
        let icon = fixture
            .paths
            .icons_dir()
            .join("hicolor/48x48/apps")
            .join(format!("{}.png", icon_name("hades", "hades")));
        fs::create_dir_all(icon.parent().unwrap()).unwrap();
        fs::write(&icon, b"png").unwrap();
        assert!(has_icon(&fixture.paths, "hades", "hades"));

        // The entry is now out of date, since it has no Icon line yet.
        assert_eq!(
            fixture.reconcile(&environment),
            [Change::Updated {
                target: "hades".to_owned()
            }]
        );
        assert!(
            fixture
                .entry("hades")
                .unwrap()
                .contains("\nIcon=wineo-hades-hades\n")
        );

        remove_all(&fixture.paths, "hades").unwrap();
        assert!(!icon.exists());
    }
}
