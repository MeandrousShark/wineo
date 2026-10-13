//! Applying a native GOG game's update, reinstalling it, or finishing an
//! interrupted one.
//!
//! `plan` resolves what the game is rebuilt from and reports sizes, reading
//! only. `apply` takes the plan, rechecks it under the locks, and does the
//! work: download every package, open them and check space, write the
//! journal, stage every product, layer them, write the journal again with
//! every entry it is about to write, publish, clean up, sync, and commit.
//!
//! Downloading and checking the packages holds no lock, so the game can
//! still be played meanwhile. Then, in lock order: the owner's operation
//! lock, the folder's in-use lock (refusing while anything runs from the
//! folder, with the packages left cached), and the state lock, held from the
//! recheck through the first journal write to the commit.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wineo_gog::{DownloadCache, Pin, Verification};

use super::publish::{self, PublishProgress, Publisher, Report, SavedAside};
use super::windows::{self, SetupError};
use super::{Check, UpdateError, UpdateState, installer_version, matching_installer, status_with};
use crate::cancel::Token;
use crate::environment::Environment;
use crate::gog::account::{AccountError, CallError, Session};
use crate::gog::folder::{
    self, Attempted, Committed, EntryType, FolderError, FolderLock, FolderState, Journal,
    JournalKind, Package, Platform, Product, ProductStatus, Target, Track, WinningEntry, failpoint,
};
use crate::gog::migrate::host_only;
use crate::gog::names;
use crate::gog::package::{self, PackageError};
use crate::gog::product;
use crate::operation::{Guard, Locks, Operation};
use crate::paths::Paths;

/// What `plan` is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// Rebuild from current packages even when nothing changed.
    pub reinstall: bool,
    /// Fetch GOG's product details again instead of using an hour-old copy.
    pub refresh: bool,
    /// Write nothing while planning, for a dry run: product details aren't
    /// cached and another tool's build isn't recorded.
    pub read_only: bool,
}

/// What updating one game would do.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Plan {
    /// Nothing to do: the game matches GOG and has no journal. JSON `none`.
    UpToDate {
        environment: String,
        version: Option<String>,
    },
    Update(Pending),
}

/// An update, reinstall, or recovery, resolved and ready to apply.
#[derive(Debug, Clone)]
pub struct Pending {
    environment: String,
    folder: PathBuf,
    kind: JournalKind,
    recovery: bool,
    offline: bool,
    target: Target,
    products: Vec<PlannedProduct>,
    files: Vec<PlannedFile>,
    seen: Seen,
    warnings: Vec<String>,
}

/// One product of the rebuild, for a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProduct {
    pub id: u64,
    /// GOG's title, when its details list one.
    pub title: Option<String>,
    /// The name the folder's `goggame-*.info` gives it, for when GOG's
    /// details have no title, as in a recovery from saved packages.
    pub name: Option<String>,
    /// What the folder's state records now.
    pub installed: Option<String>,
    /// The version the rebuild installs. None for a product carried over
    /// unchanged because it failed or wasn't available.
    pub version: Option<String>,
    pub status: ProductStatus,
    /// Its package's size, zero when carried over.
    pub download_size: u64,
}

/// One GOG file the rebuild uses.
#[derive(Debug, Clone)]
pub(super) struct PlannedFile {
    pub(super) product: u64,
    pub(super) label: String,
    pub(super) download_id: String,
    pub(super) file_id: String,
    /// GOG's listed size, rounded to whole MiB, until it's downloaded or
    /// read from a journal, then the exact size.
    pub(super) size: u64,
    /// The checksum a journal recorded, for a recovery from the cache.
    pub(super) md5: Option<String>,
    /// GOG's listing, to download from. None when finishing from the cache.
    pub(super) download: Option<wineo_gog::Download>,
}

/// What the folder's state was when planned, compared again under the
/// locks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    owner: Option<String>,
    gog_id: Option<u64>,
    committed: Option<Committed>,
    journal: Option<Journal>,
}

impl Seen {
    fn of(state: &FolderState) -> Self {
        Self {
            owner: state.owner.clone(),
            gog_id: state.gog_id(),
            committed: state.committed.clone(),
            journal: state.journal.clone(),
        }
    }
}

