//! Finding an emulator: a name on PATH, a Flatpak's export script, or a file
//! the user picks. Every search root is passed in, so tests never look at
//! the host.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::Emulator;
use crate::error::{Error, Result};
use crate::launch::{WINDOWS_EXTENSIONS, native_on_path};
use crate::paths::Paths;
use crate::{runner, winpath};

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
/// Flatpak. Names that lead to the same binary (a `mgba` link to `mgba-qt`)
/// count once, under the first in table order. `flatpak_exports` is the
/// folders to look in, system-wide first, as `flatpak_exports` returns them;
/// only the first with the script counts.
pub fn installs(emulator: Emulator, path_var: &OsStr, flatpak_exports: &[PathBuf]) -> Vec<Install> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut found: Vec<Install> = Vec::new();
    for name in emulator.path_names() {
        let Some(path) = native_on_path(path_var, name) else {
            continue;
        };
        let real = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen.contains(&real) {
            seen.push(real);
            found.push(Install::Path { name, found: path });
        }
    }
    if let Some(app) = emulator.flatpak_id() {
        // A link left behind by an uninstall points nowhere, and a folder
        // isn't a script, so neither counts.
        let script = flatpak_exports
            .iter()
            .map(|dir| dir.join(app))
            .find(|script| script.is_file());
        if let Some(script) = script {
            found.push(Install::Flatpak { script });
        }
    }
    found
}

/// A file the user picked to run `emulator`, checked the way a native
/// target's exe is: `~/` is expanded, it must be absolute, exist as a file,
/// be executable, and not be a Windows program. One that is the emulator's
/// own Flatpak script in one of `flatpak_exports` is that Flatpak install,
/// so it gets its folder shared like one.
pub fn pick_file(
    paths: &Paths,
    emulator: Emulator,
    flatpak_exports: &[PathBuf],
    picked: &str,
) -> Result<Install> {
    let lower = picked.to_ascii_lowercase();
    if winpath::is_windows_path(picked) || WINDOWS_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
    {
        return Err(Error::Emulator(format!(
            "`{picked}` is a Windows program, not a native one."
        )));
    }
    let path = paths.expand(picked);
    let shown = path.display().to_string();
    if !path.is_absolute() {
        return Err(Error::RelativeExe(shown));
    }
    if !path.is_file() {
        return Err(Error::ExeNotFound(shown));
    }
    if !runner::is_executable(&path) {
        return Err(Error::NotExecutable(shown));
    }
    let is_export = emulator.flatpak_id().is_some_and(|app| {
        path.file_name().is_some_and(|name| name == app)
            && path
                .parent()
                .is_some_and(|dir| flatpak_exports.iter().any(|export| same_dir(export, dir)))
    });
    Ok(if is_export {
        Install::Flatpak { script: path }
    } else {
        Install::File(path)
    })
}

/// Whether two folders are the same, also through symlinks: on Fedora
/// Atomic `/home` links to `/var/home`, so a picker can hand back either.
/// Only folders are resolved, since an export script is itself a link into
/// its app.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}
