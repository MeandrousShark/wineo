//! Installing a GOG game that isn't installed, into a fresh folder, as a new
//! environment with `gog_id`: native from GOG's Linux packages, or Wine or
//! Proton running GOG's Windows installers.
//!
//! A fresh native install is a rebuild of every product into an empty
//! folder, so it shares an update's machinery in `apply`. A Windows install
//! writes the same state and journal, then creates the environment and runs
//! each product's installer in it (see `windows`). `plan` resolves the game and its
//! owned DLC and reads only. `apply` downloads every package, opens them and
//! checks they fit, then under the locks creates the folder, writes its
//! state with an `install` journal and no `[committed]`, writes the
//! environment's config, and rebuilds. Anything that stops it after the journal is
//! written leaves the journal, which `wineo update NAME` finishes like an
//! interrupted update.
//!
//! GOG's playtime and art aren't fetched here. Like after a migration, the
//! frontend runs `extras::import` once `apply` has returned.
//!
//! Lock order: the adoption lock, so two installs or migrations into nested
//! folders can't both pass the overlap check, then the environment's
//! operation lock, the folder's in-use lock, its state lock, and the config
//! lock.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wineo_gog::DownloadCache;

use super::apply::{
    self, Packages, PlannedFile, Progress, Wanted, Work, check_fit, download, free_space,
    is_cancel, lock_operation, lock_operation_on, open_package, operation_id, pin_packages,
    rebuild,
};
use super::{Check, SavedAside, UpdateError};
use crate::cancel::Token;
use crate::config::{Arch, GlobalConfig, Target as TargetConfig};
use crate::create::{self, CreateRequest};
use crate::environment;
use crate::gog::account::{AccountError, Session};
use crate::gog::folder::{
    self, AdoptionLock, Attempted, EntryType, FolderLock, FolderState, Journal, JournalKind,
    Package, Platform, Product, ProductStatus, Target, Track, failpoint,
};
use crate::gog::install_dir::install_folder;
use crate::gog::migrate::host_only;
use crate::gog::package::{self, SpaceNeeded};
use crate::gog::{language, names, product};
use crate::import::{self, ImportedSettings};
use crate::paths::Paths;
use crate::runner::Runner;

use super::windows::{self, Event, SetupError};

/// The language every GOG product falls back to.
const FALLBACK: &str = "en";

/// The target a new game's environment gets, as a migrated one does.
const TARGET: &str = windows::TARGET;

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The base game has no installer wineo can use for the platform.
    #[error("{title} can't be installed: {why}")]
    NoInstaller { title: String, why: String },
    /// The environment or folder isn't what the plan saw any more.
    #[error("{what} changed since the install was planned; plan it again.")]
    Changed { what: String },
    /// The name's own install stopped in another folder.
    #[error("{name}'s install in {folder} didn't finish. Install it again there to finish it.")]
    PendingElsewhere { name: String, folder: PathBuf },
    /// The name's own install is a Galaxy one, which this offline planner
    /// can't finish. Only a direct caller reaches it, since front ends plan
    /// through `galaxy::plan`.
    #[error("{name}'s Galaxy install didn't finish, which galaxy::plan finishes.")]
    GalaxyPending { name: String },
}

/// What to install, and where.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// The new environment's name.
    pub name: String,
    pub gog_id: u64,
    /// Where the game goes instead of `<games_root>/<title>`.
    pub dir: Option<PathBuf>,
    /// A locale tag like `fr-FR`. None uses the global `gog.language`.
    pub language: Option<String>,
    /// Whether the target's desktop entry is turned on. The frontend writes
    /// the entry, as after a migration.
    pub desktop: bool,
    /// Fetch GOG's product details again instead of using a copy under an
    /// hour old.
    pub refresh: bool,
    /// Write nothing while planning, for a dry run: neither GOG's details
    /// are cached.
    pub read_only: bool,
    /// None installs natively when GOG has a Linux installer, else from its
    /// Windows installer. An interrupted install carries on as it began.
    pub platform: Option<Platform>,
    /// The runner and arch a Windows game's new environment gets. None uses
    /// the global defaults, like `new`.
    pub runner: Option<String>,
    pub arch: Option<Arch>,
    /// Run the base game's Windows installer as its own wizard instead of
    /// silently, after it stopped before installing anything.
    pub wizard: bool,
    /// The Galaxy branch or build to install, the default branch's latest
    /// by default. Only a Galaxy install reads it.
    pub builds: super::galaxy::Choice,
    /// A password entered for the private branch `builds` names. It is saved
    /// once GOG lists the branch with it.
    pub password: Option<wineo_gog::Secret>,
}

/// One product an install would put in the folder, for a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub id: u64,
    /// GOG's title, when its details list one.
    pub title: Option<String>,
    /// The installer's version. None when unavailable.
    pub version: Option<String>,
    /// GOG's code for the installer's language. None when unavailable.
    pub language: Option<String>,
    /// `Installed` when it will be installed, else `Unavailable`.
    pub status: ProductStatus,
    /// Why it's unavailable, or that it falls back to English.
    pub note: Option<String>,
    /// The size GOG lists for its package, rounded to whole MiB, or zero
    /// when unavailable.
    pub download_size: u64,
}

