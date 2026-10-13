//! Installing a native GOG game that isn't installed, into a fresh folder,
//! as a new native environment with `gog_id`.
//!
//! A fresh install is a rebuild of every product into an empty folder, so it
//! shares an update's machinery in `apply`. `plan` resolves the game and its
//! owned DLC and reads only. `apply` downloads every package, opens them and
//! checks they fit, then under the locks writes the environment's config,
//! creates the folder, writes its state with an `install` journal and no
//! `[committed]`, and rebuilds. Anything that stops it after the journal is
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
    Packages, PlannedFile, Progress, Work, check_fit, download, free_space, is_cancel,
    lock_operation, open_package, operation_id, pin_packages, rebuild,
};
use super::{SavedAside, UpdateError};
use crate::cancel::Token;
use crate::config::{GlobalConfig, Target as TargetConfig};
use crate::environment;
use crate::gog::account::{AccountError, Session};
use crate::gog::folder::{
    self, AdoptionLock, FolderLock, FolderState, Journal, JournalKind, Package, Platform, Product,
    ProductStatus, Target, Track, failpoint,
};
use crate::gog::install_dir::install_folder;
use crate::gog::migrate::host_only;
use crate::gog::package::{self, SpaceNeeded};
use crate::gog::{language, names, owned, product};
use crate::import::{self, ImportedSettings};
use crate::paths::Paths;

/// The language every GOG product falls back to.
const FALLBACK: &str = "en";

