//! Updating a Galaxy-tracked Windows game to the latest build on the default
//! branch, or finishing an interrupted update.
//!
//! `plan` reads only, but for recording a build another tool installed, as
//! a check does. It picks the target build and products (DLC owned since
//! and failed or unavailable DLC the build now has join, as for installer
//! games), and what the update starts from: the installed manifest the
//! record stores, else the installed build's manifest fetched from GOG, else
//! nothing (question 4, answer b), when nothing is deleted.
//!
//! `apply` compares the folder with the target before taking any lock, as an
//! install measures what it needs, and checks space. Then, under the locks:
//! `pending` with what it starts from, the journal, and every new or changed
//! file downloaded beside its path, reusing chunks its old copy still holds,
//! and renamed into place refusing to replace. What stood there is deleted
//! only when it holds a hash wineo recorded for the path, and saved aside
//! otherwise. After publishing, old files the target doesn't list are
//! deleted, again only on a hash match, then folders they leave empty. Then
//! the setup steps whose identity changed, `installed`, the commit, and
//! dropping `pending`.
//!
//! Anything that stops it after the journal leaves it, and updating again
//! finishes from the journal and `pending`, keeping every file already in
//! place. Without `pending`, the journal's builds are fetched again, or the
//! latest when GOG no longer lists them (question 12), which deletes
//! nothing.
//!
//! Lock order, as for any update: the environment's operation lock with its
//! prefix, the folder's in-use lock, its state lock.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::AtFlags;

use super::build;
use super::fetch::{self, Job, Reuse};
use super::follow::{self, Choice, Who};
use super::install::{
    self, GalaxyError, Kind, Links, NAME_BATCH, Placer, PlannedProduct, Progress, Retarget,
    StoreNeed, Waiting, flush, merged, name_entry, place, recorded, sizes,
};
use super::private;
use super::record::{self, Base, FileEntry, Follow, Manifest};
use super::setup;
use super::store;
use super::switch::{FollowPlan, Pinned};
use crate::cancel::Token;
use crate::config::Arch;
use crate::environment::Environment;
use crate::gog::account::Session;
use crate::gog::folder::{
    self, Committed, EntryType, FolderError, FolderLock, FolderState, Journal, JournalKind,
    Platform, Product, ProductStatus, Target, Track, failpoint,
};
use crate::gog::migrate::host_only;
use crate::gog::package;
use crate::gog::update::UpdateError;
use crate::gog::update::apply::{Options, Updated, is_cancel, lock_operation_on, operation_id};
use crate::gog::update::publish::{self, Current, Known, Publisher};
use crate::gog::update::windows::{self, Event};
use crate::gog::{names, product};
use crate::paths::Paths;

/// What an update compares the target with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The installed manifest wineo stored at its last commit.
    Stored,
    /// The installed build's manifest, fetched from GOG, for a game another
    /// tool installed. Files the update keeps are checked by size.
    Fetched,
    /// None: the target's manifest is the starting point. Each file at a
    /// target path is hashed, one that doesn't match is moved aside, and
    /// nothing is deleted.
    Target,
}

/// What a review of the update should say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// Nothing is deleted, so files of the installed build the new one
    /// doesn't list stay.
    NothingDeleted,
    /// What setup writes in the prefix can't be known ahead, so the space
    /// check leaves it out.
    SetupSpaceUncounted,
}

/// What updating a Galaxy game would do.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Planned {
    /// The latest build and every product GOG offers are installed, and
    /// nothing is unfinished.
    UpToDate {
        environment: String,
        build: Option<String>,
        warnings: Vec<String>,
    },
    /// The game is pinned and asked to update, which changes nothing.
    Pinned(Pinned),
    /// Only the branch or pin changes: the build wanted is the one installed.
    Follow(FollowPlan),
    Update(Plan),
}

/// A Galaxy update, or the end of an interrupted one, ready to apply.
#[derive(Debug, Clone)]
pub struct Plan {
    environment: String,
    folder: PathBuf,
    gog_id: u64,
    title: String,
    kind: JournalKind,
    recovery: bool,
    seen: FolderState,
    arch: Arch,
    installed_build: Option<String>,
    /// GOG's name for the installed build, or the installer version a move
    /// comes from.
    installed_version: Option<String>,
    version_name: Option<String>,
    products: Vec<PlannedProduct>,
    target: Target,
    manifest: Manifest,
    base: Option<Base>,
    store: Vec<StoreNeed>,
    retarget: Option<Retarget>,
    /// What the game follows now, and after this.
    from: Follow,
    to: Follow,
    notes: Vec<Note>,
    warnings: Vec<String>,
}

impl Plan {
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

    /// `Update`, `Reinstall` (including a move onto Galaxy), or `Repair`; an
    /// interrupted one has its journal's own kind.
    pub fn kind(&self) -> JournalKind {
        self.kind
    }

    /// Whether every file is hashed against the target rather than trusted
    /// to be as the record says, which a repair or reinstall does.
    pub fn hashes_all(&self) -> bool {
        self.kind != JournalKind::Update
    }

    /// Whether this finishes an interrupted update.
    pub fn is_recovery(&self) -> bool {
        self.recovery
    }

    /// Whether the folder has no owner, so applying claims it.
    pub fn claims(&self) -> bool {
        self.seen.owner.is_none()
    }

    /// The base game's committed build.
    pub fn installed_build(&self) -> Option<&str> {
        self.installed_build.as_deref()
    }

    /// The base game's target build.
    pub fn build(&self) -> &str {
        &self.manifest.build
    }

    /// GOG's name for the installed build, when it was listed; for a move
    /// off the installer track, the version the installer left.
    pub fn installed_version(&self) -> Option<&str> {
        self.installed_version.as_deref()
    }

    /// GOG's name for the target build, when it was listed.
    pub fn version_name(&self) -> Option<&str> {
        self.version_name.as_deref()
    }

    /// The track the game is moving off, for a reinstall onto Galaxy.
    pub fn moves_from(&self) -> Option<Track> {
        self.seen
            .committed
            .as_ref()
            .map(|committed| committed.track)
            .filter(|track| *track != Track::Galaxy && self.kind == JournalKind::Reinstall)
    }

    /// How many support files the target build has.
    pub fn support_files(&self) -> usize {
        self.manifest.support.len()
    }

    /// The files and bytes a repair or reinstall checks: every game file
    /// and support file.
    pub fn checked(&self) -> (usize, u64) {
        (
            self.manifest.files.len() + self.manifest.support.len(),
            sizes(&self.manifest.files) + sizes(&self.manifest.support),
        )
    }

    /// The setup steps that run, by name: "script interpreter", "setup
    /// program", or a redistributable's id. A repair or reinstall runs them
    /// all; an update only the ones the record doesn't show done.
    pub fn setup_steps(&self, paths: &Paths) -> Vec<String> {
        let steps = setup::planned_steps(
            paths,
            &self.folder,
            &self.manifest,
            &self.target,
            &self.title,
        );
        let done = if self.hashes_all() || self.recovery {
            Vec::new()
        } else {
            install::files_record(paths, &self.folder)
                .filter(|record| record::carries_setup(&self.seen, record).unwrap_or(false))
                .map(|record| record.setup)
                .unwrap_or_default()
        };
        let wanted: Vec<_> = steps.iter().map(|step| step.identity.clone()).collect();
        setup::remaining(&wanted, &done, false)
            .into_iter()
            .map(|index| steps[index].name.clone())
            .collect()
    }

    pub fn arch(&self) -> Arch {
        self.arch
    }

    /// The state the update commits.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The base game first, then each DLC: those installed, then newly
    /// owned ones.
    pub fn products(&self) -> &[PlannedProduct] {
        &self.products
    }

    pub fn start(&self) -> Start {
        match &self.base {
            Some(base) if base.fetched => Start::Fetched,
            Some(_) => Start::Stored,
            None => Start::Target,
        }
    }

