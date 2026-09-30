//! A detached run's session log: the soft size cap the supervisor enforces,
//! and the counter that lets a reader notice the log was cleared.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};

use crate::cancel::Token;
use crate::error::{self, IoContext, Result};
use crate::runtime::Runtime;

/// How often the supervisor checks the log's size while a run is going.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

const BYTES_PER_MB: u64 = 1024 * 1024;
const TAIL_CHUNK_SIZE: usize = 64 * 1024;
const TAIL_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A run's log, together with the reset counter beside it. The counter lives in
/// its own file so clearing the log never has to touch the run's state.
#[derive(Debug, Clone)]
pub struct Log {
    path: PathBuf,
    resets: PathBuf,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        let mut resets = path.clone().into_os_string();
        resets.push(".reset");
        Self {
            path,
            resets: PathBuf::from(resets),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn resets_path(&self) -> &Path {
        &self.resets
    }

    /// Starts a run's counter over, so each run's resets are counted on their own.
    pub fn begin(&self) -> Result<()> {
        let file = self.open_resets()?;
        let _held = self.hold(&file, true)?;
        self.store(&file, 0)
    }

    /// Empties `file` if the log has passed `limit_mb`, leaving a line saying
    /// output was dropped. Returns true if it cleared anything. A limit of 0
    /// turns the cap off.
    pub fn enforce(&self, file: &File, limit_mb: u32) -> Result<bool> {
        if limit_mb == 0 {
            return Ok(false);
        }
        let limit = u64::from(limit_mb) * BYTES_PER_MB;
        let len = file
            .metadata()
            .context(|| format!("failed to read {}", self.path.display()))?
            .len();
        if len < limit {
            return Ok(false);
        }
        self.clear(
            file,
            &format!(
                "wineo: the log passed {limit_mb} MB; all output before this reset was dropped"
            ),
        )?;
        Ok(true)
    }

    /// Empties the log and counts the reset, leaving `note` as its first line.
    ///
    /// The counter is bumped while the log is held, so a reader either sees the
    /// old counter and the old contents or the new counter and the new ones.
    /// Wine keeps writing to the same file in append mode, so its next output
    /// follows the note.
    pub fn clear(&self, file: &File, note: &str) -> Result<()> {
        let resets = self.open_resets()?;
        let _held = self.hold(&resets, true)?;
        let count = self.load(&resets)?;
        file.set_len(0)
            .context(|| format!("failed to clear {}", self.path.display()))?;
        self.store(&resets, count + 1)?;
        let mut writer = file;
        writeln!(writer, "{note}").context(|| format!("failed to write {}", self.path.display()))
    }

    /// Reads with the log held against a reset. `read` is given the current
    /// reset counter, and must return promptly since a reset waits for it.
    pub fn read_guarded<T>(&self, read: impl FnOnce(u64) -> io::Result<T>) -> Result<T> {
        let resets = self.open_resets()?;
        let _held = self.hold(&resets, false)?;
        let count = self.load(&resets)?;
        read(count).context(|| format!("failed to read {}", self.path.display()))
    }

    fn open_resets(&self) -> Result<File> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.resets)
            .context(|| format!("failed to open {}", self.resets.display()))
    }

    /// Blocking, so a reset waits its turn behind a reader rather than
    /// clearing the log out from under it.
    fn hold<'a>(&self, file: &'a File, exclusive: bool) -> Result<Held<'a>> {
        let operation = if exclusive {
            FlockOperation::LockExclusive
        } else {
            FlockOperation::LockShared
        };
        flock(file, operation)
            .map_err(io::Error::from)
            .context(|| format!("failed to lock {}", self.resets.display()))?;
        Ok(Held(file))
    }

    /// An unreadable or empty counter file reads as zero, which only costs a
    /// reader one extra jump back to the start of the log.
    fn load(&self, file: &File) -> Result<u64> {
        let mut text = String::new();
        let mut reader = file;
        reader
            .read_to_string_at(&mut text)
            .context(|| format!("failed to read {}", self.resets.display()))?;
        Ok(text.trim().parse().unwrap_or(0))
    }

    fn store(&self, file: &File, count: u64) -> Result<()> {
        let text = format!("{count}\n");
        file.set_len(0)
            .and_then(|()| std::os::unix::fs::FileExt::write_all_at(file, text.as_bytes(), 0))
            .context(|| format!("failed to write {}", self.resets.display()))
    }
}