/// The target a new game's environment gets, as a migrated one does.
const TARGET: &str = "game";

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The base game can't be installed natively.
    #[error("{title} can't be installed: {why}")]
    NoInstaller { title: String, why: String },
    /// The environment or folder isn't what the plan saw any more.
    #[error("{what} changed since the install was planned; plan it again.")]
    Changed { what: String },
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
    /// Fetch GOG's product details and owned list again instead of using
    /// copies under an hour old.
    pub refresh: bool,
    /// Write nothing while planning, for a dry run: neither GOG's details
    /// nor the owned list are cached.
    pub read_only: bool,
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
    products: Vec<Planned>,
    files: Vec<PlannedFile>,
    /// The folder's state when planned: none, or the resumed install's.
    seen: Option<FolderState>,
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

    /// GOG's code for the language the base game installs in, which the
    /// folder's state records.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Whether this carries on the environment's own interrupted install
    /// instead of creating the environment.
    pub fn resumes(&self) -> bool {
        self.resume
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
        self.files.iter().map(|file| file.size).sum()
    }

    /// An estimate of the space the installed game takes, which is only
    /// known once the packages are downloaded: their size, since GOG's Linux
    /// packages are barely compressed.
    pub fn installed_estimate(&self) -> u64 {
        self.download_size()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// What the install commits if every package installs.
    fn target(&self) -> Target {
        Target {
            gog_id: self.gog_id,
            platform: Platform::Linux,
            language: self.language.clone(),
            track: Track::Installer,
            products: self
                .products
                .iter()
                .map(|planned| Product {
                    id: planned.id,
                    version: planned.version.clone(),
                    build: None,
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
    pub overwritten: Vec<String>,
    /// Including why each failed DLC failed.
    pub warnings: Vec<String>,
}

/// Works out what installing `request.gog_id` natively does. Reads only: it
/// takes no locks and writes nothing but GOG's product details and owned
/// list caches, unless `read_only`, and a renewed login.
///
/// Refused in a portable root, for a name already taken (unless it is this
/// game's environment with its own interrupted install in the folder), for
/// a folder that overlaps another game's or isn't empty, and for a game
/// with no Linux installer in the language or English.
pub fn plan(paths: &Paths, session: &mut Session, request: &Request) -> Result<Plan> {
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
    let resume = match &seen {
        // Only this environment's own install gets here with a state.
        Some(_) if exists => super::recorded_gog_id(paths, name)?
            .is_some_and(|recorded| recorded == request.gog_id.to_string()),
        _ => false,
    };
    if exists && !resume {
        environment::check_new_name(paths, name)?;
    }
    if seen.is_some() && !resume {
        return Err(folder::FolderError::NotEmpty { folder }.into());
    }

    let mut products = Vec::new();
    let mut files = Vec::new();
    let base = match pick(&game, &title, &code) {
        Ok(picked) => picked,
        Err(why) => return Err(InstallError::NoInstaller { title, why }.into()),
    };
    if base.language != code {
        warnings.push(format!(
            "{title} has no Linux installer in {requested}, so it installs in English."
        ));
    }
    let language = base.language.clone();
    base.add(
        game.id,
        (Some(title.clone()), title.clone()),
        None,
        &mut products,
        &mut files,
    );

    let owned = owned::load_with(session, request.refresh, !request.read_only)?;
    warnings.extend(owned.warning);
    let owned: HashSet<u64> = owned.games.iter().map(|game| game.id).collect();
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
        let picked = match entry {
            Some(entry) => pick(entry, &label, &code),
            None => Err(format!("GOG lists no details for {label}.")),
        };
        match picked {
            Ok(picked) => {
                let note = (picked.language != code).then(|| {
                    format!(
                        "{label} has no Linux installer in {requested}, so it installs in English."
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
        products,
        files,
        seen,
        warnings,
    })
}

/// A product's Linux installer, picked for a language.
struct Picked<'a> {
    installer: &'a wineo_gog::Download,
    file: &'a wineo_gog::DownloadFile,
    version: String,
    /// GOG's code for the installer's language.
    language: String,
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
        files.push(PlannedFile {
            product: id,
            label,
            download_id: self.installer.id.clone(),
            file_id: self.file.id.clone(),
            size: self.file.size,
            md5: None,
            download: Some(self.installer.clone()),
        });
        products.push(Planned {
            id,
            title,
            version: Some(self.version),
            language: Some(self.language),
            status: ProductStatus::Installed,
            note,
            download_size: self.file.size,
        });
    }
}

/// A product's Linux installer in GOG's language `code`, else in English,
/// or why there is none it can use.
fn pick<'a>(entry: &'a wineo_gog::Product, label: &str, code: &str) -> Result<Picked<'a>, String> {
    let linux: Vec<&wineo_gog::Download> = entry
        .installers
        .iter()
        .filter(|installer| installer.os == "linux")
        .collect();
    if linux.is_empty() {
        return Err(format!("{label} has no Linux installer."));
    }
    let mut chosen = None;
    for language in [code, FALLBACK] {
        let matching: Vec<_> = linux
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
                    "{label} has more than one Linux installer in {language}."
                ));
            }
        }
    }
    let Some(installer) = chosen else {
        return Err(format!(
            "{label} has no Linux installer in {code} or English."
        ));
    };
    let [file] = installer.files.as_slice() else {
        return Err(format!(
            "{label}'s Linux installer comes in {} files, which wineo can't unpack.",
            installer.files.len()
        ));
    };
    let version = installer
        .version
        .clone()
        .ok_or_else(|| format!("GOG lists no version for {label}'s Linux installer."))?;
    Ok(Picked {
        installer,
        file,
        version,
        language: installer.language.clone(),
    })
}