    /// Set when an interrupted update's build is gone from GOG's list and
    /// it finishes with the latest one instead, which the review asks about.
    pub fn retarget(&self) -> Option<&Retarget> {
        self.retarget.as_ref()
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// The branch and pin the game has now.
    pub fn from(&self) -> &Follow {
        &self.from
    }

    /// The branch and pin it has after.
    pub fn to(&self) -> &Follow {
        &self.to
    }

    /// The most it downloads: every file, support file, and redistributable
    /// the target has that the starting point doesn't have as it is. What's
    /// already in place is only known once `apply` looks.
    pub fn download_size(&self) -> u64 {
        // A repair or reinstall hashes everything, so any file may be wrong.
        let old: HashMap<String, &FileEntry> = self
            .base
            .iter()
            .filter(|_| !self.hashes_all())
            .flat_map(|base| &base.manifest.files)
            .map(|file| (file.path.to_lowercase(), file))
            .collect();
        let files: u64 = self
            .manifest
            .files
            .iter()
            .filter(|file| {
                old.get(&file.path.to_lowercase())
                    .is_none_or(|old| !same_content(old, file))
            })
            .map(|file| file.size)
            .sum();
        let store: u64 = self
            .store
            .iter()
            .flat_map(|need| need.files.iter().flatten())
            .map(|file| file.size)
            .sum();
        files + sizes(&self.manifest.support) + store
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

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Whether two entries of a path hold the same content: the same chunks,
/// and the same MD5 where both have one.
fn same_content(a: &FileEntry, b: &FileEntry) -> bool {
    a.size == b.size
        && a.chunks.len() == b.chunks.len()
        && a.chunks
            .iter()
            .zip(&b.chunks)
            .all(|(a, b)| a.md5 == b.md5 && a.size == b.size)
        && match (&a.md5, &b.md5) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// Works out what updating `name`'s Galaxy game does. Reads only, but for
/// recording a build another tool installed (unless `read_only`), GOG's
/// product details cache, and a renewed login.
///
/// `options` picks the operation:
/// - an update to the latest build, which is up to date when nothing changed;
/// - a reinstall (`options.reinstall`), the same target with every file
///   hashed, every setup step run, and no early "up to date";
/// - a move onto Galaxy from the installer track, a reinstall asking for
///   `Track::Galaxy`, which starts from nothing (answer b);
/// - a repair of the installed build (`options.repair`), which doesn't ask
///   for the latest build at all.
///
/// A folder with a journal is finished as its own kind, whatever `options`
/// say. An unfinished Galaxy install is finished by installing again.
pub fn plan(
    paths: &Paths,
    session: &mut Session,
    name: &str,
    mut options: Options,
    _token: &Token,
) -> Result<Planned> {
    host_only(paths)?;
    options.check()?;
    Environment::ensure_exists(paths, name)?;
    // Setup runs programs in the prefix, as an install's does.
    windows::check_graphical()?;
    let mut state = crate::gog::update::game_of(paths, name)?;
    if let Some(journal) = &state.journal {
        if journal.target.track != Track::Galaxy {
            bail!("{name}'s unfinished GOG operation isn't a Galaxy one");
        }
        // The operation carries the branch it began with, so a new one waits.
        // Naming that same branch changes nothing, and is how its password
        // is given again when it was lost.
        if !options.choice.is_keep() {
            let followed = install::files_record(paths, &state.folder).map(|record| {
                record::pending(&state, &record)
                    .and_then(|pending| pending.follow.clone())
                    .unwrap_or_else(|| record.follow.clone())
            });
            match (&options.choice, followed.and_then(|follow| follow.branch)) {
                (Choice::Branch(named), Some(followed)) if *named == followed => {
                    options.choice = Choice::Keep;
                }
                _ => {
                    return Err(GalaxyError::Unfinished {
                        name: name.to_owned(),
                    }
                    .into());
                }
            }
        }
        let gog_id = journal.target.gog_id;
        return match journal.kind {
            JournalKind::Update | JournalKind::Reinstall | JournalKind::Repair => {
                resumed(paths, session, name, state.clone(), options).map(Planned::Update)
            }
            JournalKind::Install => Err(match &state.owner {
                Some(owner) => FolderError::UnfinishedInstall {
                    name: owner.clone(),
                    gog_id,
                },
                None => FolderError::OwnerlessInstall {
                    gog_id,
                    folder: state.folder.clone(),
                },
            }
            .into()),
        };
    }

    let mut warnings = Vec::new();
    // A build another tool installed is recorded first, as a check does, so
    // the update starts from it.
    let on_galaxy = state
        .committed
        .as_ref()
        .is_some_and(|committed| committed.track == Track::Galaxy);
    let adopted = if state.owner.is_none() || !on_galaxy {
        None
    } else if options.read_only {
        crate::gog::update::seen_build(&state)
    } else {
        crate::gog::update::adopt_build(paths, &state)?
    };
    if let Some(adopted) = adopted {
        if adopted.recorded {
            warnings.push(format!(
                "Recorded the build {}, which another tool installed.",
                crate::gog::update::base_build(&adopted.committed).unwrap_or_default()
            ));
        }
        state.committed = Some(adopted.committed);
    }
    let committed = state
        .committed
        .clone()
        .ok_or_else(|| anyhow!("{name}'s GOG install hasn't started."))?;
    if committed.platform != Platform::Windows {
        return Err(GalaxyError::NotWindows.into());
    }
    // A game on the installer track comes over only by a reinstall that
    // asks for Galaxy.
    let moving = committed.track == Track::Installer;
    if moving && !(options.reinstall && options.track == Some(Track::Galaxy)) {
        if options.repair {
            return Err(UpdateError::NotGalaxy {
                name: name.to_owned(),
                windows: true,
            }
            .into());
        }
        bail!("{name}'s GOG game isn't a Galaxy one");
    }
    if !moving && options.track == Some(Track::Installer) {
        bail!("Moving {name} to the installer track isn't a Galaxy operation");
    }
    let kind = if options.repair {
        JournalKind::Repair
    } else if options.reinstall {
        JournalKind::Reinstall
    } else {
        JournalKind::Update
    };
    let gog_id = committed.gog_id;
    let folder_names = names::product_names(&state.folder);
    let arch = Environment::load(paths, name)?.wine()?.arch;
    let bits = build::bits(arch);
    let installed_build = crate::gog::update::base_build(&committed).map(str::to_owned);
    // A game coming over from the installers has nothing to follow yet.
    let from = if moving {
        Follow::default()
    } else {
        super::follow_of(paths, &state.folder)
    };

    if kind == JournalKind::Repair {
        // The product's details only give titles here, so a failure costs
        // those and nothing else.
        let details = product::load_with(
            paths,
            session.client(),
            gog_id,
            options.refresh,
            !options.read_only,
        )
        .ok();
        let title = details
            .as_ref()
            .map(|details| details.product.title.clone())
            .filter(|title| !title.is_empty())
            .or_else(|| folder_names.get(&gog_id).cloned())
            .unwrap_or_else(|| format!("GOG game {gog_id}"));
        warnings.extend(details.as_ref().and_then(|details| details.warning.clone()));
        let target = Target {
            gog_id,
            platform: Platform::Windows,
            language: committed.language.clone(),
            track: Track::Galaxy,
            products: committed.products.clone(),
        };
        let (manifest, base, store) = repair_manifest(
            paths,
            session,
            name,
            &state,
            &title,
            &target,
            installed_build.as_deref(),
            bits,
            from.branch.as_deref(),
            &Entered {
                secret: options.password.as_ref(),
                read_only: options.read_only,
            },
            &mut warnings,
        )?;
        let store = install::complete_store(session, store)?;
        let products = planned_products(&target, &manifest, &title, |id| {
            details
                .as_ref()
                .and_then(|details| details.product.dlcs.iter().find(|dlc| dlc.id == id))
                .map(|dlc| dlc.title.clone())
                .filter(|title| !title.is_empty())
        });
        let notes = notes(&manifest, base.is_some());
        return Ok(Planned::Update(Plan {
            environment: name.to_owned(),
            folder: state.folder.clone(),
            gog_id,
            title,
            kind,
            recovery: false,
            seen: state,
            arch,
            installed_build,
            installed_version: manifest.setup.version_name.clone(),
            version_name: manifest.setup.version_name.clone(),
            products,
            target,
            manifest,
            base,
            store,
            retarget: None,
            to: from.clone(),
            from,
            notes,
            warnings,
        }));
    }

    let details = product::load_with(
        paths,
        session.client(),
        gog_id,
        options.refresh,
        !options.read_only,
    )?;
    warnings.extend(details.warning);
    let game = details.product;
    let title = if game.title.is_empty() {
        format!("GOG game {gog_id}")
    } else {
        game.title.clone()
    };
    let no_build = || GalaxyError::NoBuild {
        title: title.clone(),
    };
    let (branch, required) = private::wanted(&options.choice, &from);
    let fetched = match install::builds_of(session, gog_id, branch, options.password.as_ref()) {
        Ok(fetched) => fetched,
        // No content system for the game, as a check reads it.
        Err(error)
            if error
                .downcast_ref::<wineo_gog::Error>()
                .is_some_and(|error| error.kind() == wineo_gog::ErrorKind::Status(404)) =>
        {
            return Err(no_build().into());
        }
        Err(error) => return Err(error),
    };
    if let (Some(branch), true) = (branch, required) {
        let game = private::Game {
            title: &title,
            gog_id,
            name: Some(name),
        };
        warnings.extend(fetched.require(session, &game, branch, options.read_only)?);
    }
    let builds = fetched.builds;
    // A pinned game isn't moved to another build; reinstalling it rebuilds
    // the pin. An update holds the pinned build, and only installs DLC from
    // it, so it carries on only when the pin is what's installed.
    let holding = kind == JournalKind::Update && options.choice.is_keep() && from.pinned.is_some();
    let pinned_game = |warnings: Vec<String>| -> Planned {
        let pinned = from.pinned.clone().unwrap_or_default();
        Planned::Pinned(Pinned {
            environment: name.to_owned(),
            version: builds
                .items
                .iter()
                .find(|listed| listed.build_id == pinned)
                .and_then(|listed| listed.version_name.clone())
                .filter(|version| !version.is_empty()),
            build: pinned,
            warnings,
        })
    };
    if let (true, Some(pinned)) = (holding, &from.pinned)
        && follow::by_id(&builds, pinned, from.branch.as_deref()).map(|held| &held.build_id)
            != installed_build.as_ref()
    {
        return Ok(pinned_game(warnings));
    }
    let who = Who {
        title: &title,
        name: Some(name),
    };
    let picked = follow::pick(
        &builds,
        &options.choice,
        &from,
        kind == JournalKind::Reinstall || holding,
        &who,
    )?;
    let latest = picked.build.clone();
    let to = picked.to;
    if latest.link.is_none() {
        return Err(no_build().into());
    }
    let repository = session
        .client()
        .repository(&latest)
        .with_context(|| format!("Couldn't get {title}'s build {}", latest.build_id))?;

    // Ownership is only asked for when GOG lists DLC the folder doesn't
    // have, as a check does.
    let unlisted = game
        .dlc_ids
        .iter()
        .any(|id| !committed.products.iter().any(|product| product.id == *id));
    let owned = if unlisted {
        match install::call(session, |client, access| client.owned_ids(access)) {
            Ok(ids) => Some(ids.into_iter().collect::<HashSet<u64>>()),
            Err(error) => {
                warnings.push(format!(
                    "Couldn't check which DLC you own, so DLC you bought since isn't installed: {error:#}"
                ));
                None
            }
        }
    } else {
        None
    };
    let selected = select(
        &title,
        &game,
        &folder_names,
        &committed,
        &repository,
        bits,
        owned.as_ref(),
        latest
            .version_name
            .as_deref()
            .filter(|name| !name.is_empty()),
    )?;
    let target = Target {
        gog_id,
        platform: Platform::Windows,
        language: committed.language.clone(),
        track: Track::Galaxy,
        products: selected.products,
    };
    // A reinstall runs whatever it finds.
    if kind == JournalKind::Update
        && installed_build.as_deref() == Some(latest.build_id.as_str())
        && !selection_changed(&committed, &target.products)
    {
        if holding {
            // Nothing for the pinned build to install.
            return Ok(pinned_game(warnings));
        }
        if from == to {
            return Ok(Planned::UpToDate {
                environment: name.to_owned(),
                build: installed_build,
                warnings,
            });
        }
        // The build is the one wanted, so only the record changes.
        let installed_version = installed_build
            .as_deref()
            .and_then(|id| builds.items.iter().find(|build| build.build_id == id))
            .and_then(|build| build.version_name.clone())
            .filter(|name| !name.is_empty());
        return Ok(Planned::Follow(FollowPlan {
            environment: name.to_owned(),
            folder: state.folder.clone(),
            gog_id,
            title,
            seen: state,
            installed_build,
            installed_version,
            from,
            to,
            warnings,
        }));
    }

    let built = install::manifests(
        session,
        &title,
        &repository,
        &selected.chosen,
        bits,
        latest.version_name.clone(),
    )?;
    warnings.extend(built.warnings);
    let store = install::complete_store(session, built.store)?;
    // A stored manifest is the starting point when the record is trusted. An
    // update without one fetches the installed build's from GOG (question
    // 4); a reinstall, which hashes everything anyway, starts from nothing
    // (answer b), so it deletes only what a stored manifest vouches for.
    let base = match stored(paths, &state)? {
        Some(manifest) => Some(Base {
            manifest,
            fetched: false,
        }),
        None if kind == JournalKind::Update => fetched_base(
            session,
            &title,
            &builds,
            &committed,
            installed_build.as_deref(),
            &repository,
            bits,
        )?,
        None => None,
    };
    let mut products = selected.planned;
    for product in &mut products {
        product.size = install::product_size(&built.manifest, product.id);
    }
    let notes = notes(&built.manifest, base.is_some());
    Ok(Planned::Update(Plan {
        environment: name.to_owned(),
        folder: state.folder.clone(),
        gog_id,
        title,
        kind,
        recovery: false,
        seen: state,
        arch,
        installed_version: if moving {
            committed
                .products
                .iter()
                .find(|product| product.id == gog_id)
                .and_then(|product| product.version.clone())
        } else {
            installed_build
                .as_deref()
                .and_then(|id| builds.items.iter().find(|build| build.build_id == id))
                .and_then(|build| build.version_name.clone())
                .filter(|name| !name.is_empty())
        },
        installed_build,
        version_name: latest.version_name.clone(),
        products,
        target,
        manifest: built.manifest,
        base,
        store,
        retarget: None,
        from,
        to,
        notes,
        warnings,
    }))
}

/// Whether `target` differs from what `committed` has by a product to
/// install. On the same build, that is the only reason to update. A newly
/// owned DLC the build doesn't have is recorded as unavailable when an
/// update runs anyway, and a language another tool recorded is recorded as
/// GOG's depots spell it, but neither is a reason to run one.
pub(in crate::gog::update) fn selection_changed(committed: &Committed, target: &[Product]) -> bool {
    let selection = |products: &[Product]| -> Vec<(u64, ProductStatus)> {
        products
            .iter()
            .filter(|product| {
                product.status != ProductStatus::Unavailable
                    || committed.products.iter().any(|old| old.id == product.id)
            })
            .map(|product| (product.id, product.status))
            .collect()
    };
    selection(target) != selection(&committed.products)
}

/// The products updating `name`'s game to `latest` would select, for a
/// check that asks which DLC an update would install.
#[allow(clippy::too_many_arguments)]
pub(in crate::gog::update) fn selected_for_check(
    session: &mut Session,
    paths: &Paths,
    name: &str,
    title: &str,
    game: &wineo_gog::Product,
    folder_names: &HashMap<u64, String>,
    committed: &Committed,
    latest: &wineo_gog::Build,
    owned: Option<&HashSet<u64>>,
) -> Result<Vec<Product>> {
    if latest.link.is_none() {
        bail!("GOG lists no file list for build {}", latest.build_id);
    }
    let repository = session
        .client()
        .repository(latest)
        .with_context(|| format!("Couldn't get {title}'s build {}", latest.build_id))?;
    let bits = build::bits(Environment::load(paths, name)?.wine()?.arch);
    let version = latest
        .version_name
        .as_deref()
        .filter(|name| !name.is_empty());
    Ok(select(
        title,
        game,
        folder_names,
        committed,
        &repository,
        bits,
        owned,
        version,
    )?
    .products)
}

/// The products of `target` for a review, titled by `dlc_title` where GOG
/// gave one.
fn planned_products(
    target: &Target,
    manifest: &Manifest,
    title: &str,
    dlc_title: impl Fn(u64) -> Option<String>,
) -> Vec<PlannedProduct> {
    target
        .products
        .iter()
        .map(|product| PlannedProduct {
            id: product.id,
            title: if product.id == target.gog_id {
                Some(title.to_owned())
            } else {
                dlc_title(product.id)
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
            size: install::product_size(manifest, product.id),
        })
        .collect()
}

/// What a repair of `target`'s selection is made against: the stored
/// manifest when the record is trusted, which is also its starting point, so
/// nothing is asked of GOG's build list. Else the installed build's manifest
/// from GOG, as the target with no starting point, so nothing is deleted
/// (answer b). A repair never changes the build, so one GOG no longer lists
/// is refused.
/// The password an operation was given, and whether it may be saved.
struct Entered<'a> {
    secret: Option<&'a wineo_gog::Secret>,
    read_only: bool,
}

#[allow(clippy::too_many_arguments)]
fn repair_manifest(
    paths: &Paths,
    session: &mut Session,
    name: &str,
    state: &FolderState,
    title: &str,
    target: &Target,
    build: Option<&str>,
    bits: &str,
    branch: Option<&str>,
    password: &Entered,
    warnings: &mut Vec<String>,
) -> Result<(Manifest, Option<Base>, Vec<StoreNeed>)> {
    if let Some(manifest) = stored(paths, state)? {
        let store = manifest
            .store_versions()
            .into_iter()
            .map(|version| StoreNeed {
                version,
                files: None,
            })
            .collect();
        let base = Base {
            manifest: manifest.clone(),
            fetched: false,
        };
        return Ok((manifest, Some(base), store));
    }
    let gone = |build: &str| GalaxyError::BuildGone {
        title: title.to_owned(),
        build: build.to_owned(),
        name: name.to_owned(),
    };
    let build = build.ok_or_else(|| GalaxyError::BuildUnknown {
        title: title.to_owned(),
        name: name.to_owned(),
    })?;
    let gog_id = target.gog_id;
    let fetched = install::builds_of(session, gog_id, branch, password.secret)?;
    if let Some(branch) = branch {
        let game = private::Game {
            title,
            gog_id,
            name: Some(name),
        };
        warnings.extend(fetched.require(session, &game, branch, password.read_only)?);
    }
    let builds = fetched.builds;
    let listed = builds
        .items
        .iter()
        .find(|listed| listed.build_id == build && listed.generation == Some(2))
        .ok_or_else(|| gone(build))?;
    if listed.link.is_none() {
        return Err(gone(build).into());
    }
    let repository = session
        .client()
        .repository(listed)
        .with_context(|| format!("Couldn't get {title}'s build {build}"))?;
    let mut chosen = Vec::new();
    for product in target
        .products
        .iter()
        .filter(|product| product.status == ProductStatus::Installed)
    {
        let wanted = product.language.as_deref().unwrap_or(&target.language);
        let Some(language) = resolve(&repository.depots, product.id, wanted, bits) else {
            return Err(GalaxyError::ProductGone {
                title: title.to_owned(),
                build: listed
                    .version_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| build.to_owned()),
                installed: true,
                label: if product.id == gog_id {
                    title.to_owned()
                } else {
                    names::dlc_label(title, product.id, None)
                },
            }
            .into());
        };
        chosen.push((product.id, language.code));
    }
    let built = install::manifests(
        session,
        title,
        &repository,
        &chosen,
        bits,
        listed.version_name.clone(),
    )?;
    Ok((built.manifest, None, built.store))
}

fn notes(manifest: &Manifest, base: bool) -> Vec<Note> {
    let mut notes = Vec::new();
    if !base {
        notes.push(Note::NothingDeleted);
    }
    let setup = &manifest.setup;
    if setup.script_interpreter
        || !setup.redist.is_empty()
        || setup
            .products
            .iter()
            .any(|product| product.temp_executable.is_some())
    {
        notes.push(Note::SetupSpaceUncounted);
    }
    notes
}

/// The record's `installed`, when it describes what `state` has committed.
fn stored(paths: &Paths, state: &FolderState) -> Result<Option<Manifest>> {
    let Some(record) = install::files_record(paths, &state.folder) else {
        return Ok(None);
    };
    Ok(record::installed(state, &record)?.map(|installed| installed.manifest.clone()))
}

/// The installed build's manifest for the committed selection, fetched from
/// GOG, when the build is known and GOG still lists it as a generation 2
/// build. None otherwise, which is answer b. `latest` is the target build's
/// repository, used again when it is the installed build.
fn fetched_base(
    session: &mut Session,
    title: &str,
    builds: &wineo_gog::Builds,
    committed: &Committed,
    installed: Option<&str>,
    latest: &wineo_gog::Repository,
    bits: &str,
) -> Result<Option<Base>> {
    let Some(installed) = installed else {
        return Ok(None);
    };
    let Some(listed) = builds
        .items
        .iter()
        .find(|build| build.build_id == installed && build.generation == Some(2))
    else {
        return Ok(None);
    };
    let fetched;
    let repository = if latest.build_id == installed {
        latest
    } else {
        if listed.link.is_none() {
            return Ok(None);
        }
        fetched = session
            .client()
            .repository(listed)
            .with_context(|| format!("Couldn't get {title}'s build {installed}"))?;
        &fetched
    };
    // A product the build has no depot for just has nothing to compare.
    let chosen: Vec<(u64, String)> = committed
        .products
        .iter()
        .filter(|product| product.status == ProductStatus::Installed)
        .filter_map(|product| {
            let wanted = product.language.as_deref().unwrap_or(&committed.language);
            let language = resolve(&repository.depots, product.id, wanted, bits)?;
            Some((product.id, language.code))
        })
        .collect();
    let built = install::manifests(session, title, repository, &chosen, bits, None)?;
    Ok(Some(Base {
        manifest: built.manifest,
        fetched: true,
    }))
}

/// The language `id`'s depots in `depots` are in, picked from `wanted` as
/// an install picks it, when it has any for a prefix of `bits`.
fn resolve(
    depots: &[wineo_gog::Depot],
    id: u64,
    wanted: &str,
    bits: &str,
) -> Option<build::Language> {
    build::pick_language(depots, id, wanted).filter(|language| {
        depots
            .iter()
            .any(|depot| build::keeps(depot, id, &language.code, bits))
    })
}

/// The products an update installs.
struct Selected {
    products: Vec<Product>,
    planned: Vec<PlannedProduct>,
    /// Each product installed, with the depot language it installs in.
    chosen: Vec<(u64, String)>,
}

/// Every committed product on the new build, then each DLC owned since, in
/// GOG's order. A failed or unavailable DLC the build now has is installed,
/// and one still missing stays as it is. An installed product the build
/// lacks refuses the update.
#[allow(clippy::too_many_arguments)]
fn select(
    title: &str,
    game: &wineo_gog::Product,
    folder_names: &HashMap<u64, String>,
    committed: &Committed,
    repository: &wineo_gog::Repository,
    bits: &str,
    owned: Option<&HashSet<u64>>,
    version: Option<&str>,
) -> Result<Selected> {
    let build_id = &repository.build_id;
    let folder_language = &committed.language;
    let pick = |id: u64, wanted: &str| resolve(&repository.depots, id, wanted, bits);
    let dlc_title = |id: u64| {
        game.dlcs
            .iter()
            .find(|dlc| dlc.id == id)
            .map(|dlc| dlc.title.clone())
            .filter(|title| !title.is_empty())
    };
    let label = |id: u64| {
        if id == committed.gog_id {
            title.to_owned()
        } else {
            let known = dlc_title(id).or_else(|| folder_names.get(&id).cloned());
            names::dlc_label(title, id, known.as_deref())
        }
    };
    let planned = |product: &Product, note: Option<String>| PlannedProduct {
        id: product.id,
        title: if product.id == committed.gog_id {
            Some(title.to_owned())
        } else {
            dlc_title(product.id)
        },
        build: product.build.clone(),
        language: (product.status == ProductStatus::Installed).then(|| {
            product
                .language
                .clone()
                .unwrap_or_else(|| folder_language.clone())
        }),
        status: product.status,
        note,
        size: 0,
    };
    let mut selected = Selected {
        products: Vec::new(),
        planned: Vec::new(),
        chosen: Vec::new(),
    };
    let add = |selected: &mut Selected, product: Product, note: Option<String>| {
        if product.status == ProductStatus::Installed {
            let language = product
                .language
                .clone()
                .unwrap_or_else(|| folder_language.clone());
            selected.chosen.push((product.id, language));
        }
        selected.planned.push(planned(&product, note));
        selected.products.push(product);
    };
    let english = |id: u64, language: &build::Language, wanted: &str| {
        language.english.then(|| {
            format!(
                "{} has no Galaxy files in {wanted}, so it installs in English.",
                label(id)
            )
        })
    };
    let fresh = |id: u64, language: build::Language| {
        let note = english(id, &language, folder_language);
        let product = Product {
            id,
            version: None,
            build: Some(build_id.clone()),
            language: (language.code != *folder_language).then_some(language.code),
            reported_language: None,
            status: ProductStatus::Installed,
        };
        (product, note)
    };
    for product in &committed.products {
        match product.status {
            ProductStatus::Installed => {
                // Picked as an install picks it, so a language another tool
                // recorded, like `en`, finds GOG's `en-US` depots.
                let wanted = product.language.as_deref().unwrap_or(folder_language);
                let Some(language) = pick(product.id, wanted) else {
                    return Err(GalaxyError::ProductGone {
                        title: title.to_owned(),
                        build: version.map_or_else(|| build_id.clone(), str::to_owned),
                        label: label(product.id),
                        installed: true,
                    }
                    .into());
                };
                let note = english(product.id, &language, wanted);
                let product = Product {
                    build: Some(build_id.clone()),
                    language: (language.code != *folder_language).then_some(language.code),
                    ..product.clone()
                };
                add(&mut selected, product, note);
            }
            ProductStatus::Failed | ProductStatus::Unavailable => {
                match pick(product.id, folder_language) {
                    Some(language) => {
                        let (product, note) = fresh(product.id, language);
                        add(&mut selected, product, note);
                    }
                    None => add(&mut selected, product.clone(), None),
                }
            }
        }
    }
    let mut seen = HashSet::new();
    for &id in &game.dlc_ids {
        if !owned.is_some_and(|owned| owned.contains(&id))
            || committed.products.iter().any(|product| product.id == id)
            || !seen.insert(id)
        {
            continue;
        }
        match pick(id, folder_language) {
            Some(language) => {
                let (product, note) = fresh(id, language);
                add(&mut selected, product, note);
            }
            None => {
                let note = format!("{} isn't in build {build_id} of {title}.", label(id));
                let product = Product {
                    id,
                    version: None,
                    build: None,
                    language: None,
                    reported_language: None,
                    status: ProductStatus::Unavailable,
                };
                add(&mut selected, product, Some(note));
            }
        }
    }
    Ok(selected)
}

/// Plans finishing the folder's interrupted update, reinstall, or repair. Its
/// record's `pending` is the target, with what it started from, when it
/// belongs to the journal. Else the journal's builds are fetched again, or
/// GOG's latest when the base game's is no longer listed, which then starts
/// from nothing and deletes nothing (question 12). A repair never changes
/// the build, so it is refused instead.
fn resumed(
    paths: &Paths,
    session: &mut Session,
    name: &str,
    state: FolderState,
    options: Options,
) -> Result<Plan> {
    let journal = state
        .journal
        .clone()
        .expect("a resumed update has a journal");
    let gog_id = journal.target.gog_id;
    let kind = journal.kind;
    let folder_names = names::product_names(&state.folder);
    // Not needed to finish, so a failure only costs the titles.
    let details = product::load_with(
        paths,
        session.client(),
        gog_id,
        options.refresh,
        !options.read_only,
    )
    .ok()
    .map(|details| details.product);
    let title = details
        .as_ref()
        .map(|details| details.title.clone())
        .filter(|title| !title.is_empty())
        .or_else(|| folder_names.get(&gog_id).cloned())
        .unwrap_or_else(|| format!("GOG game {gog_id}"));
    let arch = Environment::load(paths, name)?.wine()?.arch;
    let bits = build::bits(arch);
    let mut warnings = Vec::new();
    let mut target = journal.target.clone();
    let mut retarget = None;
    let installed_build = state
        .committed
        .as_ref()
        .and_then(crate::gog::update::base_build)
        .map(str::to_owned);

    let record = install::files_record(paths, &state.folder);
    let pending = record
        .as_ref()
        .and_then(|record| record::pending(&state, record));
    let from = record
        .as_ref()
        .map(|record| record.follow.clone())
        .unwrap_or_default();
    // The operation's own follow, else the one the game has. Without
    // `pending` nothing says which branch or pin it was for.
    let mut to = pending
        .and_then(|pending| pending.follow.clone())
        .unwrap_or_else(|| from.clone());
    let (manifest, base, store, version_name) = match pending {
        Some(pending) => {
            let store = pending
                .store
                .iter()
                .map(|version| StoreNeed {
                    version: version.clone(),
                    files: None,
                })
                .collect();
            (
                pending.manifest.clone(),
                pending.base.clone(),
                store,
                pending.manifest.setup.version_name.clone(),
            )
        }
        None if kind == JournalKind::Repair => {
            let began = target
                .products
                .iter()
                .find(|product| product.id == gog_id)
                .and_then(|base| base.build.clone());
            let (manifest, base, store) = repair_manifest(
                paths,
                session,
                name,
                &state,
                &title,
                &target,
                began.as_deref(),
                bits,
                from.branch.as_deref(),
                &Entered {
                    secret: options.password.as_ref(),
                    read_only: options.read_only,
                },
                &mut warnings,
            )?;
            let version_name = manifest.setup.version_name.clone();
            (manifest, base, store, version_name)
        }
        None => {
            // The branch the operation was for, which is where its build is.
            let branch = from.branch.as_deref();
            let fetched = install::builds_of(session, gog_id, branch, options.password.as_ref())?;
            if let Some(branch) = branch {
                let game = private::Game {
                    title: &title,
                    gog_id,
                    name: Some(name),
                };
                warnings.extend(fetched.require(session, &game, branch, options.read_only)?);
            }
            let builds = fetched.builds;
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
                        name: Some(name),
                    };
                    let latest = follow::latest_on(&builds, from.branch.as_deref(), &who)?;
                    let pin = from
                        .pinned
                        .as_deref()
                        .filter(|pinned| *pinned == began)
                        .map(|pinned| format!(" The pin to build {pinned} is removed."))
                        .unwrap_or_default();
                    to = Follow {
                        branch: from.branch.clone(),
                        pinned: None,
                    };
                    warnings.push(format!(
                        "GOG no longer lists build {began} of {title}, so the update finishes with build {} instead.{pin}",
                        latest.build_id
                    ));
                    retarget = Some(Retarget {
                        from: began.clone(),
                        to: latest.build_id.clone(),
                    });
                    latest
                }
            };
            if chosen.link.is_none() {
                return Err(GalaxyError::NoBuild {
                    title: title.clone(),
                }
                .into());
            }
            let repository = session
                .client()
                .repository(chosen)
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
                            folder_names.get(&product.id).map(String::as_str),
                        )
                    };
                    return Err(GalaxyError::ProductGone {
                        title,
                        build: chosen
                            .version_name
                            .clone()
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| repository.build_id.clone()),
                        label,
                        installed: true,
                    }
                    .into());
                }
                product.build = Some(repository.build_id.clone());
                chosen_products.push((product.id, language));
            }
            let built = install::manifests(
                session,
                &title,
                &repository,
                &chosen_products,
                bits,
                chosen.version_name.clone(),
            )?;
            warnings.extend(built.warnings);
            let base = if retarget.is_some() {
                None
            } else if let Some(manifest) = stored(paths, &state)? {
                Some(Base {
                    manifest,
                    fetched: false,
                })
            } else if kind == JournalKind::Reinstall {
                // A reinstall without a trusted record deletes nothing.
                None
            } else {
                match &state.committed {
                    Some(committed) => fetched_base(
                        session,
                        &title,
                        &builds,
                        committed,
                        installed_build.as_deref(),
                        &repository,
                        bits,
                    )?,
                    None => None,
                }
            };
            (
                built.manifest,
                base,
                built.store,
                chosen.version_name.clone(),
            )
        }
    };
    let store = install::complete_store(session, store)?;
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
            size: install::product_size(&manifest, product.id),
        })
        .collect();
    let notes = notes(&manifest, base.is_some());
    Ok(Plan {
        environment: name.to_owned(),
        folder: state.folder.clone(),
        gog_id,
        title,
        kind,
        recovery: true,
        seen: state,
        arch,
        installed_build,
        installed_version: None,
        version_name,
        products,
        target,
        manifest,
        base,
        store,
        retarget,
        from,
        to,
        notes,
        warnings,
    })
}

