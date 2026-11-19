//! RetroArch's cores: where each install keeps them, what their `.info`
//! files say, and the system a core's game is. wineo ships no core data of
//! its own and reads only what is installed.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{Emulator, Install};
use crate::paths::Paths;

/// Where one RetroArch install keeps its cores and their info files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreFolders {
    /// Searched in order. A core's info file is looked for beside it first.
    pub cores: Vec<PathBuf>,
    /// Where the info files for those cores are, after the core's own folder.
    pub info: Vec<PathBuf>,
}

/// The core and info folders for `install`. `system_root` is `/` outside
/// tests: the distro folders and the system-wide Flatpak hang off it. A
/// distro or downloaded core is never offered for the Flatpak, or the
/// Flatpak's own for a native RetroArch.
pub fn core_folders(paths: &Paths, install: &Install, system_root: &Path) -> CoreFolders {
    let system = |path: &str| system_root.join(path);
    match install {
        Install::Flatpak { .. } => {
            let app = Emulator::RetroArch
                .flatpak_id()
                .expect("RetroArch has a Flatpak");
            let info = "current/active/files/share/libretro/info";
            CoreFolders {
                cores: vec![flatpak_cores_dir(paths)],
                info: vec![
                    system("var/lib/flatpak/app").join(app).join(info),
                    paths.xdg_data().join("flatpak/app").join(app).join(info),
                ],
            }
        }
        Install::Path { .. } | Install::File(_) => {
            // RetroArch reads the host's config folder, so a portable root
            // doesn't move it. Portable imports are refused anyway.
            let config = paths
                .xdg_config()
                .map_or_else(|| paths.home().join(".config"), Path::to_path_buf);
            CoreFolders {
                cores: vec![
                    system("usr/lib/libretro"),
                    system("usr/lib64/libretro"),
                    system("usr/lib/x86_64-linux-gnu/libretro"),
                    config.join("retroarch/cores"),
                ],
                info: vec![system("usr/share/libretro/info")],
            }
        }
    }
}

/// Where the RetroArch Flatpak keeps the cores it downloads.
pub fn flatpak_cores_dir(paths: &Paths) -> PathBuf {
    let app = Emulator::RetroArch
        .flatpak_id()
        .expect("RetroArch has a Flatpak");
    paths
        .home()
        .join(".var/app")
        .join(app)
        .join("config/retroarch/cores")
}

/// One installed core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Core {
    /// The `NAME_libretro.so` file.
    pub path: PathBuf,
    pub info: Option<CoreInfo>,
}

/// What a core's `.info` file says, as far as wineo uses it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreInfo {
    pub display_name: Option<String>,
    pub systemid: Option<String>,
    /// Lowercase, without the dot. Empty for a core that plays no files.
    pub extensions: Vec<String>,
    /// The systems the core plays, by libretro's database names.
    pub database: Vec<String>,
}

const CORE_SUFFIX: &str = "_libretro.so";

impl Core {
    /// The core's file name, like `snes9x_libretro.so`.
    pub fn file_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }

    /// The core's id, its file name less `_libretro.so`, like `snes9x`.
    pub fn id(&self) -> &str {
        let name = self.file_name();
        name.strip_suffix(CORE_SUFFIX).unwrap_or(name)
    }

    /// Its display name, or its file name when it has no info.
    pub fn name(&self) -> &str {
        self.info
            .as_ref()
            .and_then(|info| info.display_name.as_deref())
            .unwrap_or_else(|| self.file_name())
    }

    /// The system its info names, if wineo knows it.
    pub fn system(&self) -> Option<&'static str> {
        self.info
            .as_ref()
            .and_then(|info| info.systemid.as_deref())
            .and_then(system_for_id)
    }

    /// The extensions it plays. None for a core without info, which is
    /// offered every file.
    pub fn extensions(&self) -> Option<&[String]> {
        self.info.as_ref().map(|info| info.extensions.as_slice())
    }

    /// Whether it plays a file with `extension`, lowercase and without the
    /// dot. A core without info plays anything.
    pub fn plays(&self, extension: Option<&str>) -> bool {
        match self.extensions() {
            None => true,
            Some(known) => extension.is_some_and(|extension| known.iter().any(|k| k == extension)),
        }
    }

    /// The system of a game with `extension`. A core whose databases name
    /// more than one of wineo's systems places a file by its extension and
    /// leaves one it can't place without; any other core gives its own
    /// system, so an add-on like the Satellaview doesn't split the SNES.
    pub fn file_system(&self, extension: Option<&str>) -> Option<&'static str> {
        let info = self.info.as_ref()?;
        let mut systems: Vec<&str> = info
            .database
            .iter()
            .filter_map(|name| system_for_database(name))
            .collect();
        systems.sort_unstable();
        systems.dedup();
        if systems.len() > 1 {
            return extension.and_then(system_for_extension);
        }
        self.system()
    }
}

