//! Where a new GOG game's folder goes.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::folder::FolderError;

/// A GOG title as a single folder name: path separators, control characters,
/// and invisible direction and format characters (an RTL override could make
/// the name display as something else) removed, then trailing dots and
/// spaces. `\` goes too, since Inno gets the folder as a Windows path. A title with nothing left, like
/// `...`, is an error, since any stand-in would be a guess; `--dir` is the way
/// around it.
pub fn safe_folder_name(title: &str) -> Result<String, FolderError> {
    let kept: String = title
        .chars()
        .filter(|c| !matches!(c, '/' | '\\') && !c.is_control() && !is_format(*c))
        .collect();
    let name = kept.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return Err(FolderError::UnusableTitle {
            title: title.to_owned(),
        });
    }
    Ok(name.to_owned())
}

/// Every character of Unicode general category Cf (format), as of Unicode
/// 16.0. Bidi controls, joiners, invisible operators, and tag characters
/// change how a name displays or joins and have no place in a folder name.
const FORMAT_CHARS: &[RangeInclusive<char>] = &[
    '\u{ad}'..='\u{ad}',
    '\u{600}'..='\u{605}',
    '\u{61c}'..='\u{61c}',
    '\u{6dd}'..='\u{6dd}',
    '\u{70f}'..='\u{70f}',
    '\u{890}'..='\u{891}',
    '\u{8e2}'..='\u{8e2}',
    '\u{180e}'..='\u{180e}',
    '\u{200b}'..='\u{200f}',
    '\u{202a}'..='\u{202e}',
    '\u{2060}'..='\u{2064}',
    '\u{2066}'..='\u{206f}',
    '\u{feff}'..='\u{feff}',
    '\u{fff9}'..='\u{fffb}',
    '\u{110bd}'..='\u{110bd}',
    '\u{110cd}'..='\u{110cd}',
    '\u{13430}'..='\u{1343f}',
    '\u{1bca0}'..='\u{1bca3}',
    '\u{1d173}'..='\u{1d17a}',
    '\u{e0001}'..='\u{e0001}',
    '\u{e0020}'..='\u{e007f}',
];

fn is_format(c: char) -> bool {
    FORMAT_CHARS.iter().any(|range| range.contains(&c))
}

/// Where a game's folder goes: `dir` when given, else the title under
/// `games_root`. Neither is created or checked here.
pub fn install_folder(games_root: &Path, title: &str, dir: Option<&Path>) -> Result<PathBuf> {
    match dir {
        Some(dir) => Ok(dir.to_owned()),
        None => Ok(games_root.join(safe_folder_name(title)?)),
    }
}

#[cfg(test)]
mod tests;