/// Releases the lock when it goes out of scope, including on an early return.
struct Held<'a>(&'a File);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let _ = flock(self.0, FlockOperation::Unlock);
    }
}

/// Reads a whole file without disturbing anyone else's file position.
trait ReadAt {
    fn read_to_string_at(&mut self, out: &mut String) -> io::Result<()>;
}

impl ReadAt for &File {
    fn read_to_string_at(&mut self, out: &mut String) -> io::Result<()> {
        let mut buf = [0u8; 32];
        let read = std::os::unix::fs::FileExt::read_at(*self, &mut buf, 0)?;
        out.push_str(&String::from_utf8_lossy(&buf[..read]));
        Ok(())
    }
}

/// Watches a run's log and clears it whenever it passes the limit.
pub struct Cap {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Cap {
    /// Starts checking every second. A limit of 0 starts nothing at all.
    pub fn start(log: &Log, file: File, limit_mb: u32) -> Self {
        if limit_mb == 0 {
            return Self {
                stop: None,
                thread: None,
            };
        }
        let (stop, stopping) = mpsc::channel();
        let log = log.clone();
        let thread = thread::spawn(move || {
            loop {
                let last = !matches!(
                    stopping.recv_timeout(CHECK_INTERVAL),
                    Err(RecvTimeoutError::Timeout)
                );
                if let Err(err) = log.enforce(&file, limit_mb) {
                    let mut writer = &file;
                    let _ = writeln!(writer, "wineo: warning: {}", error::chain(&err));
                }
                if last {
                    break;
                }
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Cap {
    /// One last check on the way out, so a burst shorter than the interval is
    /// still caught before the run is recorded as over.
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reads a log, noticing when the supervisor clears it. Every read is bounded
/// and rechecks the reset counter under the same hold, so a log that was
/// cleared and has already grown past the last read position is still picked up
/// from the start of its new contents rather than from the middle.
pub struct Tail {
    file: File,
    session: Option<Log>,
    seen: u64,
    buf: Vec<u8>,
}

impl Tail {
    pub fn new(file: File, session: Option<Log>) -> Self {
        Self {
            file,
            session,
            seen: 0,
            buf: vec![0u8; TAIL_CHUNK_SIZE],
        }
    }

    pub fn seek_to_last_lines(&mut self, lines: u64) -> anyhow::Result<()> {
        let Self {
            file,
            session,
            seen,
            ..
        } = self;
        guarded(session.as_ref(), |count| {
            *seen = count;
            let offset = tail_offset(file, lines, TAIL_CHUNK_SIZE)?;
            file.seek(SeekFrom::Start(offset))?;
            Ok(())
        })
    }

    /// Copies everything up to the log's current end. `Ok(false)` means the
    /// writer closed its end, like `wineo logs x | head`.
    pub fn stream(&mut self, writer: &mut impl Write) -> anyhow::Result<bool> {
        loop {
            let Self {
                file,
                session,
                seen,
                buf,
            } = self;
            let read = guarded(session.as_ref(), |count| {
                if count != *seen {
                    *seen = count;
                    // Cleared since the last read, so the new output starts at zero.
                    file.seek(SeekFrom::Start(0))?;
                }
                file.read(buf)
            })?;
            if read == 0 {
                return Ok(flush(writer)?);
            }
            if !write_all(writer, &self.buf[..read])? {
                return Ok(false);
            }
        }
    }
}

/// Runs `read` with the log held against a reset, when there is one to hold.
fn guarded<T>(session: Option<&Log>, read: impl FnOnce(u64) -> io::Result<T>) -> anyhow::Result<T> {
    match session {
        Some(log) => Ok(log.read_guarded(read)?),
        None => Ok(read(0)?),
    }
}

impl Tail {
    /// Watches the current run until it ends, the log rotates, or cancellation is requested.
    pub fn follow(
        &mut self,
        runtime: &Runtime,
        path: &Path,
        writer: &mut impl Write,
        token: &Token,
    ) -> anyhow::Result<()> {
        self.follow_until(writer, token, |file| run_is_over(runtime, path, file))
    }

    /// Like `follow`, for a log with no run behind it: `over` says when the
    /// writer is done. It gets the same last read afterwards as a run's end does.
    pub fn follow_until(
        &mut self,
        writer: &mut impl Write,
        token: &Token,
        mut over: impl FnMut(&File) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        loop {
            if token.is_canceled() {
                return Ok(());
            }
            token.wait_timeout(TAIL_POLL_INTERVAL);
            if token.is_canceled() || !self.stream(writer)? {
                return Ok(());
            }
            if over(&self.file)? {
                // Whatever was written between the last read and the end.
                self.stream(writer)?;
                return Ok(());
            }
        }
    }
}

/// True once nothing is running, or `path` was rotated to a new file out from under us.
fn run_is_over(runtime: &Runtime, path: &Path, file: &File) -> anyhow::Result<bool> {
    if !runtime.is_running()? {
        return Ok(true);
    }
    let ino = file.metadata()?.ino();
    Ok(!fs::metadata(path).is_ok_and(|meta| meta.ino() == ino))
}

fn write_all(writer: &mut impl Write, buf: &[u8]) -> io::Result<bool> {
    match writer.write_all(buf) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(err),
    }
}

fn flush(writer: &mut impl Write) -> io::Result<bool> {
    match writer.flush() {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(err),
    }
}

/// Finds the byte offset where the last `lines` lines of `file` start, reading
/// backwards in chunks so the whole file never has to fit in memory. A final
/// line without a trailing newline still counts as a line.
fn tail_offset(file: &mut (impl Read + Seek), lines: u64, chunk_size: usize) -> io::Result<u64> {
    let len = file.seek(SeekFrom::End(0))?;
    if lines == 0 || len == 0 {
        return Ok(len);
    }

    let mut pos = len;
    let mut found = 0u64;
    let mut buf = vec![0u8; chunk_size];
    while pos > 0 {
        let read_len = chunk_size.min(pos as usize);
        pos -= read_len as u64;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..read_len])?;
        for (i, &byte) in buf[..read_len].iter().enumerate().rev() {
            if byte != b'\n' {
                continue;
            }
            let byte_pos = pos + i as u64;
            // The file's final newline ends the last line rather than starting an empty one.
            if byte_pos == len - 1 {
                continue;
            }
            found += 1;
            if found == lines {
                return Ok(byte_pos + 1);
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::Instant;

    use super::*;

    const MARKER: &str = "the log passed 1 MB";

    fn session(dir: &Path) -> (Log, File) {
        let path = dir.join("output.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        (Log::new(path), file)
    }

    /// Appends roughly `mb` megabytes, in lines so the result stays readable.
    fn fill(file: &File, mb: usize) {
        let line = format!("{}\n", "x".repeat(1023));
        let mut writer = file;
        for _ in 0..(mb * 1024) {
            writer.write_all(line.as_bytes()).unwrap();
        }
    }

    fn contents(log: &Log) -> String {
        let mut text = String::new();
        File::open(log.path())
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    fn count(log: &Log) -> u64 {
        log.read_guarded(Ok).unwrap()
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_log_under_the_limit_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        fill(&file, 1);
        // Exactly at the limit is not past it yet.
        assert!(!log.enforce(&file, 2).unwrap());
        assert_eq!(contents(&log).len(), 1024 * 1024);
        assert_eq!(count(&log), 0);
    }

    #[test]
    fn crossing_the_limit_clears_the_log_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        fill(&file, 2);
        assert!(log.enforce(&file, 1).unwrap());

        let text = contents(&log);
        assert!(text.contains(MARKER), "{text}");
        assert!(
            text.contains("all output before this reset was dropped"),
            "{text}"
        );
        assert!(!text.contains('x'), "old output survived the reset");

        // Wine keeps appending to the same file, so later output follows the note.
        let mut writer = &file;
        writer.write_all(b"after the reset\n").unwrap();
        let text = contents(&log);
        assert!(text.ends_with("after the reset\n"), "{text}");
        assert_eq!(text.lines().count(), 2, "{text}");
    }

    #[test]
    fn a_zero_limit_turns_the_cap_off() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        fill(&file, 2);
        assert!(!log.enforce(&file, 0).unwrap());
        assert_eq!(contents(&log).len(), 2 * 1024 * 1024);
        assert_eq!(count(&log), 0);
    }

    #[test]
    fn every_reset_bumps_the_counter() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        assert_eq!(count(&log), 0);
        for expected in 1..=3 {
            fill(&file, 2);
            assert!(log.enforce(&file, 1).unwrap());
            assert_eq!(count(&log), expected);
        }
        // A new run starts its own count.
        log.begin().unwrap();
        assert_eq!(count(&log), 0);
    }

    #[test]
    fn a_reader_sees_the_new_counter_and_the_new_contents_together() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        fill(&file, 2);
        log.enforce(&file, 1).unwrap();

        let (seen, text) = log
            .read_guarded(|count| {
                let mut text = String::new();
                File::open(log.path())?.read_to_string(&mut text)?;
                Ok((count, text))
            })
            .unwrap();
        assert_eq!(seen, 1);
        assert!(text.contains(MARKER), "{text}");
    }

    #[test]
    fn the_cap_clears_during_a_run_and_once_more_at_the_end() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        let cap = Cap::start(&log, file.try_clone().unwrap(), 1);

        fill(&file, 2);
        wait_for("the first reset", || count(&log) == 1);

        // A burst right at the end, shorter than the checking interval.
        fill(&file, 2);
        drop(cap);
        assert_eq!(count(&log), 2, "the final check was skipped");
        assert!(!contents(&log).contains('x'));
    }

    #[test]
    fn follow_until_reads_what_arrives_before_the_end() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        let mut writer = &file;
        writer.write_all(b"first\n").unwrap();
        let mut tail = Tail::new(File::open(log.path()).unwrap(), None);
        let mut out = Vec::new();
        let mut checks = 0;
        tail.follow_until(&mut out, &Token::new(), |_| {
            checks += 1;
            if checks == 1 {
                writer.write_all(b"last\n").unwrap();
            }
            Ok(checks == 2)
        })
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "first\nlast\n");
    }

    #[test]
    fn a_disabled_cap_never_touches_the_log() {
        let root = tempfile::tempdir().unwrap();
        let (log, file) = session(root.path());
        let cap = Cap::start(&log, file.try_clone().unwrap(), 0);
        fill(&file, 2);
        drop(cap);
        assert_eq!(count(&log), 0);
        assert_eq!(contents(&log).len(), 2 * 1024 * 1024);
    }
}

#[cfg(test)]
mod tail_tests {
    use std::io::Cursor;