/// An install, resolved and ready to apply.
#[derive(Debug, Clone)]
pub struct Plan {
    environment: String,
    gog_id: u64,
    title: String,
    folder: PathBuf,
    requested: String,
    language: String,
    desktop: bool,
    /// Whether this environment's own interrupted install is carried on.
    resume: bool,
    /// Whether the environment's config exists, which a resumed install's
    /// may not yet.
    has_config: bool,
    /// A resumed install's target: its journal's, or the same products
    /// resolved again from GOG's current packages.
    resumed_target: Option<Target>,
    /// Whether a resumed install finishes from the packages its journal
    /// names, already in the download cache.
    offline: bool,
    products: Vec<Planned>,
    files: Vec<PlannedFile>,
    /// The folder's state when planned: none, or the resumed install's.
    seen: Option<FolderState>,
    warnings: Vec<String>,
    platform: Platform,
    /// How a Windows game's environment is created, when it has no config.
    creation: Option<Creation>,
    wizard: bool,
    /// What the review calls the base game's language, like `English`.
    language_name: String,
}

/// A Windows game's new environment, resolved while planning.
#[derive(Debug, Clone)]
pub struct Creation {
    runner: Runner,
    arch: Option<Arch>,
    /// What `arch` comes to, for a review.
    resolved_arch: Arch,
    prefix: PathBuf,
    adopted: bool,
}

impl Creation {
    pub fn runner(&self) -> &Runner {
        &self.runner
    }

    /// The arch asked for. None uses what the runner and global config say.
    pub fn arch(&self) -> Option<Arch> {
        self.arch
    }

    /// The arch the prefix gets, from the request or the defaults.
    pub fn resolved_arch(&self) -> Arch {
        self.resolved_arch
    }

    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// Whether a prefix is already there to adopt, as one an interrupted
    /// install built leaves.
    pub fn adopted(&self) -> bool {
        self.adopted
    }
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

    /// GOG's code for the language the base game installs in, which the
    /// folder's state records.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// GOG's name for the language the base game installs in, like
    /// `English`. Finishing from saved packages without GOG's details, it's
    /// the tag asked for or GOG's code instead.
    pub fn language_name(&self) -> &str {
        &self.language_name
    }

    /// Whether this carries on the environment's own interrupted install
    /// instead of creating the environment.
    pub fn resumes(&self) -> bool {
        self.resume
    }

    /// Whether a resumed install finishes from packages already in the
    /// download cache, without GOG.
    pub fn is_offline(&self) -> bool {
        self.offline
    }

    /// Whether the environment's config is already written, as a resumed
    /// install's may be.
    pub fn has_config(&self) -> bool {
        self.has_config
    }

    /// The base game first, then each owned DLC in GOG's order.
    pub fn products(&self) -> &[Planned] {
        &self.products
    }

    /// A DLC as `NAME (ID)`.
    pub fn dlc_label(&self, id: u64) -> String {
        let title = self
            .products
            .iter()
            .find(|product| product.id == id)
            .and_then(|product| product.title.as_deref());
        names::dlc_label(&self.title, id, title)
    }

    /// What has to be downloaded at most, from the sizes GOG lists, which
    /// are rounded to whole MiB. Files already cached aren't fetched again.
    pub fn download_size(&self) -> u64 {
        if self.offline {
            return 0;
        }
        self.files.iter().map(|file| file.size).sum()
    }

    /// An estimate of the space the installed game takes. Natively it's
    /// only known once the packages are downloaded, so until then it's their
    /// size, since GOG's Linux packages are barely compressed. Windows
    /// installers never say, so it stays twice their size.
    pub fn installed_estimate(&self) -> u64 {
        let size = self.files.iter().map(|file| file.size).sum();
        match self.platform {
            Platform::Linux => size,
            Platform::Windows => windows::estimate(size),
        }
    }

    /// Whether the game installs natively or from its Windows installers.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// How a Windows game's environment is created. None for a native game,
    /// or an environment whose config is already written.
    pub fn creation(&self) -> Option<&Creation> {
        self.creation.as_ref()
    }

