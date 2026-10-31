use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, IoContext, Result};

/// What wineo recognizes a file or directory by, without following a final
/// symlink.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Observed {
    pub(crate) kind: rustix::fs::FileType,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) mount: Option<u64>,
    pub(crate) birth: Option<(i64, u32)>,
    /// The statfs filesystem ID, or None when the filesystem reports none.
    /// btrfs derives it from the filesystem UUID and subvolume, so unlike
    /// `device` it survives reboots and differs between a subvolume and its
    /// snapshots.
    pub(crate) fsid: Option<u64>,
}

/// Reads every field through one descriptor, so a path swapped between
/// lookups can't mix two objects into one identity. None when nothing is
/// there.
pub(crate) fn observe(path: &Path) -> io::Result<Option<Observed>> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, StatxFlags};

    let fd = match rustix::fs::open(
        path,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let stat = rustix::fs::fstat(&fd)?;
    let kind = FileType::from_raw_mode(stat.st_mode);
    let (mount, birth) = match rustix::fs::statx(
        &fd,
        "",
        AtFlags::EMPTY_PATH,
        StatxFlags::MNT_ID | StatxFlags::BTIME,
    ) {
        Ok(found) => (
            (found.stx_mask & StatxFlags::MNT_ID.bits() != 0).then_some(found.stx_mnt_id),
            (found.stx_mask & StatxFlags::BTIME.bits() != 0)
                .then_some((found.stx_btime.tv_sec, found.stx_btime.tv_nsec)),
        ),
        Err(rustix::io::Errno::NOSYS) => (None, None),
        Err(err) => return Err(err.into()),
    };
    let fsid = if kind == FileType::Symlink {
        None
    } else {
        Some(rustix::fs::fstatvfs(&fd)?.f_fsid).filter(|&id| id != 0)
    };
    Ok(Some(Observed {
        kind,
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
        mount,
        birth,
        fsid,
    }))
}

/// Device, inode, and birth time of the object behind an open descriptor.
/// All three come from that one descriptor, so a path swapped between
/// lookups can't mix two files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileStamp {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    /// None when the filesystem or kernel reports no birth time.
    pub(crate) birth: Option<(i64, u32)>,
}

/// Stamps an open descriptor. The birth time is what tells a file apart from
/// a later one that reused its inode, which ext4 does right away.
pub(crate) fn stamp(fd: impl std::os::fd::AsFd) -> io::Result<FileStamp> {
    use rustix::fs::{AtFlags, StatxFlags};

    let stat = rustix::fs::fstat(&fd)?;
    let birth = match rustix::fs::statx(&fd, "", AtFlags::EMPTY_PATH, StatxFlags::BTIME) {
        Ok(found) => (found.stx_mask & StatxFlags::BTIME.bits() != 0)
            .then_some((found.stx_btime.tv_sec, found.stx_btime.tv_nsec)),
        Err(rustix::io::Errno::NOSYS) => None,
        Err(err) => return Err(err.into()),
    };
    Ok(FileStamp {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
        birth,
    })
}

