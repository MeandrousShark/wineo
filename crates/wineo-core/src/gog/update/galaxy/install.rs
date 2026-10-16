//! Installing a Windows game from a Galaxy build, into a fresh folder, as a
//! new Wine or Proton environment with `gog_id`.
//!
//! `plan` resolves the latest generation 2 build on the default branch,
//! fetches and filters its manifests, and reads only. `apply` checks space,
//! then under the locks writes the record's `pending`, the folder's state
//! with an `install` journal naming every entry it writes, and the
//! environment, then places the files: folders and links first, then every
//! file downloaded 4 at a time into a temporary file beside its path and
//! renamed into place once whole. Support files go to the folder's support
//! folder in the data directory and redistributables to the shared store.
//! Then the record's `installed`, the commit, and dropping `pending`.
//!
//! Anything that stops it after the journal leaves it, and installing again
//! finishes from the journal and `pending`, keeping every file already in
//! place that matches. Without `pending`, the journal's builds are fetched
//! again, or GOG's latest when a build is no longer listed.
//!
//! Lock order, as for any install: the adoption lock, the environment's
//! operation lock with its prefix, the folder's in-use lock, its state lock.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use md5::{Digest as _, Md5};
use rustix::fs::{FileType, Mode, OFlags};
use sha2::Sha256;
use wineo_gog::{ChunkLinks, LinkScope};

use super::build::{self, LayoutError, Owner};
use super::fetch::{self, Job};
use super::follow::{self, Who};
use super::record::{self, FileEntry, Follow, Manifest, Record, Redist, SetupInfo, StoreVersion};
use super::setup;
use super::store;
use crate::cancel::Token;
use crate::config::{Arch, GlobalConfig};
use crate::create::{self, CreateEvent, CreateRequest, Hooks};
use crate::environment::{self, Environment};
use crate::gog::account::{CallError, Session};
use crate::gog::folder::{
    self, AdoptionLock, Attempted, EntryType, FolderLock, FolderState, Journal, JournalKind,
    Platform, Product, ProductStatus, Target, Track, WinningEntry, failpoint,
};
use crate::gog::install_dir::install_folder;
use crate::gog::migrate::{self, host_only};
use crate::gog::package::{self, SpaceNeeded};
use crate::gog::update::UpdateError;
use crate::gog::update::apply::{free_space, is_cancel, lock_operation_on, operation_id};
use crate::gog::update::install::{self as offline, Creation, Installed, Request};
use crate::gog::update::publish::{self, Current, Known, Publisher};
use crate::gog::update::windows::{self, Event};
use crate::gog::{language, names, product};
use crate::paths::Paths;
use crate::supervisor::Watched;

/// The id GOG's script interpreter has among the redistributables.
pub(super) const SCRIPT_INTERPRETER: &str = "ISI";
/// Where a redistributable's program is when it belongs in the store
/// rather than the game folder.
const STORE_PREFIX: &str = "__redist";
/// Files downloaded without a known MD5 wait for the journal to name them
/// in batches of this many before they're put in place.
pub(super) const NAME_BATCH: usize = 32;

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum GalaxyError {
    #[error("Galaxy builds are only for Windows games.")]
    NotWindows,
    /// No generation 2 build on the default branch.
    #[error("GOG lists no Galaxy build of {title} that wineo can install.")]
    NoBuild { title: String },
    #[error("{title}'s build {build} has no files in {language} or English.")]
    NoLanguage {
        title: String,
        build: String,
        language: String,
    },
    /// The environment's own unfinished install is from offline installers.
    #[error(
        "{name}'s install didn't finish. Install it again from its offline installer to finish it."
    )]
    InstallerPending { name: String },
    /// A product the interrupted install began with isn't in the build it
    /// finishes with, or one a game has installed isn't in the build an
    /// update, reinstall, move, or repair goes to. `build` is the build's
    /// version name where one is known, else its id.
    #[error("{}", product_gone(.title, .build, .label, *.installed))]
    ProductGone {
        title: String,
        build: String,
        label: String,
        /// The product is installed, not just begun by an install.
        installed: bool,
    },
    /// A setup step in the prefix failed. The journal stays, and installing
    /// again carries on from that step.
    #[error("Setting up {title} stopped: {label} failed ({detail}). Install it again to finish.")]
    Setup {
        title: String,
        label: String,
        detail: String,
    },
    /// A repair of a build GOG no longer lists. A repair never changes the
    /// build, so it can't carry on with another.
    #[error(
        "GOG no longer lists build {build} of {title}. Run wineo update {name} to move to the latest."
    )]
    BuildGone {
        title: String,
        build: String,
        name: String,
    },
    /// A repair of a game whose installed build isn't known.
    #[error(
        "The installed build of {title} isn't known. Run wineo update {name} to move to the latest."
    )]
    BuildUnknown { title: String, name: String },
    /// `--branch` names a branch GOG lists no generation 2 build on, which
    /// includes every private branch until passwords are supported.
    #[error("{}", no_branch(.title, .branch, .name.as_deref()))]
    NoBranch {
        title: String,
        branch: String,
        /// The environment, for the way to its listing. None before a game
        /// is installed.
        name: Option<String>,
    },
    /// `--build` names a build GOG doesn't list as a generation 2 build.
    #[error("{}", no_such_build(.build, .title, .name.as_deref()))]
    NoSuchBuild {
        build: String,
        title: String,
        name: Option<String>,
    },
    /// `--build` names a version more than one build has.
    #[error("{version} matches builds {}. Pick one with --build ID.", joined(.builds))]
    AmbiguousBuild {
        version: String,
        builds: Vec<String>,
    },
    /// The branch a game follows has no generation 2 build now.
    #[error("{}", branch_empty(.name, .branch))]
    BranchEmpty { name: String, branch: String },
    /// A reinstall of a game pinned to a build GOG no longer lists.
    #[error(
        "GOG no longer lists build {build} of {title}, which {name} is pinned to. Run wineo update {name} --latest to follow its branch again."
    )]
    PinGone {
        title: String,
        build: String,
        name: String,
    },
    /// A branch or build asked for while an operation is waiting to finish.
    #[error("{name} has an unfinished GOG operation, so it can't change its branch or build yet.")]
    Unfinished { name: String },
    #[error(transparent)]
    Layout(#[from] LayoutError),
}

fn branch_empty(name: &str, branch: &str) -> String {
    crate::gog::update::Reason::Galaxy {
        name: name.to_owned(),
        branch: Some(branch.to_owned()),
    }
    .note()
}

fn see_builds(name: Option<&str>) -> String {
    name.map(|name| format!(" See: wineo update {name} --builds"))
        .unwrap_or_default()
}

fn no_branch(title: &str, branch: &str, name: Option<&str>) -> String {
    format!(
        "GOG lists no branch {branch} for {title}.{}",
        see_builds(name)
    )
}

fn no_such_build(build: &str, title: &str, name: Option<&str>) -> String {
    format!(
        "GOG doesn't list build {build} of {title}.{}",
        see_builds(name)
    )
}

/// `A`, `A and B`, or `A, B, and C`.
fn joined(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

fn product_gone(title: &str, build: &str, label: &str, installed: bool) -> String {
    if installed {
        format!("Build {build} of {title} doesn't have {label}, which is installed.")
    } else {
        format!("Build {build} of {title} no longer has {label}, which the install began with.")
    }
}

/// One product a Galaxy install would put in the folder, for a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProduct {
    pub id: u64,
    /// GOG's title, when its details list one.
    pub title: Option<String>,
    /// The build it comes from. None when unavailable.
    pub build: Option<String>,
    /// The depot language it installs in. None when unavailable.
    pub language: Option<String>,
    pub status: ProductStatus,
    /// Why it's unavailable, or that it falls back to English.
    pub note: Option<String>,
    /// What its files take, support files included.
    pub size: u64,
}

/// A finished install's build that GOG no longer lists, and the one it
/// finishes with instead. The review shows it, which is its confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retarget {
    pub from: String,
    pub to: String,
}

/// A redistributable version the install uses, with its files when it
/// isn't in the store yet.
#[derive(Debug, Clone)]
pub(super) struct StoreNeed {
    pub(super) version: StoreVersion,
    pub(super) files: Option<Vec<FileEntry>>,
}

/// A Galaxy install, resolved and ready to apply.
#[derive(Debug, Clone)]
pub struct Plan {
    environment: String,
    gog_id: u64,
    title: String,
    folder: PathBuf,
    requested: String,
    desktop: bool,
    resume: bool,
    has_config: bool,
    seen: Option<FolderState>,
    creation: Option<Creation>,
    arch: Arch,
    version_name: Option<String>,
    products: Vec<PlannedProduct>,
    target: Target,
    manifest: Manifest,
    store: Vec<StoreNeed>,
    retarget: Option<Retarget>,
    /// What the record said the game follows when planning looked, and what
    /// the install leaves it following.
    from: Follow,
    follow: Follow,
    warnings: Vec<String>,
}

impl Plan {
    pub fn environment(&self) -> &str {
        &self.environment
    }