    /// Whether the base game's installer runs as its own wizard.
    pub fn wizard(&self) -> bool {
        self.wizard
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// What the install commits if every package installs.
    fn target(&self) -> Target {
        if let Some(target) = &self.resumed_target {
            return target.clone();
        }
        Target {
            gog_id: self.gog_id,
            platform: self.platform,
            language: self.language.clone(),
            track: Track::Installer,
            products: self
                .products
                .iter()
                .map(|planned| Product {
                    reported_language: None,
                    id: planned.id,
                    version: planned.version.clone(),
                    build: None,
                    // Recorded only where it differs from the folder's.
                    language: planned
                        .language
                        .clone()
                        .filter(|language| *language != self.language),
                    status: planned.status,
                })
                .collect(),
        }
    }
}

/// What `apply` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub environment: String,
    pub folder: PathBuf,
    pub gog_id: u64,
    pub title: String,
    /// Whether it carried on an interrupted install.
    pub resumed: bool,
    /// The products as committed, with any DLC that failed.
    pub products: Vec<Product>,
    /// Anything already in the folder where the game puts something: only
    /// possible when resuming.
    pub saved: Vec<SavedAside>,
    /// Files an earlier attempt wrote that you changed and the game no
    /// longer ships: only possible when resuming with a newer plan.
    pub kept: Vec<String>,
    pub overwritten: Vec<String>,
    /// Including why each failed DLC failed, and a language the installed
    /// game reports that isn't the one asked for.
    pub warnings: Vec<String>,
    /// A Windows game's default target, from its `goggame-<id>.info`. None
    /// when it names no program to start, so the frontend offers the
    /// programs it finds instead, like `new`; or when it's native, whose
    /// target is always `start.sh`.
    pub target: Option<PathBuf>,
    /// GOG's code for the language a Windows game's info file reports.
    pub reported_language: Option<String>,
    /// Redistributables a Galaxy install set up that asked for a restart,
    /// by id. Wine has nothing to restart, so they're only listed.
    pub needs_restart: Vec<String>,
}

/// Works out what installing `request.gog_id` natively does. Reads only: it
/// takes no locks and writes nothing but GOG's product details cache,
/// unless `read_only`, and a renewed login.
///
/// Refused in a portable root, for a name already taken (unless it is this
/// game's environment with its own interrupted install in the folder), for
/// a folder that overlaps another game's or isn't empty, and for a game
/// with no Linux installer in the language or English.
pub fn plan(
    paths: &Paths,
    session: &mut Session,
    request: &Request,
    token: &Token,
) -> Result<Plan> {
    host_only(paths)?;
    let global = GlobalConfig::load(paths)?;
    let requested = request
        .language
        .clone()
        .unwrap_or_else(|| global.gog_language().to_owned());
    let code = language::installer_code(&requested).map_err(anyhow::Error::msg)?;
    let name = request.name.as_str();
    let exists = paths.environment_config(name).exists();
    if !exists {
        environment::check_new_name(paths, name)?;
    }

    // The environment's own unfinished install is carried on from its
    // journal, which needs neither GOG nor its details when every package
    // it names is still cached.
    let mut checked_cache = None;
    if let Some(pending) = own_pending(paths, request)? {
        refuse_galaxy(&pending, name)?;
        if exists && super::recorded_gog_id(paths, name)? != Some(request.gog_id.to_string()) {
            environment::check_new_name(paths, name)?;
        }
        let journal = pending
            .journal
            .as_ref()
            .expect("a pending install has a journal");
        let cached = apply::cached(paths, journal, token)?;
        if let Some(files) = cached {
            let folder = folder::check_fresh_install(paths, &pending.folder, name)?;
            check_pending(paths, name, &folder)?;
            let title = names::product_names(&folder)
                .remove(&request.gog_id)
                .unwrap_or_else(|| format!("GOG game {}", request.gog_id));
            return resumed(
                paths,
                session,
                request,
                Some(files),
                Resumed {
                    title,
                    folder,
                    requested,
                    has_config: exists,
                    seen: pending,
                    warnings: Vec::new(),
                },
            );
        }
        checked_cache = Some(pending.folder);
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
    // An empty folder passes the fresh-install check whatever its state, so
    // the state is checked to be this environment's own install here.
    let own_install = seen.as_ref().is_some_and(|state| {
        state.owner.as_deref() == Some(name)
            && state.gog_id() == Some(request.gog_id)
            && state
                .journal
                .as_ref()
                .is_some_and(|journal| journal.kind == JournalKind::Install)
    });
    // Its config is written after its state, so an install that stopped in
    // between has no config yet and is carried on all the same.
    let resume = own_install
        && (!exists || super::recorded_gog_id(paths, name)? == Some(request.gog_id.to_string()));
    if exists && !resume {
        environment::check_new_name(paths, name)?;
    }
    check_pending(paths, name, &folder)?;
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
        refuse_galaxy(&seen, name)?;
        let journal = seen
            .journal
            .as_ref()
            .expect("a resumed install has a journal");
        // Already found not all cached, unless that was another folder.
        let cached = if checked_cache.as_deref() == Some(folder.as_path()) {
            None
        } else {
            apply::cached(paths, journal, token)?
        };
        return resumed(
            paths,
            session,
            request,
            cached,
            Resumed {
                title,
                folder,
                requested,
                has_config: exists,
                seen,
                warnings,
            },
        );
    }

    // Natively when GOG has a Linux installer, and when it has neither, so
    // the refusal names the platform wineo prefers.
    let offers = |os: &str| game.installers.iter().any(|installer| installer.os == os);
    let platform = request
        .platform
        .unwrap_or(if offers("windows") && !offers("linux") {
            Platform::Windows
        } else {
            Platform::Linux
        });
    if platform == Platform::Windows {
        windows::check_graphical()?;
    }
    let shown = os_name(platform);
    let mut products = Vec::new();
    let mut files = Vec::new();
    let base = match pick(&game, &title, &code, platform) {
        Ok(picked) => picked,
        Err(why) => return Err(InstallError::NoInstaller { title, why }.into()),
    };
    if base.language != code {
        warnings.push(format!(
            "{title} has no {shown} installer in {requested}, so it installs in English."
        ));
    }
    let language = base.language.clone();
    let language_name = base
        .installer
        .language_full
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| language.clone());
    base.add(
        game.id,
        (Some(title.clone()), title.clone()),
        None,
        &mut products,
        &mut files,
    );

    // The owned games list may leave DLC out, so ownership comes from the
    // account's full list of owned ids. Without it the game still installs,
    // just without DLC.
    let owned: HashSet<u64> = match session.call(|client, token| client.owned_ids(token)) {
        Ok(ids) => ids.into_iter().collect(),
        Err(error) => {
            if !game.dlc_ids.is_empty() {
                warnings.push(format!(
                    "Couldn't check which DLC you own, so none is installed: {error}"
                ));
            }
            HashSet::new()
        }
    };
    let mut seen_dlc = HashSet::new();
    for &id in &game.dlc_ids {
        if !owned.contains(&id) || !seen_dlc.insert(id) {
            continue;
        }
        let entry = game.dlcs.iter().find(|dlc| dlc.id == id);
        let dlc_title = entry
            .map(|entry| entry.title.clone())
            .filter(|title| !title.is_empty());
        let label = names::dlc_label(&title, id, dlc_title.as_deref());
        // In the language the base game's installer was resolved in, which
        // may already be English. Packages are all fetched before anything
        // runs, so a different language a Windows game reports once
        // installed is only reported, never followed.
        let picked = match entry {
            Some(entry) => pick(entry, &label, &language, platform),
            None => Err(format!("GOG lists no details for {label}.")),
        };
        match picked {
            Ok(picked) => {
                let note = (picked.language != language).then(|| {
                    format!(
                        "{label} has no {shown} installer in {requested}, so it installs in English."
                    )
                });
                picked.add(id, (dlc_title, label), note, &mut products, &mut files);
            }
            Err(why) => products.push(Planned {
                id,
                title: dlc_title,
                version: None,
                language: None,
                status: ProductStatus::Unavailable,
                note: Some(why),
                download_size: 0,
            }),
        }
    }

    Ok(Plan {
        environment: name.to_owned(),
        gog_id: request.gog_id,
        title,
        folder,
        requested,
        language,
        desktop: request.desktop,
        resume,
        has_config: exists,
        resumed_target: None,
        offline: false,
        products,
        files,
        seen,
        warnings,
        platform,
        creation: creation(paths, request, platform, exists)?,
        wizard: request.wizard && platform == Platform::Windows,
        language_name,
    })
}

