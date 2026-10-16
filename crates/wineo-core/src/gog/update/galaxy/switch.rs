//! Changes that touch no file: pinning a game to the build it already has,
//! following a branch whose latest build is already installed, or going
//! back to following. They only change the record's `follow`, under the
//! folder's state lock, so there is no journal and nothing to finish.
//!
//! A pinned game that is asked to update is a `Pinned`: it changes nothing
//! and says how to unpin.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::record::{self, Follow};
use crate::cancel::Token;
use crate::gog::folder::{FolderLock, FolderState, JournalKind};
use crate::gog::migrate::host_only;
use crate::gog::update::UpdateError;
use crate::gog::update::apply::Updated;
use crate::paths::Paths;

/// A game pinned to a build, asked to update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    pub environment: String,
    pub build: String,
    /// GOG's name for the build, when it is listed.
    pub version: Option<String>,
    pub warnings: Vec<String>,
}

impl Pinned {
    /// What a plain update says instead of updating.
    pub fn message(&self) -> String {
        let version = self
            .version
            .as_deref()
            .filter(|version| !version.is_empty())
            .map(|version| format!(" ({version})"))
            .unwrap_or_default();
        format!(
            "{name} is pinned to build {build}{version}. Run wineo update {name} --latest to follow its branch again.",
            name = self.environment,
            build = self.build,
        )
    }
}

/// A change of branch or pin that needs no file work, ready to apply.
#[derive(Debug, Clone)]
pub struct FollowPlan {
    pub(super) environment: String,
    pub(super) folder: PathBuf,
    pub(super) gog_id: u64,
    pub(super) title: String,
    pub(super) seen: FolderState,
    pub(super) installed_build: Option<String>,
    pub(super) installed_version: Option<String>,
    pub(super) from: Follow,
    pub(super) to: Follow,
    pub(super) warnings: Vec<String>,
}

impl FollowPlan {
    pub fn environment(&self) -> &str {
        &self.environment
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn gog_id(&self) -> u64 {
        self.gog_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the folder has no owner, so applying claims it.
    pub fn claims(&self) -> bool {
        self.seen.owner.is_none()
    }

    /// The build installed, which the change leaves as it is.
    pub fn installed_build(&self) -> Option<&str> {
        self.installed_build.as_deref()
    }

    /// GOG's name for the installed build, when it is listed.
    pub fn installed_version(&self) -> Option<&str> {
        self.installed_version.as_deref()
    }

    /// What the game follows now.
    pub fn from(&self) -> &Follow {
        &self.from
    }

    /// What it follows after.
    pub fn to(&self) -> &Follow {
        &self.to
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Records the new follow. Refused when the game changed since planning.
pub fn apply(paths: &Paths, plan: &FollowPlan, token: &Token) -> Result<Updated> {
    host_only(paths)?;
    let name = plan.environment.as_str();
    let changed = || UpdateError::Changed {
        name: name.to_owned(),
    };
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: false }.into());
    }
    let lock = FolderLock::exclusive(paths, &plan.folder, &|| token.is_canceled())?;
    if crate::gog::update::recorded_gog_id(paths, name)? != Some(plan.gog_id.to_string()) {
        return Err(changed().into());
    }
    let state = lock.read()?.ok_or_else(changed)?;
    let unowned = |state: &FolderState| FolderState {
        owner: None,
        ..state.clone()
    };
    let claimed = plan.seen.owner.is_none();
    if unowned(&state) != unowned(&plan.seen) || (!claimed && state.owner != plan.seen.owner) {
        return Err(changed().into());
    }
    if claimed {
        lock.reclaim(name)?;
    }
    let mut record = record::read(&lock)?.unwrap_or_default();
    // The record holds the same follow it was planned from, unless another
    // wineo changed it in between.
    if record.follow != plan.from {
        return Err(changed().into());
    }
    record.follow = plan.to.clone();
    record::write(&lock, &record)?;
    Ok(Updated {
        environment: name.to_owned(),
        folder: plan.folder.clone(),
        kind: JournalKind::Update,
        recovered: false,
        claimed,
        products: state
            .committed
            .map(|committed| committed.products)
            .unwrap_or_default(),
        saved: Vec::new(),
        kept: Vec::new(),
        overwritten: Vec::new(),
        warnings: plan.warnings.clone(),
        needs_restart: Vec::new(),
    })
}
