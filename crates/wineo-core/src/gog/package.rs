//! GOG native packages: a `.sh` holding a shell header, MojoSetup, and a zip
//! whose `data/noarch/` entries are the game files.
//!
//! `open` reads and validates the zip's directory without writing anything;
//! `stage` unpacks one product into a staging folder inside the game folder,
//! where publishing can later rename the files into place.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::gog::folder::EntryType;
use crate::paths::Paths;
use crate::units;

mod stage;

pub use stage::{StageProgress, remove_staging, stage, staging_name};
pub(crate) use stage::{open_dir_at, open_folder, open_parent, remove_at};

/// SHA-256 of every GOG `support/postinst.sh` wineo knows how to stand in
/// for. Each only assembles split files.
pub const KNOWN_POSTINST: &[[u8; 32]] = &[
    // RimWorld
    [
        0x6f, 0x91, 0xf1, 0xbb, 0x9b, 0xdc, 0xc3, 0x3d, 0xa5, 0xec, 0x5a, 0x62, 0x79, 0xb5, 0xe7,
        0x27, 0x14, 0x6e, 0xbf, 0x2e, 0xb8, 0x3e, 0xf9, 0xd1, 0xb2, 0x06, 0x36, 0x4a, 0x69, 0x82,
        0x97, 0x56,
    ],
];

const GAME_PREFIX: &[u8] = b"data/noarch/";
const POSTINST: &str = "support/postinst.sh";
const SPLIT_FILES: &str = "support/split_files";
/// Small files read whole while opening: the setup script, the split list,
/// and symlink targets.
const SMALL_LIMIT: u64 = 1024 * 1024;
/// Like Linux's limit on symlinks followed in one lookup.
const MAX_LINK_HOPS: usize = 40;

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("This GOG package has a setup script wineo doesn't support.")]
    UnsupportedScript,
    #[error("This isn't a valid GOG package: {0}.")]
    Invalid(String),
    #[error(
        "Not enough space: needs {}, {} free on {place}.",
        units::size(*needed),
        units::size(*free)
    )]
    NoSpace {
        needed: u64,
        free: u64,
        /// The game folder's parent, with `~` for the home directory.
        place: String,
    },
    #[error("Canceled.")]
    Canceled,
}

fn invalid(detail: impl Into<String>) -> PackageError {
    PackageError::Invalid(detail.into())
}

/// A game entry in a package, relative to the game folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: EntryType,
    /// The declared uncompressed size, for files. Zero otherwise.
    pub size: u64,
    /// Symlinks only.
    pub target: Option<String>,
    /// The mode staging writes: `0o755` or `0o644` for files, `0o755` for
    /// directories, `0o777` for symlinks.
    pub mode: u32,
    /// The zip entry, or None for a parent directory the zip only implies.
    index: Option<usize>,
}

/// A file GOG ships in pieces, joined in order by staging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub path: String,
    /// In byte-wise name order.
    pub pieces: Vec<String>,
    /// The assembled size.
    pub size: u64,
}

/// An opened, validated native package.
#[derive(Debug)]
pub struct Package {
    archive: ZipArchive<File>,
    /// Sorted by path, so parents come before their children.
    entries: Vec<Entry>,
    splits: Vec<Split>,
    unpacked: u64,
    assembled: u64,
}

impl Package {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn splits(&self) -> &[Split] {
        &self.splits
    }

    /// The sum of every game file's declared size, split pieces included.
    pub fn unpacked_size(&self) -> u64 {
        self.unpacked
    }

    /// The extra space assembled split files take while their pieces exist.
    pub fn assembled_size(&self) -> u64 {
        self.assembled
    }

    /// The declared size of a file entry.
    fn file_size(&self, path: &str) -> Option<u64> {
        self.entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .ok()
            .map(|index| &self.entries[index])
            .filter(|entry| entry.kind == EntryType::File)
            .map(|entry| entry.size)
    }
}

/// Sums sizes, refusing a total that doesn't fit, since a lying ZIP64 size
/// could otherwise wrap around to something small.
fn checked_sum(sizes: impl IntoIterator<Item = u64>) -> Result<u64, PackageError> {
    sizes.into_iter().try_fold(0u64, |sum, size| {
        sum.checked_add(size)
            .ok_or_else(|| invalid("its sizes are too large"))
    })
}

/// Reads and validates a native package. Nothing is written.
pub fn open(path: &Path) -> Result<Package> {
    open_with(path, KNOWN_POSTINST)
}