/// Refuses to carry on a Galaxy install with offline installers, which would
/// run none of its files.
fn refuse_galaxy(state: &FolderState, name: &str) -> Result<()> {
    if state
        .journal
        .as_ref()
        .is_some_and(|journal| journal.target.track == Track::Galaxy)
    {
        return Err(InstallError::GalaxyPending {
            name: name.to_owned(),
        }
        .into());
    }
    Ok(())
}

fn os_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "Linux",
        Platform::Windows => "Windows",
    }
}

/// How a Windows game's environment will be created, checked like `new`
/// checks it, when it has no config yet.
pub(super) fn creation(
    paths: &Paths,
    request: &Request,
    platform: Platform,
    has_config: bool,
) -> Result<Option<Creation>> {
    if platform != Platform::Windows || has_config {
        return Ok(None);
    }
    let plan = create::plan(
        paths,
        CreateRequest {
            name: request.name.clone(),
            runner: request.runner.clone(),
            arch: request.arch,
            gog_id: Some(request.gog_id.to_string()),
            ..CreateRequest::default()
        },
    )?;
    create::check_arch(plan.runner(), request.arch)?;
    Ok(Some(Creation {
        runner: plan.runner().clone(),
        arch: request.arch,
        resolved_arch: plan.arch(),
        prefix: plan.prefix().to_owned(),
        adopted: plan.adopted(),
    }))
}

/// Refuses a name whose install stopped in another folder before its config
/// was written: a state there still names it as owner.
pub(super) fn check_pending(paths: &Paths, name: &str, folder: &Path) -> Result<()> {
    for state in folder::owned_by(paths, name, true)? {
        if state.folder != folder {
            return Err(InstallError::PendingElsewhere {
                name: name.to_owned(),
                folder: state.folder,
            }
            .into());
        }
    }
    Ok(())
}

