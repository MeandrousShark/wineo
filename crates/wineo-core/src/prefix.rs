use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::Stdio;

use crate::config::Arch;
use crate::error::{Error, IoContext, Result};
use crate::runner::Runner;

pub fn is_prefix(dir: &Path) -> bool {
    dir.join("system.reg").is_file()
}

/// True when `dir` holds a wine prefix directly, or a Proton one in `pfx/`.
/// Used where the runner isn't known, such as removing a broken environment.
pub fn holds_prefix(dir: &Path) -> bool {
    is_prefix(dir) || is_prefix(&dir.join("pfx"))
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

/// Initializes a prefix in `dir`, sending wine's output to `log`.
pub fn create(runner: &Runner, dir: &Path, arch: Arch, log: &Path) -> Result<()> {
    let output = File::create(log).context(|| format!("failed to create {}", log.display()))?;
    let stderr = output
        .try_clone()
        .context(|| format!("failed to open {}", log.display()))?;
    // umu creates the prefix when it is handed an empty program, and waits for
    // it. Proton prefixes are always win64, so WINEARCH means nothing there.
    let mut command = runner.command(dir, None)?;
    if runner.is_proton() {
        command.arg("");
    } else {
        command
            .env("WINEARCH", arch.as_str())
            .args(["wineboot", "--init"]);
    }
    let status = command
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(stderr)
        .status()
        .context(|| "failed to run wineboot".to_owned())?;

    // A missing or failed prefix would make another wine command create a
    // fresh one, so only write the override once wineboot has initialized it.
    // Its own error is folded into the check below since wine is still
    // running either way and must be allowed to shut down.
    let menu_builder_disabled = status.success() && disable_menu_builder(runner, dir, log).is_ok();

    // Wine only writes the registry files once wineserver shuts down.
    runner.wait_idle(dir)?;

    if menu_builder_disabled && is_prefix(&runner.wine_prefix(dir)) {
        Ok(())
    } else {
        Err(Error::PrefixCreation {
            reason: last_wine_message(log),
            log: log.to_owned(),
        })
    }
}

/// Writes `HKCU\Software\Wine\DllOverrides\winemenubuilder.exe=""`, so wine
/// run outside wineo doesn't build menu entries in this prefix either.
/// Shared by a freshly created prefix and one `new` adopts as is.
pub fn disable_menu_builder(runner: &Runner, dir: &Path, log: &Path) -> Result<()> {
    let output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .context(|| format!("failed to open {}", log.display()))?;
    let stderr = output
        .try_clone()
        .context(|| format!("failed to open {}", log.display()))?;
    let status = runner
        .command(dir, None)?
        .args([
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
        ])
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(stderr)
        .status()
        .context(|| "failed to run wine reg add".to_owned())?;
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