/// Applies a planned Galaxy update. A failure or cancel after the journal is
/// written leaves it, so the game refuses to launch until updating again
/// finishes it. `UpdateError::Canceled` says whether it did.
pub fn apply(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
) -> Result<Updated> {
    apply_with_events(paths, session, plan, token, progress, |_| {})
}

/// `apply`, reporting setup's supervision notices to `events`.
pub fn apply_with_events(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
    mut events: impl FnMut(Event),
) -> Result<Updated> {
    host_only(paths)?;
    let mut journaled = plan.recovery;
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

/// What to do with one of the target's files.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// What's there is kept, with its MD5 when that's known.
    Keep(Option<String>),
    Fetch,
}

/// The folder compared with the target.
struct Compared {
    /// The target, each path spelled as it is on disk.
    manifest: Manifest,
    /// The starting point, spelled the same way.
    base: Option<Base>,
    /// One for each of the target's files.
    files: Vec<Verdict>,
    /// The MD5s of files found holding their base version, by lowercased
    /// path, for when the base lists none.
    verified: HashMap<String, String>,
    /// The support files to download, by index.
    support: Vec<usize>,
    game_bytes: u64,
    support_bytes: u64,
}

/// Compares the folder with the plan's target, reading only. A file the
/// update doesn't change is left alone with a stored manifest, and checked
/// by size with a fetched one, since hashing a whole game takes minutes; any
/// other file there is hashed against its new version, and against its old
/// one, which says whether it is wineo's to replace. A repair or reinstall
/// hashes every file and support file and reports how far it got.
fn compare(
    paths: &Paths,
    plan: &Plan,
    token: &Token,
    progress: &mut dyn FnMut(Progress),
) -> Result<Compared> {
    let root = package::open_folder(&plan.folder)?;
    let mut manifest = install::on_disk(&root, &plan.manifest);
    let base = plan.base.as_ref().map(|base| Base {
        manifest: install::on_disk(&root, &base.manifest),
        fetched: base.fetched,
    });
    let old: HashMap<String, &FileEntry> = base
        .iter()
        .flat_map(|base| &base.manifest.files)
        .map(|file| (file.path.to_lowercase(), file))
        .collect();
    let stored = base.as_ref().is_some_and(|base| !base.fetched);
    let full = plan.hashes_all();
    let mut checked = Checked {
        files: 0,
        files_total: manifest.files.len() + manifest.support.len(),
        bytes: 0,
        bytes_total: sizes(&manifest.files) + sizes(&manifest.support),
    };
    let mut verified = HashMap::new();
    let mut files = Vec::new();
    for file in &manifest.files {
        let old = old.get(&file.path.to_lowercase()).copied();
        files.push(judge(&root, file, old, stored, full, token, &mut verified)?);
        if full {
            checked.add(file, progress);
        }
    }
    let support_root = paths.gog_support_dir(&folder::key(&plan.folder));
    let support =
        install::support_to_fetch(&support_root, &mut manifest.support, token, &mut |file| {
            if full {
                checked.add(file, progress);
            }
        })?;
    let game_bytes = manifest
        .files
        .iter()
        .zip(&files)
        .filter(|(_, verdict)| **verdict == Verdict::Fetch)
        .map(|(file, _)| file.size)
        .sum();
    let support_bytes = support
        .iter()
        .map(|&index| manifest.support[index].size)
        .sum();
    Ok(Compared {
        manifest,
        base,
        files,
        verified,
        support,
        game_bytes,
        support_bytes,
    })
}