/// The state of the environment's own unfinished install of this game, in
/// the folder `request.dir` names when it names one.
pub(super) fn own_pending(paths: &Paths, request: &Request) -> Result<Option<FolderState>> {
    let wanted = match &request.dir {
        Some(dir) => match std::fs::canonicalize(dir) {
            Ok(dir) => Some(dir),
            // Not there, so not the pending install's folder.
            Err(_) => return Ok(None),
        },
        None => None,
    };
    Ok(folder::owned_by(paths, &request.name, true)?
        .into_iter()
        .find(|state| {
            state.gog_id() == Some(request.gog_id)
                && state
                    .journal
                    .as_ref()
                    .is_some_and(|journal| journal.kind == JournalKind::Install)
                && wanted.as_ref().is_none_or(|dir| *dir == state.folder)
        }))
}

/// What planning found before it saw that the install is resumed.
struct Resumed {
    title: String,
    folder: PathBuf,
    requested: String,
    has_config: bool,
    seen: FolderState,
    warnings: Vec<String>,
}

/// Plans carrying on the environment's own interrupted install, as `wineo
/// update` finishes one: from the exact packages its journal names when
/// they are all in the cache and verify, else from GOG's current packages
/// for the same products. The products it records are never fewer, since
/// some of them may already be in the folder.
fn resumed(
    paths: &Paths,
    session: &mut Session,
    request: &Request,
    cached: Option<Vec<PlannedFile>>,
    found: Resumed,
) -> Result<Plan> {
    let Resumed {
        title,
        folder,
        requested,
        has_config,
        seen,
        mut warnings,
    } = found;
    let journal = seen
        .journal
        .clone()
        .expect("a resumed install has a journal");
    let platform = journal.target.platform;
    if platform == Platform::Windows {
        windows::check_graphical()?;
    }
    let names = names::product_names(&folder);
    let (target, files, offline) = match cached {
        Some(files) => (journal.target.clone(), files, true),
        None => {
            let resolved = apply::resolve(
                paths,
                session,
                &request.name,
                &seen,
                Wanted {
                    platform,
                    language: &journal.target.language,
                    products: &journal.target.products,
                    names: &names,
                },
                Check {
                    refresh: request.refresh,
                    read_only: request.read_only,
                },
            )?;
            warnings.extend(resolved.warnings);
            (resolved.target, resolved.files, false)
        }
    };
    let products = target
        .products
        .iter()
        .map(|product| Planned {
            id: product.id,
            title: (product.id == target.gog_id).then(|| title.clone()),
            version: product.version.clone(),
            language: (product.status == ProductStatus::Installed).then(|| {
                product
                    .language
                    .clone()
                    .unwrap_or_else(|| target.language.clone())
            }),
            status: product.status,
            note: None,
            download_size: files
                .iter()
                .filter(|file| file.product == product.id)
                .map(|file| file.size)
                .sum(),
        })
        .collect();
    // GOG's name for the language isn't known without its details, so the
    // tag asked for, when it is the same language.
    let language_name =
        if language::installer_code(&requested).as_deref() == Ok(target.language.as_str()) {
            requested.clone()
        } else {
            target.language.clone()
        };
    Ok(Plan {
        environment: request.name.clone(),
        gog_id: request.gog_id,
        title,
        folder,
        requested,
        language_name,
        language: target.language.clone(),
        desktop: request.desktop,
        resume: true,
        has_config,
        resumed_target: Some(target),
        offline,
        products,
        files,
        seen: Some(seen),
        warnings,
        platform,
        creation: creation(paths, request, platform, has_config)?,
        wizard: request.wizard && platform == Platform::Windows,
    })
}

/// A product's installer, picked for a platform and language.
pub(super) struct Picked<'a> {
    installer: &'a wineo_gog::Download,
    /// One package for a Linux installer; a Windows one's setup program and
    /// its parts.
    files: Vec<&'a wineo_gog::DownloadFile>,
    pub(super) version: String,
    /// GOG's code for the installer's language.
    pub(super) language: String,
}

impl Picked<'_> {
    /// Adds the product, labeled `label` in messages, to the plan.
    fn add(
        self,
        id: u64,
        (title, label): (Option<String>, String),
        note: Option<String>,
        products: &mut Vec<Planned>,
        files: &mut Vec<PlannedFile>,
    ) {
        for file in &self.files {
            files.push(PlannedFile {
                product: id,
                label: label.clone(),
                download_id: self.installer.id.clone(),
                file_id: file.id.clone(),
                size: file.size,
                md5: None,
                download: Some(self.installer.clone()),
            });
        }
        products.push(Planned {
            id,
            title,
            version: Some(self.version),
            language: Some(self.language),
            status: ProductStatus::Installed,
            note,
            download_size: self.files.iter().map(|file| file.size).sum(),
        });
    }
}

