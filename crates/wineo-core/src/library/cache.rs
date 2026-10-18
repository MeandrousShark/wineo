use crate::files::{self, Stamp};
use crate::paths::Paths;

use super::{Library, ReadFailure, document_from_bytes, library_from};

/// `load` for a caller that reads often, like the GUI's rescan: the file is
/// parsed again only when its stamp changed.
///
/// Nothing can put a value in the cache. Every write path calls `refresh`
/// afterwards, whether the write succeeded or not, so the cache only ever
/// holds what is on disk and never a value that failed to save.
#[derive(Debug, Default)]
pub struct Cache {
    library: Library,
    seen: Seen,
    #[cfg(test)]
    reads: usize,
}

/// The file the cached `Library` was read from.
#[derive(Debug, Default, PartialEq, Eq)]
enum Seen {
    /// Never read, or it changed while being read.
    #[default]
    Untrusted,
    Missing,
    File(Stamp),
}

impl Seen {
    fn now(paths: &Paths) -> Self {
        match Stamp::of(&paths.library_file()) {
            Ok(Some(stamp)) => Self::File(stamp),
            Ok(None) => Self::Missing,
            Err(_) => Self::Untrusted,
        }
    }
}

impl Cache {
    /// The library, read again only when the file changed since last time.
    pub fn read(&mut self, paths: &Paths) -> &Library {
        let now = Seen::now(paths);
        if now == Seen::Untrusted || now != self.seen {
            self.reload(paths);
        }
        &self.library
    }

    /// The library as it is on disk now.
    pub fn refresh(&mut self, paths: &Paths) -> &Library {
        self.reload(paths);
        &self.library
    }

    /// The bytes and the stamp come from one read of one handle, so a write
    /// landing meanwhile leaves the cache untrusted instead of pairing the
    /// old contents with the new stamp.
    fn reload(&mut self, paths: &Paths) {
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let (read, seen) = match files::read_stamped(&paths.library_file()) {
            Ok(None) => (Ok(None), Seen::Missing),
            Ok(Some((bytes, stamp))) => (
                document_from_bytes(bytes).map(Some),
                stamp.map_or(Seen::Untrusted, Seen::File),
            ),
            Err(error) => (Err(ReadFailure::Io(error)), Seen::Untrusted),
        };
        self.library = library_from(read);
        self.seen = seen;
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    use super::*;

    struct Fixture {
        _root: tempfile::TempDir,
        paths: Paths,
        cache: Cache,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home");
            let paths = Paths::from_lookup(move |key| {
                (key == "HOME").then(|| OsString::from(home.as_os_str()))
            })
            .unwrap();
            fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
            Self {
                _root: root,
                paths,
                cache: Cache::default(),
            }
        }

        fn write(&self, text: &str) {
            fs::write(self.paths.library_file(), text).unwrap();
        }

        fn mtime(&self) -> SystemTime {
            fs::metadata(self.paths.library_file())
                .unwrap()
                .modified()
                .unwrap()
        }

        fn set_mtime(&self, time: SystemTime) {
            File::options()
                .write(true)
                .open(self.paths.library_file())
                .unwrap()
                .set_modified(time)
                .unwrap();
        }

        /// Reads through the cache, returning whether the file was parsed.
        fn read(&mut self) -> bool {
            let before = self.cache.reads;
            self.cache.read(&self.paths);
            self.cache.reads != before
        }

        fn favorite(&mut self, name: &str) -> bool {
            self.cache
                .read(&self.paths)
                .environments
                .get(name)
                .is_some_and(|entry| entry.favorite)
        }
    }

    const FAVORITE: &str = "[environments.game]\nfavorite = true\n";
    const PLAIN: &str = "[environments.game]\nfavorite = false\n";

    // File times come from a clock that can be a few ms coarse, so a test
    // that needs a new ctime waits past one tick first.
    fn next_tick() {
        std::thread::sleep(Duration::from_millis(20));
    }

    #[test]
    fn an_unchanged_file_is_not_read_again() {
        let mut fixture = Fixture::new();
        fixture.write(FAVORITE);
        assert!(fixture.read());
        assert!(fixture.favorite("game"));
        assert!(!fixture.read());
        assert!(!fixture.read());
    }

