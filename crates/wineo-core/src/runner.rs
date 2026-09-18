use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};

/// umu-launcher, which runs Proton the way Steam does.
const UMU: &str = "umu-run";

/// The wine build an environment runs with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runner {
    name: String,
    kind: Kind,
}

/// Proton is driven entirely through umu, so it shares no executables with a
/// plain wine build rather than being another way to reach the same ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Wine { wine: PathBuf, wineserver: PathBuf },
    Proton { root: PathBuf },
}

impl Runner {
    /// The reserved name meaning whatever wine is on `PATH`.
    pub const SYSTEM: &'static str = "wine";

    /// The system wine, found on `PATH`.
    pub fn system() -> Result<Self> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Ok(Self {
            name: Self::SYSTEM.to_owned(),
            kind: Kind::Wine {
                wine: find_in_path(&path, "wine").ok_or(Error::ProgramNotFound("wine"))?,
                wineserver: find_in_path(&path, "wineserver")
                    .ok_or(Error::ProgramNotFound("wineserver"))?,
            },
        })
    }

    /// An installed wine build, with its executables already located.
    pub(crate) fn wine_build(name: String, wine: PathBuf, wineserver: PathBuf) -> Self {
        Self {
            name,
            kind: Kind::Wine { wine, wineserver },
        }
    }

    /// An installed Proton build. Only its root matters, since umu takes that
    /// as `PROTONPATH` and runs everything itself.
    pub(crate) fn proton(name: String, root: PathBuf) -> Self {
        Self {
            name,
            kind: Kind::Proton { root },
        }
    }

    /// A runner whose `wine` and `wineserver` both resolve to `true` on PATH,
    /// so tests can run real commands without wine installed.
    #[cfg(test)]
    pub(crate) fn fake(name: &str) -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let true_bin = find_in_path(&path, "true").expect("`true` must be on PATH");
        Self::wine_build(name.to_owned(), true_bin.clone(), true_bin)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::Wine { .. } => "wine",
            Kind::Proton { .. } => "proton",
        }
    }

    pub fn is_proton(&self) -> bool {
        matches!(self.kind, Kind::Proton { .. })
    }

    /// The resolved executables, for handing to a program that runs wine
    /// itself. Absolute, so a different PATH can't select another build.
    /// None for Proton, which has no wine wineo is meant to run directly.
    pub fn wine_path(&self) -> Option<&Path> {
        match &self.kind {
            Kind::Wine { wine, .. } => Some(wine),
            Kind::Proton { .. } => None,
        }
    }

    pub fn wineserver_path(&self) -> Option<&Path> {
        match &self.kind {
            Kind::Wine { wineserver, .. } => Some(wineserver),
            Kind::Proton { .. } => None,
        }
    }

    /// The wine prefix inside a configured prefix directory. Proton is given
    /// the compatdata directory and keeps its prefix in `pfx/` under it.
    pub fn wine_prefix(&self, prefix: &Path) -> PathBuf {
        match self.kind {
            Kind::Wine { .. } => prefix.to_owned(),
            Kind::Proton { .. } => prefix.join("pfx"),
        }
    }

    /// A command that runs a Windows program in `prefix`, through wine or
    /// through umu. The caller appends the program and its arguments. Also
    /// blocks wine's own menu builder, unless the inherited environment already
    /// opts back in. `prefix` is the configured directory, not `wine_prefix`:
    /// umu makes `pfx/` underneath it itself.
    pub fn command(&self, prefix: &Path, wrapper: Option<&Path>) -> Result<Command> {
        let mut command = match &self.kind {
            Kind::Wine { wine, .. } => wrapped(wine, wrapper),
            Kind::Proton { root } => {
                let umu = find_on_path(UMU).ok_or(Error::ProgramNotFound(UMU))?;
                let mut command = wrapped(&umu, wrapper);
                command.env("PROTONPATH", root);
                command
            }
        };
        let overrides =
            with_menu_builder_disabled(std::env::var("WINEDLLOVERRIDES").ok().as_deref());
        command
            .env("WINEPREFIX", prefix)
            .env("WINEDLLOVERRIDES", overrides);
        Ok(command)
    }

    /// A `wineserver` invocation bound to `prefix`. Proton has no wineserver
    /// wineo can reach, so its runs are ended through the process group instead.
    fn wineserver(&self, prefix: &Path) -> Option<Command> {
        let wineserver = self.wineserver_path()?;
        let mut command = Command::new(wineserver);
        command.env("WINEPREFIX", self.wine_prefix(prefix));
        Some(command)
    }

    /// Blocks until every process in the prefix has exited and wineserver has
    /// shut down. A Proton run is over when umu returns, so there is nothing
    /// to wait for.
    pub fn wait_idle(&self, prefix: &Path) -> Result<()> {
        let Some(mut command) = self.wineserver(prefix) else {
            return Ok(());
        };
        quietly(command.arg("-w"))
            .status()
            .context(|| "failed to run wineserver -w".to_owned())?;
        Ok(())
    }

    /// Asks running programs to close, like a Windows logoff.
    pub fn end_session(&self, prefix: &Path) -> Result<()> {
        if self.is_proton() {
            return Ok(());
        }
        quietly(
            self.command(prefix, None)?
                .args(["wineboot", "--end-session"]),
        )
        .status()
        .context(|| "failed to run wineboot --end-session".to_owned())?;
        Ok(())
    }

    /// Kills everything in the prefix without giving programs a chance to clean up.
    pub fn kill(&self, prefix: &Path) -> Result<()> {
        let Some(mut command) = self.wineserver(prefix) else {
            return Ok(());
        };
        quietly(command.arg("-k"))
            .status()
            .context(|| "failed to run wineserver -k".to_owned())?;
        Ok(())
    }

    /// The version this runner reports: what wine prints, or what Proton
    /// records in its own `version` file.
    pub fn version(&self) -> Option<String> {
        match &self.kind {
            Kind::Wine { wine, .. } => {
                let output = Command::new(wine)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()?;
                let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (output.status.success() && !version.is_empty()).then_some(version)
            }
            // Proton writes a build timestamp and a name, like `1717451631 GE-Proton9-4`.
            Kind::Proton { root } => {
                let text = std::fs::read_to_string(root.join("version")).ok()?;
                let version = text.split_whitespace().next_back()?.to_owned();
                (!version.is_empty()).then_some(version)
            }
        }
    }
}

/// `program`, or `wrapper program` when something like gamemoderun wraps it.
fn wrapped(program: &Path, wrapper: Option<&Path>) -> Command {
    match wrapper {
        Some(wrapper) => {
            let mut command = Command::new(wrapper);
            command.arg(program);
            command
        }
        None => Command::new(program),
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