/// A product's installer for `platform` in GOG's language `code`, else in
/// English, or why there is none it can use.
pub(super) fn pick<'a>(
    entry: &'a wineo_gog::Product,
    label: &str,
    code: &str,
    platform: Platform,
) -> Result<Picked<'a>, String> {
    let (os, shown) = match platform {
        Platform::Linux => ("linux", "Linux"),
        Platform::Windows => ("windows", "Windows"),
    };
    let offered: Vec<&wineo_gog::Download> = entry
        .installers
        .iter()
        .filter(|installer| installer.os == os)
        .collect();
    if offered.is_empty() {
        return Err(format!("{label} has no {shown} installer."));
    }
    let mut chosen = None;
    for language in [code, FALLBACK] {
        let matching: Vec<_> = offered
            .iter()
            .filter(|installer| installer.language == language)
            .collect();
        match matching.as_slice() {
            [] => continue,
            [only] => {
                chosen = Some(**only);
                break;
            }
            _ => {
                return Err(format!(
                    "{label} has more than one {shown} installer in {language}."
                ));
            }
        }
    }
    let Some(installer) = chosen else {
        return Err(format!(
            "{label} has no {shown} installer in {code} or English."
        ));
    };
    let files: Vec<&wineo_gog::DownloadFile> = installer.files.iter().collect();
    match (platform, files.len()) {
        (_, 0) => {
            return Err(format!(
                "GOG lists no files for {label}'s {shown} installer."
            ));
        }
        (Platform::Linux, 1) | (Platform::Windows, _) => {}
        (Platform::Linux, count) => {
            return Err(format!(
                "{label}'s Linux installer comes in {count} files, which wineo can't unpack."
            ));
        }
    }
    let version = installer
        .version
        .clone()
        .ok_or_else(|| format!("GOG lists no version for {label}'s {shown} installer."))?;
    Ok(Picked {
        installer,
        files,
        version,
        language: installer.language.clone(),
    })
}

/// Installs a planned game. Every package is downloaded first, holding no
/// lock and changing nothing. A DLC whose package can't be downloaded or
/// opened is recorded as failed and the game installs without it; the base
/// game's failing refuses the install. Then everything the plan rested on
/// is checked again under the locks, and the folder, its state with an
/// `install` journal, and then the environment are written before any game
/// file. A Windows game's environment is created, and each installer run,
/// after the state; see `windows::install`.
///
/// A failure or cancel after the journal is written leaves it, so the game
/// refuses to launch until `wineo update NAME` finishes the install.
/// `UpdateError::Canceled` says whether it did. Nothing a Windows installer
/// wrote is ever removed.
pub fn apply(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
) -> Result<Installed> {
    apply_with_events(paths, session, plan, token, progress, |_| {})
}

