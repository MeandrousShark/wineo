//! Steam's client log, `logs/console_log.txt`. Its `GameAction` lines say
//! what Steam is doing for a launch, which the game's manifest doesn't:
//!
//! ```text
//! [2026-10-30 11:11:47] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to ProcessingShaderCache with ""
//! ```
//!
//! After `-applaunch`, Steam may process Vulkan shaders for the game before it
//! starts it, for longer than a launch waits.

use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{Install, client_log};

const SHADER_TASK: &str = "ProcessingShaderCache";

/// Where the log stood when a launch was dispatched. Steam rotates its logs
/// when the client starts, so a file with another identity or a shorter
/// length is a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleMark {
    /// Device and inode, or none when there was no log.
    file: Option<(u64, u64)>,
    len: u64,
}

/// The mark of the install's console log as it is now.
pub fn console_mark(install: &Install) -> ConsoleMark {
    mark_of(&log_path(install))
}

/// Whether Steam is processing shaders for `app_id` right now: the newest
/// `LaunchApp` line for it, written after `mark`, says so. A pass an earlier
/// launch left unfinished sits before the mark and doesn't count. A log that
/// can't be read or isn't understood says no, so a launch waits no longer
/// than it did before.
pub fn shader_pass(install: &Install, mark: &ConsoleMark, app_id: &str) -> bool {
    pass_since(&log_path(install), mark, app_id)
}

fn log_path(install: &Install) -> PathBuf {
    install.root().join("logs/console_log.txt")
}

fn mark_of(path: &Path) -> ConsoleMark {
    match std::fs::metadata(path) {
        Ok(metadata) => ConsoleMark {
            file: Some((metadata.dev(), metadata.ino())),
            len: metadata.len(),
        },
        Err(_) => ConsoleMark { file: None, len: 0 },
    }
}

fn pass_since(path: &Path, mark: &ConsoleMark, app_id: &str) -> bool {
    read_since(path, mark).is_some_and(|text| processing_shaders(&text, app_id))
}

fn read_since(path: &Path, mark: &ConsoleMark) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let same = mark.file == Some((metadata.dev(), metadata.ino())) && metadata.len() >= mark.len;
    let floor = if same { mark.len } else { 0 };
    client_log::tail_after(&mut file, metadata.len(), floor).ok()
}

/// Only whole lines count: one Steam is still writing might be cut anywhere.
fn processing_shaders(text: &str, app_id: &str) -> bool {
    let whole = text.rfind('\n').map_or("", |end| &text[..=end]);
    whole
        .lines()
        .rev()
        .find_map(|line| launch_task(client_log::message(line), app_id))
        == Some(SHADER_TASK)
}

/// The task a `LaunchApp changed task to` line for `app_id` names, whatever
/// its ActionID.
fn launch_task<'a>(message: &'a str, app_id: &str) -> Option<&'a str> {
    let rest = message.strip_prefix("GameAction [AppID ")?;
    let (id, rest) = rest.split_once(", ActionID ")?;
    if id != app_id {
        return None;
    }
    let (action, rest) = rest.split_once("] : LaunchApp changed task to ")?;
    if action.is_empty() || !action.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(rest.split_whitespace().next().unwrap_or(""))
}

#[cfg(test)]
mod tests;
