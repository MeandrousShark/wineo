//! Staging a copied tree beside where it lands, and publishing it there.
//!
//! A copy is written into a fresh directory next to its destination, checked,
//! then renamed into place without replacing anything. Until it is published
//! the staging directory is removed on drop, so an error or a cancel leaves
//! nothing behind but the destination's install lock file.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::cancel::Token;
use crate::download::{self, InstallLock};
use crate::import::{self, ImportLayout, ImportProgress, Links, Staging};

/// A staging directory, removed on drop unless it was published whole.
pub(crate) struct Stage {
    staging: Staging,
}

impl Stage {
    /// Creates `parent` if needed and a staging directory in it named
    /// `name_prefix` plus a random suffix.
    pub(crate) fn new(name_prefix: &str, parent: &Path) -> Result<Self> {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let staging = Staging::new_in(name_prefix, parent).with_context(|| {
            format!("failed to create staging directory in {}", parent.display())
        })?;
        Ok(Self { staging })
    }

    pub(crate) fn path(&self) -> &Path {
        self.staging.path()
    }

    /// Publishes the whole staging directory as `destination`, then flushes
    /// its filesystem so the tree is on disk before a config names it.
    pub(crate) fn publish(self, destination: &Path) -> Result<()> {
        download::publish(self.path(), destination)?;
        // Only after the rename, so a failed one still cleans up.
        self.staging.keep();
        import::sync_filesystem(destination)
    }
}

/// Copies a whole prefix to a new destination by way of a staging directory
/// beside it.
pub(crate) struct PrefixCopy<'a> {
    pub source: &'a Path,
    pub destination: &'a Path,
    pub layout: ImportLayout,
    pub links: &'a Links,
    /// The staging directory name prefix, like `.wineo-clone-`.
    pub staging_prefix: &'a str,
    /// Names the operation in errors, like `clone`.
    pub label: &'a str,
}

