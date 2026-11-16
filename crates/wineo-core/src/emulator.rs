//! The emulators wineo knows by name, for an emulated game's Source.

use std::path::{Path, PathBuf};

use crate::paths::Paths;

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

/// The app a target's `flatpak run ... ID ...` runs, if that's its form.
pub fn flatpak_app<'a>(exe: &str, args: &'a [String]) -> Option<&'a str> {
    if Path::new(exe).file_name()? != "flatpak" {
        return None;
    }
    let (run, rest) = args.split_first()?;
    if run != "run" {
        return None;
    }
    rest.iter()
        .map(String::as_str)
        .find(|arg| !arg.starts_with('-'))
}

/// Whether `app` still has its script in one of `exports`. A link left
/// behind by an uninstall points nowhere, so it doesn't count.
pub fn flatpak_installed(exports: &[PathBuf], app: &str) -> bool {
    exports.iter().any(|dir| dir.join(app).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_ids_and_names_are_fixed() {
        assert_eq!(
            Emulator::BUILT_IN.map(Emulator::id),
            [
                "dolphin",
                "retroarch",
                "cemu",
                "pcsx2",
                "duckstation",
                "ppsspp",
                "rpcs3",
                "melonds",
                "mgba",
                "xemu",
                "azahar",
            ]
        );
        assert_eq!(
            Emulator::BUILT_IN.map(Emulator::name),
            [
                "Dolphin",
                "RetroArch",
                "Cemu",
                "PCSX2",
                "DuckStation",
                "PPSSPP",
                "RPCS3",
                "melonDS",
                "mGBA",
                "xemu",
                "Azahar",
            ]
        );
    }

    #[test]
    fn a_config_id_reads_back_and_anything_else_is_other() {
        for emulator in Emulator::BUILT_IN {
            assert_eq!(Emulator::from_config(emulator.id()), emulator);
        }
        for id in [CUSTOM, "eden", "Dolphin", "", "emulated"] {
            assert_eq!(Emulator::from_config(id), Emulator::Other, "{id}");
        }
        assert_eq!(Emulator::Other.name(), "Emulated");
    }

    #[test]
    fn the_flatpak_app_is_the_first_word_after_run() {
        let args = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let run = args(&[
            "run",
            "--filesystem=/games/ps2",
            "net.pcsx2.PCSX2",
            "-batch",
            "--",
            "/games/ps2/a.iso",
        ]);
        assert_eq!(flatpak_app("flatpak", &run), Some("net.pcsx2.PCSX2"));
        assert_eq!(
            flatpak_app("/usr/bin/flatpak", &run),
            Some("net.pcsx2.PCSX2")
        );
        assert_eq!(flatpak_app("dolphin-emu", &run), None);
        assert_eq!(flatpak_app("flatpak", &args(&["info", "x"])), None);
        assert_eq!(flatpak_app("flatpak", &args(&["run", "--verbose"])), None);
        assert_eq!(flatpak_app("flatpak", &[]), None);
    }

    #[test]
    fn an_app_counts_as_installed_only_while_its_script_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system");
        let user = dir.path().join("user");
        std::fs::create_dir_all(&system).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let exports = [system.clone(), user.clone()];
        assert!(!flatpak_installed(&exports, "org.DolphinEmu.dolphin-emu"));
        std::fs::write(user.join("org.DolphinEmu.dolphin-emu"), "").unwrap();
        assert!(flatpak_installed(&exports, "org.DolphinEmu.dolphin-emu"));
        std::os::unix::fs::symlink(dir.path().join("gone"), system.join("net.pcsx2.PCSX2"))
            .unwrap();
        assert!(!flatpak_installed(&exports, "net.pcsx2.PCSX2"));
    }
}