/// `open` with the list of known setup scripts supplied, so tests can use
/// their own.
pub(crate) fn open_with(path: &Path, known: &[[u8; 32]]) -> Result<Package> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let walk = file
        .try_clone()
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|error| invalid(error.to_string()))?;
    check_raw_names(walk, &archive)?;

    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let zipped = archive
            .by_index_raw(index)
            .map_err(|error| invalid(error.to_string()))?;
        let Some(rest) = zipped.name_raw().strip_prefix(GAME_PREFIX) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let name = std::str::from_utf8(rest).map_err(|_| invalid("a path isn't UTF-8"))?;
        if zipped.encrypted() {
            return Err(invalid(format!("{name} is encrypted")).into());
        }
        let (name, marked_dir) = match name.strip_suffix('/') {
            Some(name) => (name, true),
            None => (name, false),
        };
        check_path(name)?;
        let unix = zipped.unix_mode();
        let kind = match (unix.map(|mode| mode & S_IFMT), marked_dir) {
            (None | Some(0) | Some(S_IFDIR), true) | (Some(S_IFDIR), false) => EntryType::Directory,
            (None | Some(0) | Some(S_IFREG), false) => EntryType::File,
            (Some(S_IFLNK), false) => EntryType::Symlink,
            _ => return Err(invalid(format!("{name} isn't a file, folder, or symlink")).into()),
        };
        let mode = match kind {
            EntryType::Directory => 0o755,
            EntryType::Symlink => 0o777,
            EntryType::File if unix.unwrap_or(0) & 0o111 != 0 => 0o755,
            EntryType::File => 0o644,
        };
        let entry = Entry {
            path: name.to_owned(),
            kind,
            size: if kind == EntryType::File {
                zipped.size()
            } else {
                0
            },
            target: None,
            mode,
            index: Some(index),
        };
        if entries.insert(name.to_owned(), entry).is_some() {
            return Err(invalid(format!("{name} appears twice")).into());
        }
    }

    add_parents(&mut entries)?;

    let links: Vec<(String, usize)> = entries
        .values()
        .filter(|entry| entry.kind == EntryType::Symlink)
        .map(|entry| (entry.path.clone(), entry.index.expect("zip entry")))
        .collect();
    for (path, index) in links {
        let bytes = read_small(&mut archive, index, &path)?;
        let target = String::from_utf8(bytes)
            .map_err(|_| invalid(format!("{path}'s target isn't UTF-8")))?;
        if target.is_empty() || target.starts_with('/') || target.contains('\0') {
            return Err(invalid(format!("{path} points outside the game")).into());
        }
        entries.get_mut(&path).expect("listed").target = Some(target);
    }
    check_links(&entries)?;

    let splits = match entries.get(POSTINST) {
        None => Vec::new(),
        Some(script) => {
            let index = match (script.kind, script.index) {
                (EntryType::File, Some(index)) => index,
                _ => return Err(PackageError::UnsupportedScript.into()),
            };
            if script.size > SMALL_LIMIT {
                return Err(PackageError::UnsupportedScript.into());
            }
            let bytes = read_small(&mut archive, index, POSTINST)?;
            let hash: [u8; 32] = Sha256::digest(&bytes).into();
            if !known.contains(&hash) {
                return Err(PackageError::UnsupportedScript.into());
            }
            split_files(&mut archive, &entries)?
        }
    };

    let unpacked = checked_sum(
        entries
            .values()
            .filter(|entry| entry.kind == EntryType::File)
            .map(|entry| entry.size),
    )?;
    let assembled = checked_sum(splits.iter().map(|split| split.size))?;
    // Staging counts both in one progress total.
    checked_sum([unpacked, assembled])?;
    Ok(Package {
        archive,
        entries: entries.into_values().collect(),
        splits,
        unpacked,
        assembled,
    })
}

/// The zip crate keys entries by name, so an exact duplicate would silently
/// hide one entry. Walks the central directory to see every name.
fn check_raw_names(file: File, archive: &ZipArchive<File>) -> Result<()> {
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(archive.central_directory_start()))?;
    let mut seen = HashSet::new();
    let mut count = 0;
    loop {
        let mut header = [0u8; 46];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        if header[0..4] != [0x50, 0x4b, 0x01, 0x02] {
            break;
        }
        let field = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
        let mut name = vec![0; usize::from(field(28))];
        reader.read_exact(&mut name)?;
        reader.seek_relative(i64::from(field(30)) + i64::from(field(32)))?;
        if name.starts_with(GAME_PREFIX) && !seen.insert(name.clone()) {
            let shown = String::from_utf8_lossy(&name[GAME_PREFIX.len()..]).into_owned();
            return Err(invalid(format!("{shown} appears twice")).into());
        }
        count += 1;
    }
    if count < archive.len() {
        return Err(invalid("its zip directory is damaged").into());
    }
    Ok(())
}