impl Pending {
    pub fn environment(&self) -> &str {
        &self.environment
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// `Update` or `Reinstall`, or a recovered journal's own kind.
    pub fn kind(&self) -> JournalKind {
        self.kind
    }

    /// Whether this finishes an interrupted update.
    pub fn is_recovery(&self) -> bool {
        self.recovery
    }

    /// Whether it finishes from packages already in the download cache,
    /// without GOG.
    pub fn is_offline(&self) -> bool {
        self.offline
    }

    /// Whether the folder has no owner, so applying claims it.
    pub fn claims(&self) -> bool {
        self.seen.owner.is_none()
    }

    /// The state the rebuild commits.
    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn products(&self) -> &[PlannedProduct] {
        &self.products
    }

    /// A DLC as `NAME (ID)`: GOG's title if its details gave one, else the
    /// name in the folder. The base game's own title or name is what a DLC's
    /// leading `Game - ` is dropped against.
    pub fn dlc_label(&self, id: u64) -> String {
        let name = |product: &PlannedProduct| {
            product
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .or_else(|| product.name.clone())
        };
        let base = self
            .products
            .iter()
            .find(|product| product.id == self.target.gog_id)
            .and_then(name)
            .unwrap_or_default();
        let dlc = self.products.iter().find(|product| product.id == id);
        names::dlc_label(&base, id, dlc.and_then(name).as_deref())
    }

    /// What has to be downloaded at most: every package's size, or zero when
    /// finishing from the cache. Files already cached aren't fetched again.
    pub fn download_size(&self) -> u64 {
        if self.offline {
            return 0;
        }
        self.files.iter().map(|file| file.size).sum()
    }

    /// An estimate of the space unpacking takes, which is only known once
    /// the packages are downloaded: their size, since GOG's Linux packages
    /// are barely compressed.
    pub fn unpacked_estimate(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// How far `apply` has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Bytes of one GOG file there so far.
    Download {
        product: u64,
        file_id: String,
        done: u64,
        total: u64,
    },
    /// Checking a cached package against its recorded checksum.
    Verify { product: u64 },
    /// Bytes unpacked into one product's staging folder.
    Stage { product: u64, done: u64, total: u64 },
    /// Entries published into the game folder.
    Publish { done: usize, total: usize },
}

/// What `apply` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    pub environment: String,
    pub folder: PathBuf,
    pub kind: JournalKind,
    /// Whether it finished an interrupted update.
    pub recovered: bool,
    /// Whether it claimed an ownerless folder for the environment.
    pub claimed: bool,
    /// The products as committed.
    pub products: Vec<Product>,
    /// Your entries that stood where the new version puts something,
    /// moved aside.
    pub saved: Vec<SavedAside>,
    /// Old shipped files you changed, which the new version doesn't ship,
    /// kept where they are.
    pub kept: Vec<String>,
    /// Shipped files you changed, overwritten by the new version.
    pub overwritten: Vec<String>,
    pub warnings: Vec<String>,
}

/// Works out what updating `name`'s native GOG game does. Reads only: it
/// takes no locks and writes nothing but GOG's product details cache and,
/// when the login is renewed, the login.
///
/// A folder with a journal is finished: from the cached packages its
/// journal names when they all verify, which needs no session, else from
/// GOG's current packages. Without a journal, `Plan::UpToDate` when nothing
/// changed and `options.reinstall` is off.
pub fn plan(
    paths: &Paths,
    session: Option<&mut Session>,
    name: &str,
    options: Options,
    token: &Token,
) -> Result<Plan> {
    host_only(paths)?;
    Environment::ensure_exists(paths, name)?;
    let state = super::game_of(paths, name)?;
    let seen = Seen::of(&state);
    let folder = state.folder.clone();
    // Read once for the review and for every message that names a DLC.
    let names = names::product_names(&folder);

    if let Some(journal) = &state.journal {
        // An interrupted native install is a rebuild like an update, so it
        // is finished the same way. An interrupted Windows install runs its
        // installers again.
        match journal.kind {
            JournalKind::Repair => {
                return Err(UpdateError::LaterRelease {
                    name: name.to_owned(),
                    kind: kind_word(journal.kind),
                }
                .into());
            }
            JournalKind::Install | JournalKind::Update | JournalKind::Reinstall => {}
        }
        let target = &journal.target;
        if target.platform == Platform::Windows
            && target.track == Track::Installer
            && journal.kind == JournalKind::Install
        {
            windows::check_graphical()?;
        } else {
            native_only(name, target.platform, target.track)?;
        }
        if let Some(files) = cached(paths, journal, token)? {
            let products = journal
                .target
                .products
                .iter()
                .map(|product| PlannedProduct {
                    id: product.id,
                    title: None,
                    name: names.get(&product.id).cloned(),
                    installed: installed_version(&state, product.id),
                    version: product.version.clone(),
                    status: product.status,
                    download_size: files
                        .iter()
                        .filter(|file| file.product == product.id)
                        .map(|file| file.size)
                        .sum(),
                })
                .collect();
            return Ok(Plan::Update(Pending {
                environment: name.to_owned(),
                folder,
                kind: journal.kind,
                recovery: true,
                offline: true,
                target: journal.target.clone(),
                products,
                files,
                seen,
                warnings: Vec::new(),
            }));
        }
        let session = session.ok_or(AccountError::LoggedOut)?;
        let resolved = resolve(
            paths,
            session,
            name,
            &state,
            Wanted {
                platform: journal.target.platform,
                language: &journal.target.language,
                products: &journal.target.products,
                names: &names,
            },
            Check {
                refresh: options.refresh,
                read_only: options.read_only,
            },
        )?;
        return Ok(Plan::Update(resolved.pending(
            name,
            folder,
            journal.kind,
            true,
            seen,
        )));
    }

    let committed = state
        .committed
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{name}'s GOG install hasn't started."))?;
    native_only(name, committed.platform, committed.track)?;
    let session = session.ok_or(AccountError::LoggedOut)?;
    let mut refresh = options.refresh;
    if !options.reinstall {
        let status = status_with(
            session,
            paths,
            name,
            state.clone(),
            Check {
                refresh: options.refresh,
                read_only: options.read_only,
            },
            &names,
        )?;
        match status.state {
            UpdateState::UpToDate => {
                return Ok(Plan::UpToDate {
                    environment: name.to_owned(),
                    version: status.installed,
                });
            }
            UpdateState::Available => {}
            UpdateState::CantCompare | UpdateState::OnGog => {
                return Err(UpdateError::CantCompare {
                    name: name.to_owned(),
                    why: status
                        .reason
                        .map(|reason| reason.as_str().to_owned())
                        .unwrap_or_default(),
                }
                .into());
            }
        }
        // Just fetched and cached, so the cached details are current. A
        // read-only plan cached nothing, so it fetches again rather than
        // reuse an older copy that is still fresh.
        refresh = refresh && options.read_only;
    }
    let resolved = resolve(
        paths,
        session,
        name,
        &state,
        Wanted {
            platform: Platform::Linux,
            language: &committed.language,
            products: &committed.products,
            names: &names,
        },
        Check {
            refresh,
            read_only: options.read_only,
        },
    )?;
    let kind = if options.reinstall {
        JournalKind::Reinstall
    } else {
        JournalKind::Update
    };
    Ok(Plan::Update(
        resolved.pending(name, folder, kind, false, seen),
    ))
}

fn kind_word(kind: JournalKind) -> &'static str {
    match kind {
        JournalKind::Install => "install",
        JournalKind::Update => "update",
        JournalKind::Reinstall => "reinstall",
        JournalKind::Repair => "repair",
    }
}