/// Reads an info file's `key = "value"` lines. Spacing around `=` and quotes
/// around the value are optional, and keys it doesn't use are ignored.
pub fn parse_info(text: &str) -> CoreInfo {
    let mut info = CoreInfo::default();
    let split = |value: &str| -> Vec<String> {
        value
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        // A quoted value ends at its closing quote, so a comment after it
        // is dropped; an unclosed one keeps the rest.
        let value = value
            .strip_prefix('"')
            .map_or(value, |rest| {
                rest.split_once('"').map_or(rest, |(inner, _)| inner)
            })
            .trim();
        let text = (!value.is_empty()).then(|| value.to_owned());
        match key.trim() {
            "display_name" => info.display_name = text,
            "systemid" => info.systemid = text,
            "supported_extensions" => {
                info.extensions = split(value)
                    .into_iter()
                    .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
                    .filter(|extension| !extension.is_empty())
                    .collect();
            }
            "database" => info.database = split(value),
            _ => {}
        }
    }
    info
}

/// The most of an info file that is read. Real ones are a few kilobytes.
const INFO_LIMIT: u64 = 1 << 20;

fn read_info(path: &Path) -> Option<CoreInfo> {
    if !path.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(INFO_LIMIT)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(parse_info(&String::from_utf8_lossy(&bytes)))
}

/// The cores in `folders`, sorted by name, then path. A core is installed
/// when its `.so` is there; an info file alone doesn't count. A core reached
/// through two folders, like `/usr/lib64` linking to `/usr/lib`, counts once.
pub fn cores(folders: &CoreFolders) -> Vec<Core> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut found = Vec::new();
    for dir in &folders.cores {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.len() > CORE_SUFFIX.len() && name.ends_with(CORE_SUFFIX)
                    })
                    && path.is_file()
            })
            .collect();
        paths.sort();
        for path in paths {
            let real = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(real) {
                continue;
            }
            let stem = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".so"))
                .unwrap_or_default();
            let info_name = format!("{stem}.info");
            let info = std::iter::once(dir)
                .chain(&folders.info)
                .find_map(|info_dir| read_info(&info_dir.join(&info_name)));
            found.push(Core { path, info });
        }
    }
    found.sort_by_cached_key(|core| (crate::name::fold_case(core.name()), core.path.clone()));
    found
}

