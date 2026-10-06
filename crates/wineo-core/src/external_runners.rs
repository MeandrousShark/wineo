//! Proton and Wine builds that Steam, Heroic, and Lutris installed. wineo
//! reads them in place and never changes or deletes them.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::heroic::{HeroicOrigin, HeroicSource};
use crate::name;
use crate::paths::Paths;
use crate::runner::Runner;

const STEAM_FLATPAK: &str = "com.valvesoftware.Steam";
const LUTRIS_FLATPAK: &str = "net.lutris.Lutris";

/// Where a runner lives. The order is the priority when names collide.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    #[default]
    Wineo,
    Steam,
    Heroic,
    Lutris,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Wineo => "wineo",
            Source::Steam => "Steam",
            Source::Heroic => "Heroic",
            Source::Lutris => "Lutris",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A folder another launcher keeps its builds in.
#[derive(Debug, Clone)]
pub struct Folder {
    pub source: Source,
    pub path: PathBuf,
    /// A Steam library's `steamapps/common` mostly holds games, and Valve's
    /// runtimes sit there under `Proton` names too, so only folders named
    /// `Proton...` that really are Proton count there.
    proton_only: bool,
}

/// Every existing folder that may hold builds, in priority order, with
/// repeats dropped by canonical path since launchers symlink into each other.
pub fn folders(paths: &Paths) -> Vec<Folder> {
    let home = paths.home();
    let mut candidates = Vec::new();

    let steam_roots = [
        home.join(".steam/root"),
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".var/app")
            .join(STEAM_FLATPAK)
            .join(".local/share/Steam"),
    ];
    for root in &steam_roots {
        candidates.push((Source::Steam, root.join("compatibilitytools.d"), false));
    }
    for root in &steam_roots {
        for library in steam_libraries(root) {
            candidates.push((Source::Steam, library.join("steamapps/common"), true));
        }
    }

    let mut heroic = HeroicSource::discover(home, paths.xdg_config(), Some(paths.xdg_data()))
        .unwrap_or_default();
    heroic.sort_by_key(|source| source.origin() != HeroicOrigin::Native);
    for source in &heroic {
        for kind in ["proton", "wine"] {
            candidates.push((
                Source::Heroic,
                source.config_dir().join("tools").join(kind),
                false,
            ));
        }
    }

    candidates.push((
        Source::Lutris,
        paths.xdg_data().join("lutris/runners/wine"),
        false,
    ));
    candidates.push((
        Source::Lutris,
        home.join(".var/app")
            .join(LUTRIS_FLATPAK)
            .join("data/lutris/runners/wine"),
        false,
    ));

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|(_, path, _)| {
            fs::canonicalize(path)
                .is_ok_and(|canonical| canonical.is_dir() && seen.insert(canonical))
        })
        .map(|(source, path, proton_only)| Folder {
            source,
            path,
            proton_only,
        })
        .collect()
}

/// The launcher whose folder holds `root` or is `root` itself, after resolving
/// symlinks. Builds in such a root launch as wineo's, but they're really that
/// launcher's files, so removal must leave them alone. This doesn't depend on
/// the `external_runners` setting. A portable folder's root is the portable
/// folder's own, and a root that only contains a launcher folder is unaffected.
pub(crate) fn root_owner(paths: &Paths, root: &Path) -> Option<Source> {
    if paths.portable_root().is_some() {
        return None;
    }
    let root = fs::canonicalize(root).ok()?;
    folders(paths)
        .into_iter()
        .find(|folder| fs::canonicalize(&folder.path).is_ok_and(|path| root.starts_with(path)))
        .map(|folder| folder.source)
}

/// The Steam root itself, plus every library `libraryfolders.vdf` names.
fn steam_libraries(root: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![root.to_owned()];
    if let Ok(text) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        libraries.extend(vdf_paths(&text).into_iter().map(PathBuf::from));
    }
    libraries
}

/// Every `"path"` value in a VDF file. The structure around them doesn't
/// matter, since each library entry has exactly one.
fn vdf_paths(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '"' {
            continue;
        }
        let mut token = String::new();
        while let Some(character) = chars.next() {
            match character {
                '"' => break,
                '\\' => token.extend(chars.next()),
                other => token.push(other),
            }
        }
        tokens.push(token);
    }
    tokens
        .windows(2)
        .filter(|pair| pair[0].eq_ignore_ascii_case("path"))
        .map(|pair| pair[1].clone())
        .collect()
}

