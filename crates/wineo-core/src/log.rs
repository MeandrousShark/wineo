//! A detached run's session log: the soft size cap the supervisor enforces,
//! and the counter that lets a reader notice the log was cleared.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};

use crate::error::{self, IoContext, Result};

/// How often the supervisor checks the log's size while a run is going.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

const BYTES_PER_MB: u64 = 1024 * 1024;

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