/// libretro's `systemid` to wineo's system id, for the common consoles and
/// handhelds. Ids that are already wineo's (`gamecube`, `nds`, `3ds`, ...)
/// map to themselves.
const SYSTEM_IDS: &[(&str, &str)] = &[
    ("nes", "nes"),
    ("super_nes", "snes"),
    ("nintendo_64", "n64"),
    ("n64", "n64"),
    ("gamecube", "gamecube"),
    ("wii", "wii"),
    ("wiiu", "wiiu"),
    ("game_boy", "gb"),
    ("game_boy_color", "gbc"),
    ("game_boy_advance", "gba"),
    ("nds", "nds"),
    ("3ds", "3ds"),
    ("virtual_boy", "virtualboy"),
    ("playstation", "ps1"),
    ("playstation2", "ps2"),
    ("playstation3", "ps3"),
    ("playstation_portable", "psp"),
    ("xbox", "xbox"),
    ("master_system", "mastersystem"),
    ("game_gear", "gamegear"),
    ("mega_drive", "genesis"),
    ("sega_saturn", "saturn"),
    ("dreamcast", "dreamcast"),
    ("pc_engine", "pcengine"),
    ("atari_2600", "atari2600"),
    ("atari_lynx", "lynx"),
    ("neo_geo_pocket", "ngp"),
    ("neo_geo_pocket_color", "ngpc"),
    ("wonderswan", "wonderswan"),
    ("wonderswan_color", "wonderswancolor"),
];

/// libretro's database names, which an info file's `database` key lists, to
/// wineo's system ids. A name that isn't here, like an add-on or an arcade
/// board, counts for nothing.
const DATABASE_NAMES: &[(&str, &str)] = &[
    ("Nintendo - Nintendo Entertainment System", "nes"),
    ("Nintendo - Family Computer Disk System", "nes"),
    ("Nintendo - Super Nintendo Entertainment System", "snes"),
    ("Nintendo - Satellaview", "snes"),
    ("Nintendo - Sufami Turbo", "snes"),
    ("Nintendo - Nintendo 64", "n64"),
    ("Nintendo - Nintendo 64DD", "n64"),
    ("Nintendo - GameCube", "gamecube"),
    ("Nintendo - Wii", "wii"),
    ("Nintendo - Wii (Digital)", "wii"),
    ("Nintendo - Wii U", "wiiu"),
    ("Nintendo - Game Boy", "gb"),
    ("Nintendo - Game Boy Color", "gbc"),
    ("Nintendo - Game Boy Advance", "gba"),
    ("Nintendo - Game Boy Advance (e-Cards)", "gba"),
    ("Nintendo - Nintendo DS", "nds"),
    ("Nintendo - Nintendo DS (Download Play)", "nds"),
    ("Nintendo - Nintendo DSi", "nds"),
    ("Nintendo - Nintendo DSi Decrypted", "nds"),
    ("Nintendo - Nintendo DSi (Digital)", "nds"),
    ("Nintendo - Nintendo 3DS", "3ds"),
    ("Nintendo - Virtual Boy", "virtualboy"),
    ("Sony - PlayStation", "ps1"),
    ("Sony - PlayStation 2", "ps2"),
    ("Sony - PlayStation 3", "ps3"),
    ("Sony - PlayStation Portable", "psp"),
    ("Microsoft - Xbox", "xbox"),
    ("Sega - Master System - Mark III", "mastersystem"),
    ("Sega - Game Gear", "gamegear"),
    ("Sega - Mega Drive - Genesis", "genesis"),
    ("Sega - Saturn", "saturn"),
    ("Sega - Dreamcast", "dreamcast"),
    ("NEC - PC Engine - TurboGrafx 16", "pcengine"),
    // The PC Engine family is one system to wineo. Sega's Mega-CD and 32X
    // have no id, so they count for nothing.
    ("NEC - PC Engine SuperGrafx", "pcengine"),
    ("NEC - PC Engine CD - TurboGrafx-CD", "pcengine"),
    ("Atari - 2600", "atari2600"),
    ("Atari - Lynx", "lynx"),
    ("SNK - Neo Geo Pocket", "ngp"),
    ("SNK - Neo Geo Pocket Color", "ngpc"),
    ("Bandai - WonderSwan", "wonderswan"),
    ("Bandai - WonderSwan Color", "wonderswancolor"),
];