/// Relative, normalized, and free of characters wineo can't record.
fn check_path(path: &str) -> Result<(), PackageError> {
    let bad = path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if bad {
        return Err(invalid(format!("{path:?} isn't a safe path")));
    }
    Ok(())
}

fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

/// Adds folders the zip only implies, and refuses entries beneath a file or
/// symlink.
fn add_parents(entries: &mut BTreeMap<String, Entry>) -> Result<(), PackageError> {
    let paths: Vec<String> = entries.keys().cloned().collect();
    for path in paths {
        let mut current = path.as_str();
        while let Some(parent) = parent_of(current) {
            match entries.get(parent).map(|entry| entry.kind) {
                Some(EntryType::Directory) => {}
                Some(EntryType::Symlink) => {
                    return Err(invalid(format!("{path} is beneath the symlink {parent}")));
                }
                Some(EntryType::File) => {
                    return Err(invalid(format!("{path} is beneath the file {parent}")));
                }
                None => {
                    entries.insert(
                        parent.to_owned(),
                        Entry {
                            path: parent.to_owned(),
                            kind: EntryType::Directory,
                            size: 0,
                            target: None,
                            mode: 0o755,
                            index: None,
                        },
                    );
                }
            }
            current = parent;
        }
    }
    Ok(())
}

/// Every symlink must resolve inside the game, following the package's other
/// symlinks the way the kernel would.
fn check_links(entries: &BTreeMap<String, Entry>) -> Result<(), PackageError> {
    let links: BTreeMap<&str, &str> = entries
        .values()
        .filter_map(|entry| Some((entry.path.as_str(), entry.target.as_deref()?)))
        .collect();
    check_link_targets(&links)
}

/// Every link, by its path in the game, must resolve inside the game,
/// following the others the way the kernel would.
pub(crate) fn check_link_targets(links: &BTreeMap<&str, &str>) -> Result<(), PackageError> {
    for (&path, &target) in links {
        let mut resolver = Resolver {
            links,
            active: vec![path],
            hops: 0,
            link: path,
        };
        let base = parent_of(path)
            .map(|parent| parent.split('/').collect())
            .unwrap_or_default();
        resolver.walk(base, target)?;
    }
    Ok(())
}

struct Resolver<'a> {
    links: &'a BTreeMap<&'a str, &'a str>,
    /// Links being expanded right now; meeting one again is a cycle.
    active: Vec<&'a str>,
    hops: usize,
    /// The link being checked, for messages.
    link: &'a str,
}

impl<'a> Resolver<'a> {
    fn walk(
        &mut self,
        mut at: Vec<&'a str>,
        target: &'a str,
    ) -> Result<Vec<&'a str>, PackageError> {
        for part in target.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if at.pop().is_none() {
                        return Err(invalid(format!("{} points outside the game", self.link)));
                    }
                }
                name => {
                    at.push(name);
                    let joined = at.join("/");
                    let Some((&key, &next)) = self.links.get_key_value(joined.as_str()) else {
                        continue;
                    };
                    self.hops += 1;
                    if self.active.contains(&key) || self.hops > MAX_LINK_HOPS {
                        return Err(invalid(format!("{} is a symlink loop", self.link)));
                    }
                    at.pop();
                    self.active.push(key);
                    at = self.walk(at, next)?;
                    self.active.pop();
                }
            }
        }
        Ok(at)
    }
}

