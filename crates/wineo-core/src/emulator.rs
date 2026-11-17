//! The emulators wineo knows by name, for an emulated game's Source.

mod build;
mod install;
mod table;

use std::path::{Path, PathBuf};

use crate::paths::Paths;

pub use build::{Command, build_target, custom_target};
pub use install::{Install, installs, pick_file};
pub use table::{ExtSystem, FolderGame};

/// The `[emulation] emulator` a custom emulator writes. It names no built-in,
/// so its games show as `Emulator::Other`.
pub const CUSTOM: &str = "custom";

/// A built-in emulator, or `Other` for a custom one and any id this wineo
/// doesn't know, like one a newer wineo wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Emulator {
    Dolphin,
    RetroArch,
    Cemu,
    Pcsx2,
    DuckStation,
    Ppsspp,
    Rpcs3,
    MelonDs,
    Mgba,
    Xemu,
    Azahar,
    Other,
}

impl Emulator {
    pub const BUILT_IN: [Self; 11] = [
        Self::Dolphin,
        Self::RetroArch,
        Self::Cemu,
        Self::Pcsx2,
        Self::DuckStation,
        Self::Ppsspp,
        Self::Rpcs3,
        Self::MelonDs,
        Self::Mgba,
        Self::Xemu,
        Self::Azahar,
    ];

    /// The id `[emulation] emulator` and saved presets name it by. `Other`
    /// has no config id of its own, so its id is the preset one, `emulated`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Dolphin => "dolphin",
            Self::RetroArch => "retroarch",
            Self::Cemu => "cemu",
            Self::Pcsx2 => "pcsx2",
            Self::DuckStation => "duckstation",
            Self::Ppsspp => "ppsspp",
            Self::Rpcs3 => "rpcs3",
            Self::MelonDs => "melonds",
            Self::Mgba => "mgba",
            Self::Xemu => "xemu",
            Self::Azahar => "azahar",
            Self::Other => "emulated",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dolphin => "Dolphin",
            Self::RetroArch => "RetroArch",
            Self::Cemu => "Cemu",
            Self::Pcsx2 => "PCSX2",
            Self::DuckStation => "DuckStation",
            Self::Ppsspp => "PPSSPP",
            Self::Rpcs3 => "RPCS3",
            Self::MelonDs => "melonDS",
            Self::Mgba => "mGBA",
            Self::Xemu => "xemu",
            Self::Azahar => "Azahar",
            Self::Other => "Emulated",
        }
    }

    /// What an `[emulation] emulator` value names. Never fails, so a game
    /// from a newer wineo still has a Source.
    pub fn from_config(id: &str) -> Self {
        Self::BUILT_IN
            .into_iter()
            .find(|emulator| emulator.id() == id)
            .unwrap_or(Self::Other)
    }
}

/// Where Flatpak puts the scripts that run its apps, system-wide first.
pub fn flatpak_exports(paths: &Paths) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/var/lib/flatpak/exports/bin"),
        paths.xdg_data().join("flatpak/exports/bin"),
    ]
}

/// `flatpak run` options that take a value, which they may be given as the
/// next argument instead of after an `=`.
const FLATPAK_RUN_VALUE_OPTIONS: &[&str] = &[
    "--arch",
    "--branch",
    "--command",
    "--cwd",
    "--commit",
    "--runtime",
    "--runtime-version",
    "--filesystem",
    "--share",
    "--unshare",
    "--socket",
    "--nosocket",
    "--device",
    "--nodevice",
    "--allow",
    "--disallow",
    "--env",
    "--env-fd",
    "--unset-env",
    "--add-policy",
    "--remove-policy",
    "--persist",
    "--usb",
    "--nousb",
    "--own-name",
    "--talk-name",
    "--no-talk-name",
    "--system-own-name",
    "--system-talk-name",
    "--system-no-talk-name",
    "--sandbox-flag",
    "--app-path",
    "--usr-path",
    "--bind-mount",
    "--instance-id-fd",
    "--a11y-own-name",
    "--installation",
    "--nofilesystem",
    "--share-if",
    "--socket-if",
    "--device-if",
    "--allow-if",
    "--parent-pid",
];

/// An app id is at least three dot-separated parts of letters, digits,
/// `_` and `-`.
fn is_app_id(word: &str) -> bool {
    let mut parts = 0;
    for part in word.split('.') {
        if part.is_empty()
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return false;
        }
        parts += 1;
    }
    parts >= 3
}

/// The app a target's `flatpak [options] run [options] ID ...` runs, if that's
/// its form. Fails open: anything it can't read as an app id gives None, so a
/// form it doesn't know never warns.
pub fn flatpak_app<'a>(exe: &str, args: &'a [String]) -> Option<&'a str> {
    if Path::new(exe).file_name()? != "flatpak" {
        return None;
    }
    let mut args = args.iter().map(String::as_str);
    loop {
        let arg = args.next()?;
        if arg == "run" {
            break;
        }
        if !arg.starts_with('-') {
            return None;
        }
        if arg == "--installation" {
            args.next()?;
        }
    }
    let mut after_options = false;
    let candidate = loop {
        let arg = args.next()?;
        if after_options || !arg.starts_with('-') {
            break arg;
        }
        if arg == "--" {
            after_options = true;
        } else if FLATPAK_RUN_VALUE_OPTIONS.contains(&arg) {
            args.next()?;
        }
    };
    // A ref may carry an arch and branch after the id: ID//stable, ID/arch/branch.
    let id = candidate.split('/').next()?;
    is_app_id(id).then_some(id)
}

/// Whether `app` still has its script in one of `exports`. A link left
/// behind by an uninstall points nowhere, so it doesn't count.
pub fn flatpak_installed(exports: &[PathBuf], app: &str) -> bool {
    // A path-shaped value would escape the exports directory.
    is_app_id(app) && exports.iter().any(|dir| dir.join(app).exists())
}

#[cfg(test)]
mod tests;
