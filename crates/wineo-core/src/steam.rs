//! Steam as wineo reads it: the installs, their libraries, and the manifests
//! of the games and tools in them. Nothing here writes, and `registry.vdf` is
//! never read. Every function takes the install or home it looks in, so tests
//! can build fixture trees under a temporary directory.

mod account;
pub mod api;
pub mod key;
pub mod owned;
pub mod process;
pub mod vdf;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::art;

pub use account::{
    Playtime, active_account, active_steam_id, compat_tool, compat_tool_in, compat_tools, playtime,
};
pub use process::{
    Baseline, Reaper, baseline, eligible_reapers, matching_reapers, open_reaper, pidfd_exited,
    reapers_by_app, stop_tree, wait_pidfd_exit,
};
use vdf::{Object, Value};

const FLATPAK_ID: &str = "com.valvesoftware.Steam";

/// Steam's tools have manifests like games, and only Steam's binary
/// `appinfo.vdf` says what they are. These are the ones that matter.
const TOOL_APP_IDS: [&str; 5] = ["228980", "1070560", "1391110", "1628350", "4183110"];
// The trailing spaces keep games like "Protonic" from matching.
const TOOL_NAME_PREFIXES: [&str; 3] = ["Proton ", "Steam Linux Runtime", "Steamworks "];

const UPDATE_REQUIRED: u32 = 2;
const FULLY_INSTALLED: u32 = 4;
const UNINSTALLING: u32 = 2048;
const UPDATING: u32 =
    256 | 512 | 1024 | 131_072 | 262_144 | 524_288 | 1_048_576 | 2_097_152 | 4_194_304;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallKind {
    Native,
    Flatpak,
}

/// A Steam client folder. `root` is the path it was found at, so a symlinked
/// `~/.steam/root` stays as `~/.steam/root`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Install {
    kind: InstallKind,
    root: PathBuf,
}