    pub fn gog_id(&self) -> u64 {
        self.gog_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The install folder, in canonical form.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The locale tag asked for.
    pub fn requested_language(&self) -> &str {
        &self.requested
    }

    /// The depot language the base game installs in, which the folder's
    /// state records.
    pub fn language(&self) -> &str {
        &self.target.language
    }

    /// The base game's build.
    pub fn build(&self) -> &str {
        &self.manifest.build
    }

    /// GOG's name for the build, like `1.0.4`, when it was listed.
    pub fn version_name(&self) -> Option<&str> {
        self.version_name.as_deref()
    }

    /// The prefix arch whose depots install.
    pub fn arch(&self) -> Arch {
        self.arch
    }

    pub fn resumes(&self) -> bool {
        self.resume
    }

    pub fn has_config(&self) -> bool {
        self.has_config
    }

    /// The base game first, then each owned DLC in GOG's order.
    pub fn products(&self) -> &[PlannedProduct] {
        &self.products
    }

    pub fn creation(&self) -> Option<&Creation> {
        self.creation.as_ref()
    }

    /// Set when an interrupted install's build is gone from GOG's list and
    /// it finishes with the latest one instead.
    pub fn retarget(&self) -> Option<&Retarget> {
        self.retarget.as_ref()
    }

    /// The branch and pin the install leaves the game with.
    pub fn follow(&self) -> &Follow {
        &self.follow
    }

    /// The files, support files, and redistributables it takes.
    pub fn installed_size(&self) -> u64 {
        let store: u64 = self
            .store
            .iter()
            .flat_map(|need| need.files.iter().flatten())
            .map(|file| file.size)
            .sum();
        sizes(&self.manifest.files) + sizes(&self.manifest.support) + store
    }

    pub fn support_files(&self) -> usize {
        self.manifest.support.len()
    }

    /// The redistributables setup uses, by id.
    pub fn redistributables(&self) -> Vec<&str> {
        self.manifest
            .setup
            .redist
            .iter()
            .map(|redist| redist.dependency.as_str())
            .collect()
    }

    /// The setup steps that run, by name: "script interpreter", "setup
    /// program", or a redistributable's id.
    pub fn setup_steps(&self, paths: &Paths) -> Vec<String> {
        setup::planned_steps(
            paths,
            &self.folder,
            &self.manifest,
            &self.target,
            &self.title,
        )
        .into_iter()
        .map(|step| step.name)
        .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    #[cfg(test)]
    pub(super) fn manifest_for_tests(&self) -> &Manifest {
        &self.manifest
    }
}

pub(super) fn sizes(files: &[FileEntry]) -> u64 {
    files.iter().map(|file| file.size).sum()
}

/// How far `apply` has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Bytes downloaded so far, of what isn't in place yet.
    Download { done: u64, total: u64 },
    /// Setup steps run in the prefix so far, of those to run, and the one
    /// starting now (None once all are done): "script interpreter", "setup
    /// program", or a redistributable's id.
    Setup {
        done: usize,
        total: usize,
        step: Option<String>,
    },
    /// Files hashed against the manifest so far, support files included,
    /// before a repair or reinstall fetches what is wrong.
    Verify {
        files: usize,
        files_total: usize,
        bytes: u64,
        bytes_total: u64,
    },
}

/// Works out what installing `request.gog_id` from its latest Galaxy build
/// does. Reads only: it takes no locks and writes nothing but GOG's product
/// details cache, unless `read_only`, and a renewed login.
///
/// The environment's own interrupted Galaxy install is carried on instead,
/// from its journal and record. Refused in a portable root, for a native
/// platform, for a name already taken, for a folder that overlaps another
/// game's or isn't empty, and for a game with no generation 2 build on its
/// default branch.
pub fn plan(
    paths: &Paths,
    session: &mut Session,
    request: &Request,
    _token: &Token,
) -> Result<Plan> {
    host_only(paths)?;
    if request.platform == Some(Platform::Linux) {
        return Err(GalaxyError::NotWindows.into());
    }
    // Setup runs programs in the prefix, as a Windows installer does.
    windows::check_graphical()?;
    let global = GlobalConfig::load(paths)?;
    let requested = request
        .language
        .clone()
        .unwrap_or_else(|| global.gog_language().to_owned());
    language::validate(&requested).map_err(anyhow::Error::msg)?;
    let name = request.name.as_str();
    let exists = paths.environment_config(name).exists();
    if !exists {
        environment::check_new_name(paths, name)?;
    }

    if let Some(pending) = offline::own_pending(paths, request)? {
        if exists
            && crate::gog::update::recorded_gog_id(paths, name)? != Some(request.gog_id.to_string())
        {
            environment::check_new_name(paths, name)?;
        }
        let folder = folder::check_fresh_install(paths, &pending.folder, name)?;
        offline::check_pending(paths, name, &folder)?;
        return resumed(paths, session, request, folder, requested, exists, pending);
    }

    let details = product::load_with(
        paths,
        session.client(),
        request.gog_id,
        request.refresh,
        !request.read_only,
    )?;
    let mut warnings: Vec<String> = details.warning.into_iter().collect();
    let game = details.product;
    let title = if game.title.is_empty() {
        format!("GOG game {}", request.gog_id)
    } else {
        game.title.clone()
    };
    let folder = install_folder(&global.games_root(paths)?, &title, request.dir.as_deref())?;
    let folder = folder::check_fresh_install(paths, &folder, name)?;
    let seen = folder::peek(paths, &folder)?;
    let own_install = seen.as_ref().is_some_and(|state| {
        state.owner.as_deref() == Some(name)
            && state.gog_id() == Some(request.gog_id)
            && state
                .journal
                .as_ref()
                .is_some_and(|journal| journal.kind == JournalKind::Install)
    });
    let resume = own_install
        && (!exists
            || crate::gog::update::recorded_gog_id(paths, name)?
                == Some(request.gog_id.to_string()));
    if exists && !resume {
        environment::check_new_name(paths, name)?;
    }
    offline::check_pending(paths, name, &folder)?;
    if let Some(state) = &seen
        && !resume
    {
        let install = state
            .journal
            .as_ref()
            .is_some_and(|journal| journal.kind == JournalKind::Install);
        return Err(match &state.owner {
            Some(owner) if owner != name => folder::FolderError::OwnedByOther {
                folder,
                owner: owner.clone(),
            },
            None if install => folder::unfinished(paths, state),
            _ => folder::FolderError::Tracked { folder },
        }
        .into());
    }
    if resume {
        let seen = seen.expect("a resumed install has a state");
        return resumed(paths, session, request, folder, requested, exists, seen);
    }

    let gog_id = request.gog_id;
    let builds = builds_of(session, gog_id, None)?;
    let no_build = || GalaxyError::NoBuild {
        title: title.clone(),
    };
    let who = Who {
        title: &title,
        name: None,
    };
    // A new install follows the default branch unless it names another.
    let picked = follow::pick(&builds, &request.builds, &Follow::default(), false, &who)?;
    let latest = picked.build;
    let link = latest.link.clone().ok_or_else(no_build)?;
    let repository = session
        .client()
        .repository(&link)
        .with_context(|| format!("Couldn't get {title}'s build {}", latest.build_id))?;
    let creation = offline::creation(paths, request, Platform::Windows, exists)?
        .ok_or_else(|| anyhow!("{name} already has a config"))?;
    let arch = creation.resolved_arch();
    let bits = build::bits(arch);

    let base = build::pick_language(&repository.depots, gog_id, &requested)
        .filter(|language| {
            repository
                .depots
                .iter()
                .any(|depot| build::keeps(depot, gog_id, &language.code, bits))
        })
        .ok_or_else(|| GalaxyError::NoLanguage {
            title: title.clone(),
            build: repository.build_id.clone(),
            language: requested.clone(),
        })?;
    if base.english {
        warnings.push(format!(
            "{title} has no Galaxy files in {requested}, so it installs in English."
        ));
    }

    // The owned games list may leave DLC out, so ownership comes from the
    // account's full list of owned ids, as for an offline install.
    let owned: HashSet<u64> = match call(session, |client, access| client.owned_ids(access)) {
        Ok(ids) => ids.into_iter().collect(),
        Err(error) => {
            if !game.dlc_ids.is_empty() {
                warnings.push(format!(
                    "Couldn't check which DLC you own, so none is installed: {error:#}"
                ));
            }
            HashSet::new()
        }
    };
    let mut products = vec![PlannedProduct {
        id: gog_id,
        title: Some(title.clone()),
        build: Some(repository.build_id.clone()),
        language: Some(base.code.clone()),
        status: ProductStatus::Installed,
        note: None,
        size: 0,
    }];
    let mut chosen = vec![(gog_id, base.code.clone())];
    let mut seen_dlc = HashSet::new();
    for &id in &game.dlc_ids {
        if !owned.contains(&id) || !seen_dlc.insert(id) {
            continue;
        }
        let dlc_title = game
            .dlcs
            .iter()
            .find(|dlc| dlc.id == id)
            .map(|dlc| dlc.title.clone())
            .filter(|title| !title.is_empty());
        let label = names::dlc_label(&title, id, dlc_title.as_deref());
        // In the language the base game installs in, then English.
        let picked = build::pick_language(&repository.depots, id, &base.code).filter(|language| {
            repository
                .depots
                .iter()
                .any(|depot| build::keeps(depot, id, &language.code, bits))
        });
        match picked {
            Some(language) => {
                let note = language.english.then(|| {
                    format!(
                        "{label} has no Galaxy files in {requested}, so it installs in English."
                    )
                });
                chosen.push((id, language.code.clone()));
                products.push(PlannedProduct {
                    id,
                    title: dlc_title,
                    build: Some(repository.build_id.clone()),
                    language: Some(language.code),
                    status: ProductStatus::Installed,
                    note,
                    size: 0,
                });
            }
            None => products.push(PlannedProduct {
                id,
                title: dlc_title,
                build: None,
                language: None,
                status: ProductStatus::Unavailable,
                note: Some(format!(
                    "{label} isn't in build {} of {title}.",
                    repository.build_id
                )),
                size: 0,
            }),
        }
    }

    let built = manifests(
        session,
        &title,
        &repository,
        &chosen,
        bits,
        latest.version_name.clone(),
    )?;
    warnings.extend(built.warnings);
    let store = complete_store(session, built.store)?;
    for product in &mut products {
        product.size = product_size(&built.manifest, product.id);
    }
    let target = Target {
        gog_id,
        platform: Platform::Windows,
        language: base.code.clone(),
        track: Track::Galaxy,
        products: products
            .iter()
            .map(|planned| Product {
                id: planned.id,
                version: None,
                build: planned.build.clone(),
                // Recorded only where it differs from the folder's.
                language: planned
                    .language
                    .clone()
                    .filter(|language| *language != base.code),
                reported_language: None,
                status: planned.status,
            })
            .collect(),
    };
    let from = files_record(paths, &folder)
        .map(|record| record.follow)
        .unwrap_or_default();
    Ok(Plan {
        environment: name.to_owned(),
        gog_id,
        title,
        folder,
        requested,
        desktop: request.desktop,
        resume: false,
        has_config: false,
        seen,
        creation: Some(creation),
        arch,
        version_name: latest.version_name.clone(),
        products,
        target,
        manifest: built.manifest,
        store,
        retarget: None,
        from,
        follow: picked.to,
        warnings,
    })
}

/// An authenticated request, its login failures kept as they are.
pub(super) fn call<T>(
    session: &mut Session,
    request: impl Fn(&wineo_gog::Client, &str) -> Result<T, wineo_gog::Error>,
) -> Result<T> {
    session.call(request).map_err(|error| match error {
        CallError::Account(error) => anyhow::Error::from(error),
        CallError::Request(error) => anyhow::Error::from(error),
    })
}

/// The builds GOG lists for a game. A private branch's builds need its
/// password, which is the seam `_password` is for; none is sent yet.
pub(super) fn builds_of(
    session: &mut Session,
    gog_id: u64,
    _password: Option<&str>,
) -> Result<wineo_gog::Builds> {
    call(session, |client, access| {
        client.windows_builds(gog_id, Some(access))
    })
}

pub(super) fn product_size(manifest: &Manifest, id: u64) -> u64 {
    manifest
        .files
        .iter()
        .chain(&manifest.support)
        .filter(|file| file.product == Some(id))
        .map(|file| file.size)
        .sum()
}

/// Plans carrying on the environment's own interrupted Galaxy install. Its
/// record's `pending` is the target when it belongs to the journal; else
/// the journal's builds are fetched again, or GOG's latest when the base
/// game's is no longer listed. Every product it began with stays.
#[allow(clippy::too_many_arguments)]
fn resumed(
    paths: &Paths,
    session: &mut Session,
    request: &Request,
    folder: PathBuf,
    requested: String,
    has_config: bool,
    seen: FolderState,
) -> Result<Plan> {
    let name = request.name.as_str();
    let journal = seen
        .journal
        .clone()
        .expect("a resumed install has a journal");
    if journal.target.track != Track::Galaxy {
        return Err(GalaxyError::InstallerPending {
            name: name.to_owned(),
        }
        .into());
    }
    let gog_id = journal.target.gog_id;
    let names = names::product_names(&folder);
    // Not needed to finish, so a failure only costs the titles.
    let details = product::load_with(
        paths,
        session.client(),
        gog_id,
        request.refresh,
        !request.read_only,
    )
    .ok()
    .map(|details| details.product);
    let title = details
        .as_ref()
        .map(|details| details.title.clone())
        .filter(|title| !title.is_empty())
        .or_else(|| names.get(&gog_id).cloned())
        .unwrap_or_else(|| format!("GOG game {gog_id}"));
    let creation = offline::creation(paths, request, Platform::Windows, has_config)?;
    let arch = match &creation {
        Some(creation) => creation.resolved_arch(),
        None => Environment::load(paths, name)?.wine()?.arch,
    };
    let bits = build::bits(arch);
    let mut warnings = Vec::new();
    let mut target = journal.target.clone();
    let mut retarget = None;
    let version_name;

    let record = files_record(paths, &folder);
    // A record that survived says what the install is for, even when
    // `pending` didn't.
    let from = record
        .as_ref()
        .map(|record| record.follow.clone())
        .unwrap_or_default();
    let mut follow = from.clone();
    let (manifest, store) = match record
        .as_ref()
        .and_then(|record| record::pending(&seen, record))
    {
        Some(pending) => {
            let store = pending
                .store
                .iter()
                .map(|version| StoreNeed {
                    version: version.clone(),
                    files: None,
                })
                .collect();
            version_name = pending.manifest.setup.version_name.clone();
            follow = pending.follow.clone().unwrap_or_else(|| from.clone());
            (pending.manifest.clone(), store)
        }
        None => {
            let builds = builds_of(session, gog_id, None)?;
            let began = target
                .products
                .iter()
                .find(|product| product.id == gog_id)
                .and_then(|base| base.build.clone())
                .unwrap_or_default();
            let listed = builds
                .items
                .iter()
                .find(|build| build.build_id == began && build.generation == Some(2));
            let chosen = match listed {
                Some(build) => build,
                None => {
                    // The latest of the branch followed, with the pin
                    // dropped, since the build it named is gone.
                    let who = Who {
                        title: &title,
                        name: None,
                    };
                    let latest = follow::latest_on(&builds, from.branch.as_deref(), &who)?;
                    let pin = from
                        .pinned
                        .as_deref()
                        .filter(|pinned| *pinned == began)
                        .map(|pinned| format!(" The pin to build {pinned} is removed."))
                        .unwrap_or_default();
                    follow = Follow {
                        branch: from.branch.clone(),
                        pinned: None,
                    };
                    warnings.push(format!(
                        "GOG no longer lists build {began} of {title}, so the install finishes with build {} instead.{pin}",
                        latest.build_id
                    ));
                    retarget = Some(Retarget {
                        from: began.clone(),
                        to: latest.build_id.clone(),
                    });
                    latest
                }
            };
            version_name = chosen.version_name.clone();
            let link = chosen.link.clone().ok_or_else(|| GalaxyError::NoBuild {
                title: title.clone(),
            })?;
            let repository = session
                .client()
                .repository(&link)
                .with_context(|| format!("Couldn't get {title}'s build {}", chosen.build_id))?;
            let mut chosen_products = Vec::new();
            for product in &mut target.products {
                if product.status != ProductStatus::Installed {
                    continue;
                }
                let language = product
                    .language
                    .clone()
                    .unwrap_or_else(|| target.language.clone());
                if !repository
                    .depots
                    .iter()
                    .any(|depot| build::keeps(depot, product.id, &language, bits))
                {
                    let label = if product.id == gog_id {
                        title.clone()
                    } else {
                        names::dlc_label(
                            &title,
                            product.id,
                            names.get(&product.id).map(String::as_str),
                        )
                    };
                    return Err(GalaxyError::ProductGone {
                        title,
                        build: repository.build_id.clone(),
                        label,
                        installed: false,
                    }
                    .into());
                }
                product.build = Some(repository.build_id.clone());
                chosen_products.push((product.id, language));
            }
            let built = manifests(
                session,
                &title,
                &repository,
                &chosen_products,
                bits,
                chosen.version_name.clone(),
            )?;
            warnings.extend(built.warnings);
            (built.manifest, built.store)
        }
    };
    let store = complete_store(session, store)?;
    let products = target
        .products
        .iter()
        .map(|product| PlannedProduct {
            id: product.id,
            title: if product.id == gog_id {
                Some(title.clone())
            } else {
                details
                    .as_ref()
                    .and_then(|details| details.dlcs.iter().find(|dlc| dlc.id == product.id))
                    .map(|dlc| dlc.title.clone())
                    .filter(|title| !title.is_empty())
            },
            build: product.build.clone(),
            language: (product.status == ProductStatus::Installed).then(|| {
                product
                    .language
                    .clone()
                    .unwrap_or_else(|| target.language.clone())
            }),
            status: product.status,
            note: None,
            size: product_size(&manifest, product.id),
        })
        .collect();
    Ok(Plan {
        environment: name.to_owned(),
        gog_id,
        title,
        folder,
        requested,
        desktop: request.desktop,
        resume: true,
        has_config,
        seen: Some(seen),
        creation,
        arch,
        version_name,
        products,
        target,
        manifest,
        store,
        retarget,
        from,
        follow,
        warnings,
    })
}

/// The folder's record read without its lock, for planning.
pub(super) fn files_record(paths: &Paths, folder: &Path) -> Option<Record> {
    let path = folder::sidecar_file(paths, folder);
    let bytes = std::fs::read(path).ok()?;
    Record::parse(&bytes)
}

/// What a build's manifests come to.
pub(super) struct Built {
    pub(super) manifest: Manifest,
    pub(super) store: Vec<StoreNeed>,
    pub(super) warnings: Vec<String>,
}

/// Fetches the depot manifests of each chosen product's depots and of the
/// redistributables the build lists, and puts them together. Those whose
/// program lives under `__redist` go to the store; the rest, including ones
/// with no program, are part of the game.
pub(super) fn manifests(
    session: &mut Session,
    title: &str,
    repository: &wineo_gog::Repository,
    chosen: &[(u64, String)],
    bits: &str,
    version_name: Option<String>,
) -> Result<Built> {
    let failed = || format!("Couldn't get {title}'s file lists");
    let mut fetched = Vec::new();
    for (id, language) in chosen {
        for depot in &repository.depots {
            if build::keeps(depot, *id, language, bits) {
                let manifest = session
                    .client()
                    .depot_manifest(&depot.manifest)
                    .with_context(failed)?;
                fetched.push((Owner::Product(*id), manifest));
            }
        }
    }

    let mut wanted: Vec<String> = repository.dependencies.clone();
    if repository.script_interpreter && !wanted.iter().any(|id| id == SCRIPT_INTERPRETER) {
        wanted.push(SCRIPT_INTERPRETER.to_owned());
    }
    let mut warnings = Vec::new();
    let mut redist = Vec::new();
    let mut store = Vec::new();
    if !wanted.is_empty() {
        let found = call(session, |client, access| {
            client.dependency_repository(Some(access))
        })
        .and_then(|repo| Ok(session.client().dependency_manifest(&repo)?))
        .context("Couldn't get GOG's redistributables")?;
        let mut seen = HashSet::new();
        for id in &wanted {
            if !seen.insert(id.as_str()) {
                continue;
            }
            // Like GOG's own clients, a listed id GOG doesn't offer is left
            // out rather than failing the install.
            let Some(dependency) = found.dependencies.iter().find(|dep| dep.id == *id) else {
                warnings.push(format!(
                    "GOG's redistributables don't include {id}, which {title} lists, so it's left out."
                ));
                continue;
            };
            if !store::valid_id(&dependency.id) {
                anyhow::bail!(
                    "GOG's redistributable {:?} has an unusable id",
                    dependency.id
                );
            }
            let in_store = dependency.executable.as_ref().is_some_and(|exe| {
                exe.path
                    .replace('\\', "/")
                    .starts_with(&format!("{STORE_PREFIX}/"))
            });
            if in_store {
                let version = StoreVersion {
                    dependency: dependency.id.clone(),
                    manifest: dependency.manifest.clone(),
                };
                redist.push(Redist {
                    dependency: dependency.id.clone(),
                    manifest: dependency.manifest.clone(),
                    executable: dependency
                        .executable
                        .as_ref()
                        .map(|exe| exe.path.replace('\\', "/")),
                    arguments: dependency
                        .executable
                        .as_ref()
                        .map(|exe| exe.arguments.clone())
                        .unwrap_or_default(),
                });
                store.push(StoreNeed {
                    version,
                    files: None,
                });
            } else {
                let manifest = session
                    .client()
                    .dependency_depot_manifest(&dependency.manifest)
                    .with_context(failed)?;
                fetched.push((Owner::Dependency(dependency.id.clone()), manifest));
            }
        }
    }
    let kept: Vec<u64> = chosen.iter().map(|(id, _)| *id).collect();
    let setup = SetupInfo {
        script_interpreter: repository.script_interpreter,
        products: build::product_setups(repository, &kept),
        redist,
        version_name,
    };
    let sources: Vec<(Owner, &wineo_gog::DepotManifest)> = fetched
        .iter()
        .map(|(owner, manifest)| (owner.clone(), manifest))
        .collect();
    let manifest =
        build::assemble(&repository.build_id, &sources, setup).map_err(GalaxyError::Layout)?;
    Ok(Built {
        manifest,
        store,
        warnings,
    })
}

/// Fetches the file list of each store version that isn't in the store yet.
pub(super) fn complete_store(
    session: &mut Session,
    store: Vec<StoreNeed>,
) -> Result<Vec<StoreNeed>> {
    let paths = session.paths().clone();
    store
        .into_iter()
        .map(|mut need| {
            if need.files.is_none() && !store::version_dir(&paths, &need.version).is_dir() {
                need.files = Some(store_files(session, &need.version)?);
            }
            Ok(need)
        })
        .collect()
}

pub(super) fn store_files(session: &Session, version: &StoreVersion) -> Result<Vec<FileEntry>> {
    let manifest = session
        .client()
        .dependency_depot_manifest(&version.manifest)
        .with_context(|| format!("Couldn't get the file list of {}", version.dependency))?;
    let built = build::assemble(
        "",
        &[(Owner::Dependency(version.dependency.clone()), &manifest)],
        SetupInfo::default(),
    )
    .map_err(GalaxyError::Layout)?;
    Ok(built.files)
}

/// Installs a planned Galaxy game. A failure or cancel after the journal is
/// written leaves it, so the game refuses to launch until installing again
/// finishes it. `UpdateError::Canceled` says whether it did.
pub fn apply(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
) -> Result<Installed> {
    apply_with_events(paths, session, plan, token, progress, |_| {})
}

/// `apply`, reporting the environment's creation to `events`.
pub fn apply_with_events(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
    mut events: impl FnMut(Event),
) -> Result<Installed> {
    host_only(paths)?;
    let mut journaled = plan.resume;
    let progress = RefCell::new(progress);
    match run(
        paths,
        session,
        plan,
        token,
        &progress,
        &mut events,
        &mut journaled,
    ) {
        Err(error) if token.is_canceled() && is_cancel(&error) => {
            Err(UpdateError::Canceled { journal: journaled }.into())
        }
        result => result,
    }
}

/// Passes environment creation's events on.
pub(super) struct Forward<'a, 'b> {
    pub(super) events: &'a mut (dyn FnMut(Event) + 'b),
}