/// Refuses a Windows game, naming the plan whose release updates it.
fn native_only(name: &str, platform: Platform, track: Track) -> Result<()> {
    let name = name.to_owned();
    match (platform, track) {
        (Platform::Linux, Track::Installer) => Ok(()),
        (Platform::Windows, Track::Installer) => Err(UpdateError::WindowsInstaller { name }.into()),
        (Platform::Windows, Track::Galaxy) => Err(UpdateError::Galaxy { name }.into()),
        (Platform::Linux, Track::Galaxy) => Err(UpdateError::CantCompare {
            name,
            why: "GOG's Galaxy builds are only for Windows games.".to_owned(),
        }
        .into()),
    }
}

fn installed_version(state: &FolderState, id: u64) -> Option<String> {
    state
        .committed
        .as_ref()?
        .products
        .iter()
        .find(|product| product.id == id)?
        .version
        .clone()
}

/// The packages a journal names, when every one is in the download cache
/// and matches its recorded size and checksum.
pub(super) fn cached(
    paths: &Paths,
    journal: &Journal,
    token: &Token,
) -> Result<Option<Vec<PlannedFile>>> {
    let installed: Vec<u64> = journal
        .target
        .products
        .iter()
        .filter(|product| product.status == ProductStatus::Installed)
        .map(|product| product.id)
        .collect();
    let mut covered: Vec<u64> = journal
        .packages
        .iter()
        .map(|package| package.product)
        .collect();
    covered.sort_unstable();
    covered.dedup();
    let mut wanted = installed;
    wanted.sort_unstable();
    if journal.packages.is_empty() || covered != wanted {
        return Ok(None);
    }
    let cache = DownloadCache::new(paths.gog_download_cache());
    let mut files = Vec::new();
    for package in &journal.packages {
        if verify_cached(&cache, package, token)?.is_none() {
            return Ok(None);
        }
        files.push(PlannedFile {
            product: package.product,
            label: format!("GOG product {}", package.product),
            download_id: package.download_id.clone(),
            file_id: package.file_id.clone(),
            size: package.size,
            md5: package.md5.clone(),
            download: None,
        });
    }
    Ok(Some(files))
}

/// Checks one of a journal's packages in the cache. Only a journal's packages
/// are checked this way, so a cancel here leaves that journal.
fn verify_cached(
    cache: &DownloadCache,
    package: &Package,
    token: &Token,
) -> Result<Option<PathBuf>> {
    let lookup = wineo_gog::Package {
        product_id: package.product,
        download_id: &package.download_id,
        file_id: &package.file_id,
        size: package.size,
        siblings: &[],
    };
    match cache.complete(&lookup, package.md5.as_deref(), &|| token.is_canceled()) {
        Ok(found) => Ok(found),
        Err(error) if error.kind() == wineo_gog::ErrorKind::Canceled => {
            Err(UpdateError::Canceled { journal: true }.into())
        }
        Err(error) => Err(error.into()),
    }
}

/// A target and its packages, resolved from GOG's current details.
pub(super) struct Resolved {
    pub(super) target: Target,
    pub(super) products: Vec<PlannedProduct>,
    pub(super) files: Vec<PlannedFile>,
    pub(super) warnings: Vec<String>,
}

impl Resolved {
    fn pending(
        self,
        name: &str,
        folder: PathBuf,
        kind: JournalKind,
        recovery: bool,
        seen: Seen,
    ) -> Pending {
        Pending {
            environment: name.to_owned(),
            folder,
            kind,
            recovery,
            offline: false,
            target: self.target,
            products: self.products,
            files: self.files,
            seen,
            warnings: self.warnings,
        }
    }
}