/// The extensions that tell a system on their own, for a core that plays
/// several. Generic ones like `bin`, `iso`, and `cue` never do.
const EXTENSION_SYSTEMS: &[(&str, &str)] = &[
    ("nes", "nes"),
    ("fds", "nes"),
    ("sfc", "snes"),
    ("smc", "snes"),
    ("swc", "snes"),
    ("fig", "snes"),
    ("bs", "snes"),
    ("st", "snes"),
    ("n64", "n64"),
    ("ndd", "n64"),
    ("v64", "n64"),
    ("z64", "n64"),
    ("gcm", "gamecube"),
    ("tgc", "gamecube"),
    ("wbfs", "wii"),
    ("wad", "wii"),
    ("gb", "gb"),
    ("dmg", "gb"),
    ("sgb", "gb"),
    ("gbc", "gbc"),
    ("cgb", "gbc"),
    ("gba", "gba"),
    ("agb", "gba"),
    ("nds", "nds"),
    ("dsi", "nds"),
    ("ids", "nds"),
    ("srl", "nds"),
    ("3ds", "3ds"),
    ("md", "genesis"),
    ("gen", "genesis"),
    ("smd", "genesis"),
    ("sms", "mastersystem"),
    ("gg", "gamegear"),
    ("cdi", "dreamcast"),
    ("gdi", "dreamcast"),
    ("pce", "pcengine"),
    ("ngp", "ngp"),
    ("ngc", "ngpc"),
    ("ngpc", "ngpc"),
    ("ws", "wonderswan"),
    ("wsc", "wonderswancolor"),
];

/// wineo's system id for a libretro `systemid`, if it's in the table.
pub fn system_for_id(systemid: &str) -> Option<&'static str> {
    SYSTEM_IDS
        .iter()
        .find(|(id, _)| *id == systemid)
        .map(|(_, system)| *system)
}

/// wineo's system id for a libretro database name, if it's in the table.
pub fn system_for_database(name: &str) -> Option<&'static str> {
    DATABASE_NAMES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, system)| *system)
}

/// The system an extension, lowercase and without the dot, tells on its own.
pub fn system_for_extension(extension: &str) -> Option<&'static str> {
    EXTENSION_SYSTEMS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, system)| *system)
}

/// The file wineo appends to RetroArch's config, in the host data folder.
pub fn quit_file(paths: &Paths) -> PathBuf {
    paths.host_data().join("retroarch-quit.cfg")
}

/// What the quit file holds: with `quit_with_game` on, closing the game
/// quits RetroArch (when it was started with a game, as wineo starts it),
/// and nothing is saved back into the main config on the way out, since
/// RetroArch would otherwise keep the appended setting for good. Off, it's
/// empty, so RetroArch behaves as it does on its own.
pub fn quit_file_contents(quit_with_game: bool) -> &'static str {
    if quit_with_game {
        "quit_on_close_content = \"2\"\nconfig_save_on_exit = \"false\"\n"
    } else {
        ""
    }
}

/// Writes the quit file for the setting, only when what's there differs.
/// Returns whether it wrote.
pub fn write_quit_file(paths: &Paths, quit_with_game: bool) -> crate::error::Result<bool> {
    let path = quit_file(paths);
    let contents = quit_file_contents(quit_with_game);
    if fs::read(&path).is_ok_and(|old| old == contents.as_bytes()) {
        return Ok(false);
    }
    crate::files::write_atomic(&path, contents)?;
    Ok(true)
}

/// Writes the quit file for `quit_with_game`. A failure comes back as a
/// warning to show, since a game still runs without it.
pub fn refresh_quit_file(paths: &Paths, quit_with_game: bool) -> Option<String> {
    write_quit_file(paths, quit_with_game).err().map(|error| {
        format!(
            "Can't write RetroArch's quit file: {}.",
            crate::error::chain(&error)
        )
    })
}

/// The core a RetroArch target's arguments load: the value of `-L` or
/// `--libretro`.
pub fn target_core(args: &[String]) -> Option<&str> {
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        if arg == "-L" || arg == "--libretro" {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix("--libretro=") {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests;
