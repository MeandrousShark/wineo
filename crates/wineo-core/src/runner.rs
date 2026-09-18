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
    /// The reserved name meaning whatever wine is on `PATH`.
    pub const SYSTEM: &'static str = "wine";

    /// The system wine, found on `PATH`.
    pub fn system() -> Result<Self> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Ok(Self {
            name: Self::SYSTEM.to_owned(),
            wine: find_in_path(&path, "wine").ok_or(Error::ProgramNotFound("wine"))?,
            wineserver: find_in_path(&path, "wineserver")
                .ok_or(Error::ProgramNotFound("wineserver"))?,
        })
    }

    /// An installed build, with its executables already located.
    pub(crate) fn at(name: String, wine: PathBuf, wineserver: PathBuf) -> Self {
        Self {
            name,
            wine,
            wineserver,
        }
    }

    /// A runner whose `wine` and `wineserver` both resolve to `true` on PATH,
    /// so tests can run real commands without wine installed.
    #[cfg(test)]
    pub(crate) fn fake(name: &str) -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let true_bin = find_in_path(&path, "true").expect("`true` must be on PATH");
        Self {
            name: name.to_owned(),
            wine: true_bin.clone(),
            wineserver: true_bin,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved executables, for handing to a program that runs wine
    /// itself. Absolute, so a different PATH can't select another build.
    pub fn wine_path(&self) -> &Path {
        &self.wine
    }

    pub fn wineserver_path(&self) -> &Path {
        &self.wineserver
    }

    /// A `wine` invocation bound to `prefix`. Also blocks wine's own menu
    /// builder, unless the inherited environment already opts back in.
    pub fn wine(&self, prefix: &Path) -> Command {
        self.wine_through(prefix, None)
    }

    /// The same, run through `wrapper` when there is one, like gamemoderun.
    pub fn wine_through(&self, prefix: &Path, wrapper: Option<&Path>) -> Command {
        let mut command = match wrapper {
            Some(wrapper) => {
                let mut command = Command::new(wrapper);
                command.arg(&self.wine);
                command
            }
            None => Command::new(&self.wine),
        };
        let overrides =
            with_menu_builder_disabled(std::env::var("WINEDLLOVERRIDES").ok().as_deref());
        command
            .env("WINEPREFIX", prefix)
            .env("WINEDLLOVERRIDES", overrides);
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

/// Adds `winemenubuilder.exe=d` to a `WINEDLLOVERRIDES` value, unless it
/// already mentions winemenubuilder, so a user's own override always wins.
pub fn with_menu_builder_disabled(current: Option<&str>) -> String {
    const DISABLED: &str = "winemenubuilder.exe=d";
    match current {
        None | Some("") => DISABLED.to_owned(),
        Some(current) if mentions_menu_builder(current) => current.to_owned(),
        Some(current) => format!("{current};{DISABLED}"),
    }
}

/// Wine's `WINEDLLOVERRIDES` syntax is `name1,name2=order;name3=order2`.
fn mentions_menu_builder(overrides: &str) -> bool {
    overrides.split(';').any(|entry| {
        let names = entry.split_once('=').map_or(entry, |(names, _)| names);
        names.split(',').any(|name| {
            let name = name.trim();
            name.eq_ignore_ascii_case("winemenubuilder")
                || name.eq_ignore_ascii_case("winemenubuilder.exe")
        })
    })
}

fn quietly(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

/// Looks `program` up on the current PATH.
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    find_in_path(&std::env::var_os("PATH").unwrap_or_default(), program)
}

fn find_in_path(path_var: &OsStr, program: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn is_executable(path: &Path) -> bool {
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
    fn menu_builder_is_disabled_when_overrides_are_unset() {
        assert_eq!(with_menu_builder_disabled(None), "winemenubuilder.exe=d");
        assert_eq!(
            with_menu_builder_disabled(Some("")),
            "winemenubuilder.exe=d"
        );
    }

    #[test]
    fn menu_builder_override_is_appended_to_existing_overrides() {
        assert_eq!(
            with_menu_builder_disabled(Some("foo.dll=n")),
            "foo.dll=n;winemenubuilder.exe=d"
        );
    }

    #[test]
    fn existing_menu_builder_mentions_are_left_alone() {
        for value in [
            "winemenubuilder.exe=n",
            "WineMenuBuilder=b",
            "foo.dll=n;winemenubuilder.exe=n",
            "foo,winemenubuilder.exe=n",
            " winemenubuilder.exe =n",
        ] {
            assert_eq!(with_menu_builder_disabled(Some(value)), value, "{value}");
        }
    }

    #[test]
    fn substring_matches_do_not_count_as_a_mention() {
        assert_eq!(
            with_menu_builder_disabled(Some("winemenubuilderextra.exe=n")),
            "winemenubuilderextra.exe=n;winemenubuilder.exe=d"
        );
        assert_eq!(
            with_menu_builder_disabled(Some("extrawinemenubuilder.exe=n")),
            "extrawinemenubuilder.exe=n;winemenubuilder.exe=d"
        );
    }
}