/// What a rebuild is planned for: the platform and language, the products
/// to carry, and the folder's names for labeling them.
pub(super) struct Wanted<'a> {
    pub(super) platform: Platform,
    pub(super) language: &'a str,
    pub(super) products: &'a [Product],
    pub(super) names: &'a HashMap<u64, String>,
}

/// Every installed product's current installer for the platform and
/// `language`, in order: a Linux package, or a Windows setup program and its
/// parts. Failed and unavailable products are carried over unchanged. Any
/// installed product GOG no longer offers refuses the whole rebuild.
pub(super) fn resolve(
    paths: &Paths,
    session: &mut Session,
    name: &str,
    state: &FolderState,
    want: Wanted,
    how: Check,
) -> Result<Resolved> {
    let Wanted {
        platform,
        language,
        products,
        names,
    } = want;
    let (os, shown) = match platform {
        Platform::Linux => ("linux", "Linux"),
        Platform::Windows => ("windows", "Windows"),
    };
    let gog_id = state
        .gog_id()
        .ok_or_else(|| anyhow::anyhow!("{name}'s GOG state names no game"))?;
    let details = product::load_with(paths, session.client(), gog_id, how.refresh, !how.read_only)?;
    let warnings: Vec<String> = details.warning.into_iter().collect();
    let refuse = |why: String| UpdateError::NoInstaller {
        name: name.to_owned(),
        why,
    };
    if !products
        .iter()
        .any(|product| product.id == gog_id && product.status == ProductStatus::Installed)
    {
        return Err(refuse("The base game isn't installed.".to_owned()).into());
    }
    let mut target = Vec::new();
    let mut planned = Vec::new();
    let mut files = Vec::new();
    for product in products {
        let entry = if product.id == gog_id {
            Some(&details.product)
        } else {
            details.product.dlcs.iter().find(|dlc| dlc.id == product.id)
        };
        let title = entry
            .map(|entry| entry.title.clone())
            .filter(|title| !title.is_empty());
        let installed = installed_version(state, product.id);
        if product.status != ProductStatus::Installed {
            target.push(product.clone());
            planned.push(PlannedProduct {
                id: product.id,
                title,
                name: names.get(&product.id).cloned(),
                installed,
                version: None,
                status: product.status,
                download_size: 0,
            });
            continue;
        }
        let label = if product.id == gog_id {
            title
                .clone()
                .unwrap_or_else(|| format!("GOG game {}", product.id))
        } else {
            let name = names.get(&product.id).map(String::as_str);
            names::dlc_label(
                &details.product.title,
                product.id,
                title.as_deref().or(name),
            )
        };
        let product_language = product.language.as_deref().unwrap_or(language);
        let installer = matching_installer(os, &label, product_language, entry).map_err(refuse)?;
        let version = installer_version(os, &label, installer).map_err(refuse)?;
        let parts = match (platform, installer.files.as_slice()) {
            (_, []) => {
                return Err(refuse(format!(
                    "GOG lists no files for {label}'s {shown} installer."
                ))
                .into());
            }
            (Platform::Linux, [_]) | (Platform::Windows, _) => installer.files.as_slice(),
            (Platform::Linux, more) => {
                return Err(refuse(format!(
                    "{label}'s Linux installer comes in {} files, which wineo can't unpack.",
                    more.len()
                ))
                .into());
            }
        };
        for file in parts {
            files.push(PlannedFile {
                product: product.id,
                label: label.clone(),
                download_id: installer.id.clone(),
                file_id: file.id.clone(),
                size: file.size,
                md5: None,
                download: Some(installer.clone()),
            });
        }
        planned.push(PlannedProduct {
            id: product.id,
            title,
            name: names.get(&product.id).cloned(),
            installed,
            version: Some(version.clone()),
            status: ProductStatus::Installed,
            download_size: parts.iter().map(|file| file.size).sum(),
        });
        target.push(Product {
            id: product.id,
            version: Some(version),
            build: None,
            language: product.language.clone(),
            status: ProductStatus::Installed,
        });
    }
    Ok(Resolved {
        target: Target {
            gog_id,
            platform,
            language: language.to_owned(),
            track: Track::Installer,
            products: target,
        },
        products: planned,
        files,
        warnings,
    })
}

/// Applies a planned update. Everything the plan rested on is checked again
/// under the locks first, and nothing changes in the game folder until its
/// packages are downloaded, opened, and known to fit.
///
/// A failure or cancel after the journal is written leaves it, and the
/// game's launches blocked, until `plan` and `apply` run again and finish
/// it. `UpdateError::Canceled` says whether it did.
pub fn apply(
    paths: &Paths,
    session: Option<&mut Session>,
    pending: &Pending,
    token: &Token,
    progress: impl FnMut(Progress),
) -> Result<Updated> {
    host_only(paths)?;
    // A recovery's journal is there from the start.
    let mut journaled = pending.recovery;
    let progress = RefCell::new(progress);
    let result = run(paths, session, pending, token, &progress, &mut journaled);
    match result {
        Err(error) if token.is_canceled() && is_cancel(&error) => {
            Err(UpdateError::Canceled { journal: journaled }.into())
        }
        result => result,
    }
}

