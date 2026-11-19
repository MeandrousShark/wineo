//! What wineo knows about each built-in emulator: where to find it, what it
//! plays, and which files are its games. Written from each project's own
//! source and docs (see docs/sketches/emulated-games.md), not from anyone's
//! data files.

use super::Emulator;

/// Which system a file extension belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSystem {
    /// The extension tells the system.
    Is(&'static str),
    /// The emulator plays more than one system with this extension, so the
    /// file alone can't say.
    Ambiguous,
}

/// A kind of game that is a folder, not a file. The ROM is the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderGame {
    /// A Wii U title folder with `code/`, `content/`, and `meta/`.
    CemuTitle,
    /// A PSP folder holding `EBOOT.PBP`.
    PpssppEboot,
    /// A PS3 disc folder with `PS3_GAME/USRDIR/EBOOT.BIN`.
    Rpcs3Disc,
}

struct Spec {
    path_names: &'static [&'static str],
    flatpak: &'static str,
    systems: &'static [&'static str],
    extensions: &'static [(&'static str, ExtSystem)],
    folder: Option<FolderGame>,
    fullscreen: bool,
}

const GAMECUBE: &str = "gamecube";
const WII: &str = "wii";
const WIIU: &str = "wiiu";
const PS1: &str = "ps1";
const PS2: &str = "ps2";
const PS3: &str = "ps3";
const PSP: &str = "psp";
const NDS: &str = "nds";
const N3DS: &str = "3ds";
const GB: &str = "gb";
const GBC: &str = "gbc";
const GBA: &str = "gba";
const XBOX: &str = "xbox";

use ExtSystem::{Ambiguous, Is};

const DOLPHIN: Spec = Spec {
    path_names: &["dolphin-emu"],
    flatpak: "org.DolphinEmu.dolphin-emu",
    systems: &[GAMECUBE, WII],
    extensions: &[
        ("gcm", Is(GAMECUBE)),
        ("tgc", Is(GAMECUBE)),
        ("wbfs", Is(WII)),
        ("wad", Is(WII)),
        ("iso", Ambiguous),
        ("ciso", Ambiguous),
        ("gcz", Ambiguous),
        ("wia", Ambiguous),
        ("rvz", Ambiguous),
        ("elf", Ambiguous),
        ("dol", Ambiguous),
    ],
    folder: None,
    // It has no flag for it.
    fullscreen: false,
};

const RETROARCH: Spec = Spec {
    path_names: &["retroarch"],
    flatpak: "org.libretro.RetroArch",
    systems: &[],
    // Its cores list their own.
    extensions: &[],
    folder: None,
    fullscreen: true,
};

const CEMU: Spec = Spec {
    path_names: &["cemu", "Cemu"],
    flatpak: "info.cemu.Cemu",
    systems: &[WIIU],
    extensions: &[
        ("wud", Is(WIIU)),
        ("wux", Is(WIIU)),
        ("wua", Is(WIIU)),
        ("wuhb", Is(WIIU)),
    ],
    folder: Some(FolderGame::CemuTitle),
    fullscreen: true,
};

const PCSX2: Spec = Spec {
    path_names: &["pcsx2-qt", "pcsx2"],
    flatpak: "net.pcsx2.PCSX2",
    systems: &[PS2],
    // Not cue: nothing in PCSX2 reads one.
    extensions: &[
        ("iso", Is(PS2)),
        ("bin", Is(PS2)),
        ("img", Is(PS2)),
        ("mdf", Is(PS2)),
        ("chd", Is(PS2)),
        ("cso", Is(PS2)),
        ("zso", Is(PS2)),
        ("gz", Is(PS2)),
    ],
    folder: None,
    fullscreen: true,
};

const DUCKSTATION: Spec = Spec {
    path_names: &["duckstation-qt", "duckstation-nogui"],
    flatpak: "org.duckstation.DuckStation",
    systems: &[PS1],
    extensions: &[
        ("cue", Is(PS1)),
        ("chd", Is(PS1)),
        ("iso", Is(PS1)),
        ("pbp", Is(PS1)),
        ("ecm", Is(PS1)),
        ("mds", Is(PS1)),
        ("ccd", Is(PS1)),
        ("m3u", Is(PS1)),
        ("bin", Is(PS1)),
        ("img", Is(PS1)),
    ],
    folder: None,
    fullscreen: true,
};