/// `stamp` for a path, following a final symlink so a config kept in a
/// dotfile repo is recognized by the file it points at. None when nothing is
/// there. Opened O_PATH so a file that can't be read still has an identity.
pub(crate) fn stamp_following(path: &Path) -> io::Result<Option<FileStamp>> {
    use rustix::fs::{Mode, OFlags};

    match rustix::fs::open(path, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => stamp(fd).map(Some),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(err) => Err(err.into()),
    }
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
#[cfg(test)]
pub(crate) fn write_atomic_synced(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let options = Options {
        sync: true,
        ..Options::default()
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    write_with(path, contents.as_ref(), &COUNTER, options)
        .and_then(|()| sync_dir(dir))
        .context(|| format!("failed to write {}", path.display()))
}

/// `write_atomic` for a config the user may keep elsewhere: a symlink at `path`
/// stays, and the write lands on the file it leads to. `target` is what the
/// config's lock was keyed on, and the write is refused if `path` no longer
/// resolves to it. An existing file keeps its permissions, and a new one is
/// created like `write_atomic`'s.
pub(crate) fn write_config(path: &Path, target: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let destination = Destination::find(path, target)?;
    if let Some(dir) = destination.file.parent() {
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    }
    write_with(
        destination.file,
        contents.as_ref(),
        &COUNTER,
        destination.options(false),
    )
    .map_err(|error| destination.error(error))
}

/// `write_config` that survives a power loss, like `write_atomic_synced`.
pub(crate) fn write_config_synced(
    path: &Path,
    target: &Path,
    contents: impl AsRef<[u8]>,
) -> Result<()> {
    let destination = Destination::find(path, target)?;
    let dir = destination.file.parent().unwrap_or(Path::new("."));
    write_with(
        destination.file,
        contents.as_ref(),
        &COUNTER,
        destination.options(true),
    )
    .and_then(|()| sync_dir(dir))
    .map_err(|error| destination.error(error))
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

/// `write_config_synced` through a private temp file, so a new file comes out
/// `0600`. The parent directory must already exist.
pub(crate) fn persist_config_synced(path: &Path, target: &Path, contents: &[u8]) -> Result<()> {
    let destination = Destination::find(path, target)?;
    let dir = destination.file.parent().unwrap_or(Path::new("."));
    let persist = || -> io::Result<()> {
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        if let Some(mode) = destination.mode {
            temp.as_file()
                .set_permissions(fs::Permissions::from_mode(mode))?;
        }
        temp.write_all(contents)?;
        temp.as_file().sync_all()?;
        destination.pin.recheck()?;
        temp.persist(destination.file)
            .map_err(|error| error.error)?;
        sync_dir(dir)
    };
    persist().map_err(|error| destination.error(error))
}

/// A write refused to protect the config, reported as is rather than as a
/// failed write.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Refused(String);

/// A config's path and the target its lock was keyed on.
#[derive(Clone, Copy)]
struct Pin<'a> {
    path: &'a Path,
    target: &'a Path,
    /// Whether `path` was a symlink when the write began.
    linked: bool,
}

impl Pin<'_> {
    fn check(self) -> io::Result<()> {
        let now = crate::paths::resolve(self.path);
        if now != self.target {
            return Err(io::Error::other(Refused(format!(
                "it now leads to {} instead of the locked {}, so nothing was written; try again",
                now.display(),
                self.target.display()
            ))));
        }
        // A dangling link resolves to its own path, so a matching answer
        // doesn't rule out a link that appeared during the write.
        if is_symlink(self.path) && !self.linked {
            let leads_to = fs::read_link(self.path).unwrap_or_default();
            return Err(io::Error::other(Refused(format!(
                "it became a symlink to {} during the write, so nothing was written; try again",
                leads_to.display()
            ))));
        }
        Ok(())
    }

    /// The last look before the rename. The rename then lands on the locked
    /// file whatever happens to the link, so the lock always covers the write.
    fn recheck(self) -> io::Result<()> {
        #[cfg(test)]
        config_write_hooks::run_before_rename();
        self.check()
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|found| found.is_symlink())
}

/// The permissions of a regular file at `path`, or None if nothing to keep is
/// there. A parent that isn't a directory is left for the write to report.
fn regular_mode(path: &Path) -> io::Result<Option<u32>> {
    match fs::symlink_metadata(path) {
        Ok(found) => Ok(found.is_file().then(|| found.mode() & 0o7777)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Where a config write lands.
struct Destination<'a> {
    pin: Pin<'a>,
    file: &'a Path,
    /// The mode of the file being replaced, if there is one.
    mode: Option<u32>,
}

impl<'a> Destination<'a> {
    fn find(path: &'a Path, target: &'a Path) -> Result<Self> {
        #[cfg(test)]
        config_write_hooks::run_before_find();
        let refused = |error| Self::failure(path, error);
        let linked = is_symlink(path);
        let pin = Pin {
            path,
            target,
            linked,
        };
        if !linked {
            return Ok(Self {
                pin,
                file: path,
                mode: regular_mode(path).map_err(refused)?,
            });
        }
        match fs::metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let leads_to = fs::read_link(path).unwrap_or_default();
                return Err(refused(io::Error::other(Refused(format!(
                    "it's a symlink to {}, which doesn't exist",
                    leads_to.display()
                )))));
            }
            Err(error) => return Err(refused(error)),
            Ok(_) => {}
        }
        pin.check().map_err(refused)?;
        let mode = fs::metadata(target).map_err(refused)?.mode() & 0o7777;
        Ok(Self {
            pin,
            file: target,
            mode: Some(mode),
        })
    }

    fn options(&self, sync: bool) -> Options<'a> {
        Options {
            mode: self.mode,
            exact_mode: true,
            sync,
            pin: Some(self.pin),
        }
    }

    fn error(&self, error: io::Error) -> Error {
        Self::failure(self.pin.path, error)
    }

    fn failure(path: &Path, source: io::Error) -> Error {
        match source
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<Refused>())
        {
            Some(Refused(message)) => Error::ConfigInvalid {
                path: path.to_owned(),
                message: message.clone(),
            },
            None => Error::Io {
                context: format!("failed to write {}", path.display()),
                source,
            },
        }
    }
}