impl PrefixCopy<'_> {
    /// Stages, copies, verifies, and publishes the prefix. `verify` runs once
    /// the copy is complete and before anything is published; an error from
    /// it, the copy, or a cancel removes the staging directory.
    ///
    /// The returned install lock is held by the caller for as long as it
    /// still relies on the destination being its own.
    pub(crate) fn run(
        &self,
        cancel: &Token,
        progress: &mut impl FnMut(ImportProgress),
        verify: impl FnOnce(&mut dyn FnMut(ImportProgress)) -> Result<()>,
    ) -> Result<InstallLock> {
        let destination = self.destination;
        if destination.exists() {
            bail!(
                "{} destination appeared during review: {}",
                self.label,
                destination.display()
            );
        }
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow!("destination has no parent: {}", destination.display()))?;
        let stage = Stage::new(self.staging_prefix, parent)?;
        let staging_path: PathBuf = stage.path().to_owned();
        let publish_lock = InstallLock::acquire(destination)?;
        import::copy_tree_with(
            self.source,
            &staging_path,
            destination,
            self.links,
            cancel,
            progress,
        )?;
        import::check_cancelled(cancel)?;
        verify(progress)?;
        import::validate_staged_prefix(&staging_path, destination, self.layout)?;
        if destination.exists() {
            bail!(
                "{} destination appeared during publication: {}",
                self.label,
                destination.display()
            );
        }
        stage.publish(destination)?;
        Ok(publish_lock)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::process::Command;

    use super::*;
    use crate::import::Cancelled;

    fn prefix(root: &Path) {
        fs::create_dir_all(root.join("drive_c/game")).unwrap();
        fs::write(root.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        fs::write(root.join("drive_c/game/save.dat"), "save").unwrap();
    }

    fn copy<'a>(source: &'a Path, destination: &'a Path, links: &'a Links) -> PrefixCopy<'a> {
        PrefixCopy {
            source,
            destination,
            layout: ImportLayout::Wine,
            links,
            staging_prefix: ".wineo-test-",
            label: "test",
        }
    }

    /// Everything in `dir` except the destination's install lock file.
    fn left_behind(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with(".wineo-lock-"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn publishes_after_verification() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let verified = Cell::new(false);
        let mut events = Vec::new();
        let _lock = copy(&source, &destination, &links)
            .run(&Token::new(), &mut |event| events.push(event), |_| {
                assert!(!destination.exists());
                let staged = left_behind(&parent);
                assert_eq!(staged.len(), 1);
                assert!(staged[0].starts_with(".wineo-test-"));
                assert!(parent.join(&staged[0]).join("system.reg").is_file());
                verified.set(true);
                Ok(())
            })
            .unwrap();
        assert!(verified.get());
        assert_eq!(
            fs::read_to_string(destination.join("drive_c/game/save.dat")).unwrap(),
            "save"
        );
        assert_eq!(left_behind(&parent), ["game"]);
        assert!(matches!(events[0], ImportProgress::Started { .. }));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ImportProgress::Finished { .. }))
        );
    }

    #[test]
    fn verification_failure_leaves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let err = copy(&source, &destination, &links)
            .run(&Token::new(), &mut |_| {}, |_| bail!("source changed"))
            .err()
            .expect("the copy fails");
        assert_eq!(err.to_string(), "source changed");
        assert!(!destination.exists());
        assert!(left_behind(&parent).is_empty());
    }

    #[test]
    fn copy_error_leaves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        // A read-only directory checks that cleanup can still empty it.
        fs::create_dir(source.join("drive_c/locked")).unwrap();
        fs::write(source.join("drive_c/locked/file"), "x").unwrap();
        let status = Command::new("mkfifo")
            .arg(source.join("drive_c/zz-fifo"))
            .status()
            .unwrap();
        assert!(status.success());
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            source.join("drive_c/locked"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let err = copy(&source, &destination, &links)
            .run(&Token::new(), &mut |_| {}, |_| {
                panic!("verify must not run after a failed copy")
            })
            .err()
            .expect("the copy fails");
        fs::set_permissions(
            source.join("drive_c/locked"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(
            err.to_string().contains("unsupported special file"),
            "{err}"
        );
        assert!(!destination.exists());
        assert!(left_behind(&parent).is_empty());
    }

    #[test]
    fn cancel_leaves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let cancel = Token::new();
        let err = copy(&source, &destination, &links)
            .run(
                &cancel,
                &mut |event| {
                    if matches!(event, ImportProgress::Copied { .. }) {
                        cancel.cancel();
                    }
                },
                |_| panic!("verify must not run after a cancel"),
            )
            .err()
            .expect("the copy fails");
        assert!(err.is::<Cancelled>(), "{err}");
        assert!(!destination.exists());
        assert!(left_behind(&parent).is_empty());
    }

    #[test]
    fn cancel_during_verification_leaves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let cancel = Token::new();
        let err = copy(&source, &destination, &links)
            .run(&cancel, &mut |_| {}, |_| {
                cancel.cancel();
                import::check_cancelled(&cancel)
            })
            .err()
            .expect("the copy fails");
        assert!(err.is::<Cancelled>(), "{err}");
        assert!(!destination.exists());
        assert!(left_behind(&parent).is_empty());
    }

    #[test]
    fn invalid_staged_prefix_is_not_published() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("drive_c")).unwrap();
        fs::create_dir(source.join("system.reg")).unwrap();
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let err = copy(&source, &destination, &links)
            .run(&Token::new(), &mut |_| {}, |_| Ok(()))
            .err()
            .expect("the copy fails");
        assert_eq!(
            err.to_string(),
            "staged import is missing system.reg or drive_c"
        );
        assert!(!destination.exists());
        assert!(left_behind(&parent).is_empty());
    }

    #[test]
    fn destination_appearing_is_not_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        let links = Links::default();
        let err = copy(&source, &destination, &links)
            .run(&Token::new(), &mut |_| {}, |_| {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("theirs"), "keep").unwrap();
                Ok(())
            })
            .err()
            .expect("the copy fails");
        assert_eq!(
            err.to_string(),
            format!(
                "test destination appeared during publication: {}",
                destination.display()
            )
        );
        assert_eq!(
            fs::read_to_string(destination.join("theirs")).unwrap(),
            "keep"
        );
        assert_eq!(left_behind(&parent), ["game"]);
    }

    #[test]
    fn existing_destination_is_refused_before_staging() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        prefix(&source);
        let parent = temp.path().join("prefixes");
        let destination = parent.join("game");
        fs::create_dir_all(&destination).unwrap();
        let links = Links::default();
        let err = copy(&source, &destination, &links)
            .run(&Token::new(), &mut |_| {}, |_| Ok(()))
            .err()
            .expect("the copy fails");
        assert_eq!(
            err.to_string(),
            format!(
                "test destination appeared during review: {}",
                destination.display()
            )
        );
        assert_eq!(left_behind(&parent), ["game"]);
    }
}