/// How many files and bytes a hashing pass has checked.
struct Checked {
    files: usize,
    files_total: usize,
    bytes: u64,
    bytes_total: u64,
}

impl Checked {
    fn add(&mut self, file: &FileEntry, progress: &mut dyn FnMut(Progress)) {
        self.files += 1;
        self.bytes += file.size;
        progress(Progress::Verify {
            files: self.files,
            files_total: self.files_total,
            bytes: self.bytes,
            bytes_total: self.bytes_total,
        });
    }
}

/// What one target file needs, against what is at its path and its base
/// version `old`, if the base lists one. `stored` says the base is the
/// manifest wineo stored at its last commit. `full` is a repair or
/// reinstall: no file is trusted or judged by its size, so each is hashed
/// whatever the base says.
fn judge(
    root: &std::os::fd::OwnedFd,
    file: &FileEntry,
    old: Option<&FileEntry>,
    stored: bool,
    full: bool,
    token: &Token,
    verified: &mut HashMap<String, String>,
) -> Result<Verdict> {
    let path = file.path.as_str();
    let unchanged_by_update = old.filter(|old| !full && same_content(old, file));
    // Left as it is, there or not, whatever its size: you may have deleted
    // an intro video or edited a setting on purpose, and putting a file
    // back is what `--repair` is for.
    if stored && let Some(old) = unchanged_by_update {
        return Ok(Verdict::Keep(file.md5.clone().or_else(|| old.md5.clone())));
    }
    let Ok(dir) = fetch::open_dir(root, publish::parent_of(path)) else {
        return Ok(Verdict::Fetch);
    };
    let leaf = publish::leaf_of(path);
    if publish::current(&dir, leaf, path)? != Some(Current::File) {
        return Ok(Verdict::Fetch);
    }
    if let Some(old) = unchanged_by_update {
        return unchanged(&dir, leaf, path, file, old);
    }
    let mut entries = vec![file];
    entries.extend(old);
    Ok(
        match install::verify_all(&dir, leaf, path, &entries, token)? {
            Some((md5, same)) if same[0] => Verdict::Keep(Some(md5)),
            Some((md5, same)) => {
                if same.get(1) == Some(&true) {
                    verified.insert(path.to_lowercase(), md5);
                }
                Verdict::Fetch
            }
            None => Verdict::Fetch,
        },
    )
}