/// Installs a planned game. Every package is downloaded first, holding no
/// lock and changing nothing. A DLC whose package can't be downloaded or
/// opened is recorded as failed and the game installs without it; the base
/// game's failing refuses the install. Then everything the plan rested on
/// is checked again under the locks, and the environment, the folder, and
/// its state with an `install` journal are written before any game file.
///
/// A failure or cancel after the journal is written leaves it, so the game
/// refuses to launch until `wineo update NAME` finishes the install.
/// `UpdateError::Canceled` says whether it did.
pub fn apply(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    token: &Token,
    progress: impl FnMut(Progress),
) -> Result<Installed> {
    host_only(paths)?;
    let mut journaled = false;
    let progress = RefCell::new(progress);
    match run(paths, session, plan, token, &progress, &mut journaled) {
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
    journaled: &mut bool,
) -> Result<Installed> {
    let name = plan.environment.as_str();
    let folder = plan.folder.as_path();
    let cancel = || token.is_canceled();
    let changed = |what: &str| InstallError::Changed {
        what: what.to_owned(),
    };
    let mut warnings = plan.warnings.clone();
    let mut target = plan.target();
    let cache = DownloadCache::new(paths.gog_download_cache());
    check_space_before(paths, &cache, plan)?;

    // Downloading and opening the packages changes nothing, so it holds no
    // lock. A DLC that fails here hasn't changed anything either, so it is
    // recorded as failed and the game installs without it.
    let mut fetched = Vec::new();
    let mut failed = BTreeMap::new();
    for file in &plan.files {
        match download(session, &cache, file, token, progress, &mut warnings) {
            Ok(file) => fetched.push(file),
            Err(error) if file.product == plan.gog_id || !dlc_can_fail(&error) => {
                return Err(error);
            }
            Err(error) => {
                failed.insert(file.product, format!("{error:#}"));
            }
        }
    }
    let (pins, located) = pin_packages(&cache, name, fetched, token, progress)?;
    let mut kept = Vec::new();
    let mut opened = Vec::new();
    for (file, path) in located {
        match open_package(&path) {
            Ok(package) => {
                opened.push((file.product, package));
                kept.push((file, path));
            }
            Err(error) if file.product == plan.gog_id || is_cancel(&error) => {
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
    check_fit(paths, folder, &opened)?;

    let _adoption = AdoptionLock::acquire(paths, &cancel)?;
    let _operation = lock_operation(paths, name)?;
    let _in_use = folder::lock_for_update(paths, folder, name)?;
    let lock = FolderLock::exclusive(paths, folder, &cancel)?;
    #[cfg(test)]
    super::apply::tests::install::before_create();
    let state = lock.read()?;
    if state != plan.seen {
        return Err(changed(&folder.display().to_string()).into());
    }
    if plan.resume {
        if super::recorded_gog_id(paths, name)? != Some(plan.gog_id.to_string()) {
            return Err(changed(name).into());
        }
    } else {
        environment::check_new_name(paths, name)?;
    }
    if folder::check_fresh_install(paths, folder, name)? != folder {
        return Err(changed(&folder.display().to_string()).into());
    }
    // The last check before the first write; from here the environment, the
    // folder, and its state are written together.
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: false }.into());
    }
    failpoint("install.create.before")?;

    let state = match state {
        Some(state) => state,
        None => {
            let config = import::native_config(paths, &settings(plan))?;
            import::write_native_config(paths, name, &config)?;
            failpoint("install.config.after")?;
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
                packages: kept
                    .iter()
                    .map(|(file, _)| Package {
                        product: file.product,
                        download_id: file.download_id.clone(),
                        file_id: file.file_id.clone(),
                        size: file.size,
                        md5: file.md5.clone(),
                    })
                    .collect(),
                attempted: Vec::new(),
            });
            lock.write(&state)?;
            *journaled = true;
            failpoint("install.state.after")?;
            state
        }
    };
    // The state now covers the folder, so other installs and migrations see
    // it in their overlap checks.
    drop(_adoption);

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
    )?;
    Ok(Installed {
        environment: name.to_owned(),
        folder: folder.to_owned(),
        gog_id: plan.gog_id,
        title: plan.title.clone(),
        resumed: plan.resume,
        products: target.products,
        saved: report.saved,
        overwritten: report.overwritten,
        warnings,
    })
}

/// Whether a DLC's download failing leaves the rest to install: anything
/// but a cancel or a login that stopped working, which would stop the rest
/// too.
fn dlc_can_fail(error: &anyhow::Error) -> bool {
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
    let needed = SpaceNeeded {
        unpacked: remaining.saturating_add(plan.installed_estimate()),
        assembled: 0,
    };
    package::check_space_with(paths, &plan.folder, needed, free_space(&plan.folder)?)?;
    Ok(())
}
