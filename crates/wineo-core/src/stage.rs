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

    /// Publishes `staged`, a tree inside the staging directory, as
    /// `destination`. The rest stays staged and is still removed on drop.
    /// Nothing is flushed, so the caller syncs once everything has landed.
    pub(crate) fn publish_part(&self, staged: &Path, destination: &Path) -> Result<()> {
        debug_assert!(staged.starts_with(self.path()));
        download::publish(staged, destination)
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
mod tests;