/// Test-only: each hook runs once, just before a config writer's first or
/// last look at its path, to change the path at exactly the moment that
/// matters.
#[cfg(test)]
pub(crate) mod config_write_hooks {
    use std::cell::RefCell;
    use std::thread::LocalKey;

    type Hook = Box<dyn FnOnce()>;
    type Slot = RefCell<Option<Hook>>;

    thread_local! {
        static BEFORE_FIND: Slot = const { RefCell::new(None) };
        static BEFORE_RENAME: Slot = const { RefCell::new(None) };
    }

    pub(crate) fn before_find(hook: impl FnOnce() + 'static) {
        BEFORE_FIND.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    pub(crate) fn before_rename(hook: impl FnOnce() + 'static) {
        BEFORE_RENAME.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    fn run(slot: &'static LocalKey<Slot>) {
        if let Some(hook) = slot.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
    }

    pub(crate) fn run_before_find() {
        run(&BEFORE_FIND);
    }

    pub(super) fn run_before_rename() {
        run(&BEFORE_RENAME);
    }
}

/// One counter for every kind of writer, so they never race for a temp name.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// How the temp file is created before it is renamed into place.
#[derive(Clone, Copy, Default)]
struct Options<'a> {
    /// Set at creation, so the file is never briefly wider than this.
    mode: Option<u32>,
    /// Undoes the umask on `mode`, for a file that must match another.
    exact_mode: bool,
    sync: bool,
    /// Checked right before the rename.
    pin: Option<Pin<'a>>,
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
        ..Options::default()
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
    options: Options<'_>,
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
        let result = match options.mode.filter(|_| options.exact_mode) {
            Some(mode) => file.set_permissions(fs::Permissions::from_mode(mode)),
            None => Ok(()),
        }
        .and_then(|()| file.write_all(contents))
        .and_then(|()| {
            if options.sync {
                file.sync_all()
            } else {
                Ok(())
            }
        })
        .and_then(|()| options.pin.map_or(Ok(()), Pin::recheck))
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
pub(crate) mod link_check;

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

    fn resolve(path: &Path) -> std::path::PathBuf {
        crate::paths::resolve(path)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    type ConfigWrite = fn(&Path, &Path, &[u8]) -> Result<()>;

    const CONFIG_WRITES: [ConfigWrite; 3] = [
        |path, target, text| write_config(path, target, text),
        |path, target, text| write_config_synced(path, target, text),
        persist_config_synced,
    ];

    #[test]
    fn each_config_write_goes_through_a_link() {
        for write in CONFIG_WRITES {
            let root = tempfile::tempdir().unwrap();
            let (link, target) = (root.path().join("link"), root.path().join("target"));
            fs::write(&target, "old").unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            write(&link, &resolve(&link), b"new").unwrap();
            assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
            assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        }
    }

    #[test]
    fn each_config_write_refuses_a_dangling_link_by_name() {
        for write in CONFIG_WRITES {
            let root = tempfile::tempdir().unwrap();
            let link = root.path().join("link");
            let missing = root.path().join("missing");
            std::os::unix::fs::symlink(&missing, &link).unwrap();
            let error = write(&link, &resolve(&link), b"new").unwrap_err();
            assert!(
                error.to_string().contains(&*link.to_string_lossy()),
                "{error}"
            );
            assert!(
                error.to_string().contains(&*missing.to_string_lossy()),
                "{error}"
            );
            assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
            assert!(!missing.exists());
        }
    }

    #[test]
    fn each_config_write_refuses_a_link_repointed_mid_write() {
        for write in CONFIG_WRITES {
            let root = tempfile::tempdir().unwrap();
            let (a, b, link) = (
                root.path().join("a"),
                root.path().join("b"),
                root.path().join("link"),
            );
            fs::write(&a, "from a").unwrap();
            fs::write(&b, "from b").unwrap();
            std::os::unix::fs::symlink(&a, &link).unwrap();
            let locked = resolve(&link);
            let (swap_link, swap_b) = (link.clone(), b.clone());
            config_write_hooks::before_rename(move || {
                fs::remove_file(&swap_link).unwrap();
                std::os::unix::fs::symlink(&swap_b, &swap_link).unwrap();
            });
            write(&link, &locked, b"new").unwrap_err();
            assert_eq!(fs::read_to_string(&a).unwrap(), "from a");
            assert_eq!(fs::read_to_string(&b).unwrap(), "from b");
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
        }
    }

    #[test]
    fn each_config_write_refuses_a_regular_file_that_became_a_dangling_link() {
        for existing in [true, false] {
            for write in CONFIG_WRITES {
                let root = tempfile::tempdir().unwrap();
                let (path, missing) = (root.path().join("config"), root.path().join("missing"));
                if existing {
                    fs::write(&path, "old").unwrap();
                }
                let locked = resolve(&path);
                let (swap_path, swap_missing) = (path.clone(), missing.clone());
                config_write_hooks::before_rename(move || {
                    let _ = fs::remove_file(&swap_path);
                    std::os::unix::fs::symlink(&swap_missing, &swap_path).unwrap();
                });
                let error = write(&path, &locked, b"new").unwrap_err();
                assert!(
                    error.to_string().contains(&*path.to_string_lossy()),
                    "{error}"
                );
                assert!(
                    error.to_string().contains(&*missing.to_string_lossy()),
                    "{error}"
                );
                assert_eq!(fs::read_link(&path).unwrap(), missing);
                assert!(!missing.exists());
                assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
            }
        }
    }

    #[test]
    fn a_config_write_refuses_a_link_repointed_before_it_starts() {
        let root = tempfile::tempdir().unwrap();
        let (a, b, link) = (
            root.path().join("a"),
            root.path().join("b"),
            root.path().join("link"),
        );
        fs::write(&a, "from a").unwrap();
        fs::write(&b, "from b").unwrap();
        std::os::unix::fs::symlink(&b, &link).unwrap();
        let error = write_config(&link, &a, "new").unwrap_err();
        assert!(matches!(error, Error::ConfigInvalid { .. }), "{error}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "from a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "from b");
    }

    #[test]
    fn a_config_write_refuses_a_path_that_became_a_link() {
        let root = tempfile::tempdir().unwrap();
        let (path, other) = (root.path().join("config"), root.path().join("other"));
        let locked = resolve(&path);
        fs::write(&other, "other").unwrap();
        std::os::unix::fs::symlink(&other, &path).unwrap();
        write_config(&path, &locked, "new").unwrap_err();
        assert_eq!(fs::read_to_string(&other).unwrap(), "other");
    }

    #[test]
    fn a_config_write_to_a_new_file_matches_write_atomic() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("plain/a/config.toml");
        write_atomic(&plain, "text").unwrap();
        let config = root.path().join("config/a/config.toml");
        write_config(&config, &resolve(&config), "text").unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), "text");
        assert_eq!(mode(&config), mode(&plain));

