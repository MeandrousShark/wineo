use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{IoContext, Result};

/// The statfs filesystem ID, or None when the filesystem reports none. btrfs
/// derives it from the filesystem UUID and subvolume, so unlike `st_dev` it
/// survives reboots and differs between a subvolume and its snapshots.
pub(crate) fn filesystem_id(path: &Path) -> io::Result<Option<u64>> {
    let id = rustix::fs::statvfs(path)?.f_fsid;
    Ok((id != 0).then_some(id))
}

/// Serde for a stored filesystem ID. TOML integers are signed and most IDs
/// use the top bit, so the ID is kept as the i64 with the same bits.
pub(crate) mod fsid_bits {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(id: &Option<u64>, out: S) -> Result<S::Ok, S::Error> {
        id.map(|id| id as i64).serialize(out)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<Option<u64>, D::Error> {
        Ok(Option::<i64>::deserialize(input)?.map(|id| id as u64))
    }
}

/// Writes through a temp file and a rename, so readers never see a half-written file.
pub(crate) fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    // Created here, not left to write_unique, so a blocked parent keeps its own error.
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    }
    write_unique(path, contents.as_ref()).context(|| format!("failed to write {}", path.display()))
}

/// `write_atomic` that survives a power loss: the temp file reaches the disk
/// before the rename, and the rename before this returns.
pub(crate) fn write_atomic_synced(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let options = Options {
        mode: None,
        sync: true,
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    write_with(path, contents.as_ref(), &COUNTER, options)
        .and_then(|()| sync_dir(dir))
        .context(|| format!("failed to write {}", path.display()))
}

/// Reads a file, or `None` when there is none.
pub(crate) fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    found(fs::read(path))
}

/// Reads a text file, or `None` when there is none.
pub(crate) fn read_to_string_optional(path: &Path) -> io::Result<Option<String>> {
    found(fs::read_to_string(path))
}

/// Removes a file, returning whether there was one.
pub(crate) fn remove_if_present(path: &Path) -> io::Result<bool> {
    Ok(found(fs::remove_file(path))?.is_some())
}

fn found<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// What a cache compares to tell whether a file changed without reading it.
///
/// mtime and size alone miss a same-size rewrite whose mtime was set back,
/// which `touch -d` or a copy that keeps times can do. ctime changes on every
/// write and can't be set by hand, and a replace by rename changes the inode.
/// The blind spot left is a filesystem with coarse timestamps (FAT keeps
/// 2 s): two same-size in-place writes in one tick look alike until the next
/// change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    mtime: (i64, i64),
    ctime: (i64, i64),
    size: u64,
    dev: u64,
    ino: u64,
}

impl Stamp {
    /// The stamp of the file at `path`, or `None` when there is none.
    pub(crate) fn of(path: &Path) -> io::Result<Option<Self>> {
        Ok(found(fs::metadata(path))?.map(|metadata| Self::from_metadata(&metadata)))
    }

    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
            size: metadata.size(),
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
}

/// Reads a file with the stamp it had while being read, or `None` when there
/// is none. The stamp is `None` if the file changed during the read or the
/// path now names another file; the bytes are returned either way.
pub(crate) fn read_stamped(path: &Path) -> io::Result<Option<(Vec<u8>, Option<Stamp>)>> {
    let Some(mut file) = found(File::open(path))? else {
        return Ok(None);
    };
    // Stamped through the handle that is read, so a path that is repointed
    // meanwhile can't lend its stamp to another file's bytes.
    let before = Stamp::from_metadata(&file.metadata()?);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    #[cfg(test)]
    between_read_and_check::run();
    let after = Stamp::from_metadata(&file.metadata()?);
    let at_path = Stamp::of(path).ok().flatten();
    let stable = after == before && at_path == Some(before);
    Ok(Some((bytes, stable.then_some(before))))
}

/// Test-only: runs once between the read and the checks of `read_stamped`,
/// to change the file at exactly the moment a cache is at risk.
#[cfg(test)]
pub(crate) mod between_read_and_check {
    use std::cell::RefCell;

    type Hook = Box<dyn FnOnce()>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(crate) fn set(hook: impl FnOnce() + 'static) {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    pub(super) fn run() {
        if let Some(hook) = HOOK.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
    }
}