impl Hooks for Forward<'_, '_> {
    type Stop = Infallible;

    fn event(&mut self, event: CreateEvent) {
        (self.events)(Event::Create(event));
    }

    fn watched(&mut self, watched: Watched) {
        (self.events)(Event::Watched(watched));
    }
}

fn run(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    events: &mut dyn FnMut(Event),
    journaled: &mut bool,
) -> Result<Installed> {
    let name = plan.environment.as_str();
    let folder = plan.folder.as_path();
    let cancel = || token.is_canceled();
    let changed = |what: &str| offline::InstallError::Changed {
        what: what.to_owned(),
    };
    let mut warnings = plan.warnings.clone();
    windows::check_graphical()?;
    let (game, support) = unverified_sizes(paths, plan, token)?;
    check_space(
        paths,
        plan,
        game,
        support,
        planned_store(paths, &plan.store),
    )?;

    let prefix = match &plan.creation {
        Some(creation) => crate::paths::resolve(creation.prefix()),
        None => crate::paths::resolve(&Environment::load(paths, name)?.prefix(paths)?),
    };
    let adoption = AdoptionLock::acquire(paths, &cancel)?;
    let guard = lock_operation_on(paths, name, &[prefix.as_path()])?;
    let _in_use = folder::lock_for_update(paths, folder, name)?;
    let lock = FolderLock::exclusive(paths, folder, &cancel)?;
    let state = lock.read()?;
    if state != plan.seen {
        return Err(changed(&folder.display().to_string()).into());
    }
    // Another wineo may have pinned the game since planning.
    if record::follow_now(&lock)? != plan.from {
        return Err(changed(&folder.display().to_string()).into());
    }
    if paths.environment_config(name).exists() != plan.has_config {
        return Err(changed(name).into());
    }
    if plan.has_config {
        if crate::gog::update::recorded_gog_id(paths, name)? != Some(plan.gog_id.to_string()) {
            return Err(changed(name).into());
        }
    } else {
        environment::check_new_name(paths, name)?;
    }
    offline::check_pending(paths, name, folder)?;
    if folder::check_fresh_install(paths, folder, name)? != folder {
        return Err(changed(&folder.display().to_string()).into());
    }
    // Every store version is held from before it's looked for until the
    // install ends, so cleanup can't take one between its download and its
    // use. This comes before anything is written: a version that was in the
    // store when planning looked may be gone by now, and its size has to
    // count toward the space check, which refuses before the first change.
    let mut held = Vec::new();
    let mut store_jobs = Vec::new();
    for need in &plan.store {
        let version = store::hold(paths, &need.version, token)?;
        if !version.present() {
            let files = match &need.files {
                Some(files) => files.clone(),
                None => store_files(session, &need.version)?,
            };
            store_jobs.push((held.len(), files));
        }
        held.push(version);
    }
    let missing: u64 = store_jobs.iter().map(|(_, files)| sizes(files)).sum();
    check_space(paths, plan, game, support, missing)?;
    // The last check before the first write.
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: false }.into());
    }
    failpoint("galaxy.create.before")?;

    let mut state = match state {
        Some(state) => state,
        None => {
            folder::create_dir_durable(folder)?;
            let mut state = FolderState::new(folder.to_owned(), Some(name.to_owned()));
            state.device = Some(
                std::fs::metadata(folder)
                    .with_context(|| format!("failed to inspect {}", folder.display()))?
                    .dev(),
            );
            state
        }
    };
    let root = package::open_folder(folder)?;
    let support_root = lock.support_dir().to_owned();
    let operation = state
        .journal
        .as_ref()
        .map_or_else(operation_id, |journal| journal.operation.clone());
    let mut publisher = Publisher::new(root.try_clone()?, &operation, token).require_no_replace();
    if let Some(journal) = &state.journal {
        // Whatever the interrupted attempt left half done goes first.
        let dirs = publish::remove_temporaries(&root, &journal.operation)?;
        publish::sync_folders(&root, dirs, token)?;
        if support_root.is_dir() {
            let support = package::open_folder(&support_root)?;
            let dirs = publish::remove_temporaries(&support, &journal.operation)?;
            publish::sync_folders(&support, dirs, token)?;
        }
        // What it had moved out of the way is deleted only on a hash match.
        publisher.recover_asides(|path| recorded(&journal.attempted, path))?;
    }

    // What GOG calls each path, spelled as it already is on disk.
    let mut manifest = on_disk(&root, &plan.manifest);
    let expected = record::expected(&state, JournalKind::Install, &plan.target);
    let selection = record::digest(&expected);
    let mut sidecar = record::read(&lock)?.unwrap_or_default();
    // Setup steps recorded count only while the record is this operation's
    // or the committed selection's; anything else runs every step again.
    if !record::carries_setup(&state, &sidecar)? {
        sidecar.setup.clear();
    }
    sidecar.pending = Some(record::Pending {
        operation: operation.clone(),
        selection: selection.clone(),
        manifest: manifest.clone(),
        store: manifest.store_versions(),
        base: None,
        follow: (!plan.follow.is_default()).then(|| plan.follow.clone()),
    });
    // Before the journal, so a journal always has its pending written.
    record::write(&lock, &sidecar)?;
    failpoint("galaxy.pending.after")?;

    let mut journal = state.journal.clone().unwrap_or_else(|| Journal {
        operation: operation.clone(),
        kind: JournalKind::Install,
        target: plan.target.clone(),
        packages: Vec::new(),
        attempted: Vec::new(),
    });
    journal.target = plan.target.clone();
    for dir in &manifest.directories {
        name_entry(
            &mut journal.attempted,
            &dir.path,
            EntryType::Directory,
            None,
        );
    }
    for link in &manifest.links {
        name_entry(
            &mut journal.attempted,
            &link.path,
            EntryType::Symlink,
            Some(&link.target),
        );
    }
    for file in &manifest.files {
        if let Some(md5) = &file.md5 {
            name_entry(
                &mut journal.attempted,
                &file.path,
                EntryType::File,
                Some(md5),
            );
        }
    }
    state.journal = Some(journal);
    if token.is_canceled() {
        return Err(UpdateError::Canceled {
            journal: *journaled,
        }
        .into());
    }
    lock.write(&state)?;
    *journaled = true;
    failpoint("galaxy.journal.after")?;
    // The state covers the folder now, so other installs and migrations see
    // it in their overlap checks.
    drop(adoption);

    // The state comes before the environment's config, so a crash between
    // them leaves a journal that installing again finishes.
    let guard = match &plan.creation {
        Some(creation) if !plan.has_config => {
            let request = CreateRequest {
                name: name.to_owned(),
                resolved_runner: Some(creation.runner().clone()),
                arch: creation.arch(),
                expected_adopted: Some(creation.adopted()),
                gog_id: Some(plan.gog_id.to_string()),
                ..CreateRequest::default()
            };
            let mut hooks = Forward { events };
            let created =
                create::create_and_install_held(paths, request, None, guard, token, &mut hooks)?;
            created
                .guard
                .ok_or_else(|| anyhow!("creating {name} let go of its locks"))?
        }
        _ => guard,
    };
    failpoint("galaxy.environment.after")?;

    // Folders and links first, deciding what each file needs.
    let attempted = state
        .journal
        .as_ref()
        .expect("written above")
        .attempted
        .clone();
    publisher.note_folders(
        manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .chain(manifest.directories.iter().map(|dir| dir.path.as_str()))
            .chain(manifest.links.iter().map(|link| link.path.as_str()))
            .chain(attempted.iter().map(|entry| entry.path.as_str())),
    );
    let known = |path: &str| recorded(&attempted, path);
    let mut placer = Placer {
        publisher: &mut publisher,
        known: &known,
        token,
    };
    let mut fetch_files = Vec::new();
    let mut in_place = Vec::new();
    for (path, kind) in entries_in_order(&manifest) {
        match kind {
            Kind::Directory => placer.directory(path)?,
            Kind::Link(target) => placer.link(path, target)?,
            Kind::File(index) => match placer.file(&manifest.files[index])? {
                Some(md5) => in_place.push((index, md5)),
                None => fetch_files.push(index),
            },
        }
    }
    for (index, md5) in in_place {
        manifest.files[index].md5 = Some(md5);
    }
    failpoint("galaxy.placed.after")?;

    // Support files: the folder is wineo's own, so anything there that
    // doesn't match is simply replaced.
    let support_fetch = support_to_fetch(&support_root, &mut manifest.support, token, &mut |_| {})?;
    let total: u64 = fetch_files
        .iter()
        .map(|&index| manifest.files[index].size)
        .chain(
            support_fetch
                .iter()
                .map(|&index| manifest.support[index].size),
        )
        .chain(
            store_jobs
                .iter()
                .flat_map(|(_, files)| files.iter().map(|file| file.size)),
        )
        .sum();
    let mut base = 0u64;
    let report = |done: u64| (progress.borrow_mut())(Progress::Download { done, total });
    // Each product's chunks come through its own secure link.
    let links = Links::new(
        fetch_files
            .iter()
            .map(|&index| &manifest.files[index])
            .chain(support_fetch.iter().map(|&index| &manifest.support[index])),
    );

    for (index, files) in &store_jobs {
        let mut counted = |done: u64| report(base + done);
        store::publish(
            paths,
            session,
            &held[*index],
            files,
            &links.dependencies,
            &operation,
            token,
            &mut counted,
        )?;
        base += sizes(files);
    }
    #[cfg(test)]
    super::tests::after_store(paths);

    {
        let mut counted = |done: u64| report(base + done);
        fetch_support(
            session,
            &support_root,
            &mut manifest.support,
            &support_fetch,
            &mut publisher,
            &links,
            token,
            &mut counted,
        )?;
        base += support_fetch
            .iter()
            .map(|&index| manifest.support[index].size)
            .sum::<u64>();
    }
    failpoint("galaxy.support.placed")?;
    sync_support(&support_root, &manifest.support)?;

    // The game's files, each renamed over its path once whole. One whose MD5
    // isn't known until it's downloaded waits for the journal to name it.
    let files = manifest.files.clone();
    let mut jobs = Vec::new();
    for &index in &fetch_files {
        let file = &files[index];
        jobs.push(Job {
            root: &root,
            parent: publish::parent_of(&file.path).to_owned(),
            name: publisher.temp_name(),
            file,
            links: links.of(file),
            reuse: None,
        });
    }
    let index_of: HashMap<&str, usize> = fetch_files
        .iter()
        .map(|&index| (files[index].path.as_str(), index))
        .collect();
    let mut waiting: Vec<Waiting> = Vec::new();
    let nothing = |_: &str| Known::default();
    {
        let mut counted = |done: u64| report(base + done);
        fetch::run(session, &jobs, token, &mut counted, &mut |job, md5| {
            let index = index_of[job.file.path.as_str()];
            let known = manifest.files[index].md5.is_some();
            manifest.files[index].md5 = Some(md5.clone());
            if known {
                place(&mut publisher, &state, &job.name, &job.file.path, &nothing)?;
            } else {
                waiting.push((job.file.path.clone(), job.name.clone(), md5));
                if waiting.len() >= NAME_BATCH {
                    flush(&lock, &mut waiting, &mut state, &mut publisher, &nothing)?;
                }
            }
            failpoint("galaxy.file.after")
        })?;
    }
    flush(&lock, &mut waiting, &mut state, &mut publisher, &nothing)?;
    publisher.finish(&[])?;
    let report_saved = publisher.into_report();
    failpoint("galaxy.files.after")?;

    // Setup in the prefix, between the files and the commit, holding the
    // environment's operation lock and the store versions in `held`.
    let label = |id: u64| {
        if id == plan.gog_id {
            return plan.title.clone();
        }
        let title = plan
            .products
            .iter()
            .find(|product| product.id == id)
            .and_then(|product| product.title.as_deref());
        names::dlc_label(&plan.title, id, title)
    };
    let (steps, setup_warnings) = setup::steps(&setup::Sources {
        paths,
        folder,
        support: &support_root,
        manifest: &manifest,
        target: &plan.target,
        label: &label,
        review: false,
    })?;
    warnings.extend(setup_warnings);
    let (guard, ran) = setup::run(
        &setup::Setup {
            paths,
            name,
            title: &plan.title,
            prefix: &prefix,
            lock: &lock,
            token,
        },
        guard,
        &mut sidecar,
        &steps,
        false,
        &mut |done, total, step| {
            (progress.borrow_mut())(Progress::Setup {
                done,
                total,
                step: step.map(str::to_owned),
            })
        },
        events,
    )?;
    failpoint("galaxy.setup.after")?;
    // Exactly the steps this selection has, so one it no longer lists
    // isn't kept as done.
    sidecar.setup = steps.into_iter().map(|step| step.identity).collect();

    // The default target comes from the game's info file, as for an offline
    // install.
    let gog_id = plan.gog_id;
    let info = windows::read_info(folder, gog_id);
    let launch = info.as_ref().and_then(|root| {
        migrate::windows_launch(folder, folder.join(format!("goggame-{gog_id}.info")), root).ok()
    });
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: true }.into());
    }
    let added = match &launch {
        Some(launch) => windows::add_target(paths, name, &plan.title, launch, plan.desktop)?,
        None => None,
    };

    // `installed` before the commit, so a crash after it already finds the
    // record matching what's committed.
    build::sort(&mut manifest);
    sidecar.installed = Some(record::Installed {
        selection,
        info: record::info_hash(folder, gog_id)?,
        manifest,
    });
    sidecar.follow = plan.follow.clone();
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: true }.into());
    }
    record::write(&lock, &sidecar)?;
    failpoint("galaxy.installed.after")?;
    publish::record_sync("commit".to_owned());
    lock.commit(expected)?;
    failpoint("galaxy.commit.after")?;
    sidecar.pending = None;
    if let Err(error) = record::write(&lock, &sidecar) {
        warnings.push(format!(
            "Couldn't tidy {}'s Galaxy record: {error:#}",
            plan.title
        ));
    }
    drop(lock);
    drop(guard);
    drop(held);
    if let Err(error) = store::clean(paths) {
        warnings.push(format!(
            "Couldn't clean up GOG's redistributables: {error:#}"
        ));
    }
    let products = plan.target.products.clone();
    Ok(Installed {
        environment: name.to_owned(),
        folder: folder.to_owned(),
        gog_id,
        title: plan.title.clone(),
        resumed: plan.resume,
        products,
        saved: report_saved.saved,
        kept: Vec::new(),
        overwritten: Vec::new(),
        warnings,
        target: added,
        reported_language: None,
        needs_restart: ran.needs_restart,
    })
}