pub(super) fn is_cancel(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref(), Some(UpdateError::Canceled { .. }))
        || matches!(error.downcast_ref(), Some(FolderError::Canceled))
        || matches!(error.downcast_ref(), Some(PackageError::Canceled))
        || error
            .downcast_ref::<wineo_gog::Error>()
            .is_some_and(|error| error.kind() == wineo_gog::ErrorKind::Canceled)
}

fn run(
    paths: &Paths,
    session: Option<&mut Session>,
    pending: &Pending,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    journaled: &mut bool,
) -> Result<Updated> {
    if pending.target.platform == Platform::Windows {
        return run_windows(paths, session, pending, token, progress, journaled);
    }
    let name = pending.environment.as_str();
    let folder = pending.folder.as_path();
    let cancel = || token.is_canceled();
    let changed = || UpdateError::Changed {
        name: name.to_owned(),
    };

    // Downloading and checking the packages changes nothing, so it holds no
    // lock and the game can still be played meanwhile.
    let mut warnings = pending.warnings.clone();
    let fetched = fetch(paths, session, pending, token, progress, &mut warnings)?;
    let cache = DownloadCache::new(paths.gog_download_cache());
    let (pins, fetched) = pin_packages(&cache, name, fetched, token, progress)?;
    let mut opened = Vec::new();
    for (file, path) in &fetched {
        let package = open_package(path)
            .with_context(|| format!("failed to open {}'s package", file.label))?;
        opened.push((file.product, package));
    }
    check_fit(paths, folder, &opened)?;

    let _operation = lock_operation(paths, name)?;
    // The config can change without the operation lock.
    let gog_id = super::recorded_gog_id(paths, name)?;
    if gog_id.as_deref().and_then(|id| id.parse().ok()) != pending.seen.gog_id {
        return Err(changed().into());
    }
    // Refused like before downloading would have been; the packages stay
    // cached for the next run.
    let _in_use = folder::lock_for_update(paths, folder, name)?;
    let lock = FolderLock::exclusive(paths, folder, &cancel)?;
    let mut state = lock.read()?.ok_or_else(changed)?;
    let unowned = |seen: &Seen| Seen {
        owner: None,
        ..seen.clone()
    };
    if unowned(&Seen::of(&state)) != unowned(&pending.seen) {
        return Err(changed().into());
    }
    let claimed = pending.seen.owner.is_none();
    if claimed {
        // Claimed like a launch does: written only while still unowned, and
        // refused when another environment got there first.
        state = lock.reclaim(name)?;
    } else if state.owner != pending.seen.owner {
        return Err(changed().into());
    }

    let report = rebuild(
        paths,
        lock,
        state,
        Work {
            kind: pending.kind,
            target: &pending.target,
        },
        Packages {
            pins,
            fetched,
            opened,
        },
        token,
        progress,
        journaled,
        &mut warnings,
    )?;
    Ok(Updated {
        environment: name.to_owned(),
        folder: folder.to_owned(),
        kind: pending.kind,
        recovered: pending.recovery,
        claimed,
        products: pending.target.products.clone(),
        saved: report.saved,
        kept: report.kept,
        overwritten: report.overwritten,
        warnings,
    })
}

/// Finishes an interrupted Windows install: downloads what isn't cached,
/// checks everything again under the locks, names the packages in the
/// journal, and runs every installer again over the folder, which is how an
/// Inno installer is repaired. Nothing in the folder is removed.
fn run_windows(
    paths: &Paths,
    session: Option<&mut Session>,
    pending: &Pending,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    journaled: &mut bool,
) -> Result<Updated> {
    let name = pending.environment.as_str();
    let folder = pending.folder.as_path();
    let cancel = || token.is_canceled();
    let changed = || UpdateError::Changed {
        name: name.to_owned(),
    };
    windows::check_graphical()?;
    let mut warnings = pending.warnings.clone();
    let fetched = fetch(paths, session, pending, token, progress, &mut warnings)?;
    let cache = DownloadCache::new(paths.gog_download_cache());
    let (pins, located) = pin_packages(&cache, name, fetched, token, progress)?;
    let mut setups = BTreeMap::new();
    for (product, found) in windows::setup_programs(&located) {
        let exe = found.map_err(|(label, count)| SetupError::NoSetupProgram { label, count })?;
        setups.insert(product, exe);
    }
    windows::check_space(paths, folder, &located)?;

    let operation = lock_operation(paths, name)?;
    let gog_id = super::recorded_gog_id(paths, name)?;
    if gog_id.as_deref().and_then(|id| id.parse().ok()) != pending.seen.gog_id {
        return Err(changed().into());
    }
    let _in_use = folder::lock_for_update(paths, folder, name)?;
    let lock = FolderLock::exclusive(paths, folder, &cancel)?;
    let mut state = lock.read()?.ok_or_else(changed)?;
    let unowned = |seen: &Seen| Seen {
        owner: None,
        ..seen.clone()
    };
    if unowned(&Seen::of(&state)) != unowned(&pending.seen) {
        return Err(changed().into());
    }
    let claimed = pending.seen.owner.is_none();
    if claimed {
        state = lock.reclaim(name)?;
    } else if state.owner != pending.seen.owner {
        return Err(changed().into());
    }
    let mut target = pending.target.clone();
    let packages: Vec<Package> = located
        .iter()
        .map(|(file, _)| Package {
            product: file.product,
            download_id: file.download_id.clone(),
            file_id: file.file_id.clone(),
            size: file.size,
            md5: file.md5.clone(),
        })
        .collect();
    let journal = state.journal.as_mut().ok_or_else(changed)?;
    if journal.target != target || journal.packages != packages {
        journal.target = target.clone();
        journal.packages = packages;
        lock.write(&state)?;
    }
    *journaled = true;
    // The journal names the packages now, which keeps them cached, and
    // running the installers takes the environment's own locks.
    drop(pins);
    drop(operation);

    let title = pending
        .products
        .iter()
        .find(|product| product.id == target.gog_id)
        .and_then(|product| {
            product
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .or_else(|| product.name.clone())
        })
        .unwrap_or_else(|| format!("GOG game {}", target.gog_id));
    let base = target.gog_id;
    let label = |id: u64| {
        if id == base {
            title.clone()
        } else {
            pending.dlc_label(id)
        }
    };
    let language = target.language.clone();
    windows::install(
        paths,
        lock,
        windows::Job {
            name,
            title: &title,
            target: &mut target,
            setups: &setups,
            label: &label,
            create: None,
            tag: &language,
            requested: &language,
            wizard: false,
            desktop: false,
        },
        token,
        &mut |_| {},
        &mut warnings,
    )?;
    Ok(Updated {
        environment: name.to_owned(),
        folder: folder.to_owned(),
        kind: pending.kind,
        recovered: pending.recovery,
        claimed,
        products: target.products,
        saved: Vec::new(),
        kept: Vec::new(),
        overwritten: Vec::new(),
        warnings,
    })
}