/// A file the update doesn't change, with a fetched manifest: kept when
/// it's there at its size, as question 4 settled for a game with no stored
/// manifest. The size alone
/// can't tell an edit of the same length from the file, which `--repair`
/// finds. One of another size is downloaded again, and what was there is
/// saved aside, since it doesn't hold a hash wineo recorded.
fn unchanged(
    dir: &std::os::fd::OwnedFd,
    leaf: &str,
    path: &str,
    file: &FileEntry,
    old: &FileEntry,
) -> Result<Verdict> {
    let stat = rustix::fs::statat(dir, leaf, AtFlags::SYMLINK_NOFOLLOW)
        .with_context(|| format!("failed to inspect {path}"))?;
    Ok(if u64::try_from(stat.st_size).ok() == Some(file.size) {
        Verdict::Keep(file.md5.clone().or_else(|| old.md5.clone()))
    } else {
        Verdict::Fetch
    })
}

/// What the base records at each path, by lowercased path.
fn base_known(base: Option<&Base>) -> HashMap<String, Known> {
    let mut known: HashMap<String, Known> = HashMap::new();
    let Some(base) = base else {
        return known;
    };
    for file in &base.manifest.files {
        let entry = known.entry(file.path.to_lowercase()).or_default();
        entry.files.extend(file.md5.clone());
    }
    for link in &base.manifest.links {
        let entry = known.entry(link.path.to_lowercase()).or_default();
        entry.links.push(link.target.clone());
    }
    for dir in &base.manifest.directories {
        known.entry(dir.path.to_lowercase()).or_default().folder = true;
    }
    known
}