/// `apply`, reporting a Windows game's environment creation and installer
/// runs to `events`.
pub fn apply_with_events(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
    mut events: impl FnMut(Event),
) -> Result<Installed> {
    host_only(paths)?;
    // A resumed install's journal is there from the start.
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
    let windows = plan.platform == Platform::Windows;
    let cancel = || token.is_canceled();
    let changed = |what: &str| InstallError::Changed {
        what: what.to_owned(),
    };
    let mut warnings = plan.warnings.clone();
    let mut target = plan.target();
    let cache = DownloadCache::new(paths.gog_download_cache());
    if windows {
        windows::check_graphical()?;
    }
    check_space_before(paths, &cache, plan)?;

    // Downloading and opening the packages changes nothing, so it holds no
    // lock. A DLC that fails here hasn't changed anything either, so it is
    // recorded as failed and the game installs without it.
    let mut fetched = Vec::new();
    let mut failed = BTreeMap::new();
    for file in &plan.files {
        // Finishing from the journal's packages: checked once pinned.
        if file.download.is_none() {
            fetched.push(file.clone());
            continue;
        }
        if failed.contains_key(&file.product) {
            continue;
        }
        match download(session, &cache, file, token, progress, &mut warnings) {
            Ok(file) => fetched.push(file),
            // A resumed install may already have a DLC's files in the
            // folder, so none of its products can drop out.
            Err(error) if plan.resume || file.product == plan.gog_id || !dlc_can_fail(&error) => {
                return Err(error);
            }
            Err(error) => {
                failed.insert(file.product, format!("{error:#}"));
            }
        }
    }
    // A Windows installer whose other parts failed can't run either.
    fetched.retain(|file| !failed.contains_key(&file.product));
    let (pins, located) = pin_packages(&cache, name, fetched, token, progress)?;
    let mut kept = Vec::new();
    let mut opened = Vec::new();
    let mut setups = BTreeMap::new();
    if windows {
        for (product, found) in windows::setup_programs(&located) {
            match found {
                Ok(exe) => {
                    setups.insert(product, exe);
                }
                Err((label, count)) => {
                    let error = SetupError::NoSetupProgram { label, count };
                    if plan.resume || product == plan.gog_id {
                        return Err(error.into());
                    }
                    failed.insert(product, error.to_string());
                }
            }
        }
        kept = located
            .into_iter()
            .filter(|(file, _)| setups.contains_key(&file.product))
            .collect();
    } else {
        for (file, path) in located {
            match open_package(&path) {
                Ok(package) => {
                    opened.push((file.product, package));
                    kept.push((file, path));
                }
                Err(error) if plan.resume || file.product == plan.gog_id || is_cancel(&error) => {
                    return Err(error.context(format!("failed to open {}'s package", file.label)));
                }
                Err(error) => {
                    failed.insert(
                        file.product,
                        format!("Couldn't open {}'s package: {error:#}", file.label),
                    );
                }
            }
        }
    }
    for (id, why) in &failed {
        let product = target
            .products
            .iter_mut()
            .find(|product| product.id == *id)
            .expect("planned");
        product.status = ProductStatus::Failed;
        product.version = None;
        warnings.push(format!(
            "{} failed and wasn't installed: {why}",
            plan.dlc_label(*id)
        ));
    }
    if windows {
        windows::check_space(paths, folder, &kept)?;
    } else {
        check_fit(paths, folder, &opened)?;
    }

    // A Windows install holds its prefix as well, from here to the commit,
    // so nothing else can replace it between installers.
    let prefix = match (&plan.creation, windows) {
        (Some(creation), _) => Some(crate::paths::resolve(&creation.prefix)),
        (None, true) => Some(crate::paths::resolve(
            &environment::Environment::load(paths, name)?.prefix(paths)?,
        )),
        (None, false) => None,
    };
    let adoption = AdoptionLock::acquire(paths, &cancel)?;
    let operation = match &prefix {
        Some(prefix) => lock_operation_on(paths, name, &[prefix.as_path()])?,
        None => lock_operation(paths, name)?,
    };
    let _in_use = folder::lock_for_update(paths, folder, name)?;
    let lock = FolderLock::exclusive(paths, folder, &cancel)?;
    #[cfg(test)]
    super::apply::tests::install::before_create();
    let state = lock.read()?;
    if state != plan.seen {
        return Err(changed(&folder.display().to_string()).into());
    }
    if paths.environment_config(name).exists() != plan.has_config {
        return Err(changed(name).into());
    }
    if plan.has_config {
        if super::recorded_gog_id(paths, name)? != Some(plan.gog_id.to_string()) {
            return Err(changed(name).into());
        }
    } else {
        environment::check_new_name(paths, name)?;
    }
    check_pending(paths, name, folder)?;
    if folder::check_fresh_install(paths, folder, name)? != folder {
        return Err(changed(&folder.display().to_string()).into());
    }
    // The last check before the first write; from here the folder, its
    // state, and the environment are written together.
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: false }.into());
    }
    failpoint("install.create.before")?;

    let packages: Vec<Package> = kept
        .iter()
        .map(|(file, _)| Package {
            product: file.product,
            download_id: file.download_id.clone(),
            file_id: file.file_id.clone(),
            size: file.size,
            md5: file.md5.clone(),
        })
        .collect();
    let state = match state {
        Some(state) => state,
        None => {
            folder::create_dir_durable(folder)?;
            let mut state = FolderState::new(folder.to_owned(), Some(name.to_owned()));
            state.device = Some(
                std::fs::metadata(folder)
                    .with_context(|| format!("failed to inspect {}", folder.display()))?
                    .dev(),
            );
            state.journal = Some(Journal {
                operation: operation_id(),
                kind: JournalKind::Install,
                target: target.clone(),
                packages: packages.clone(),
                attempted: if windows {
                    windows_attempted()
                } else {
                    Vec::new()
                },
            });
            lock.write(&state)?;
            *journaled = true;
            failpoint("install.state.after")?;
            state
        }
    };

    if windows {
        // A resumed install may run other packages than its journal names,
        // which the journal has to name before they run.
        let mut state = state;
        let journal = state.journal.as_mut().expect("an install has a journal");
        if journal.target != target || journal.packages != packages {
            journal.target = target.clone();
            journal.packages = packages;
            // The last check before the journal changes.
            if token.is_canceled() {
                return Err(UpdateError::Canceled { journal: true }.into());
            }
            lock.write(&state)?;
        }
        // The journal names the packages now, which keeps them cached.
        drop(pins);
        // The state covers the folder now.
        drop(adoption);
        let label = |id: u64| {
            if id == plan.gog_id {
                plan.title.clone()
            } else {
                plan.dlc_label(id)
            }
        };
        let done = windows::install(
            paths,
            lock,
            windows::Job {
                name,
                title: &plan.title,
                target: &mut target,
                setups: &setups,
                label: &label,
                guard: operation,
                prefix: prefix
                    .as_deref()
                    .expect("a Windows install holds its prefix"),
                create: plan.creation.as_ref().map(|creation| CreateRequest {
                    name: name.to_owned(),
                    resolved_runner: Some(creation.runner.clone()),
                    arch: creation.arch,
                    expected_adopted: Some(creation.adopted),
                    gog_id: Some(plan.gog_id.to_string()),
                    ..CreateRequest::default()
                }),
                tag: &plan.requested,
                requested: &plan.language_name,
                wizard: plan.wizard,
                desktop: plan.desktop,
                committed: None,
                // A resumed install's journal was written by an earlier
                // attempt.
                fresh: !plan.resume,
            },
            token,
            events,
            &mut warnings,
        )?;
        return Ok(Installed {
            environment: name.to_owned(),
            folder: folder.to_owned(),
            gog_id: plan.gog_id,
            title: plan.title.clone(),
            resumed: plan.resume,
            products: target.products,
            saved: Vec::new(),
            kept: Vec::new(),
            overwritten: Vec::new(),
            warnings,
            target: done.target,
            reported_language: done.reported_language,
            needs_restart: Vec::new(),
        });
    }

    // The state comes first, naming this environment as owner, so a crash
    // before the config leaves a journal that installing again finishes,
    // never a config whose state is lost.
    let mut library_warning = None;
    if !plan.has_config {
        let config = import::native_config(paths, &settings(plan))?;
        import::write_native_config(paths, name, &config)?;
        if let Some(warning) = crate::library_lifecycle::fresh_warning(paths, name) {
            warnings.push(warning.clone());
            library_warning = Some(warning);
        }
        failpoint("install.config.after").map_err(|error| match &library_warning {
            Some(warning) => anyhow::anyhow!("{error:#}; {warning}"),
            None => error,
        })?;
    }
    // The state now covers the folder, so other installs and migrations see
    // it in their overlap checks.
    drop(adoption);

    let report = rebuild(
        paths,
        lock,
        state,
        Work {
            kind: JournalKind::Install,
            target: &target,
        },
        Packages {
            pins,
            fetched: kept,
            opened,
        },
        token,
        progress,
        journaled,
        &mut warnings,
    )
    .map_err(|error| match &library_warning {
        Some(warning) => anyhow::anyhow!("{error:#}; {warning}"),
        None => error,
    })?;
    drop(operation);
    Ok(Installed {
        environment: name.to_owned(),
        folder: folder.to_owned(),
        gog_id: plan.gog_id,
        title: plan.title.clone(),
        resumed: plan.resume,
        products: target.products,
        saved: report.saved,
        kept: report.kept,
        overwritten: report.overwritten,
        warnings,
        target: None,
        reported_language: None,
        needs_restart: Vec::new(),
    })
}

