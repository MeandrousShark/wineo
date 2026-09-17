use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};

/// The wine build an environment runs with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runner {
    name: String,
    wine: PathBuf,
    wineserver: PathBuf,
}

impl Runner {
    pub fn resolve(name: &str) -> Result<Self> {
        if name != "wine" {
            return Err(Error::UnsupportedRunner(name.to_owned()));
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        Ok(Self {
            name: name.to_owned(),
            wine: find_in_path(&path, "wine").ok_or(Error::ProgramNotFound("wine"))?,
            wineserver: find_in_path(&path, "wineserver")
                .ok_or(Error::ProgramNotFound("wineserver"))?,
        })
    }

    /// A runner whose binaries don't exist, for tests that never launch wine.
    #[cfg(test)]
    pub(crate) fn fake(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            wine: PathBuf::from("/nonexistent/wine"),
            wineserver: PathBuf::from("/nonexistent/wineserver"),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// A `wine` invocation bound to `prefix`.
    pub fn wine(&self, prefix: &Path) -> Command {
        let mut command = Command::new(&self.wine);
        command.env("WINEPREFIX", prefix);
        command
    }

    /// A `wineserver` invocation bound to `prefix`.
    pub fn wineserver(&self, prefix: &Path) -> Command {
        let mut command = Command::new(&self.wineserver);
        command.env("WINEPREFIX", prefix);
        command
    }

    /// Blocks until every process in the prefix has exited and wineserver has shut down.
    pub fn wait_idle(&self, prefix: &Path) -> Result<()> {
        quietly(self.wineserver(prefix).arg("-w"))
            .status()
            .context(|| "failed to run wineserver -w".to_owned())?;
        Ok(())
    }

    /// Asks running programs to close, like a Windows logoff.
    pub fn end_session(&self, prefix: &Path) -> Result<()> {
        quietly(self.wine(prefix).args(["wineboot", "--end-session"]))
            .status()
            .context(|| "failed to run wineboot --end-session".to_owned())?;
        Ok(())
    }

    /// Kills everything in the prefix without giving programs a chance to clean up.
    pub fn kill(&self, prefix: &Path) -> Result<()> {
        quietly(self.wineserver(prefix).arg("-k"))
            .status()
            .context(|| "failed to run wineserver -k".to_owned())?;
        Ok(())
    }

    /// The version wine reports, like `wine-11.17`.
    pub fn version(&self) -> Option<String> {
        let output = Command::new(&self.wine)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !version.is_empty()).then_some(version)
    }
}

fn quietly(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

fn find_in_path(path_var: &OsStr, program: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use super::*;

    #[test]
    fn finds_first_executable_on_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        // Not executable, so it must be skipped.
        fs::write(first.path().join("wine"), "").unwrap();
        let wine = second.path().join("wine");
        fs::write(&wine, "").unwrap();
        fs::set_permissions(&wine, fs::Permissions::from_mode(0o755)).unwrap();

        let path_var = std::env::join_paths([
            "relative",
            first.path().to_str().unwrap(),
            second.path().to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(find_in_path(&path_var, "wine"), Some(wine));
        assert_eq!(find_in_path(&path_var, "wineserver"), None);
        assert_eq!(find_in_path(&OsString::new(), "wine"), None);
    }

    #[test]
    fn other_runners_are_not_supported_yet() {
        assert!(matches!(
            Runner::resolve("proton-ge"),
            Err(Error::UnsupportedRunner(name)) if name == "proton-ge"
        ));
    }
}