    #[test]
    fn a_changed_size_is_read_again() {
        let mut fixture = Fixture::new();
        fixture.write(PLAIN);
        assert!(!fixture.favorite("game"));
        fixture.write(FAVORITE);
        assert!(fixture.read());
        assert!(fixture.favorite("game"));
    }

    #[test]
    fn a_changed_mtime_is_read_again() {
        let mut fixture = Fixture::new();
        fixture.write(FAVORITE);
        assert!(fixture.read());
        fixture.set_mtime(fixture.mtime() - Duration::from_secs(60));
        assert!(fixture.read());
    }

    #[test]
    fn a_same_size_rewrite_with_its_mtime_set_back_is_read_again() {
        let mut fixture = Fixture::new();
        let hidden = "[environments.game]\nhidden   = true\n";
        assert_eq!(hidden.len(), FAVORITE.len());
        fixture.write(FAVORITE);
        let mtime = fixture.mtime();
        assert!(fixture.favorite("game"));
        next_tick();
        fixture.write(hidden);
        fixture.set_mtime(mtime);
        assert!(fixture.read());
        assert!(!fixture.favorite("game"));
    }

    #[test]
    fn a_rename_over_the_file_is_read_again() {
        let mut fixture = Fixture::new();
        fixture.write(FAVORITE);
        let mtime = fixture.mtime();
        assert!(fixture.favorite("game"));
        let other = fixture.paths.library_file().with_extension("new");
        let hidden = "[environments.game]\nhidden   = true\n";
        assert_eq!(hidden.len(), FAVORITE.len());
        fs::write(&other, hidden).unwrap();
        File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        fs::rename(&other, fixture.paths.library_file()).unwrap();
        assert!(fixture.read());
        assert!(!fixture.favorite("game"));
    }

    #[test]
    fn a_missing_file_is_read_once_and_again_when_created() {
        let mut fixture = Fixture::new();
        assert!(fixture.read());
        assert!(!fixture.read());
        assert_eq!(*fixture.cache.read(&fixture.paths), Library::default());
        fixture.write(FAVORITE);
        assert!(fixture.read());
        assert!(fixture.favorite("game"));
        fs::remove_file(fixture.paths.library_file()).unwrap();
        assert!(fixture.read());
        assert!(!fixture.favorite("game"));
    }

    #[test]
    fn a_broken_file_shows_its_problem_until_fixed() {
        let mut fixture = Fixture::new();
        fixture.write("[environments.game\n");
        assert!(fixture.cache.read(&fixture.paths).problem.is_some());
        assert!(!fixture.read());
        fixture.write(FAVORITE);
        let library = fixture.cache.read(&fixture.paths);
        assert_eq!(library.problem, None);
        assert!(library.environments["game"].favorite);
    }

    #[test]
    fn refresh_always_reads() {
        let mut fixture = Fixture::new();
        fixture.write(FAVORITE);
        assert!(fixture.read());
        let before = fixture.cache.reads;
        fixture.cache.refresh(&fixture.paths);
        fixture.cache.refresh(&fixture.paths);
        assert_eq!(fixture.cache.reads, before + 2);
        assert!(!fixture.read());
    }

    #[test]
    fn a_rewrite_during_the_read_leaves_the_cache_untrusted() {
        let mut fixture = Fixture::new();
        fixture.write(PLAIN);
        let path = fixture.paths.library_file();
        files::between_read_and_check::set(move || {
            fs::write(&path, FAVORITE).unwrap();
        });
        // The first read got the old bytes, so it can't keep the new stamp.
        assert!(fixture.read());
        assert_eq!(fixture.cache.seen, Seen::Untrusted);
        assert!(fixture.read());
        assert!(fixture.favorite("game"));
        assert!(!fixture.read());
    }

    #[test]
    fn bytes_that_are_not_text_read_like_load() {
        let mut fixture = Fixture::new();
        fs::write(fixture.paths.library_file(), b"\xff\xfe").unwrap();
        let cached = fixture.cache.read(&fixture.paths).clone();
        assert_eq!(cached, super::super::load(&fixture.paths));
        assert!(cached.problem.is_some());
        assert!(!fixture.read());
    }
}