/// What a Windows install's journal says it may write: the whole folder,
/// since its installers don't say which files they write.
pub(super) fn windows_attempted() -> Vec<Attempted> {
    vec![Attempted {
        path: ".".to_owned(),
        kind: EntryType::Directory,
        md5: None,
        target: None,
    }]
}

/// Whether a DLC's download failing leaves the rest to install: anything
/// but a cancel or a login that stopped working, which would stop the rest
/// too.
pub(super) fn dlc_can_fail(error: &anyhow::Error) -> bool {
    !is_cancel(error) && error.downcast_ref::<AccountError>().is_none()
}

/// The new environment's config: `start.sh` in the folder as its default
/// target, and `gog_id`.
fn settings(plan: &Plan) -> ImportedSettings {
    let target = TargetConfig {
        exe: plan.folder.join("start.sh").to_string_lossy().into_owned(),
        args: Vec::new(),
        cwd: None,
        default: true,
        gamemode: None,
        wayland: None,
        mangohud: None,
        gamescope: None,
        hooks: None,
        desktop: plan.desktop,
        name: plan.desktop.then(|| plan.title.clone()),
    };
    ImportedSettings {
        targets: BTreeMap::from([(TARGET.to_owned(), target)]),
        gog_id: Some(plan.gog_id.to_string()),
        ..ImportedSettings::default()
    }
}

/// Refuses before downloading when the download cache shares the game
/// folder's filesystem and can't hold what is left to download and the
/// installed estimate together. On separate filesystems the folder is
/// checked with exact sizes once the packages are downloaded.
fn check_space_before(paths: &Paths, cache: &DownloadCache, plan: &Plan) -> Result<()> {
    let device = |path: &Path| std::fs::metadata(package::existing(path)).map(|meta| meta.dev());
    let (Ok(cache_device), Ok(folder_device)) = (device(cache.dir()), device(&plan.folder)) else {
        return Ok(());
    };
    if cache_device != folder_device {
        return Ok(());
    }
    let mut remaining = 0u64;
    for file in &plan.files {
        let cached = cache.files(file.product).is_ok_and(|files| {
            files
                .iter()
                .any(|cached| cached.file_id == file.file_id && cached.complete)
        });
        if !cached {
            remaining = remaining.saturating_add(file.size);
        }
    }
    let needed = remaining.saturating_add(plan.installed_estimate());
    if plan.platform == Platform::Windows {
        return windows::check_estimate(paths, &plan.folder, needed);
    }
    let needed = SpaceNeeded {
        unpacked: needed,
        assembled: 0,
    };
    package::check_space_with(paths, &plan.folder, needed, free_space(&plan.folder)?)?;
    Ok(())
}
