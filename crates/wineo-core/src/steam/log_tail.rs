//! What Steam's log files have in common: lines of `[date time] message`,
//! and a reader that follows one as it grows.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// A line without its `[date time] ` stamp.
pub(super) fn message(line: &str) -> &str {
    let line = line.trim_end_matches('\r');
    line.strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map_or(line, |(_, message)| message)
}

/// How much one read takes, so a log that grew by a lot between looks is
/// read in pieces.
const CHUNK: usize = 1 << 20;

/// What a read of a followed log finds.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event<'a> {
    /// The log is a different file now, or was cut short, and is read from
    /// its start. Whatever was learned from the old one is stale.
    Restarted,
    /// A line that was complete when read, without its line ending.
    Line(&'a str),
}

/// A position in a log, taken at some moment, from which every later read
/// gives only the whole lines written since. The log's identity is kept
/// across reads, so a replaced file is noticed however big it has grown by
/// the next look. A file truncated in place is noticed only while it's
/// shorter than what was already read; Steam replaces its logs instead.
pub(super) struct Follower {
    path: PathBuf,
    /// Device and inode, or none when there was no log.
    file: Option<(u64, u64)>,
    /// Where the next read starts.
    offset: u64,
    /// Whether `offset` is known to start a line. A position taken at some
    /// moment may land inside one Steam was writing.
    aligned: bool,
}

impl Follower {
    /// Follows the log from its end as it is now.
    pub(super) fn at_end(path: PathBuf) -> Self {
        match fs::metadata(&path) {
            Ok(metadata) => Self {
                path,
                file: Some((metadata.dev(), metadata.ino())),
                offset: metadata.len(),
                aligned: metadata.len() == 0,
            },
            Err(_) => Self::unseen(path),
        }
    }

    fn unseen(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            offset: 0,
            aligned: true,
        }
    }

    /// Reports the whole lines written since the last read. An error leaves
    /// the position where it was. A log that is gone is `Restarted` and read
    /// from its start if it comes back.
    pub(super) fn read(&mut self, mut event: impl FnMut(Event)) -> io::Result<()> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if self.file.is_some() {
                    event(Event::Restarted);
                }
                *self = Self::unseen(std::mem::take(&mut self.path));
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        let metadata = file.metadata()?;
        let identity = (metadata.dev(), metadata.ino());
        if self.file != Some(identity) || metadata.len() < self.offset {
            event(Event::Restarted);
            self.file = Some(identity);
            self.offset = 0;
            self.aligned = true;
        }
        let mut bytes = Vec::new();
        loop {
            // One byte early, to see whether the position starts a line.
            let from = self.offset - u64::from(!self.aligned);
            file.seek(SeekFrom::Start(from))?;
            bytes.clear();
            (&mut file).take(CHUNK as u64).read_to_end(&mut bytes)?;
            let full = bytes.len() == CHUNK;
            let mut body = &bytes[..];
            if !self.aligned {
                let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
                    if bytes.is_empty() || !full {
                        break;
                    }
                    self.offset = from + bytes.len() as u64;
                    continue;
                };
                body = &bytes[newline + 1..];
                self.offset = from + newline as u64 + 1;
                self.aligned = true;
            }
            let Some(last) = body.iter().rposition(|byte| *byte == b'\n') else {
                if !full {
                    break;
                }
                // A line longer than a chunk is skipped, and the position
                // after it is found as it was after the cut.
                self.offset += body.len() as u64;
                self.aligned = false;
                continue;
            };
            for line in String::from_utf8_lossy(&body[..=last]).lines() {
                event(Event::Line(line));
            }
            self.offset += last as u64 + 1;
            if !full {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