        let synced = root.path().join("synced.toml");
        write_config_synced(&synced, &resolve(&synced), "text").unwrap();
        assert_eq!(mode(&synced), mode(&plain));

        let persisted = root.path().join("persisted.toml");
        persist_config_synced(&persisted, &resolve(&persisted), b"text").unwrap();
        assert_eq!(mode(&persisted), 0o600, "as tempfile creates it");
    }

    #[test]
    fn a_config_write_replaces_a_regular_file_and_keeps_its_mode() {
        for write in CONFIG_WRITES {
            for kept in [0o640, 0o644] {
                let root = tempfile::tempdir().unwrap();
                let path = root.path().join("config.toml");
                fs::write(&path, "old").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(kept)).unwrap();
                let before = fs::metadata(&path).unwrap().ino();
                write(&path, &resolve(&path), b"new").unwrap();
                assert_eq!(fs::read_to_string(&path).unwrap(), "new");
                assert!(!fs::symlink_metadata(&path).unwrap().is_symlink());
                assert_ne!(fs::metadata(&path).unwrap().ino(), before);
                assert_eq!(mode(&path), kept);
            }
        }
    }

    #[test]
    fn a_config_write_under_a_file_still_fails_the_way_it_did() {
        let root = tempfile::tempdir().unwrap();
        let blocker = root.path().join("dir");
        fs::write(&blocker, "not a directory").unwrap();
        let path = blocker.join("config.toml");
        let error = write_config(&path, &resolve(&path), "x").unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with(&format!("failed to create {}", blocker.display())),
            "{error}"
        );
    }

    #[test]
    fn a_regular_config_in_a_linked_directory_is_written_in_that_directory() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let path = linked.join("config.toml");
        fs::write(&path, "old").unwrap();
        write_config(&path, &resolve(&path), "new").unwrap();
        assert_eq!(fs::read_to_string(real.join("config.toml")).unwrap(), "new");
        assert!(fs::symlink_metadata(&linked).unwrap().is_symlink());
    }

    #[test]
    fn a_link_loop_is_refused_as_a_failed_write() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let error = write_config(&a, &resolve(&a), "new").unwrap_err();
        assert!(error.to_string().contains(&*a.to_string_lossy()), "{error}");
        assert!(fs::symlink_metadata(&a).unwrap().is_symlink());
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