    use super::*;

    fn tail(content: &str, lines: u64, chunk_size: usize) -> String {
        let mut cursor = Cursor::new(content.as_bytes().to_vec());
        let offset = tail_offset(&mut cursor, lines, chunk_size).unwrap();
        content[offset as usize..].to_owned()
    }

    #[test]
    fn empty_file_has_nothing_to_print() {
        assert_eq!(tail("", 5, 4), "");
        assert_eq!(tail("", 0, 4), "");
    }

    #[test]
    fn zero_lines_prints_nothing() {
        assert_eq!(tail("a\nb\nc\n", 0, 4), "");
        assert_eq!(tail("a\nb\nc", 0, 4), "");
    }

    #[test]
    fn more_lines_than_the_file_has_prints_everything() {
        assert_eq!(tail("a\nb\nc\n", 10, 4), "a\nb\nc\n");
        assert_eq!(tail("a\nb\nc", 10, 4), "a\nb\nc");
    }

    #[test]
    fn trailing_line_without_a_newline_still_counts_as_a_line() {
        assert_eq!(tail("a\nb\nc", 1, 4), "c");
        assert_eq!(tail("a\nb\nc", 2, 4), "b\nc");
        assert_eq!(tail("a\nb\nc", 3, 4), "a\nb\nc");
    }

    #[test]
    fn small_chunk_sizes_still_find_the_right_offset() {
        let content = "one\ntwo\nthree\nfour\n";
        for chunk_size in 1..8 {
            assert_eq!(
                tail(content, 2, chunk_size),
                "three\nfour\n",
                "chunk_size={chunk_size}"
            );
        }
    }

    #[test]
    fn a_line_longer_than_the_chunk_size_is_still_found() {
        let content = "short\nthis line is much longer than the chunk size\nlast\n";
        assert_eq!(tail(content, 1, 4), "last\n");
        assert_eq!(
            tail(content, 2, 4),
            "this line is much longer than the chunk size\nlast\n"
        );
    }

    #[test]
    fn newline_on_a_chunk_boundary_is_still_found() {
        assert_eq!(tail("ab\ncd\n", 1, 3), "cd\n");
    }
}