/// What a rebuild commits: its journal's kind and target.
pub(super) struct Work<'a> {
    pub(super) kind: JournalKind,
    pub(super) target: &'a Target,
}

/// A rebuild's packages: pinned until its journal names them, where each is
/// in the cache, and each one opened, in product order.
pub(super) struct Packages<'a> {
    pub(super) pins: Vec<Pin<'a>>,
    pub(super) fetched: Vec<Located>,
    pub(super) opened: Vec<(u64, package::Package)>,
}

/// Refuses packages that can't be layered into one tree or don't fit on the
/// game folder's filesystem, before anything changes.
pub(super) fn check_fit(
    paths: &Paths,
    folder: &Path,
    opened: &[(u64, package::Package)],
) -> Result<()> {
    let layers: Vec<(u64, Vec<(String, EntryType)>)> = opened
        .iter()
        .map(|(product, package)| (*product, package_entries(package)))
        .collect();
    publish::check_layers(&layers)?;
    check_space(
        paths,
        folder,
        &opened
            .iter()
            .map(|(_, package)| package)
            .collect::<Vec<_>>(),
    )
}

/// The part of every native rebuild done under the folder's state lock, which
/// `lock` is: writes the journal, stages and layers every product, writes the
/// journal again with every entry it is about to write, publishes, cleans up,
/// syncs, and commits. `state` is the folder's state as it stands under the
/// lock, or as a fresh install writes it first. A journal already in it is
/// finished: its temporaries go first, and its operation and `attempted`
/// carry on. The packages are removed from the cache afterwards.
#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild(
    paths: &Paths,
    lock: FolderLock,
    mut state: FolderState,
    work: Work,
    packages: Packages,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    journaled: &mut bool,
    warnings: &mut Vec<String>,
) -> Result<Report> {
    let folder = lock.folder().to_owned();
    let folder = folder.as_path();
    let Packages {
        pins,
        fetched,
        mut opened,
    } = packages;
    let root = package::open_folder(folder)?;
    let (operation, attempted) = match &state.journal {
        Some(journal) => {
            // Whatever the interrupted attempt left half done goes first.
            let dirs = publish::remove_temporaries(&root, &journal.operation)?;
            publish::sync_folders(&root, dirs, token)?;
            (journal.operation.clone(), journal.attempted.clone())
        }
        None => (operation_id(), Vec::new()),
    };
    let mut journal = Journal {
        operation: operation.clone(),
        kind: work.kind,
        target: work.target.clone(),
        packages: fetched
            .iter()
            .map(|(file, _)| Package {
                product: file.product,
                download_id: file.download_id.clone(),
                file_id: file.file_id.clone(),
                size: file.size,
                md5: file.md5.clone(),
            })
            .collect(),
        attempted,
    };
    state.journal = Some(journal.clone());
    failpoint("update.journal.before")?;
    #[cfg(test)]
    tests::before_journal();
    lock.write(&state)?;
    *journaled = true;
    // The journal names the packages now, which keeps them cached.
    drop(pins);
    failpoint("update.journal.after")?;

    let mut staged = Vec::new();
    for (product, package) in &mut opened {
        let product = *product;
        let entries = package::stage(package, folder, &operation, product, token, |step| {
            (progress.borrow_mut())(Progress::Stage {
                product,
                done: step.done,
                total: step.total,
            })
        })?;
        staged.push(entries);
    }
    let winning = publish::layer(staged);

    let earlier = journal.attempted.clone();
    for entry in &winning {
        let attempt = Attempted {
            path: entry.path.clone(),
            kind: entry.kind,
            md5: entry.md5.clone(),
            target: entry.target.clone(),
        };
        if !journal.attempted.contains(&attempt) {
            journal.attempted.push(attempt);
        }
    }
    state.journal = Some(journal.clone());
    failpoint("update.attempted.before")?;
    lock.write(&state)?;
    failpoint("update.attempted.after")?;

    let committed_files: Vec<WinningEntry> = state
        .committed
        .as_ref()
        .map(|committed| committed.files.clone())
        .unwrap_or_default();
    let mut publisher = Publisher::new(root, &operation, token);
    publisher.publish(
        &winning,
        &committed_files,
        &journal.attempted,
        &earlier,
        |step: PublishProgress| {
            (progress.borrow_mut())(Progress::Publish {
                done: step.done,
                total: step.total,
            })
        },
    )?;
    let products: Vec<u64> = opened.iter().map(|(product, _)| *product).collect();
    publisher.finish(&products)?;
    let report: Report = publisher.into_report();

    let target = work.target;
    let committed = Committed {
        gog_id: target.gog_id,
        platform: target.platform,
        language: target.language.clone(),
        track: target.track,
        products: target.products.clone(),
        files: winning,
    };
    failpoint("update.commit.before")?;
    publish::record_sync("commit".to_owned());
    lock.commit(committed)?;
    failpoint("update.commit.after")?;
    drop(lock);

    remove_packages(paths, folder, &products, warnings);
    Ok(report)
}