#[allow(clippy::too_many_arguments)]
fn run(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    events: &mut dyn FnMut(Event),
    journaled: &mut bool,
) -> Result<Updated> {
    let name = plan.environment.as_str();
    let folder = plan.folder.as_path();
    let cancel = || token.is_canceled();
    let changed = || UpdateError::Changed {
        name: name.to_owned(),
    };
    let mut warnings = plan.warnings.clone();
    windows::check_graphical()?;
    // Compared before the locks, as an install measures what it needs: it
    // only reads, and hashing a big game takes a while. From the locks on,
    // no other wineo and no launch changes the folder, and finishing an
    // interrupted attempt under them only ever empties a path, never fills
    // one, so what this found still holds.
    let compared = compare(paths, plan, token, &mut |step| {
        (progress.borrow_mut())(step)
    })?;
    let support_root = paths.gog_support_dir(&folder::key(folder));
    let redist = paths.gog_redist_dir();
    // Staged files count whole, chunks reused or not, and deletions free
    // nothing until after publishing.
    let space = |store: u64| {
        install::space(
            paths,
            &[
                (folder, compared.game_bytes),
                (support_root.as_path(), compared.support_bytes),
                (redist.as_path(), store),
            ],
        )
    };
    space(install::planned_store(paths, &plan.store))?;

    let prefix = crate::paths::resolve(&Environment::load(paths, name)?.prefix(paths)?);
    let guard = lock_operation_on(paths, name, &[prefix.as_path()])?;
    if crate::gog::update::recorded_gog_id(paths, name)? != Some(plan.gog_id.to_string()) {
        return Err(changed().into());
    }
    let _in_use = folder::lock_for_update(paths, folder, name)?;
    let lock = FolderLock::exclusive(paths, folder, &cancel)?;
    let mut state = lock.read()?.ok_or_else(changed)?;
    let unowned = |state: &FolderState| FolderState {
        owner: None,
        ..state.clone()
    };
    if unowned(&state) != unowned(&plan.seen) {
        return Err(changed().into());
    }
    let claimed = plan.seen.owner.is_none();
    if !claimed && state.owner != plan.seen.owner {
        return Err(changed().into());
    }
    // Another wineo may have pinned or moved the game since planning, which
    // this must not write over. A game coming over from the installers has
    // no follow to compare.
    if plan.moves_from().is_none() && record::follow_now(&lock)? != plan.from {
        return Err(changed().into());
    }
    // Held from before each version is looked for until the update ends,
    // and checked for space before the first write, as an install does.
    let mut held = Vec::new();
    let mut store_jobs = Vec::new();
    for need in &plan.store {
        let version = store::hold(paths, &need.version, token)?;
        if !version.present() {
            let files = match &need.files {
                Some(files) => files.clone(),
                None => install::store_files(session, &need.version)?,
            };
            store_jobs.push((held.len(), files));
        }
        held.push(version);
    }
    space(store_jobs.iter().map(|(_, files)| sizes(files)).sum())?;
    // The last check before the first write.
    if token.is_canceled() {
        return Err(UpdateError::Canceled {
            journal: *journaled,
        }
        .into());
    }
    failpoint("galaxy.update.before")?;
    if claimed {
        // Claimed like a launch does: written only while still unowned.
        state = lock.reclaim(name)?;
    }

    let Compared {
        mut manifest,
        base,
        files: verdicts,
        verified,
        support: support_fetch,
        ..
    } = compared;
    let base_known = base_known(base.as_ref());
    let known_at = |path: &str| {
        base_known
            .get(&path.to_lowercase())
            .cloned()
            .unwrap_or_default()
    };
    let root = package::open_folder(folder)?;
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
        let attempted = journal.attempted.clone();
        publisher.recover_asides(|path| merged(recorded(&attempted, path), known_at(path)))?;
    }

    let expected = record::expected(&state, plan.kind, &plan.target);
    let selection = record::digest(&expected);
    let mut sidecar = record::read(&lock)?.unwrap_or_default();
    // Setup steps recorded count only while the record is this operation's
    // or the committed selection's; anything else runs every step again.
    // A repair or reinstall runs every step, so its first attempt starts from
    // none; one finishing a journal carries on from the steps this operation
    // recorded.
    if !record::carries_setup(&state, &sidecar)? || (plan.hashes_all() && state.journal.is_none()) {
        sidecar.setup.clear();
    }
    sidecar.pending = Some(record::Pending {
        operation: operation.clone(),
        selection: selection.clone(),
        manifest: manifest.clone(),
        store: manifest.store_versions(),
        base: base.clone(),
        // Only a change is recorded, so a record that changes nothing stays
        // readable by a wineo without branches.
        follow: (plan.to != plan.from).then(|| plan.to.clone()),
    });
    // Before the journal, so a journal always has its pending written.
    record::write(&lock, &sidecar)?;
    failpoint("galaxy.update.pending.after")?;

    let fetch_files: Vec<usize> = verdicts
        .iter()
        .enumerate()
        .filter(|(_, verdict)| **verdict == Verdict::Fetch)
        .map(|(index, _)| index)
        .collect();
    let mut journal = state.journal.clone().unwrap_or_else(|| Journal {
        operation: operation.clone(),
        kind: plan.kind,
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
    for &index in &fetch_files {
        let file = &manifest.files[index];
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
    failpoint("galaxy.update.journal.after")?;

    for (index, verdict) in verdicts.iter().enumerate() {
        if let Verdict::Keep(Some(md5)) = verdict {
            manifest.files[index].md5 = Some(md5.clone());
        }
    }
    let attempted = state
        .journal
        .as_ref()
        .expect("written above")
        .attempted
        .clone();
    let verified_at = |path: &str| Known {
        files: verified
            .get(&path.to_lowercase())
            .cloned()
            .into_iter()
            .collect(),
        ..Known::default()
    };
    let known = |path: &str| {
        merged(
            merged(recorded(&attempted, path), known_at(path)),
            verified_at(path),
        )
    };
    publisher.note_folders(
        manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .chain(manifest.directories.iter().map(|dir| dir.path.as_str()))
            .chain(manifest.links.iter().map(|link| link.path.as_str()))
            .chain(attempted.iter().map(|entry| entry.path.as_str()))
            .chain(base.iter().flat_map(|base| {
                let base = &base.manifest;
                base.files
                    .iter()
                    .map(|file| file.path.as_str())
                    .chain(base.directories.iter().map(|dir| dir.path.as_str()))
                    .chain(base.links.iter().map(|link| link.path.as_str()))
            })),
    );
    if let Some(base) = &base {
        clear_type_changes(&mut publisher, base, &manifest, &known, token)?;
    }
    {
        let mut placer = Placer {
            publisher: &mut publisher,
            known: &known,
            token,
        };
        for (path, kind) in install::entries_in_order(&manifest) {
            match kind {
                Kind::Directory => placer.directory(path)?,
                Kind::Link(target) => placer.link(path, target)?,
                Kind::File(index) if verdicts[index] == Verdict::Fetch => {
                    placer.clear_for_file(path)?;
                }
                Kind::File(_) => {}
            }
        }
    }
    failpoint("galaxy.update.placed.after")?;

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
    let mut done_before = 0u64;
    let report = |done: u64| (progress.borrow_mut())(Progress::Download { done, total });
    let links = Links::new(
        fetch_files
            .iter()
            .map(|&index| &manifest.files[index])
            .chain(support_fetch.iter().map(|&index| &manifest.support[index])),
    );
    for (index, files) in &store_jobs {
        let mut counted = |done: u64| report(done_before + done);
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
        done_before += sizes(files);
    }
    {
        let mut counted = |done: u64| report(done_before + done);
        install::fetch_support(
            session,
            &support_root,
            &mut manifest.support,
            &support_fetch,
            &mut publisher,
            &links,
            token,
            &mut counted,
        )?;
        done_before += support_fetch
            .iter()
            .map(|&index| manifest.support[index].size)
            .sum::<u64>();
    }
    install::sync_support(&support_root, &manifest.support)?;
    failpoint("galaxy.update.support.after")?;

    // Each changed file's old copy, opened before anything replaces it, for
    // the chunks it still holds.
    let files = manifest.files.clone();
    let old: HashMap<String, &FileEntry> = base
        .iter()
        .flat_map(|base| &base.manifest.files)
        .map(|file| (file.path.to_lowercase(), file))
        .collect();
    let copies: Vec<Option<File>> = fetch_files
        .iter()
        .map(|&index| {
            let path = files[index].path.as_str();
            if !old.contains_key(&path.to_lowercase()) {
                return Ok(None);
            }
            let Some(dir) = publisher.open_parent(path)? else {
                return Ok(None);
            };
            install::open_regular(&dir, publish::leaf_of(path), path)
        })
        .collect::<Result<_>>()?;
    let mut jobs = Vec::new();
    for (slot, &index) in fetch_files.iter().enumerate() {
        let file = &files[index];
        let reuse = match (&copies[slot], old.get(&file.path.to_lowercase())) {
            (Some(copy), Some(old)) => Some(Reuse {
                file: copy,
                at: fetch::reuse_offsets(old, file),
            }),
            _ => None,
        };
        jobs.push(Job {
            root: &root,
            parent: publish::parent_of(&file.path).to_owned(),
            name: publisher.temp_name(),
            file,
            links: links.of(file),
            reuse,
        });
    }
    let index_of: HashMap<&str, usize> = fetch_files
        .iter()
        .map(|&index| (files[index].path.as_str(), index))
        .collect();
    // Besides the journal: what the update started from, and the old
    // versions found in place.
    let extra = |path: &str| merged(known_at(path), verified_at(path));
    let mut waiting: Vec<Waiting> = Vec::new();
    {
        let mut counted = |done: u64| report(done_before + done);
        fetch::run(session, &jobs, token, &mut counted, &mut |job, md5| {
            let index = index_of[job.file.path.as_str()];
            let named = manifest.files[index].md5.is_some();
            manifest.files[index].md5 = Some(md5.clone());
            if named {
                place(&mut publisher, &state, &job.name, &job.file.path, &extra)?;
            } else {
                waiting.push((job.file.path.clone(), job.name.clone(), md5));
                if waiting.len() >= NAME_BATCH {
                    flush(&lock, &mut waiting, &mut state, &mut publisher, &extra)?;
                }
            }
            failpoint("galaxy.update.file.after")
        })?;
    }
    flush(&lock, &mut waiting, &mut state, &mut publisher, &extra)?;
    drop(jobs);
    drop(copies);
    failpoint("galaxy.update.files.after")?;

    // Deletions only once everything new is in place, and only from what
    // the update started from: files wineo never listed, like what setup
    // leaves in the folder, are never looked at.
    if let Some(base) = &base {
        delete_old(&mut publisher, base, &manifest, &known, token)?;
        delete_old_support(&support_root, base, &manifest, token)?;
    }
    failpoint("galaxy.update.deleted.after")?;
    publisher.finish(&[])?;
    let report_saved = publisher.into_report();

    // Setup in the prefix: only steps whose identity changed and new ones,
    // when the record's steps still count.
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
    failpoint("galaxy.update.setup.after")?;
    sidecar.setup = steps.into_iter().map(|step| step.identity).collect();

    // `installed` before the commit, so a crash after it already finds the
    // record matching what's committed.
    build::sort(&mut manifest);
    sidecar.installed = Some(record::Installed {
        selection,
        info: record::info_hash(folder, plan.gog_id)?,
        manifest,
    });
    sidecar.follow = plan.to.clone();
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: true }.into());
    }
    record::write(&lock, &sidecar)?;
    failpoint("galaxy.update.installed.after")?;
    publish::record_sync("commit".to_owned());
    lock.commit(expected)?;
    failpoint("galaxy.update.commit.after")?;
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
    Ok(Updated {
        environment: name.to_owned(),
        folder: folder.to_owned(),
        kind: plan.kind,
        recovered: plan.recovery,
        claimed,
        products: plan.target.products.clone(),
        saved: report_saved.saved,
        kept: report_saved.kept,
        overwritten: report_saved.overwritten,
        warnings,
        needs_restart: ran.needs_restart,
    })
}

