use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{IoContext, Result};

/// Writes through a temp file and a rename, so readers never see a half-written file.
pub(crate) fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    // Created here, not left to write_unique, so a blocked parent keeps its own error.
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    }
    write_unique(path, contents.as_ref()).context(|| format!("failed to write {}", path.display()))
}

/// One counter for every kind of writer, so they never race for a temp name.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// How the temp file is created before it is renamed into place.
#[derive(Clone, Copy, Default)]
struct Options {
    /// Set at creation, so the file is never briefly wider than this.
    mode: Option<u32>,
    sync: bool,
}

/// Writes through a temp file no other writer can share, then renames it
/// into place. Concurrent writers of one path each get their own file, so
/// neither can rename a half-written one into place or delete the other's.
pub(crate) fn write_unique(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_unique_with(path, contents, &COUNTER)
}

/// Like `write_unique`, for secrets: the temp file is created `0600` and
/// synced before the rename.
pub(crate) fn write_unique_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let options = Options {
        mode: Some(0o600),
        sync: true,
    };
    write_with(path, contents, &COUNTER, options)
}

pub(crate) fn write_unique_with(
    path: &Path,
    contents: &[u8],
    counter: &AtomicU64,
) -> std::io::Result<()> {
    write_with(path, contents, counter, Options::default())
}

fn write_with(
    path: &Path,
    contents: &[u8],
    counter: &AtomicU64,
    options: Options,
) -> std::io::Result<()> {
    const TRIES: usize = 8;
    let dir = path.parent().unwrap_or(Path::new(""));
    fs::create_dir_all(dir)?;
    let base = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the path has no file name",
        )
    })?;
    for _ in 0..TRIES {
        let mut name = base.to_owned();
        name.push(format!(
            ".{}-{}.tmp",
            std::process::id(),
            counter.fetch_add(1, Ordering::Relaxed)
        ));
        let tmp = dir.join(name);
        let mut open = OpenOptions::new();
        open.write(true).create_new(true);
        if let Some(mode) = options.mode {
            open.mode(mode);
        }
        let mut file = match open.open(&tmp) {
            Ok(file) => file,
            // Someone else's file, maybe a crash leftover. Leave it alone.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        // From here the temp file is this writer's own to clean up.
        let result = file
            .write_all(contents)
            .and_then(|()| {
                if options.sync {
                    file.sync_all()
                } else {
                    Ok(())
                }
            })
            .and_then(|()| fs::rename(&tmp, path));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        return result;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free temp file name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_contents_and_creates_parents() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a/b/state.toml");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_blocked_parent_reports_the_directory_not_the_file() {
        let root = tempfile::tempdir().unwrap();
        let blocker = root.path().join("dir");
        fs::write(&blocker, "not a directory").unwrap();
        let error = write_atomic(&blocker.join("state.toml"), "x").unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with(&format!("failed to create {}", blocker.display())),
            "{error}"
        );
    }

    #[test]
    fn concurrent_writers_leave_one_complete_file_and_no_temp_files() {
        const THREADS: usize = 8;
        const ROUNDS: usize = 50;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.toml");
        // Large enough that a shared temp file would show up as a torn write.
        let payloads: Vec<String> = (0..THREADS)
            .map(|n| format!("{n}\n").repeat(200_000))
            .collect();
        let barrier = std::sync::Barrier::new(THREADS);
        std::thread::scope(|scope| {
            for payload in &payloads {
                let (path, barrier) = (&path, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..ROUNDS {
                        write_atomic(path, payload).unwrap();
                    }
                });
            }
        });
        let text = fs::read_to_string(&path).unwrap();
        assert!(payloads.contains(&text), "the file is not any one write");
        let names: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }
}