/// Takes `name`'s operation lock. A run holds it shared for as long as the
/// game runs, so that refusal is the running one.
pub(super) fn lock_operation(paths: &Paths, name: &str) -> Result<Guard> {
    match Locks::new(paths).acquire(Operation::Update, name, &[]) {
        Err(crate::Error::Busy {
            holder: Some(Operation::Launch),
            ..
        }) => Err(UpdateError::Running {
            name: name.to_owned(),
        }
        .into()),
        result => Ok(result?),
    }
}

/// A new operation id: letters, digits, and dashes, unique enough that two
/// operations on one folder never share one.
pub(super) fn operation_id() -> String {
    let now = jiff::Timestamp::now();
    format!(
        "{}-{:x}",
        now.strftime("%Y%m%dT%H%M%S"),
        now.subsec_nanosecond() as u64 ^ (u64::from(std::process::id()) << 32)
    )
}

/// Every package downloaded, with the checksum it was verified against, or
/// the journal's packages when finishing offline.
fn fetch(
    paths: &Paths,
    session: Option<&mut Session>,
    pending: &Pending,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    warnings: &mut Vec<String>,
) -> Result<Vec<PlannedFile>> {
    // Finishing offline uses what the journal names, checked once pinned.
    if pending.offline {
        return Ok(pending.files.clone());
    }
    let session = session.ok_or(AccountError::LoggedOut)?;
    let cache = DownloadCache::new(paths.gog_download_cache());
    let mut fetched = Vec::new();
    for file in &pending.files {
        fetched.push(download(session, &cache, file, token, progress, warnings)?);
    }
    Ok(fetched)
}

/// Downloads one planned file, or finds it already in the cache, and returns
/// it with its exact size and the checksum it was verified against.
pub(super) fn download(
    session: &mut Session,
    cache: &DownloadCache,
    file: &PlannedFile,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
    warnings: &mut Vec<String>,
) -> Result<PlannedFile> {
    let download = file
        .download
        .as_ref()
        .expect("an online plan lists GOG's downloads");
    let gog_file = download
        .files
        .iter()
        .find(|gog_file| gog_file.id == file.file_id)
        .expect("planned from this listing");
    let package = wineo_gog::Package::of(file.product, download, gog_file);
    let report = |done: u64, total: u64| {
        (progress.borrow_mut())(Progress::Download {
            product: file.product,
            file_id: file.file_id.clone(),
            done,
            total,
        })
    };
    let downloaded = session
        .call(|client, access| {
            client.download(
                access,
                cache,
                &package,
                &gog_file.downlink,
                &|| token.is_canceled(),
                &report,
            )
        })
        .map_err(|error| match error {
            CallError::Account(error) => anyhow::Error::from(error),
            CallError::Request(error) if error.kind() == wineo_gog::ErrorKind::Canceled => {
                UpdateError::Canceled { journal: false }.into()
            }
            CallError::Request(error) => {
                anyhow::Error::from(error).context(format!("Couldn't download {}", file.label))
            }
        })?;
    let md5 = match downloaded.verification {
        Verification::Md5(md5) => Some(md5),
        Verification::SizeOnly => {
            warnings.push(format!(
                "GOG publishes no checksum for {}'s package, so only its size was checked.",
                file.label
            ));
            None
        }
    };
    // The journal records the exact size, which finding the package in the
    // cache again checks.
    Ok(PlannedFile {
        size: downloaded.size,
        md5,
        ..file.clone()
    })
}

