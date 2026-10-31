//! The one check every config writer goes through: a symlinked config stays a
//! symlink, in every shape a link takes, and an existing file keeps its mode.

use std::cell::Cell;
use std::fmt::Display;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::config_write_hooks;

/// What a writer does with a link that leads nowhere.
#[derive(Clone, Copy)]
pub(crate) enum Dangling<'a> {
    /// It gets to the write, which must refuse and name the link.
    Refused,
    /// It reads as a missing file, which this writer takes as nothing to do.
    NothingToDo,
    /// It reads as a missing file, and the error says this.
    Missing(&'a str),
}

/// What a writer does when there's no config yet.
#[derive(Clone, Copy)]
pub(crate) enum NewFile {
    /// Creates it with the default mode, the way `write_atomic` does.
    Default,
    /// Creates it `0600`.
    Private,
    /// Creates nothing: it only edits a config that's there.
    NotCreated,
}

type Run<'a, E> = &'a mut dyn FnMut() -> Result<(), E>;

/// A writer under test. `write` makes a real change to the config at `link`,
/// starting from `seed`. `no_op` starts from its own seed, which may be the
/// same one, and leaves it as it is.
pub(crate) struct Writer<'a, E> {
    pub(crate) link: &'a Path,
    pub(crate) seed: &'a str,
    pub(crate) write: Run<'a, E>,
    pub(crate) no_op: Option<(&'a str, Run<'a, E>)>,
    pub(crate) dangling: Dangling<'a>,
    pub(crate) new_file: NewFile,
}