impl Install {
    pub fn kind(&self) -> InstallKind {
        self.kind
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Every Steam install under `home` that exists, native first. Symlinks such
/// as `~/.steam/root` point at the same folder, so each install is listed
/// once, under the first path it was found at.
pub fn installs(home: &Path) -> Vec<Install> {
    // The order is the one runner discovery has always used, so a symlinked
    // install keeps the path it was listed under.
    let candidates = [
        (InstallKind::Native, home.join(".steam/root")),
        (InstallKind::Native, home.join(".steam/steam")),
        (InstallKind::Native, home.join(".local/share/Steam")),
        (
            InstallKind::Flatpak,
            home.join(".var/app")
                .join(FLATPAK_ID)
                .join(".local/share/Steam"),
        ),
    ];
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|(_, root)| {
            fs::canonicalize(root)
                .is_ok_and(|canonical| canonical.is_dir() && seen.insert(canonical))
        })
        .map(|(kind, root)| Install { kind, root })
        .collect()
}

/// The install wineo uses for games, following the global `steam` setting.
pub fn active_for(
    paths: &crate::paths::Paths,
    global: &crate::config::GlobalConfig,
) -> Option<Install> {
    active_install(&installs(paths.home()), global.steam).cloned()
}

pub fn active_install(installs: &[Install], preference: Option<InstallKind>) -> Option<&Install> {
    let native = installs
        .iter()
        .find(|install| install.kind == InstallKind::Native);
    let flatpak = installs
        .iter()
        .find(|install| install.kind == InstallKind::Flatpak);
    match (native, flatpak) {
        (Some(native), Some(flatpak)) => Some(if preference == Some(InstallKind::Flatpak) {
            flatpak
        } else {
            native
        }),
        (native, flatpak) => native.or(flatpak),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub path: PathBuf,
    /// The folder and its `steamapps` can be read. An unavailable library
    /// might hold a game, so a game missing elsewhere can't be called
    /// uninstalled.
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Libraries {
    /// The install's own root first, then the libraries `libraryfolders.vdf`
    /// lists. Each path appears once.
    pub entries: Vec<Library>,
    /// Set when `libraryfolders.vdf` exists but can't be read or parsed, so
    /// `entries` may be missing libraries.
    pub problem: Option<String>,
}

pub fn libraries(install: &Install) -> Libraries {
    let mut paths = vec![install.root.clone()];
    let mut problem = None;
    let file = install.root.join("steamapps/libraryfolders.vdf");
    match fs::read_to_string(&file) {
        Ok(text) => match library_paths(&text) {
            Ok(listed) => paths.extend(listed),
            Err(error) => problem = Some(format!("{} can't be parsed: {error}", file.display())),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => problem = Some(format!("{} can't be read: {error}", file.display())),
    }

    // Steam lists the root again as library 0, and symlinks can repeat one.
    let mut seen = HashSet::new();
    let entries = paths
        .into_iter()
        .filter(|path| seen.insert(fs::canonicalize(path).unwrap_or_else(|_| path.clone())))
        .map(|path| Library {
            available: readable_library(&path),
            path,
        })
        .collect();
    Libraries { entries, problem }
}

/// The `path` of every numbered entry under `libraryfolders`. The `apps` lists
/// in those entries can be stale, so they aren't read.
pub(crate) fn library_paths(text: &str) -> Result<Vec<PathBuf>, vdf::ParseError> {
    let document = vdf::parse(text)?;
    let Some(folders) = document.object("libraryfolders") else {
        return Ok(Vec::new());
    };
    Ok(folders
        .entries()
        .filter(|(key, _)| all_digits(key))
        .filter_map(|(_, value)| value.as_object()?.text("path"))
        .map(PathBuf::from)
        .collect())
}

fn readable_library(path: &Path) -> bool {
    // A `steamapps` that doesn't exist yet is fine: the library just has no games.
    fs::read_dir(path).is_ok()
        && match fs::read_dir(path.join("steamapps")) {
            Ok(_) => true,
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        }
}

/// What a manifest says, read from `appmanifest_<app_id>.acf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub app_id: String,
    pub name: String,
    pub install_dir: String,
    pub state_flags: u32,
    pub size_on_disk: u64,
    /// Unix seconds, or 0 when Steam hasn't recorded a launch.
    pub last_played: u64,
    pub bytes_to_download: u64,
    pub bytes_downloaded: u64,
    pub bytes_to_stage: u64,
    pub bytes_staged: u64,
}

impl Manifest {
    pub fn state(&self) -> InstallState {
        state_from_flags(self.state_flags)
    }

    /// The finer state wineo shows, which tells an install in progress from a
    /// game that isn't there.
    pub fn app_state(&self) -> AppState {
        let flags = self.state_flags;
        if flags & UNINSTALLING != 0 {
            AppState::Uninstalling
        } else if flags & FULLY_INSTALLED != 0 {
            if flags & UPDATING != 0 {
                AppState::Updating
            } else {
                AppState::Installed
            }
        } else if flags & UPDATING != 0 {
            AppState::Installing(self.install_percent())
        } else if flags & UPDATE_REQUIRED != 0 {
            AppState::InstallQueued
        } else {
            AppState::NotInstalled
        }
    }

    /// How far an install is, from 0 to 100. See `install_bytes`.
    pub fn install_percent(&self) -> Option<u8> {
        self.install_bytes()
            .map(|(done, total)| percent(done, total))
    }

    /// Bytes done and bytes in all for the phase an install is in: downloading
    /// first, then staging once the download is done, as the launch watcher
    /// reads it. None when Steam hasn't written any totals.
    pub fn install_bytes(&self) -> Option<(u64, u64)> {
        if self.bytes_to_download > 0 && self.bytes_downloaded < self.bytes_to_download {
            Some((self.bytes_downloaded, self.bytes_to_download))
        } else if self.bytes_to_stage > 0 {
            Some((
                self.bytes_staged.min(self.bytes_to_stage),
                self.bytes_to_stage,
            ))
        } else if self.bytes_to_download > 0 {
            Some((self.bytes_to_download, self.bytes_to_download))
        } else {
            None
        }
    }

    pub fn is_tool(&self) -> bool {
        is_tool(&self.app_id, &self.name)
    }

    pub fn progress(&self) -> Progress {
        Progress {
            state_flags: self.state_flags,
            bytes_downloaded: self.bytes_downloaded,
            bytes_staged: self.bytes_staged,
        }
    }
}

/// One manifest file. `manifest` is an error when the file can't be read or
/// parsed, and the app id still comes from the file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub app_id: String,
    pub library: PathBuf,
    pub manifest: Result<Manifest, String>,
}

/// Every manifest in every available library, in library order and then by
/// file name. One bad manifest never hides the others.
pub fn manifests(install: &Install) -> Vec<ManifestEntry> {
    let mut found = Vec::new();
    for library in libraries(install)
        .entries
        .into_iter()
        .filter(|library| library.available)
    {
        let Ok(read) = fs::read_dir(library.path.join("steamapps")) else {
            continue;
        };
        let mut files: Vec<(String, PathBuf)> = read
            .flatten()
            .filter_map(|entry| {
                let file_name = entry.file_name().into_string().ok()?;
                let app_id = file_name
                    .strip_prefix("appmanifest_")?
                    .strip_suffix(".acf")?;
                all_digits(app_id).then(|| (app_id.to_owned(), entry.path()))
            })
            .collect();
        files.sort();
        for (app_id, path) in files {
            let manifest = fs::read_to_string(&path)
                .map_err(|error| format!("{} can't be read: {error}", path.display()))
                .and_then(|text| {
                    parse_manifest(&text, &app_id)
                        .map_err(|error| format!("{}: {error}", path.display()))
                });
            found.push(ManifestEntry {
                app_id,
                library: library.path.clone(),
                manifest,
            });
        }
    }
    found
}

fn parse_manifest(text: &str, app_id: &str) -> Result<Manifest, String> {
    let document = vdf::parse(text).map_err(|error| error.to_string())?;
    let state = document
        .object("AppState")
        .ok_or("has no AppState section")?;
    if let Some(stated) = state.text("appid")
        && stated != app_id
    {
        return Err(format!(
            "says appid {stated}, but its file name says {app_id}"
        ));
    }
    let state_flags = state
        .get("StateFlags")
        .ok_or("has no StateFlags")
        .and_then(|value| number_value("StateFlags", value).map_err(|_| "has a bad StateFlags"))
        .and_then(|flags| u32::try_from(flags).map_err(|_| "has a StateFlags out of range"))?;
    Ok(Manifest {
        app_id: app_id.to_owned(),
        name: state.text("name").unwrap_or_default().to_owned(),
        install_dir: state.text("installdir").unwrap_or_default().to_owned(),
        state_flags,
        size_on_disk: number(state, "SizeOnDisk")?,
        last_played: number(state, "LastPlayed")?,
        bytes_to_download: number(state, "BytesToDownload")?,
        bytes_downloaded: number(state, "BytesDownloaded")?,
        bytes_to_stage: number(state, "BytesToStage")?,
        bytes_staged: number(state, "BytesStaged")?,
    })
}

/// A missing number is 0, which is what Steam writes for "nothing yet". A
/// number that isn't one is an error, since it could change the answer.
fn number(object: &Object, key: &str) -> Result<u64, String> {
    match object.get(key) {
        None => Ok(0),
        Some(value) => number_value(key, value),
    }
}

fn number_value(key: &str, value: &Value) -> Result<u64, String> {
    value
        .as_text()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| format!("{key} isn't a number"))
}

/// Whether Steam has the game fully on disk, is working on it, or doesn't
/// have it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    Installed,
    Updating,
    NotInstalled,
}

