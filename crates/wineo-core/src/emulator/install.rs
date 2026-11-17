//! Finding an emulator: a name on PATH, a Flatpak's export script, or a file
//! the user picks. Every search root is passed in, so tests never look at
//! the host.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::Emulator;
use crate::error::{Error, Result};
use crate::launch::native_on_path;
use crate::runner;

/// One way to run an emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// A name found on PATH. A target keeps the bare `name`.
    Path { name: &'static str, found: PathBuf },
    /// Its Flatpak, by the script in an `exports/bin` folder.
    Flatpak { script: PathBuf },
    /// A file the user picked, like an AppImage.
    File(PathBuf),
}

/// What `emulator` has installed, in order: each PATH name found, then its
/// Flatpak. `flatpak_exports` is the folders to look in, system-wide first,
/// as `flatpak_exports` returns them; only the first with the script counts.
pub fn installs(emulator: Emulator, path_var: &OsStr, flatpak_exports: &[PathBuf]) -> Vec<Install> {
    let mut found: Vec<Install> = emulator
        .path_names()
        .iter()
        .filter_map(|name| {
            native_on_path(path_var, name).map(|found| Install::Path { name, found })
        })
        .collect();
    if let Some(app) = emulator.flatpak_id() {
        // A link left behind by an uninstall points nowhere, so it doesn't count.
        let script = flatpak_exports
            .iter()
            .map(|dir| dir.join(app))
            .find(|script| script.exists());
        if let Some(script) = script {
            found.push(Install::Flatpak { script });
        }
    }
    found
}

/// A file the user picked to run an emulator, checked the way a native
/// target's exe is: it must be absolute, exist as a file, and be executable.
pub fn pick_file(path: &Path) -> Result<Install> {
    let shown = path.display().to_string();
    if !path.is_absolute() {
        return Err(Error::RelativeExe(shown));
    }
    if !path.is_file() {
        return Err(Error::ExeNotFound(shown));
    }
    if !runner::is_executable(path) {
        return Err(Error::NotExecutable(shown));
    }
    Ok(Install::File(path.to_owned()))
}