/// The lowercased paths of everything `manifest` puts in the folder, with
/// each one's type.
fn kinds(manifest: &Manifest) -> HashMap<String, EntryType> {
    manifest
        .files
        .iter()
        .map(|file| (file.path.to_lowercase(), EntryType::File))
        .chain(
            manifest
                .directories
                .iter()
                .map(|dir| (dir.path.to_lowercase(), EntryType::Directory)),
        )
        .chain(
            manifest
                .links
                .iter()
                .map(|link| (link.path.to_lowercase(), EntryType::Symlink)),
        )
        .collect()
}

/// Empties each base folder the target ships as a file or link, so that
/// placing it can remove the folder rather than save it aside with wineo's
/// own files in it: what's inside is deleted on a hash match like any old
/// entry.
fn clear_type_changes(
    publisher: &mut Publisher,
    base: &Base,
    target: &Manifest,
    known: &dyn Fn(&str) -> Known,
    token: &Token,
) -> Result<()> {
    let target_kinds = kinds(target);
    let changed: Vec<String> = base
        .manifest
        .directories
        .iter()
        .filter(|dir| {
            target_kinds
                .get(&dir.path.to_lowercase())
                .is_some_and(|kind| *kind != EntryType::Directory)
        })
        .map(|dir| format!("{}/", dir.path.to_lowercase()))
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let inside = |path: &str| {
        let lower = path.to_lowercase();
        changed.iter().any(|dir| lower.starts_with(dir))
    };
    remove_entries(publisher, &base.manifest, known, token, |path| inside(path))
}