/// A runner name for a folder, so `Proton 9.0` can be `Proton-9.0`. A name
/// that is already usable is kept as is. Otherwise every run of characters
/// wineo doesn't allow, along with the dashes around it, becomes one `-`.
/// Empty when nothing usable is left.
pub fn derived_name(folder: &str) -> String {
    if name::is_valid_runner(folder) {
        return folder.to_owned();
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.';
    let mut out = String::new();
    let mut gap = String::new();
    let mut replaced = false;
    for character in folder.chars() {
        if allowed(character) {
            if replaced {
                if !out.is_empty() {
                    out.push('-');
                }
            } else {
                out.push_str(&gap);
            }
            gap.clear();
            replaced = false;
            out.push(character);
        } else {
            replaced |= character != '-';
            gap.push(character);
        }
    }
    if !replaced {
        out.push_str(&gap);
    }
    let out = out.trim_start_matches(['.', '-']);
    if name::is_valid_runner(out) {
        out.to_owned()
    } else {
        String::new()
    }
}

/// A folder inside one of the launcher folders, not yet checked.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub source: Source,
    /// The derived runner name, empty when the folder name has nothing usable.
    pub name: String,
    pub directory: PathBuf,
    /// Counts only as a working Proton build, and isn't worth reporting otherwise.
    pub proton_only: bool,
}

/// Every candidate folder in priority order. Within a folder, one whose name
/// needed no deriving comes before one that derives to the same name.
pub(crate) fn entries(folders: &[Folder]) -> Vec<Entry> {
    let mut entries = Vec::new();
    for folder in folders {
        let Ok(read) = fs::read_dir(&folder.path) else {
            continue;
        };
        let mut found: Vec<(bool, String, Entry)> = Vec::new();
        for entry in read.flatten() {
            let directory = entry.path();
            let Some(raw) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // Hidden folders are launchers' scratch space, not builds.
            if raw.starts_with('.') || !directory.is_dir() {
                continue;
            }
            if folder.proton_only && !raw.starts_with("Proton") {
                continue;
            }
            let name = derived_name(&raw);
            found.push((
                name != raw,
                raw,
                Entry {
                    source: folder.source,
                    name,
                    directory,
                    proton_only: folder.proton_only,
                },
            ));
        }
        found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        entries.extend(found.into_iter().map(|(_, _, entry)| entry));
    }
    entries
}

/// Whether two same-named builds are the same build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    Same,
    Different,
    /// At least one has nothing cheap to compare, so it's never called different.
    Unknown,
}

/// A cheap stand-in for a build's contents. Hashing whole trees is too slow
/// for something a listing does.
#[derive(Debug, PartialEq, Eq)]
enum Identity {
    /// Proton's `version` file, `<timestamp> <build name>`.
    Proton(String),
    /// Size and SHA-256 of `bin/wine` and `ntdll.so`, a few MB at most.
    Wine(Vec<(u64, [u8; 32])>),
}

pub(crate) fn compare(a: &Runner, b: &Runner) -> Comparison {
    let (Some(left), Some(right)) = (a.directory(), b.directory()) else {
        return Comparison::Unknown;
    };
    if let (Ok(l), Ok(r)) = (fs::canonicalize(left), fs::canonicalize(right))
        && l == r
    {
        return Comparison::Same;
    }
    match (
        identity(left, a.is_proton()),
        identity(right, b.is_proton()),
    ) {
        (Some(l), Some(r)) if l == r => Comparison::Same,
        (Some(_), Some(_)) => Comparison::Different,
        _ => Comparison::Unknown,
    }
}

fn identity(dir: &Path, proton: bool) -> Option<Identity> {
    if proton {
        let version = fs::read_to_string(dir.join("version")).ok()?;
        let version = version.trim();
        return (!version.is_empty()).then(|| Identity::Proton(version.to_owned()));
    }
    let ntdll = [
        "lib/wine/x86_64-unix/ntdll.so",
        "lib64/wine/x86_64-unix/ntdll.so",
    ]
    .iter()
    .map(|path| dir.join(path))
    .find(|path| path.is_file())?;
    let mut parts = Vec::new();
    for path in [dir.join("bin/wine"), ntdll] {
        let bytes = fs::read(path).ok()?;
        parts.push((bytes.len() as u64, Sha256::digest(&bytes).into()));
    }
    Some(Identity::Wine(parts))
}

#[cfg(test)]
mod tests;
