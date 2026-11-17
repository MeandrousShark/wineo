//! Finding the games in a folder: files with the emulator's extensions and
//! its folder games, less the files a `.cue` or `.m3u` already covers.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::emulator::FolderGame;
use crate::paths::resolve;

/// What a scan looks for.
pub(super) struct Rules<'a> {
    /// Lowercase, without the dot.
    pub extensions: &'a [String],
    pub folder: Option<FolderGame>,
}

impl Rules<'_> {
    fn lists(&self, extension: &str) -> bool {
        self.extensions.iter().any(|known| known == extension)
    }

    fn matches(&self, path: &Path) -> bool {
        extension(path).is_some_and(|extension| self.lists(&extension))
    }
}

/// A file's extension, lowercase.
pub(super) fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

#[derive(Debug, Default)]
pub(super) struct Scanned {
    /// Every game, a file or a folder game's folder, in no set order.
    pub games: Vec<PathBuf>,
    /// Subfolders that couldn't be read.
    pub skipped: Vec<PathBuf>,
}

/// Whether `dir` is a folder game of `kind`.
pub(super) fn is_folder_game(dir: &Path, kind: FolderGame) -> bool {
    match kind {
        FolderGame::CemuTitle => {
            dir.join("content").is_dir()
                && dir.join("meta").is_dir()
                && fs::read_dir(dir.join("code")).is_ok_and(|entries| {
                    entries.flatten().any(|entry| {
                        extension(&entry.path()).as_deref() == Some("rpx") && entry.path().is_file()
                    })
                })
        }
        FolderGame::PpssppEboot => fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name().eq_ignore_ascii_case("EBOOT.PBP") && entry.path().is_file()
            })
        }),
        FolderGame::Rpcs3Disc => dir.join("PS3_GAME/USRDIR/EBOOT.BIN").is_file(),
    }
}

/// The games under `root`, a folder that can be read and isn't a folder game
/// itself. Links are followed, each real folder is read once, so a link
/// back up can't loop, and a folder game is never looked inside.
pub(super) fn scan(root: &Path, rules: &Rules<'_>) -> Scanned {
    let mut scanned = Scanned::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        if let Ok(real) = folder.canonicalize()
            && !seen.insert(real)
        {
            continue;
        }
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(_) => {
                scanned.skipped.push(folder);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Followed, so a link counts as what it leads to; a broken one
            // is nothing.
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if rules.folder.is_some_and(|kind| is_folder_game(&path, kind)) {
                    scanned.games.push(path);
                } else {
                    folders.push(path);
                }
            } else if metadata.is_file() && rules.matches(&path) {
                scanned.games.push(path);
            }
        }
    }
    hide_covered(&mut scanned.games, rules);
    scanned
}

/// The most of a `.cue` or `.m3u` that is read. Real ones are a few lines,
/// so anything longer is something else with that name.
const SHEET_LIMIT: u64 = 1 << 20;

fn read_sheet(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(SHEET_LIMIT)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The files a cue sheet's `FILE` lines name, relative to its folder.
pub(super) fn cue_files(cue: &Path) -> Vec<PathBuf> {
    let Some(text) = read_sheet(cue) else {
        return Vec::new();
    };
    let dir = cue.parent().unwrap_or(Path::new(""));
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let keyword = line.get(..4)?;
            if !keyword.eq_ignore_ascii_case("FILE") {
                return None;
            }
            let rest = line[4..].strip_prefix(char::is_whitespace)?.trim_start();
            let name = match rest.strip_prefix('"') {
                Some(quoted) => &quoted[..quoted.find('"')?],
                // Unquoted, the name is everything before the file type.
                None => rest
                    .rsplit_once(char::is_whitespace)
                    .map_or(rest, |(name, _)| name),
            };
            (!name.trim().is_empty()).then(|| dir.join(name.trim()))
        })
        .collect()
}

/// The files a playlist names, one per line relative to its folder, with
/// blank lines and `#` comments skipped.
pub(super) fn m3u_files(m3u: &Path) -> Vec<PathBuf> {
    let Some(text) = read_sheet(m3u) else {
        return Vec::new();
    };
    let dir = m3u.parent().unwrap_or(Path::new(""));
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| dir.join(line))
        .collect()
}

/// Drops what a listed `.cue` or `.m3u` names, so each game shows once by
/// its sheet. What a playlist names hides what that names in turn, so a
/// playlist of cue sheets also hides their tracks.
fn hide_covered(games: &mut Vec<PathBuf>, rules: &Rules<'_>) {
    let cue = rules.lists("cue");
    let m3u = rules.lists("m3u");
    if !cue && !m3u {
        return;
    }
    let mut hidden: HashSet<PathBuf> = HashSet::new();
    for game in games.iter() {
        match extension(game).as_deref() {
            Some("cue") if cue => hidden.extend(cue_files(game).iter().map(|file| resolve(file))),
            Some("m3u") if m3u => {
                for file in m3u_files(game) {
                    if extension(&file).as_deref() == Some("cue") {
                        hidden.extend(cue_files(&file).iter().map(|track| resolve(track)));
                    }
                    hidden.insert(resolve(&file));
                }
            }
            _ => {}
        }
    }
    if !hidden.is_empty() {
        games.retain(|game| !hidden.contains(&resolve(game)));
    }
}