/// Deletes what the base lists and the target doesn't: files and links on a
/// hash match, deepest first, then folders left empty, which gogdl does too
/// since The Witcher 3 hangs on an empty content folder.
fn delete_old(
    publisher: &mut Publisher,
    base: &Base,
    target: &Manifest,
    known: &dyn Fn(&str) -> Known,
    token: &Token,
) -> Result<()> {
    let target_kinds = kinds(target);
    remove_entries(publisher, &base.manifest, known, token, |path| {
        !target_kinds.contains_key(&path.to_lowercase())
    })
}

/// Deletes the entries of `base` that `chosen` picks: each file or link only
/// while it holds what wineo recorded for it, deepest first, then each folder
/// once it's empty.
fn remove_entries(
    publisher: &mut Publisher,
    base: &Manifest,
    known: &dyn Fn(&str) -> Known,
    token: &Token,
    chosen: impl Fn(&str) -> bool,
) -> Result<()> {
    let mut entries: Vec<(&str, Option<&FileEntry>)> = base
        .files
        .iter()
        .map(|file| (file.path.as_str(), Some(file)))
        .chain(base.links.iter().map(|link| (link.path.as_str(), None)))
        .filter(|(path, _)| chosen(path))
        .collect();
    entries.sort_by(|a, b| b.0.cmp(a.0));
    for (path, file) in entries {
        let Some(dir) = publisher.open_parent(path)? else {
            continue;
        };
        let mut known = known(path);
        // A fetched manifest may list no MD5 for a big file: one read
        // against its chunks shows whether it is GOG's.
        if let Some(file) = file.filter(|file| file.md5.is_none())
            && let Some((md5, true)) =
                install::verify(&dir, publish::leaf_of(path), path, file, token)?
        {
            known.files.push(md5);
        }
        publisher.remove_known(&dir, path, &known)?;
    }
    let mut dirs: Vec<&str> = base
        .directories
        .iter()
        .map(|dir| dir.path.as_str())
        .filter(|path| chosen(path))
        .collect();
    dirs.sort_by_key(|path| std::cmp::Reverse(install::publish_depth(path)));
    for path in dirs {
        let Some(dir) = publisher.open_parent(path)? else {
            continue;
        };
        let leaf = publish::leaf_of(path);
        if publish::current(&dir, leaf, path)? == Some(Current::Directory) {
            // Only an empty folder goes, so nothing in it can be lost.
            publisher.remove_folder(&dir, leaf, path)?;
        }
    }
    Ok(())
}

/// Deletes the base's support files the target doesn't have, each only
/// while it still holds what GOG shipped. The support folder is wineo's own,
/// read only during setup.
fn delete_old_support(root: &Path, base: &Base, target: &Manifest, token: &Token) -> Result<()> {
    let listed: HashSet<(Option<u64>, String)> = target
        .support
        .iter()
        .map(|file| (file.product, file.path.to_lowercase()))
        .collect();
    let Ok(fd) = package::open_folder(root) else {
        return Ok(());
    };
    for file in &base.manifest.support {
        if listed.contains(&(file.product, file.path.to_lowercase())) {
            continue;
        }
        let Some(product) = file.product else {
            continue;
        };
        let relative = format!("{product}/{}", file.path);
        let Ok(dir) = fetch::open_dir(&fd, publish::parent_of(&relative)) else {
            continue;
        };
        let leaf = publish::leaf_of(&relative);
        if let Some((_, true)) = install::verify(&dir, leaf, &file.path, file, token)? {
            match rustix::fs::unlinkat(&dir, leaf, AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to remove support file {relative}"));
                }
            }
        }
    }
    Ok(())
}