const PPSSPP: Spec = Spec {
    path_names: &["ppsspp", "PPSSPPSDL", "PPSSPPQt"],
    flatpak: "org.ppsspp.PPSSPP",
    systems: &[PSP],
    extensions: &[
        ("iso", Is(PSP)),
        ("cso", Is(PSP)),
        ("chd", Is(PSP)),
        ("pbp", Is(PSP)),
    ],
    folder: Some(FolderGame::PpssppEboot),
    fullscreen: true,
};

const RPCS3: Spec = Spec {
    path_names: &["rpcs3"],
    flatpak: "net.rpcs3.RPCS3",
    systems: &[PS3],
    extensions: &[("iso", Is(PS3))],
    folder: Some(FolderGame::Rpcs3Disc),
    fullscreen: true,
};

const MELONDS: Spec = Spec {
    path_names: &["melonDS", "melonds"],
    flatpak: "net.kuribo64.melonDS",
    systems: &[NDS],
    extensions: &[
        ("nds", Is(NDS)),
        ("srl", Is(NDS)),
        ("dsi", Is(NDS)),
        ("ids", Is(NDS)),
    ],
    folder: None,
    fullscreen: true,
};

const MGBA: Spec = Spec {
    path_names: &["mgba-qt", "mgba"],
    flatpak: "io.mgba.mGBA",
    systems: &[GB, GBC, GBA],
    // It also opens .bin, .rom, .zip, .7z, and .elf, which are too generic
    // to scan for.
    extensions: &[
        ("gb", Is(GB)),
        ("sgb", Is(GB)),
        ("gbc", Is(GBC)),
        ("gba", Is(GBA)),
        ("agb", Is(GBA)),
        ("mb", Is(GBA)),
    ],
    folder: None,
    fullscreen: true,
};

const XEMU: Spec = Spec {
    path_names: &["xemu"],
    flatpak: "app.xemu.xemu",
    systems: &[XBOX],
    extensions: &[("iso", Is(XBOX))],
    folder: None,
    fullscreen: true,
};

const AZAHAR: Spec = Spec {
    path_names: &["azahar"],
    flatpak: "org.azahar_emu.Azahar",
    systems: &[N3DS],
    extensions: &[
        ("cci", Is(N3DS)),
        ("3ds", Is(N3DS)),
        ("cia", Is(N3DS)),
        ("3dsx", Is(N3DS)),
        ("cxi", Is(N3DS)),
        ("app", Is(N3DS)),
        ("zcci", Is(N3DS)),
        ("zcia", Is(N3DS)),
        ("z3dsx", Is(N3DS)),
        ("zcxi", Is(N3DS)),
    ],
    folder: None,
    fullscreen: true,
};

impl Emulator {
    fn spec(self) -> Option<&'static Spec> {
        Some(match self {
            Self::Dolphin => &DOLPHIN,
            Self::RetroArch => &RETROARCH,
            Self::Cemu => &CEMU,
            Self::Pcsx2 => &PCSX2,
            Self::DuckStation => &DUCKSTATION,
            Self::Ppsspp => &PPSSPP,
            Self::Rpcs3 => &RPCS3,
            Self::MelonDs => &MELONDS,
            Self::Mgba => &MGBA,
            Self::Xemu => &XEMU,
            Self::Azahar => &AZAHAR,
            Self::Other => return None,
        })
    }

    /// The names to look for on PATH, in the order to prefer them. Empty for
    /// `Other`.
    pub fn path_names(self) -> &'static [&'static str] {
        self.spec().map_or(&[], |spec| spec.path_names)
    }

    /// The Flathub app id, which `Other` doesn't have.
    pub fn flatpak_id(self) -> Option<&'static str> {
        self.spec().map(|spec| spec.flatpak)
    }

    /// The system ids it plays. RetroArch has none here, since its cores
    /// say what they play.
    pub fn systems(self) -> &'static [&'static str] {
        self.spec().map_or(&[], |spec| spec.systems)
    }

    /// Its game file extensions, lowercase and without the dot, each with
    /// the system it tells.
    pub fn extensions(self) -> &'static [(&'static str, ExtSystem)] {
        self.spec().map_or(&[], |spec| spec.extensions)
    }

    /// What an extension, given without its dot and in any case, tells.
    /// None when the emulator doesn't list it.
    pub fn extension_system(self, extension: &str) -> Option<ExtSystem> {
        self.extensions()
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(extension))
            .map(|(_, system)| *system)
    }

    /// The kind of folder game it takes, which a scan can list.
    pub fn folder_game(self) -> Option<FolderGame> {
        self.spec().and_then(|spec| spec.folder)
    }

    /// Whether it has a flag to start in fullscreen.
    pub fn has_fullscreen_flag(self) -> bool {
        self.spec().is_some_and(|spec| spec.fullscreen)
    }
}