pub(super) fn check_space(
    paths: &Paths,
    folder: &Path,
    packages: &[&package::Package],
) -> Result<()> {
    package::check_space_with(
        paths,
        folder,
        package::space_needed(packages)?,
        free_space(folder)?,
    )?;
    Ok(())
}

/// The space free on `folder`'s filesystem, or what a test says it is.
pub(super) fn free_space(folder: &Path) -> Result<u64> {
    #[cfg(test)]
    if let Some(free) = tests::free_space() {
        return Ok(free);
    }
    package::free_space(folder)
}

/// A package and where it is in the cache.
pub(super) type Located = (PlannedFile, PathBuf);

/// Pins every product's cached files, in id order, and checks each package
/// is still there and whole, returning where it is. The pins keep another
/// update's cleanup from removing them until the journal naming them is
/// written.
pub(super) fn pin_packages<'a>(
    cache: &'a DownloadCache,
    name: &str,
    files: Vec<PlannedFile>,
    token: &Token,
    progress: &RefCell<impl FnMut(Progress)>,
) -> Result<(Vec<Pin<'a>>, Vec<Located>)> {
    let cancel = || token.is_canceled();
    let canceled = |error: wineo_gog::Error| -> anyhow::Error {
        if error.kind() == wineo_gog::ErrorKind::Canceled {
            UpdateError::Canceled { journal: false }.into()
        } else {
            error.into()
        }
    };
    let mut products: Vec<u64> = files.iter().map(|file| file.product).collect();
    products.sort_unstable();
    products.dedup();
    let mut pins = Vec::new();
    for product in products {
        pins.push(cache.pin(product, &cancel).map_err(canceled)?);
    }
    let mut found = Vec::new();
    for file in files {
        (progress.borrow_mut())(Progress::Verify {
            product: file.product,
        });
        let pin = pins
            .iter()
            .find(|pin| pin.product_id() == file.product)
            .expect("every product is pinned");
        let lookup = wineo_gog::Package {
            product_id: file.product,
            download_id: &file.download_id,
            file_id: &file.file_id,
            size: file.size,
            siblings: &[],
        };
        let path = pin
            .complete(&lookup, file.md5.as_deref(), &cancel)
            .map_err(canceled)?
            .ok_or_else(|| UpdateError::Changed {
                name: name.to_owned(),
            })?;
        found.push((file, path));
    }
    Ok((pins, found))
}

pub(super) fn open_package(path: &Path) -> Result<package::Package> {
    #[cfg(test)]
    {
        let mut known = package::KNOWN_POSTINST.to_vec();
        known.extend(tests::known_scripts());
        package::open_with(path, &known)
    }
    #[cfg(not(test))]
    package::open(path)
}

/// A package's entries as staging leaves them: split files assembled and
/// their pieces gone.
fn package_entries(package: &package::Package) -> Vec<(String, EntryType)> {
    let pieces: Vec<&String> = package
        .splits()
        .iter()
        .flat_map(|split| &split.pieces)
        .collect();
    let mut entries: Vec<(String, EntryType)> = package
        .entries()
        .iter()
        .filter(|entry| !pieces.contains(&&entry.path))
        .map(|entry| (entry.path.clone(), entry.kind))
        .collect();
    for split in package.splits() {
        if !entries.iter().any(|(path, _)| *path == split.path) {
            entries.push((split.path.clone(), EntryType::File));
        }
    }
    entries
}

/// Deletes the used packages from the download cache, unless another
/// folder's journal names them or a state that can't be read might. Each
/// product is pinned while it's decided and removed, and an update pins its
/// packages until its journal names them, so one being written meanwhile is
/// always seen. A failure is only a warning, since the update is done.
pub(super) fn remove_packages(
    paths: &Paths,
    folder: &Path,
    products: &[u64],
    warnings: &mut Vec<String>,
) {
    let cache = DownloadCache::new(paths.gog_download_cache());
    let mut products = products.to_vec();
    products.sort_unstable();
    products.dedup();
    for product in products {
        #[cfg(test)]
        tests::cleanup_step("pinning");
        let pin = match cache.pin(product, &|| false) {
            Ok(pin) => pin,
            Err(error) => {
                warnings.push(format!(
                    "Couldn't remove the downloaded GOG packages: {error}"
                ));
                continue;
            }
        };
        let named = named_by_journal(paths, folder, product);
        #[cfg(test)]
        tests::cleanup_step("decided");
        match named {
            Ok(true) => {}
            Ok(false) => {
                if let Err(error) = pin.remove() {
                    warnings.push(format!(
                        "Couldn't remove the downloaded GOG packages: {error}"
                    ));
                }
            }
            Err(error) => {
                warnings.push(format!(
                    "Kept the downloaded GOG packages in case another game needs them: {error:#}"
                ));
            }
        }
    }
}

/// Whether another folder's journal names a package of `product`. A state
/// that can't be read is an error, since it could.
fn named_by_journal(paths: &Paths, folder: &Path, product: u64) -> Result<bool> {
    for (_, state) in folder::list(paths)? {
        let state = state?;
        if state.folder != folder
            && state
                .journal
                .is_some_and(|journal| journal.packages.iter().any(|p| p.product == product))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
pub(super) mod tests;