impl<E: Display> Writer<'_, E> {
    /// Fails on the first shape of link the writer gets wrong.
    pub(crate) fn check(mut self) {
        self.through_an_absolute_link();
        self.through_a_relative_link();
        self.through_a_chain();
        self.keeps_the_targets_mode(0o600);
        self.keeps_the_targets_mode(0o664);
        self.keeps_a_regular_files_mode(0o600);
        self.keeps_a_regular_files_mode(0o664);
        self.gives_a_new_file_its_usual_mode();
        self.refuses_a_dangling_link();
        self.refuses_a_link_repointed_mid_write();
        self.leaves_the_file_alone_on_a_no_op();
    }

    fn outside(&self) -> PathBuf {
        let dir = self.link.parent().unwrap();
        dir.parent().unwrap().join("wineo-link-check")
    }

    /// Starts over with `seed` in a file outside the config directory.
    fn seeded(&self) -> PathBuf {
        self.seeded_with(self.seed)
    }

    fn seeded_with(&self, seed: &str) -> PathBuf {
        let _ = fs::remove_file(self.link);
        let _ = fs::remove_dir_all(self.outside());
        fs::create_dir_all(self.link.parent().unwrap()).unwrap();
        let target = self.outside().join("a").join("config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, seed).unwrap();
        target
    }

    fn writes(&mut self, target: &Path, shape: &str) {
        let leads_to = fs::read_link(self.link).unwrap();
        if let Err(error) = (self.write)() {
            panic!("{shape}: the write failed: {error:#}");
        }
        assert_eq!(
            fs::read_link(self.link).ok().as_ref(),
            Some(&leads_to),
            "{shape}: the link was replaced"
        );
        let text = fs::read_to_string(target).unwrap();
        assert_ne!(text, self.seed, "{shape}: the target wasn't written");
        self.no_temp_files(target, shape);
    }

    fn no_temp_files(&self, target: &Path, shape: &str) {
        for dir in [self.link.parent().unwrap(), target.parent().unwrap()] {
            let leftovers: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .filter(|name| name.to_string_lossy().contains(".tmp"))
                .collect();
            assert!(leftovers.is_empty(), "{shape}: {leftovers:?}");
        }
    }

    fn through_an_absolute_link(&mut self) {
        let target = self.seeded();
        symlink(&target, self.link).unwrap();
        self.writes(&target, "absolute link");
    }

    fn through_a_relative_link(&mut self) {
        let target = self.seeded();
        symlink("../wineo-link-check/a/config.toml", self.link).unwrap();
        self.writes(&target, "relative link");
    }

    fn through_a_chain(&mut self) {
        let target = self.seeded();
        let middle = self.outside().join("middle.toml");
        symlink("a/config.toml", &middle).unwrap();
        symlink(&middle, self.link).unwrap();
        self.writes(&target, "chain");
        assert_eq!(
            fs::read_link(&middle).unwrap(),
            Path::new("a/config.toml"),
            "chain: the middle link was replaced"
        );
    }

    fn keeps_the_targets_mode(&mut self, mode: u32) {
        let target = self.seeded();
        fs::set_permissions(&target, fs::Permissions::from_mode(mode)).unwrap();
        symlink(&target, self.link).unwrap();
        self.writes(&target, "mode");
        assert_eq!(
            fs::metadata(&target).unwrap().mode() & 0o7777,
            mode,
            "the target's mode changed"
        );
    }

    fn keeps_a_regular_files_mode(&mut self, mode: u32) {
        let _ = fs::remove_file(self.link);
        fs::create_dir_all(self.link.parent().unwrap()).unwrap();
        fs::write(self.link, self.seed).unwrap();
        fs::set_permissions(self.link, fs::Permissions::from_mode(mode)).unwrap();
        // Set by the time the temp file is complete, not after the rename.
        let seen = Rc::new(Cell::new(false));
        let (dir, temp_seen) = (self.link.parent().unwrap().to_owned(), seen.clone());
        config_write_hooks::before_rename(move || {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name().to_string_lossy().contains(".tmp") {
                    assert_eq!(entry.metadata().unwrap().mode() & 0o7777, mode);
                    temp_seen.set(true);
                }
            }
        });
        if let Err(error) = (self.write)() {
            panic!("regular file: the write failed: {error:#}");
        }
        assert!(seen.get(), "regular file: no temp file was renamed");
        let found = fs::symlink_metadata(self.link).unwrap();
        assert!(found.is_file(), "regular file: it's no longer one");
        assert_eq!(
            found.mode() & 0o7777,
            mode,
            "the regular file's mode changed"
        );
        let text = fs::read_to_string(self.link).unwrap();
        assert_ne!(text, self.seed, "regular file: it wasn't written");
        self.no_temp_files(self.link, "regular file");
    }

    fn gives_a_new_file_its_usual_mode(&mut self) {
        let target = self.seeded();
        fs::remove_file(&target).unwrap();
        let result = (self.write)().map_err(|error| format!("{error:#}"));
        let usual = match self.new_file {
            NewFile::NotCreated => {
                assert!(!self.link.exists(), "a new file was created");
                return;
            }
            NewFile::Private => 0o600,
            NewFile::Default => {
                super::write_atomic(&target, "").unwrap();
                fs::metadata(&target).unwrap().mode() & 0o7777
            }
        };
        if let Err(error) = result {
            panic!("new file: the write failed: {error}");
        }
        let found = fs::metadata(self.link).unwrap().mode() & 0o7777;
        assert_eq!(found, usual, "the new file's mode");
        self.no_temp_files(self.link, "new file");
    }

    fn refuses_a_dangling_link(&mut self) {
        let target = self.seeded();
        fs::remove_file(&target).unwrap();
        symlink(&target, self.link).unwrap();
        let result = (self.write)().map_err(|error| format!("{error:#}"));
        let named = self.link.display().to_string();
        match (self.dangling, result) {
            (Dangling::NothingToDo, Ok(())) => {}
            (Dangling::Refused, Err(error)) => assert!(error.contains(&named), "{error}"),
            (Dangling::Missing(says), Err(error)) => assert!(error.contains(says), "{error}"),
            (_, Ok(())) => panic!("a dangling link was reported as a success"),
            (_, Err(error)) => panic!("a dangling link wasn't handled: {error}"),
        }
        assert_eq!(fs::read_link(self.link).unwrap(), target);
        assert!(!target.exists(), "the link's target was created");
        self.no_temp_files(&target, "dangling link");
    }

    fn refuses_a_link_repointed_mid_write(&mut self) {
        let locked = self.seeded();
        let other = self.outside().join("b").join("config.toml");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, "other").unwrap();
        symlink(&locked, self.link).unwrap();
        let (link, repointed) = (self.link.to_owned(), other.clone());
        config_write_hooks::before_rename(move || {
            fs::remove_file(&link).unwrap();
            symlink(&repointed, &link).unwrap();
        });
        let error = match (self.write)() {
            Ok(()) => panic!("a repointed link was written through"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains(&other.display().to_string()), "{error}");
        assert_eq!(fs::read_to_string(&locked).unwrap(), self.seed);
        assert_eq!(fs::read_to_string(&other).unwrap(), "other");
        assert_eq!(fs::read_link(self.link).unwrap(), other);
        self.no_temp_files(&locked, "repointed link");
        self.no_temp_files(&other, "repointed link");
    }

    fn leaves_the_file_alone_on_a_no_op(&mut self) {
        let Some(seed) = self.no_op.as_ref().map(|(seed, _)| *seed) else {
            return;
        };
        let target = self.seeded_with(seed);
        symlink(&target, self.link).unwrap();
        let (_, no_op) = self.no_op.as_mut().unwrap();
        let before = fs::metadata(&target).unwrap();
        if let Err(error) = no_op() {
            panic!("the no-op failed: {error:#}");
        }
        let after = fs::metadata(&target).unwrap();
        assert_eq!(fs::read_link(self.link).unwrap(), target);
        assert_eq!(fs::read_to_string(&target).unwrap(), seed);
        assert_eq!(
            (after.ino(), after.mtime(), after.mtime_nsec()),
            (before.ino(), before.mtime(), before.mtime_nsec())
        );
    }
}
