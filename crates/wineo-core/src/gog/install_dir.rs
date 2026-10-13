//! Where a new GOG game's folder goes.

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

/// Unicode format characters (category Cf) that change how text displays or
/// joins, which a folder name has no use for.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
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