/// The secure links a download uses: each product's, and the
/// redistributables'.
pub(super) struct Links {
    products: BTreeMap<u64, ChunkLinks>,
    pub(super) dependencies: ChunkLinks,
}

impl Links {
    /// Links for the products of `files`.
    pub(super) fn new<'f>(files: impl IntoIterator<Item = &'f FileEntry>) -> Self {
        let mut products = BTreeMap::new();
        for file in files {
            if let Some(product) = file.product {
                products
                    .entry(product)
                    .or_insert_with(|| ChunkLinks::new(LinkScope::Product(product)));
            }
        }
        Self {
            products,
            dependencies: ChunkLinks::new(LinkScope::Dependencies),
        }
    }

    pub(super) fn of(&self, file: &FileEntry) -> &ChunkLinks {
        match file.product {
            Some(product) => &self.products[&product],
            None => &self.dependencies,
        }
    }
}

/// A support file's path in the support folder: under its product's.
fn support_path(file: &FileEntry) -> Result<String> {
    let product = file
        .product
        .ok_or_else(|| anyhow!("support file {} has no product", file.path))?;
    Ok(format!("{product}/{}", file.path))
}

/// The support files not already in `root` whole, by index. Those that are
/// get their MD5. Reads only.
pub(super) fn support_to_fetch(
    root: &Path,
    support: &mut [FileEntry],
    token: &Token,
    checked: &mut dyn FnMut(&FileEntry),
) -> Result<Vec<usize>> {
    let fd = match package::open_folder(root) {
        Ok(fd) => Some(fd),
        Err(_) if !root.exists() => None,
        Err(error) => return Err(error),
    };
    let mut fetch = Vec::new();
    for (index, file) in support.iter_mut().enumerate() {
        let relative = support_path(file)?;
        let found = match &fd {
            Some(fd) => match fetch::open_dir(fd, publish::parent_of(&relative)) {
                Ok(dir) => verify(&dir, publish::leaf_of(&relative), &file.path, file, token)?,
                Err(_) => None,
            },
            None => None,
        };
        match found {
            Some((md5, true)) => file.md5 = Some(md5),
            _ => fetch.push(index),
        }
        checked(file);
    }
    Ok(fetch)
}

