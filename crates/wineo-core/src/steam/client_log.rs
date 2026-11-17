//! The Steam bootstrapper's log, `logs/bootstrap_log.txt`, which says when
//! the client is updating itself on start. Each start writes a block that
//! begins with a `Startup - updater built` line. A startup update ends its
//! block with `Update complete, launching Steam...`, and the updated client
//! writes a new block.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::Install;

/// Only the end of the log is read. A download writes a line of about 66
/// bytes every 300 ms, so this holds the start of a block through about 40
/// minutes of one, past the 30 minutes a launch waits on the client at most
/// (`CLIENT_WAIT_CAP` in the supervisor).
const TAIL: u64 = 512 * 1024;

const BLOCK_START: &str = "Startup - updater built";

/// A startup update of the Steam client in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientUpdate {
    pub phase: ClientUpdatePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientUpdatePhase {
    /// Looking for an update, or listing what to download.
    Checking,
    /// Downloading, in kilobytes.
    Downloading { done_kb: u64, total_kb: u64 },
    /// Installing a downloaded update.
    Installing,
}

/// The startup update the newest block of the install's bootstrapper log
/// shows, if any. Steam logs often while it updates, so a log last written
/// `silence` ago or longer shows none: that update is stuck, or Steam was
/// killed during it. The file's own time makes every process that asks agree
/// on when it stopped. A log that can't be read or isn't understood shows
/// none.
pub fn client_update(install: &Install, silence: Duration) -> Option<ClientUpdate> {
    let (tail, modified) = read_tail(&install.root().join("logs/bootstrap_log.txt")).ok()?;
    if !written_within(modified, SystemTime::now(), silence) {
        return None;
    }
    newest_block_update(&tail).map(|phase| ClientUpdate { phase })
}

/// A time ahead of `now` counts as just now.
fn written_within(modified: SystemTime, now: SystemTime, limit: Duration) -> bool {
    now.duration_since(modified).map_or(true, |age| age < limit)
}

/// The last `TAIL` bytes of whole lines, and when the file was last written.
fn read_tail(path: &Path) -> io::Result<(String, SystemTime)> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();
    let modified = metadata.modified()?;
    // One byte more, so a cut that lands just after a newline keeps its line.
    let start = len.saturating_sub(TAIL + 1);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(len - start).read_to_end(&mut bytes)?;
    let whole = if start > 0 {
        match bytes.iter().position(|byte| *byte == b'\n') {
            Some(newline) => &bytes[newline + 1..],
            None => &[],
        }
    } else {
        &bytes[..]
    };
    Ok((String::from_utf8_lossy(whole).into_owned(), modified))
}

/// Only the part of a block before the client starts normally counts: a
/// download the background loop starts later applies at the next start.
fn newest_block_update(text: &str) -> Option<ClientUpdatePhase> {
    let messages: Vec<&str> = text.lines().map(message).collect();
    let start = messages
        .iter()
        .rposition(|message| message.starts_with(BLOCK_START))?;
    let mut phase = None;
    for message in &messages[start + 1..] {
        if ends_update(message) {
            return None;
        }
        if *message == "Checking for update on startup" {
            phase = Some(ClientUpdatePhase::Checking);
        } else if let Some((done_kb, total_kb)) = download_progress(message) {
            phase = Some(ClientUpdatePhase::Downloading { done_kb, total_kb });
        } else if matches!(
            *message,
            "Found pending update"
                | "Installing update..."
                | "Extracting package..."
                | "Cleaning up..."
        ) {
            phase = Some(ClientUpdatePhase::Installing);
        }
    }
    phase
}

/// A line that means the block's client is past any startup update.
fn ends_update(message: &str) -> bool {
    matches!(
        message,
        "Update complete, launching Steam..."
            | "Verification complete"
            | "Nothing to do"
            | "Shutdown"
    ) || message.starts_with("Background update loop")
        || message.starts_with("Download skipped")
}

/// A line without its `[date time] ` stamp.
fn message(line: &str) -> &str {
    let line = line.trim_end_matches('\r');
    line.strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map_or(line, |(_, message)| message)
}

/// `Downloading update (60,732 of 410,553 KB)...`
fn download_progress(message: &str) -> Option<(u64, u64)> {
    let (done, total) = message
        .strip_prefix("Downloading update (")?
        .strip_suffix(" KB)...")?
        .split_once(" of ")?;
    Some((kilobytes(done)?, kilobytes(total)?))
}

fn kilobytes(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|c| *c != ',').collect();
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests;