/// Makes the entries in `dir` durable: a created, renamed, or removed name
/// is only on disk once its directory is synced.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Replaces `path` through a synced temp file in `dir`, which must be on the
/// same filesystem. The caller syncs the directory afterwards.
pub(crate) fn persist_synced(dir: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
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
pub(crate) fn write_unique(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_unique_with(path, contents, &COUNTER)
}

/// Like `write_unique`, for secrets: the temp file is created `0600` and
/// synced before the rename.
pub(crate) fn write_unique_private(path: &Path, contents: &[u8]) -> io::Result<()> {
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
) -> io::Result<()> {
    write_with(path, contents, counter, Options::default())
}

fn write_with(
    path: &Path,
    contents: &[u8],
    counter: &AtomicU64,
    options: Options,
) -> io::Result<()> {
    const TRIES: usize = 8;
    let dir = path.parent().unwrap_or(Path::new(""));
    fs::create_dir_all(dir)?;
    let base = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
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
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
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
    fn synced_write_replaces_contents_without_leaving_temp_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.toml");
        fs::write(&path, "one").unwrap();
        write_atomic_synced(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
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

    fn stamp(path: &Path) -> Stamp {
        Stamp::of(path).unwrap().unwrap()
    }

    // File times come from a clock that can be a few ms coarse, so a test
    // that needs a new ctime waits past one tick first.
    fn next_tick() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    fn set_mtime(path: &Path, time: std::time::SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    #[test]
    fn a_missing_file_has_no_stamp_and_reading_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.toml");
        assert_eq!(Stamp::of(&path).unwrap(), None);
        fs::write(&path, "one").unwrap();
        assert_eq!(stamp(&path), stamp(&path));
    }

    #[test]
    fn a_stamp_changes_with_the_size() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.toml");
        fs::write(&path, "one").unwrap();
        let before = stamp(&path);
        fs::write(&path, "three").unwrap();
        assert_ne!(stamp(&path), before);
    }

    #[test]
    fn a_stamp_changes_with_the_mtime() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.toml");
        fs::write(&path, "one").unwrap();
        let before = stamp(&path);
        let earlier =
            fs::metadata(&path).unwrap().modified().unwrap() - std::time::Duration::from_secs(60);
        set_mtime(&path, earlier);
        assert_ne!(stamp(&path), before);
    }

    #[test]
    fn a_same_size_rewrite_with_its_mtime_set_back_is_caught_by_ctime() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.toml");
        fs::write(&path, "one").unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let before = stamp(&path);
        next_tick();
        fs::write(&path, "two").unwrap();
        set_mtime(&path, mtime);
        let after = stamp(&path);
        assert_eq!((after.mtime, after.size), (before.mtime, before.size));
        assert_ne!(after, before);
    }

    #[test]
    fn a_rename_over_the_file_is_caught_by_the_inode() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.toml");
        fs::write(&path, "one").unwrap();
        let before = stamp(&path);
        let other = root.path().join("other.toml");
        fs::write(&other, "one").unwrap();
        fs::rename(&other, &path).unwrap();
        assert_ne!(stamp(&path).ino, before.ino);
        assert_ne!(stamp(&path), before);
    }

    #[test]
    fn a_coarse_clock_hides_a_same_size_rewrite_within_one_tick() {
        // The documented blind spot: on a filesystem that keeps whole
        // seconds, an in-place same-size write in the same second leaves
        // every field equal.
        let coarse = Stamp {
            mtime: (1_792_254_000, 0),
            ctime: (1_792_254_000, 0),
            size: 3,
            dev: 1,
            ino: 2,
        };
        let rewritten = coarse;
        assert_eq!(coarse, rewritten);
    }

    #[test]
    fn a_stable_file_reads_with_its_stamp() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("log");
        assert!(read_stamped(&path).unwrap().is_none());
        fs::write(&path, "one").unwrap();
        let (bytes, found) = read_stamped(&path).unwrap().unwrap();
        assert_eq!(bytes, b"one");
        assert_eq!(found, Some(stamp(&path)));
    }

    #[test]
    fn a_read_that_fails_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(read_stamped(root.path()).is_err());
    }

    #[test]
    fn an_append_during_the_read_leaves_no_stamp() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("log");
        fs::write(&path, "one\n").unwrap();
        let appended = path.clone();
        between_read_and_check::set(move || {
            let mut file = OpenOptions::new().append(true).open(&appended).unwrap();
            file.write_all(b"two\n").unwrap();
        });
        let (bytes, found) = read_stamped(&path).unwrap().unwrap();
        assert_eq!(bytes, b"one\n");
        assert_eq!(found, None);
    }

    #[test]
    fn a_rename_over_the_path_during_the_read_leaves_no_stamp() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("log");
        fs::write(&path, "one").unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let (renamed, staged) = (path.clone(), root.path().join("staged"));
        between_read_and_check::set(move || {
            fs::write(&staged, "two").unwrap();
            set_mtime(&staged, mtime);
            fs::rename(&staged, &renamed).unwrap();
        });
        let (bytes, found) = read_stamped(&path).unwrap().unwrap();
        assert_eq!(bytes, b"one");
        assert_eq!(found, None);
    }

    #[test]
    fn a_symlink_repointed_during_the_read_is_not_trusted() {
        // The bytes come from the file the path named at open. After the
        // swap the path names B, so A's stamp must not be returned for a
        // read that no longer matches the path.
        let root = tempfile::tempdir().unwrap();
        let (a, b, link) = (
            root.path().join("a"),
            root.path().join("b"),
            root.path().join("link"),
        );
        fs::write(&a, "from a").unwrap();
        fs::write(&b, "from b").unwrap();
        std::os::unix::fs::symlink(&a, &link).unwrap();
        let (swap_link, swap_b) = (link.clone(), b.clone());
        between_read_and_check::set(move || {
            fs::remove_file(&swap_link).unwrap();
            std::os::unix::fs::symlink(&swap_b, &swap_link).unwrap();
        });
        let (bytes, found) = read_stamped(&link).unwrap().unwrap();
        assert_eq!(bytes, b"from a");
        assert_eq!(found, None);
        // Settled on B, the next read is stable and stamped as B.
        let (bytes, found) = read_stamped(&link).unwrap().unwrap();
        assert_eq!(bytes, b"from b");
        assert_eq!(found, Some(stamp(&b)));
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