/// Reads a small entry whole, checking its real size against the declared
/// one.
fn read_small(archive: &mut ZipArchive<File>, index: usize, path: &str) -> Result<Vec<u8>> {
    let zipped = archive
        .by_index(index)
        .map_err(|error| invalid(error.to_string()))?;
    let declared = zipped.size();
    if declared > SMALL_LIMIT {
        return Err(invalid(format!("{path} is too large")).into());
    }
    let mut bytes = Vec::new();
    zipped
        .take(declared + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {path} from the package"))?;
    if bytes.len() as u64 != declared {
        return Err(invalid(format!("{path}'s size doesn't match")).into());
    }
    Ok(bytes)
}

/// Reads `support/split_files` the way the known script does, plus a last
/// line with no trailing newline.
fn split_files(
    archive: &mut ZipArchive<File>,
    entries: &BTreeMap<String, Entry>,
) -> Result<Vec<Split>> {
    let Some(list) = entries.get(SPLIT_FILES) else {
        return Ok(Vec::new());
    };
    let index = match (list.kind, list.index) {
        (EntryType::File, Some(index)) => index,
        _ => return Err(invalid(format!("{SPLIT_FILES} isn't a file")).into()),
    };
    let bytes = read_small(archive, index, SPLIT_FILES)?;
    let text =
        String::from_utf8(bytes).map_err(|_| invalid(format!("{SPLIT_FILES} isn't UTF-8")))?;

    let mut splits = Vec::new();
    let mut claimed = BTreeSet::new();
    for line in text.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        check_path(path)?;
        if let Some(entry) = entries.get(path)
            && entry.kind != EntryType::File
        {
            return Err(invalid(format!("{path} isn't a file")).into());
        }
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (Some(dir), name),
            None => (None, path),
        };
        let prefix = format!("{name}.split");
        let mut pieces = Vec::new();
        let mut size = 0;
        for entry in entries.values() {
            let (entry_dir, entry_name) = match entry.path.rsplit_once('/') {
                Some((dir, name)) => (Some(dir), name),
                None => (None, entry.path.as_str()),
            };
            if entry_dir != dir || !entry_name.starts_with(&prefix) {
                continue;
            }
            if entry.kind != EntryType::File {
                return Err(invalid(format!("{} isn't a file", entry.path)).into());
            }
            pieces.push(entry.path.clone());
            size = checked_sum([size, entry.size])?;
        }
        if pieces.is_empty() {
            return Err(invalid(format!("{path} has no pieces")).into());
        }
        // BTreeMap order is already byte-wise; sort anyway so it never
        // depends on that.
        pieces.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        if !claimed.insert(path.to_owned()) {
            return Err(invalid(format!("{path} is listed twice")).into());
        }
        for piece in &pieces {
            if !claimed.insert(piece.clone()) {
                return Err(invalid(format!("{piece} is used twice")).into());
            }
        }
        splits.push(Split {
            path: path.to_owned(),
            pieces,
            size,
        });
    }
    Ok(splits)
}

/// What the packages need free on the game folder's filesystem, in terms
/// kept separate so updates can adjust them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpaceNeeded {
    /// Every package's files unpacked into staging.
    pub unpacked: u64,
    /// Assembled split files, written while their pieces still exist.
    pub assembled: u64,
}

impl SpaceNeeded {
    pub fn total(&self) -> u64 {
        self.unpacked.saturating_add(self.assembled)
    }
}

pub fn space_needed(packages: &[&Package]) -> Result<SpaceNeeded, PackageError> {
    let needed = SpaceNeeded {
        unpacked: checked_sum(packages.iter().map(|package| package.unpacked_size()))?,
        assembled: checked_sum(packages.iter().map(|package| package.assembled_size()))?,
    };
    checked_sum([needed.unpacked, needed.assembled])?;
    Ok(needed)
}

/// Refuses when the game folder's filesystem can't hold the packages. A
/// folder a fresh install hasn't created yet is judged by its nearest
/// existing parent.
pub fn check_space(paths: &Paths, folder: &Path, packages: &[&Package]) -> Result<()> {
    check_space_with(paths, folder, space_needed(packages)?, free_space(folder)?)?;
    Ok(())
}

/// The space free to unprivileged users on the filesystem `path` is on, or
/// would be on once created.
pub fn free_space(path: &Path) -> Result<u64> {
    let probe = existing(path);
    let stat = rustix::fs::statvfs(probe)
        .with_context(|| format!("failed to check free space on {}", probe.display()))?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

/// `path`, or its nearest parent that exists.
pub fn existing(path: &Path) -> &Path {
    let mut current = path;
    while fs::symlink_metadata(current).is_err() {
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    current
}

pub(crate) fn check_space_with(
    paths: &Paths,
    folder: &Path,
    needed: SpaceNeeded,
    free: u64,
) -> Result<(), PackageError> {
    if needed.total() <= free {
        return Ok(());
    }
    let shown = folder.parent().unwrap_or(folder);
    Err(PackageError::NoSpace {
        needed: needed.total(),
        free,
        place: paths
            .contract(shown)
            .unwrap_or_else(|_| shown.display().to_string()),
    })
}

#[cfg(test)]
mod tests;