/// Downloads the support files at `fetch` into `root`, each renamed over
/// whatever is at its path, since the folder is wineo's own.
#[allow(clippy::too_many_arguments)]
pub(super) fn fetch_support(
    session: &mut Session,
    root: &Path,
    support: &mut [FileEntry],
    fetch: &[usize],
    publisher: &mut Publisher,
    links: &Links,
    token: &Token,
    progress: &mut dyn FnMut(u64),
) -> Result<()> {
    if fetch.is_empty() {
        return Ok(());
    }
    let mut names = Vec::new();
    for &index in fetch {
        let relative = support_path(&support[index])?;
        folder::create_dir_durable(&root.join(publish::parent_of(&relative)))?;
        names.push((
            index,
            publish::parent_of(&relative).to_owned(),
            publisher.temp_name(),
        ));
    }
    let fd = package::open_folder(root)?;
    let files = support.to_vec();
    let jobs: Vec<Job> = names
        .iter()
        .map(|(index, parent, temp)| Job {
            root: &fd,
            parent: parent.clone(),
            name: temp.clone(),
            file: &files[*index],
            links: links.of(&files[*index]),
            reuse: None,
        })
        .collect();
    fetch::run(session, &jobs, token, progress, &mut |job, md5| {
        let (index, parent, temp) = names
            .iter()
            .find(|(_, _, temp)| *temp == job.name)
            .expect("one of ours");
        let dir = fetch::open_dir(&fd, parent)?;
        rustix::fs::renameat(&dir, temp.as_str(), &dir, publish::leaf_of(&job.file.path))
            .with_context(|| format!("failed to put {} in place", job.file.path))?;
        support[*index].md5 = Some(md5);
        Ok(())
    })
}

