use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use wineo_core::environment::Environment;
use wineo_core::log::Log;
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, Runtime, State};

const CHUNK_SIZE: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(clap::Args)]
pub struct Args {
    /// Environment whose log to print
    name: String,

    /// Keep printing new output as it's written
    #[arg(short, long, conflicts_with = "previous")]
    follow: bool,

    /// Print only the last N lines
    #[arg(short = 'n', long = "lines", value_name = "N")]
    lines: Option<u64>,

    /// Show the run before the latest one
    #[arg(long)]
    previous: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    // Deliberately doesn't load the config, so a broken one doesn't block reading logs.
    Environment::ensure_exists(paths, &args.name)?;
    let runtime = Runtime::new(paths, &args.name);

    let path = if args.previous {
        runtime.previous_log_path()
    } else {
        runtime.log_path()
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if args.previous {
                eprintln!("{} has no previous log", args.name);
            } else {
                eprintln!(
                    "{} has no log yet, only runs started with `wineo start` are logged",
                    args.name
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        Err(err) => return Err(err).with_context(|| format!("failed to open {}", path.display())),
    };

    // Attached runs aren't logged, so this log is from an earlier detached run.
    // An unreadable state file just means there's no note.
    let attached = runtime.status().is_ok_and(|status| {
        status.state == State::Running
            && status.run.is_some_and(|run| run.mode == RunMode::Attached)
    });
    if attached {
        eprintln!(
            "{} is running attached, so its output is in that terminal",
            args.name
        );
    }

    // Only the current log is ever cleared, so only that one needs coordination.
    let session = (!args.previous).then(|| Log::new(runtime.log_path()));
    let mut reader = Reader::new(file, session);
    if let Some(lines) = args.lines {
        reader.seek_to_last_lines(lines)?;
    }

    let mut stdout = io::stdout().lock();
    let open = reader.stream(&mut stdout)?;

    // Attached mode has no active detached run to follow, and this log is history.
    if open && args.follow && !attached && runtime.is_running()? {
        follow(&runtime, &path, &mut reader, &mut stdout)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Reads a log, noticing when the supervisor clears it. Every read is bounded
/// and rechecks the reset counter under the same hold, so a log that was
/// cleared and has already grown past the last read position is still picked up
/// from the start of its new contents rather than from the middle.
struct Reader {
    file: File,
    session: Option<Log>,
    seen: u64,
    buf: Vec<u8>,
}

impl Reader {
    fn new(file: File, session: Option<Log>) -> Self {
        Self {
            file,
            session,
            seen: 0,
            buf: vec![0u8; CHUNK_SIZE],
        }
    }

    fn seek_to_last_lines(&mut self, lines: u64) -> anyhow::Result<()> {
        let Self {
            file,
            session,
            seen,
            ..
        } = self;
        guarded(session.as_ref(), |count| {
            *seen = count;
            let offset = tail_offset(file, lines, CHUNK_SIZE)?;
            file.seek(SeekFrom::Start(offset))?;
            Ok(())
        })
    }

    /// Copies everything up to the log's current end. `Ok(false)` means the
    /// writer closed its end, like `wineo logs x | head`.
    fn stream(&mut self, writer: &mut impl Write) -> anyhow::Result<bool> {
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

fn follow(
    runtime: &Runtime,
    path: &Path,
    reader: &mut Reader,
    stdout: &mut impl Write,
) -> anyhow::Result<()> {
    loop {
        thread::sleep(POLL_INTERVAL);
        if !reader.stream(stdout)? {
            return Ok(());
        }
        if run_is_over(runtime, path, &reader.file)? {
            // Whatever was written between the last read and the run ending.
            reader.stream(stdout)?;
            return Ok(());
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