/// The `StateFlags` rules. Uninstalling is checked first, so a game being
/// removed is never waited on as an update. AppRunning (64) is stale and
/// ignored, and UpdateRequired (2), FilesMissing, and FilesCorrupt don't
/// change the answer, since Steam repairs those on launch.
pub fn state_from_flags(flags: u32) -> InstallState {
    if flags & UNINSTALLING != 0 {
        InstallState::NotInstalled
    } else if flags & UPDATING != 0 {
        InstallState::Updating
    } else if flags & FULLY_INSTALLED != 0 {
        InstallState::Installed
    } else {
        InstallState::NotInstalled
    }
}

/// What a manifest says about a download or staging at one moment. Two of
/// them, taken apart in time, show whether Steam is still making progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub state_flags: u32,
    pub bytes_downloaded: u64,
    pub bytes_staged: u64,
}

impl Progress {
    /// Whether this snapshot shows movement since `earlier`: either byte count
    /// went up, or the flags changed. Staging counts even when nothing
    /// downloads.
    pub fn progressed_since(&self, earlier: &Progress) -> bool {
        self.bytes_downloaded > earlier.bytes_downloaded
            || self.bytes_staged > earlier.bytes_staged
            || self.state_flags != earlier.state_flags
    }
}

pub(crate) fn percent(done: u64, total: u64) -> u8 {
    // Widened so a huge total can't overflow the multiplication.
    u8::try_from((u128::from(done) * 100 / u128::from(total)).min(100)).unwrap_or(100)
}