/// Syncs every folder holding a support file, kept from an earlier attempt
/// or written now, since an attempt that crashed may have renamed files
/// into them without syncing. Deepest first, so each folder's entries are
/// durable before its parent's are.
pub(super) fn sync_support(root: &Path, support: &[FileEntry]) -> Result<()> {
    if support.is_empty() {
        return Ok(());
    }
    let fd = package::open_folder(root)?;
    let mut dirs = BTreeSet::new();
    for file in support {
        let mut dir = support_path(file)?;
        while let Some((up, _)) = dir.rsplit_once('/') {
            dir = up.to_owned();
            dirs.insert(dir.clone());
        }
    }
    dirs.insert(String::new());
    let mut dirs: Vec<String> = dirs.into_iter().collect();
    dirs.sort_by_key(|dir| std::cmp::Reverse(publish_depth(dir)));
    for dir in dirs {
        rustix::fs::fsync(fetch::open_dir(&fd, &dir)?)
            .with_context(|| format!("failed to sync support folder {dir:?}"))?;
        publish::record_sync(format!("support:{dir}"));
    }
    Ok(())
}

/// A downloaded file waiting for the journal to name it: its path, its
/// temporary name, and its MD5.
pub(super) type Waiting = (String, String, String);

/// Names the waiting files in the journal, then puts each in place. `base`
/// is what wineo recorded at a path besides the journal.
pub(super) fn flush(
    lock: &FolderLock,
    waiting: &mut Vec<Waiting>,
    state: &mut FolderState,
    publisher: &mut Publisher,
    base: &dyn Fn(&str) -> Known,
) -> Result<()> {
    if waiting.is_empty() {
        return Ok(());
    }
    let journal = state.journal.as_mut().expect("written before any download");
    for (path, _, md5) in waiting.iter() {
        name_entry(&mut journal.attempted, path, EntryType::File, Some(md5));
    }
    lock.write(state)?;
    failpoint("galaxy.named.after")?;
    for (path, temp, _) in waiting.drain(..) {
        place(publisher, state, &temp, &path, base)?;
    }
    Ok(())
}

/// Puts a downloaded file at its path. What stands there now is checked
/// again, since a program may have put or changed it while downloading: only
/// a file holding a hash the journal or `base` names for the path is
/// replaced.
pub(super) fn place(
    publisher: &mut Publisher,
    state: &FolderState,
    temp: &str,
    path: &str,
    base: &dyn Fn(&str) -> Known,
) -> Result<()> {
    #[cfg(test)]
    super::tests::before_place(path);
    let dir = publisher
        .open_parent(path)?
        .ok_or_else(|| anyhow!("{path}'s folder is missing while publishing"))?;
    let attempted = state
        .journal
        .as_ref()
        .map_or(&[][..], |journal| journal.attempted.as_slice());
    publisher.place_new(
        &dir,
        temp,
        path,
        &merged(recorded(attempted, path), base(path)),
    )
}

/// The file hashes and symlink targets wineo recorded for `path`.
pub(super) fn recorded(attempted: &[Attempted], path: &str) -> Known {
    let at = || attempted.iter().filter(move |entry| entry.path == path);
    Known {
        files: at()
            .filter(|entry| entry.kind == EntryType::File)
            .filter_map(|entry| entry.md5.clone())
            .collect(),
        links: at()
            .filter(|entry| entry.kind == EntryType::Symlink)
            .filter_map(|entry| entry.target.clone())
            .collect(),
        folder: at().any(|entry| entry.kind == EntryType::Directory),
    }
}

