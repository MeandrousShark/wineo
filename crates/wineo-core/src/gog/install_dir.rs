//! Where a new GOG game's folder goes.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::folder::FolderError;

/// A GOG title as a single folder name: path separators and control
/// characters removed, then trailing dots and spaces. `\` goes too, since
/// Inno gets the folder as a Windows path. A title with nothing left, like
/// `...`, is an error, since any stand-in would be a guess; `--dir` is the way
/// around it.
pub fn safe_folder_name(title: &str) -> Result<String, FolderError> {
    let kept: String = title
        .chars()
        .filter(|c| !matches!(c, '/' | '\\') && !c.is_control())
        .collect();
    let name = kept.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return Err(FolderError::UnusableTitle {
            title: title.to_owned(),
        });
    }
    Ok(name.to_owned())
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