/// Whether an app is one of Steam's tools rather than a game.
pub fn is_tool(app_id: &str, name: &str) -> bool {
    TOOL_APP_IDS.contains(&app_id)
        || TOOL_NAME_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Installed,
    /// Fully installed, and Steam is updating it.
    Updating,
    NotInstalled,
    /// Not fully installed and Steam is working on it. A paused install looks
    /// the same on disk, with its byte counts dropped to zero. The percent is
    /// None when there are no totals.
    Installing(Option<u8>),
    /// Steam has the install waiting, and hasn't started it.
    InstallQueued,
    Uninstalling,
    /// A library that might hold the game can't be read, so the game may be
    /// there. Carries that library's path.
    Unavailable(PathBuf),
    /// Wineo can't tell. Steam isn't found, or a file can't be read or
    /// parsed. The text is the reason, for people to read.
    Unknown(String),
}

/// The state of one app in the active install. Libraries that are readable
/// are checked first. A game found in one is answered from its manifest, and
/// a game in none of them is `Unavailable` when a library was unreadable.
pub fn app_state(install: Option<&Install>, app_id: &str) -> AppState {
    app_snapshot(install, app_id).0
}

/// Reads state and progress together so launch polling uses one manifest snapshot.
pub(crate) fn app_snapshot(
    install: Option<&Install>,
    app_id: &str,
) -> (AppState, Option<Progress>) {
    let (state, manifest) = app_details(install, app_id);
    (state, manifest.map(|manifest| manifest.progress()))
}

/// The state of one app with the manifest it came from, when there is one.
pub fn app_details(install: Option<&Install>, app_id: &str) -> (AppState, Option<Manifest>) {
    let Some(install) = install else {
        return (AppState::Unknown("Steam isn't found".to_owned()), None);
    };
    if !valid_app_id(app_id) {
        return (
            AppState::Unknown(format!("`{app_id}` isn't a Steam app id")),
            None,
        );
    }

    let listing = libraries(install);
    let mut unavailable = None;
    for library in &listing.entries {
        if !library.available {
            unavailable.get_or_insert_with(|| library.path.clone());
            continue;
        }
        let path = library
            .path
            .join("steamapps")
            .join(format!("appmanifest_{app_id}.acf"));
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return (
                    AppState::Unknown(format!("{} can't be read: {error}", path.display())),
                    None,
                );
            }
        };
        return match parse_manifest(&text, app_id) {
            Ok(manifest) => (manifest.app_state(), Some(manifest)),
            Err(error) => (
                AppState::Unknown(format!("{}: {error}", path.display())),
                None,
            ),
        };
    }

    if let Some(library) = unavailable {
        return (AppState::Unavailable(library), None);
    }
    if let Some(problem) = listing.problem {
        return (AppState::Unknown(problem), None);
    }
    (AppState::NotInstalled, None)
}

/// Steam's cached art for an app, mapped to wineo's kinds. Only files that
/// exist are listed. The image contents aren't checked here.
pub fn art_files(install: &Install, app_id: &str) -> Vec<(art::Kind, PathBuf)> {
    if !valid_app_id(app_id) {
        return Vec::new();
    }
    let dir = install.root.join("appcache/librarycache").join(app_id);
    [
        ("library_600x900.jpg", art::Kind::Grid),
        ("library_hero.jpg", art::Kind::Hero),
        ("logo.png", art::Kind::Logo),
    ]
    .into_iter()
    .map(|(file, kind)| (kind, dir.join(file)))
    .filter(|(_, path)| path.is_file())
    .collect()
}

/// Steam app ids are digits only. Checking first keeps an id from reaching a
/// path as `..` or a slash.
fn valid_app_id(app_id: &str) -> bool {
    all_digits(app_id)
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}