/// Both of what `a` and `b` record.
pub(super) fn merged(mut a: Known, b: Known) -> Known {
    a.files.extend(b.files);
    a.links.extend(b.links);
    a.folder |= b.folder;
    a
}

/// Adds an entry to `attempted` unless it's there already.
pub(super) fn name_entry(
    attempted: &mut Vec<Attempted>,
    path: &str,
    kind: EntryType,
    content: Option<&str>,
) {
    let entry = Attempted {
        path: path.to_owned(),
        kind,
        md5: (kind == EntryType::File).then(|| content.unwrap_or_default().to_owned()),
        target: (kind == EntryType::Symlink).then(|| content.unwrap_or_default().to_owned()),
    };
    if !attempted.contains(&entry) {
        attempted.push(entry);
    }
}

pub(super) enum Kind<'a> {
    Directory,
    Link(&'a str),
    File(usize),
}

/// Every entry in the game folder by path, so each folder comes before what
/// is in it.
pub(super) fn entries_in_order(manifest: &Manifest) -> Vec<(&str, Kind<'_>)> {
    let mut entries: Vec<(&str, Kind)> = manifest
        .directories
        .iter()
        .map(|dir| (dir.path.as_str(), Kind::Directory))
        .chain(
            manifest
                .links
                .iter()
                .map(|link| (link.path.as_str(), Kind::Link(link.target.as_str()))),
        )
        .chain(
            manifest
                .files
                .iter()
                .enumerate()
                .map(|(index, file)| (file.path.as_str(), Kind::File(index))),
        )
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
}

/// Decides each game folder entry against what is there: what already
/// matches stays, and anything else is renamed out of the way first, then
/// deleted only when it holds what wineo recorded there (`known`: what the
/// journal names, and for an update what it started from) and saved aside
/// otherwise.
pub(super) struct Placer<'a, 'b> {
    pub(super) publisher: &'a mut Publisher<'b>,
    pub(super) known: &'a dyn Fn(&str) -> Known,
    pub(super) token: &'a Token,
}

impl Placer<'_, '_> {
    fn parent(&self, path: &str) -> Result<OwnedFd> {
        self.publisher
            .open_parent(path)?
            .ok_or_else(|| anyhow!("{path}'s folder is missing while publishing"))
    }

    /// Clears `path` for a file to be renamed over when something other than
    /// a file stands there.
    pub(super) fn clear_for_file(&mut self, path: &str) -> Result<()> {
        let dir = self.parent(path)?;
        match publish::current(&dir, publish::leaf_of(path), path)? {
            None | Some(Current::File) => Ok(()),
            Some(other) => self.clear(&dir, path, &other),
        }
    }

    /// Makes way at `path` for an entry of another kind: wineo's own is
    /// removed, anything else moved aside.
    pub(super) fn clear(&mut self, dir: &OwnedFd, path: &str, current: &Current) -> Result<()> {
        let leaf = publish::leaf_of(path);
        match current {
            // Checked only once renamed away, so nothing put there after
            // the check can be the one deleted.
            Current::File | Current::Symlink(_) => {
                self.publisher.move_aside(dir, path, &(self.known)(path))
            }
            // Only an empty folder is removed, so nothing can be lost.
            Current::Directory
                if (self.known)(path).folder
                    && self.publisher.remove_folder(dir, leaf, path)? =>
            {
                Ok(())
            }
            Current::Directory | Current::Other => self.publisher.save_aside(dir, leaf, path),
        }
    }

    pub(super) fn directory(&mut self, path: &str) -> Result<()> {
        let dir = self.parent(path)?;
        match publish::current(&dir, publish::leaf_of(path), path)? {
            Some(Current::Directory) => return Ok(()),
            Some(other) => self.clear(&dir, path, &other)?,
            None => {}
        }
        self.publisher.publish_one(&WinningEntry {
            path: path.to_owned(),
            kind: EntryType::Directory,
            size: None,
            md5: None,
            target: None,
            mode: 0o755,
            product: 0,
        })
    }

    pub(super) fn link(&mut self, path: &str, target: &str) -> Result<()> {
        let dir = self.parent(path)?;
        match publish::current(&dir, publish::leaf_of(path), path)? {
            Some(Current::Symlink(now)) if now == target => return Ok(()),
            Some(other) => self.clear(&dir, path, &other)?,
            None => {}
        }
        #[cfg(test)]
        super::tests::before_place(path);
        // Whatever appears meanwhile is never replaced unchecked.
        self.publisher
            .place_link(&dir, path, target, &(self.known)(path))
    }

    /// The file's MD5 when what is there already is it, else None once the
    /// path is clear for the download to be renamed over.
    pub(super) fn file(&mut self, file: &FileEntry) -> Result<Option<String>> {
        let path = file.path.as_str();
        let dir = self.parent(path)?;
        let leaf = publish::leaf_of(path);
        match publish::current(&dir, leaf, path)? {
            None => Ok(None),
            Some(Current::File) => match verify(&dir, leaf, path, file, self.token)? {
                Some((md5, true)) => Ok(Some(md5)),
                Some((md5, false)) if (self.known)(path).files.contains(&md5) => Ok(None),
                _ => {
                    self.publisher.save_aside(&dir, leaf, path)?;
                    Ok(None)
                }
            },
            Some(other) => {
                self.clear(&dir, path, &other)?;
                Ok(None)
            }
        }
    }
}

/// Reads the regular file `leaf` in `dir` once, without following a
/// symlink: its MD5, and whether it is the file `entry` describes, chunk by
/// chunk and whole. None when it isn't a regular file.
pub(super) fn verify(
    dir: &OwnedFd,
    leaf: &str,
    path: &str,
    entry: &FileEntry,
    token: &Token,
) -> Result<Option<(String, bool)>> {
    Ok(verify_all(dir, leaf, path, &[entry], token)?.map(|(md5, same)| (md5, same[0])))
}

/// One entry's check as a file is read.
struct Checking<'a> {
    entry: &'a FileEntry,
    same: bool,
    sha: Option<Sha256>,
    chunks: std::slice::Iter<'a, record::ChunkEntry>,
    chunk: Option<&'a record::ChunkEntry>,
    left: u64,
    md5: Md5,
}

impl<'a> Checking<'a> {
    fn new(entry: &'a FileEntry, length: u64) -> Self {
        let mut chunks = entry.chunks.iter();
        let chunk = chunks.next();
        Self {
            entry,
            same: length == entry.size,
            sha: entry.sha256.as_ref().map(|_| Sha256::new()),
            chunks,
            chunk,
            left: chunk.map_or(0, |chunk| chunk.size),
            md5: Md5::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        if let Some(sha) = &mut self.sha {
            sha.update(bytes);
        }
        let mut rest = bytes;
        while self.same && !rest.is_empty() {
            let Some(current) = self.chunk else {
                self.same = false;
                break;
            };
            let take = rest
                .len()
                .min(usize::try_from(self.left).unwrap_or(usize::MAX));
            self.md5.update(&rest[..take]);
            rest = &rest[take..];
            self.left -= take as u64;
            if self.left == 0 {
                let done = std::mem::replace(&mut self.md5, Md5::new());
                if crate::units::hex(done.finalize()) != current.md5 {
                    self.same = false;
                }
                self.chunk = self.chunks.next();
                self.left = self.chunk.map_or(0, |chunk| chunk.size);
            }
        }
    }

    fn finish(self, md5: &str) -> bool {
        self.same
            && self.chunk.is_none()
            && self
                .entry
                .md5
                .as_deref()
                .is_none_or(|expected| expected == md5)
            && match (&self.entry.sha256, self.sha) {
                (Some(expected), Some(sha)) => *expected == crate::units::hex(sha.finalize()),
                _ => true,
            }
    }
}

/// `verify` against several entries in one read, so a big file an update
/// compares with its old and new versions is read once. Whether it is each.
pub(super) fn verify_all(
    dir: &OwnedFd,
    leaf: &str,
    path: &str,
    entries: &[&FileEntry],
    token: &Token,
) -> Result<Option<(String, Vec<bool>)>> {
    let Some(mut file) = open_regular(dir, leaf, path)? else {
        return Ok(None);
    };
    let length = file
        .metadata()
        .with_context(|| format!("failed to check {path}"))?
        .len();
    let mut checks: Vec<Checking> = entries
        .iter()
        .map(|entry| Checking::new(entry, length))
        .collect();
    let mut whole = Md5::new();
    let mut buffer = vec![0; 256 * 1024];
    loop {
        if token.is_canceled() {
            return Err(UpdateError::Canceled { journal: true }.into());
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).with_context(|| format!("failed to read {path}")),
        };
        let bytes = &buffer[..read];
        whole.update(bytes);
        for check in &mut checks {
            check.update(bytes);
        }
    }
    let md5 = crate::units::hex(whole.finalize());
    let same = checks.into_iter().map(|check| check.finish(&md5)).collect();
    Ok(Some((md5, same)))
}

pub(super) fn open_regular(dir: &OwnedFd, leaf: &str, path: &str) -> Result<Option<File>> {
    let fd = match rustix::fs::openat(
        dir,
        leaf,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("failed to open {path}")),
    };
    let stat = rustix::fs::fstat(&fd).with_context(|| format!("failed to check {path}"))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Ok(None);
    }
    Ok(Some(File::from(fd)))
}

/// The manifest with each path spelled the way an entry already in the
/// folder spells it, matched case-insensitively, since Windows games mix
/// cases freely. Paths with nothing on disk keep GOG's spelling.
pub(super) fn on_disk(root: &OwnedFd, manifest: &Manifest) -> Manifest {
    let mut listings: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut spell = |path: &str| -> String {
        let mut resolved = String::new();
        for part in path.split('/') {
            let names = listings
                .entry(resolved.clone())
                .or_insert_with(|| list(root, &resolved));
            let chosen = match names {
                Some(names) if !names.iter().any(|name| name == part) => {
                    let lower = part.to_lowercase();
                    names
                        .iter()
                        .find(|name| name.to_lowercase() == lower)
                        .cloned()
                        .unwrap_or_else(|| part.to_owned())
                }
                _ => part.to_owned(),
            };
            if !resolved.is_empty() {
                resolved.push('/');
            }
            resolved.push_str(&chosen);
        }
        resolved
    };
    let mut out = manifest.clone();
    for file in &mut out.files {
        file.path = spell(&file.path);
    }
    for dir in &mut out.directories {
        dir.path = spell(&dir.path);
    }
    for link in &mut out.links {
        link.path = spell(&link.path);
    }
    build::sort(&mut out);
    out
}

/// The names in a folder of the game, sorted, or None when it isn't a real
/// folder.
fn list(root: &OwnedFd, relative: &str) -> Option<Vec<String>> {
    let dir = fetch::open_dir(root, relative).ok()?;
    let mut listing = rustix::fs::Dir::read_from(&dir).ok()?;
    let mut names = Vec::new();
    while let Some(entry) = listing.read() {
        let entry = entry.ok()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != "." && name != ".." {
            names.push(name);
        }
    }
    names.sort();
    Some(names)
}

/// The bytes of the game's files and support files that aren't in place
/// yet, checked by hash. Counted once, since hashing a big install is slow.
fn unverified_sizes(paths: &Paths, plan: &Plan, token: &Token) -> Result<(u64, u64)> {
    let support_root = paths.gog_support_dir(&folder::key(&plan.folder));
    let game = unverified(
        &plan.folder,
        &plan.manifest.files,
        |file| file.path.clone(),
        token,
    )?;
    let support = unverified(
        &support_root,
        &plan.manifest.support,
        |file| format!("{}/{}", file.product.unwrap_or_default(), file.path),
        token,
    )?;
    Ok((game, support))
}

/// The bytes of the store versions that were missing when planning looked.
pub(super) fn planned_store(paths: &Paths, store: &[StoreNeed]) -> u64 {
    store
        .iter()
        .filter(|need| !store::version_dir(paths, &need.version).is_dir())
        .flat_map(|need| need.files.iter().flatten())
        .map(|file| file.size)
        .sum()
}

/// Refuses before anything changes when a filesystem can't hold its peak:
/// the game's files on the folder's, support files on the data directory's,
/// and new redistributable versions on the store's, summed where they share
/// one. Files already in place at their size aren't counted.
fn check_space(paths: &Paths, plan: &Plan, game: u64, support: u64, store: u64) -> Result<()> {
    let support_root = paths.gog_support_dir(&folder::key(&plan.folder));
    space(
        paths,
        &[
            (plan.folder.as_path(), game),
            (support_root.as_path(), support),
            (paths.gog_redist_dir().as_path(), store),
        ],
    )
}

/// The size of every file in `files` that isn't already under `root` with
/// the content the manifest gives, checked by hash: one that differs, even
/// at the same size, is downloaded beside it before the old one goes.
fn unverified(
    root: &Path,
    files: &[FileEntry],
    path_of: impl Fn(&FileEntry) -> String,
    token: &Token,
) -> Result<u64> {
    let fd = match package::open_folder(root) {
        Ok(fd) => Some(fd),
        Err(_) if !root.exists() => None,
        Err(error) => return Err(error),
    };
    let mut needed = 0u64;
    for file in files {
        let path = path_of(file);
        let matches = match &fd {
            Some(fd) => match fetch::open_dir(fd, publish::parent_of(&path)) {
                Ok(dir) => matches!(
                    verify(&dir, publish::leaf_of(&path), &path, file, token)?,
                    Some((_, true))
                ),
                Err(_) => false,
            },
            None => false,
        };
        if !matches {
            needed = needed.saturating_add(file.size);
        }
    }
    Ok(needed)
}

/// Checks each filesystem among `needs` holds what is needed on it.
pub(super) fn space(paths: &Paths, needs: &[(&Path, u64)]) -> Result<()> {
    space_with(paths, needs, &device, &free)
}

fn free(path: &Path) -> Result<u64> {
    #[cfg(test)]
    if let Some(free) = super::tests::free_at(path) {
        return Ok(free);
    }
    free_space(path)
}

fn device(path: &Path) -> Result<u64> {
    #[cfg(test)]
    if let Some(device) = super::tests::device_at(path) {
        return Ok(device);
    }
    let probe = package::existing(path);
    Ok(std::fs::metadata(probe)
        .with_context(|| format!("failed to inspect {}", probe.display()))?
        .dev())
}

pub(super) fn space_with(
    paths: &Paths,
    needs: &[(&Path, u64)],
    device: &dyn Fn(&Path) -> Result<u64>,
    free: &dyn Fn(&Path) -> Result<u64>,
) -> Result<()> {
    let mut groups: Vec<(u64, &Path, u64)> = Vec::new();
    for &(path, needed) in needs {
        let device = device(path)?;
        match groups.iter_mut().find(|group| group.0 == device) {
            Some(group) => group.2 = group.2.saturating_add(needed),
            None => groups.push((device, path, needed)),
        }
    }
    for (_, path, needed) in groups {
        if needed == 0 {
            continue;
        }
        let needed = SpaceNeeded {
            unpacked: needed,
            assembled: 0,
        };
        package::check_space_with(paths, path, needed, free(path)?)?;
    }
    Ok(())
}

pub(super) fn publish_depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.matches('/').count() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gog::update::galaxy::record::{DirEntry, LinkEntry};

    fn file(path: &str) -> FileEntry {
        FileEntry {
            path: path.to_owned(),
            product: Some(1),
            dependency: None,
            size: 0,
            md5: None,
            sha256: None,
            chunks: Vec::new(),
        }
    }

    #[test]
    fn paths_reuse_the_spelling_entries_already_have_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("DATA/Maps")).unwrap();
        std::fs::write(dir.path().join("DATA/Core.DAT"), "x").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "x").unwrap();
        let manifest = Manifest {
            build: "1".to_owned(),
            files: vec![
                file("Data/core.dat"),
                file("Data/maps/new.map"),
                file("Data/fresh.dat"),
                file("README.TXT"),
                file("bin/game.exe"),
            ],
            directories: ["Data", "Data/maps", "bin"]
                .iter()
                .map(|path| DirEntry {
                    path: (*path).to_owned(),
                    product: Some(1),
                    dependency: None,
                })
                .collect(),
            links: vec![LinkEntry {
                path: "data/link".to_owned(),
                target: "Core.DAT".to_owned(),
                product: Some(1),
                dependency: None,
            }],
            ..Manifest::default()
        };
        let root = package::open_folder(dir.path()).unwrap();
        let spelled = on_disk(&root, &manifest);
        let files: Vec<&str> = spelled.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            files,
            [
                "DATA/Core.DAT",
                "DATA/Maps/new.map",
                "DATA/fresh.dat",
                "bin/game.exe",
                "readme.txt",
            ]
        );
        let dirs: Vec<&str> = spelled
            .directories
            .iter()
            .map(|d| d.path.as_str())
            .collect();
        assert_eq!(dirs, ["DATA", "DATA/Maps", "bin"]);
        assert_eq!(spelled.links[0].path, "DATA/link");
    }

    #[test]
    fn space_is_checked_per_filesystem_and_summed_where_they_share_one() {
        let (_root, paths) = crate::gog::test_support::setup();
        let folder = Path::new("/games/Game");
        let support = Path::new("/data/gog/support/k");
        let store = Path::new("/data/gog/redist");
        let needs = [(folder, 100), (support, 30), (store, 30)];
        let split = |path: &Path| Ok(if path.starts_with("/games") { 1 } else { 2 });
        let free_of = |games: u64, data: u64| {
            move |path: &Path| {
                Ok(if path.starts_with("/games") {
                    games
                } else {
                    data
                })
            }
        };
        // Each filesystem holds its own.
        assert!(space_with(&paths, &needs, &split, &free_of(100, 60)).is_ok());
        // The game folder's is short.
        let error = space_with(&paths, &needs, &split, &free_of(99, 1000)).unwrap_err();
        assert!(error.to_string().contains("/games"), "{error}");
        // The data directory's holds neither the support files and the
        // store together.
        let error = space_with(&paths, &needs, &split, &free_of(1000, 59)).unwrap_err();
        assert!(error.to_string().contains("needs 60 B"), "{error}");
        assert!(error.to_string().contains("/data/gog/support"), "{error}");
        // All on one: summed.
        let one = |_: &Path| Ok(7);
        assert!(space_with(&paths, &needs, &one, &|_: &Path| Ok(160)).is_ok());
        let error = space_with(&paths, &needs, &one, &|_: &Path| Ok(159)).unwrap_err();
        assert!(error.to_string().contains("needs 160 B"), "{error}");
    }
}
