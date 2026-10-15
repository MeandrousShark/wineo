//! Whether a GOG game has an update on GOG, and, in `apply`, `publish`, and
//! `windows`, applying an installer-tracked game's update.
//!
//! Status is computed from a folder's `[committed]` state, even while a
//! journal is present, so a half-finished update never hides what is
//! installed. Checking never touches the game's files. Its one write is
//! recording a build another tool installed (see `adopt_build`).
//!
//! State files are read without their lock: they are replaced atomically,
//! so a read is always a whole file, and a check mustn't leave lock files
//! behind.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

use super::account::{CallError, Session};
use super::folder::{
    self, Committed, FolderError, FolderLock, FolderState, Platform, Product, ProductStatus, Track,
};
use super::migrate::{host_only, id_text};
use super::names;
use super::product;
use crate::environment::{self, Environment};
use crate::files;
use crate::paths::Paths;

/// Whether a game is behind GOG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// JSON `none`.
    UpToDate,
    /// JSON `available`.
    Available,
    /// GOG has no generation 2 Galaxy build on the default branch, so
    /// wineo can't update the game. JSON `galaxy`.
    OnGog,
    /// JSON `cant_compare`.
    CantCompare,
}

impl UpdateState {
    /// The word `--json` uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "none",
            Self::Available => "available",
            Self::OnGog => "galaxy",
            Self::CantCompare => "cant_compare",
        }
    }
}

/// Why a game's update can't be applied, or can't be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A Galaxy-tracked game GOG has no Galaxy build of that wineo can
    /// update. `name` is its environment, for the way out.
    Galaxy { name: String },
    /// Why the game can't be compared, as a sentence.
    CantCompare(String),
}

impl Reason {
    /// The machine word for Galaxy, else the sentence.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Galaxy { .. } => "galaxy",
            Self::CantCompare(why) => why,
        }
    }
}

/// How one DLC stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlcState {
    UpToDate,
    Available,
    /// The DLC has no version to compare: a migrated one until the first
    /// rebuild records it, or one a Galaxy build carries. It doesn't change
    /// the game's state.
    Unknown,
    CantCompare,
    /// Its install failed. When GOG offers it again, `available` says which
    /// version and the next update retries it.
    Failed,
    /// GOG offers no installer for it in the game's language or English.
    Unavailable,
    /// Owned and offered, but not installed: bought since, or unavailable
    /// until now. The next update installs it.
    NotInstalled,
}

impl DlcState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "none",
            Self::Available => "available",
            Self::Unknown => "unknown",
            Self::CantCompare => "cant_compare",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
            Self::NotInstalled => "not_installed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlcStatus {
    pub id: u64,
    /// None when GOG's details don't list this DLC.
    pub title: Option<String>,
    /// The name in the folder's `goggame-*.info`, for when there's no title.
    pub name: Option<String>,
    pub installed: Option<String>,
    pub available: Option<String>,
    pub state: DlcState,
}

/// One game's update status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The environment that owns the game.
    pub environment: String,
    pub gog_id: u64,
    pub title: String,
    pub folder: PathBuf,
    pub platform: Platform,
    pub track: Track,
    pub language: String,
    /// The base game's installed version, or build id on the Galaxy track.
    pub installed: Option<String>,
    /// The base game's current installer version, or latest generation 2
    /// default-branch build id, when it could be told.
    pub available: Option<String>,
    /// GOG's names for the Galaxy builds `installed` and `available` are,
    /// when it lists one. None on the installer track.
    pub installed_name: Option<String>,
    pub available_name: Option<String>,
    pub state: UpdateState,
    /// Whether this wineo can apply the update.
    pub applicable: bool,
    /// Set when the state is `OnGog`, and when it is `CantCompare`.
    pub reason: Option<Reason>,
    /// A journal is present: the owner has an update to finish.
    pub unfinished: bool,
    /// The journal is a Galaxy game's first install, which installing again
    /// finishes, not `wineo update`.
    pub galaxy_install: bool,
    pub warnings: Vec<String>,
    /// Every product after the base, in the order they were added.
    pub dlc: Vec<DlcStatus>,
}

impl DlcStatus {
    /// `NAME (ID)` against the base game's title: GOG's title if it listed
    /// one, else the name in the folder.
    pub fn label(&self, base_title: &str) -> String {
        let title = self
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty());
        names::dlc_label(base_title, self.id, title.or(self.name.as_deref()))
    }
}

/// A game that couldn't be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameError {
    /// The owning environment. None for a state file that can't be read,
    /// since it can't say whose it is.
    pub environment: Option<String>,
    pub id: Option<u64>,
    pub message: String,
}

/// Every owned game's status, and the ones that couldn't be checked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Batch {
    /// By environment name.
    pub statuses: Vec<Status>,
    pub errors: Vec<GameError>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{name} doesn't track a GOG game.")]
    NotTracked { name: String },
    /// A config that can't be read as TOML could be recording a game, so it
    /// isn't taken for one that doesn't.
    #[error("{name} can't be checked: its config can't be read ({detail}).")]
    Unloadable { name: String, detail: String },
    #[error("Updating Galaxy builds comes in a later release.")]
    Galaxy { name: String },
    /// Options that can't go together.
    #[error("{why}")]
    Options { why: &'static str },
    /// A repair or a branch for a game on the installer track.
    #[error("Only Galaxy games have branches. Move it with --reinstall --track galaxy.")]
    NotGalaxy { name: String },
    /// A journal of a kind this release can't finish yet, `repair`.
    #[error("{name} has an unfinished GOG {kind}, and finishing it comes in a later release.")]
    LaterRelease { name: String, kind: &'static str },
    #[error("{name} can't be updated: {why}")]
    CantCompare { name: String, why: String },
    /// An installed product has no package to rebuild it from.
    #[error("{name} can't be updated: {why}")]
    NoInstaller { name: String, why: String },
    #[error(
        "{path} from GOG product {product} can't go inside {parent}, which GOG product {other} ships as a file or symlink."
    )]
    Layers {
        path: String,
        product: u64,
        parent: String,
        other: u64,
    },
    #[error("{name} is running. Stop it before updating.")]
    Running { name: String },
    #[error("{name}'s GOG game changed while updating; run wineo update {name} again.")]
    Changed { name: String },
    /// `journal` says whether the update had started changing the folder,
    /// which leaves it to finish with `wineo update`.
    #[error("Canceled.")]
    Canceled { journal: bool },
}

/// What a check does, besides comparing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Check {
    /// Fetch GOG's product details again instead of using a cached copy.
    pub refresh: bool,
    /// Write nothing: product details aren't cached, and a build another tool
    /// installed is used without being recorded. For a dry run.
    pub read_only: bool,
}

/// Checks the GOG game environment `name` runs. Product details are public;
/// the login lists Galaxy builds and which DLC the account owns. With
/// `refresh`, cached details are fetched again.
///
/// Refused in a portable root (`MigrateError::Portable`). Fails with
/// `UpdateError::NotTracked` when the environment has no `gog_id`, and with
/// `FolderError::StateMissing` when it has no state file or
/// `FolderError::Unreadable` when its state can't be read.
pub fn check(session: &mut Session, name: &str, refresh: bool) -> Result<Status> {
    check_with(
        session,
        name,
        Check {
            refresh,
            read_only: false,
        },
    )
}

/// `check` with the choices in `how`.
pub fn check_with(session: &mut Session, name: &str, how: Check) -> Result<Status> {
    let paths = session.paths().clone();
    host_only(&paths)?;
    Environment::ensure_exists(&paths, name)?;
    let state = game_of(&paths, name)?;
    status_of(session, &paths, name, state, how, &mut Owned::default())
}

/// `name`'s game folder: the one it owns, else an ownerless one of its game
/// that its targets run from, found the way a launch finds it. Reads only;
/// an ownerless folder comes back unclaimed.
fn game_of(paths: &Paths, name: &str) -> Result<FolderState> {
    let gog_id = recorded_gog_id(paths, name)?.ok_or_else(|| UpdateError::NotTracked {
        name: name.to_owned(),
    })?;
    folder::game_folder(paths, name, &gog_id, &named_paths(paths, name))?.ok_or_else(|| {
        FolderError::StateMissing {
            name: name.to_owned(),
            gog_id,
        }
        .into()
    })
}

/// The journal of `name`'s game folder, when it has one. Reads the folder's
/// state alone, so it says so when GOG can't be reached for a check.
pub fn journal_of(paths: &Paths, name: &str) -> Option<folder::Journal> {
    game_of(paths, name).ok()?.journal
}

/// The host paths `name`'s targets name: each one's program, working
/// directory, and absolute arguments, Windows paths translated through the
/// prefix's drives. Like a launch's, but for every target, since an update
/// isn't launching any one of them. Nothing when the config can't be loaded.
/// Reads only, and takes no locks.
pub fn named_paths(paths: &Paths, name: &str) -> Vec<PathBuf> {
    let Ok(environment) = Environment::load(paths, name) else {
        return Vec::new();
    };
    // The runner isn't resolved, so both places Wine's drives can be.
    let prefixes: Vec<PathBuf> = environment
        .prefix(paths)
        .map(|prefix| vec![prefix.join("pfx"), prefix])
        .unwrap_or_default();
    let mut named = Vec::new();
    let mut add = |value: &str| {
        if crate::winpath::is_windows_path(value) {
            for prefix in &prefixes {
                named.extend(crate::winpath::to_unix_lenient(prefix, value));
            }
        } else if let Ok(Some(path)) = paths.resolve_stored(value) {
            named.push(path);
        }
    };
    for target in environment.config.targets.values() {
        add(&target.exe);
        if let Some(cwd) = &target.cwd {
            add(cwd);
        }
        for arg in &target.args {
            if crate::winpath::is_windows_path(arg) || Path::new(arg).is_absolute() {
                add(arg);
            }
        }
    }
    named
}

/// Checks every game folder an environment owns whose `gog_id` matches, and
/// an ownerless folder of its game that an environment's targets run from,
/// without claiming it. Other ownerless folders are skipped. A game that can't be checked is reported
/// in `errors` and the rest carry on.
pub fn check_all(session: &mut Session, refresh: bool) -> Result<Batch> {
    check_all_with(
        session,
        Check {
            refresh,
            read_only: false,
        },
    )
}

/// `check_all` with the choices in `how`.
pub fn check_all_with(session: &mut Session, how: Check) -> Result<Batch> {
    let paths = session.paths().clone();
    host_only(&paths)?;
    let mut owned = Owned::default();
    let mut batch = Batch::default();
    // The environments whose game was checked or reported in the loop.
    let mut seen = BTreeSet::new();
    for (_, state) in folder::list(&paths)? {
        let state = match state {
            Ok(state) => state,
            Err(error) => {
                batch.errors.push(GameError {
                    environment: None,
                    id: None,
                    message: format!("{error:#}"),
                });
                continue;
            }
        };
        let Some(owner) = state.owner.clone() else {
            continue;
        };
        let id = state.gog_id();
        match recorded_gog_id(&paths, &owner) {
            Ok(Some(recorded)) if state.is_game(&recorded) => {
                seen.insert(owner.clone());
            }
            Ok(_) => continue,
            Err(error) => {
                seen.insert(owner.clone());
                batch.errors.push(GameError {
                    environment: Some(owner),
                    id,
                    message: error.to_string(),
                });
                continue;
            }
        }
        match status_of(session, &paths, &owner, state, how, &mut owned) {
            Ok(status) => batch.statuses.push(status),
            Err(error) => batch.errors.push(GameError {
                environment: Some(owner),
                id,
                message: format!("{error:#}"),
            }),
        }
    }
    // An environment that records a game but owns no folder for it lost its
    // state, the same answer `check` gives. A config that can't be read is
    // only reported above, when it owns a folder, since nothing says it
    // records a game.
    for name in environment::names(&paths)? {
        if seen.contains(&name) {
            continue;
        }
        let Ok(Some(gog_id)) = recorded_gog_id(&paths, &name) else {
            continue;
        };
        let message = match folder::game_folder(&paths, &name, &gog_id, &named_paths(&paths, &name))
        {
            // An ownerless folder its targets run from, which its next
            // update or launch claims. Only read here.
            Ok(Some(state)) if state.owner.is_none() => {
                let id = state.gog_id();
                match status_of(session, &paths, &name, state, how, &mut owned) {
                    Ok(status) => batch.statuses.push(status),
                    Err(error) => batch.errors.push(GameError {
                        environment: Some(name),
                        id,
                        message: format!("{error:#}"),
                    }),
                }
                continue;
            }
            Ok(Some(_)) => continue,
            Ok(None) => FolderError::StateMissing {
                name: name.clone(),
                gog_id: gog_id.clone(),
            }
            .to_string(),
            // A state file that can't be read could be this one's.
            Err(error) => format!("{error:#}"),
        };
        batch.errors.push(GameError {
            environment: Some(name),
            id: gog_id.parse().ok(),
            message,
        });
    }
    batch
        .statuses
        .sort_by(|a, b| a.environment.cmp(&b.environment));
    batch
        .errors
        .sort_by(|a, b| a.environment.cmp(&b.environment));
    Ok(batch)
}

/// The environment that records each GOG game, by game id, the first by
/// name when several do. A config that can't be read doesn't say. GOG games
/// live in the host library, so a portable root records none.
pub fn recorded_games(paths: &Paths) -> BTreeMap<u64, String> {
    let mut recorded = BTreeMap::new();
    if paths.portable_root().is_some() {
        return recorded;
    }
    for name in environment::names(paths).unwrap_or_default() {
        if let Ok(Some(id)) = recorded_gog_id(paths, &name)
            && let Ok(id) = id.parse()
        {
            recorded.entry(id).or_insert(name);
        }
    }
    recorded
}

/// The `gog_id` an environment's config records, read as plain TOML. None
/// when the config is gone or records none.
pub fn recorded_gog_id(paths: &Paths, name: &str) -> Result<Option<String>, UpdateError> {
    let unloadable = |detail: String| UpdateError::Unloadable {
        name: name.to_owned(),
        detail,
    };
    let Some(text) = files::read_to_string_optional(&paths.environment_config(name))
        .map_err(|error| unloadable(error.to_string()))?
    else {
        return Ok(None);
    };
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| unloadable(error.message().to_owned()))?;
    match doc.get("gog_id") {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|id| Some(id.to_owned()))
            .ok_or_else(|| unloadable("gog_id isn't text".to_owned())),
    }
}

/// What a state says is installed. A first install still in flight has
/// nothing committed, so the journal's target stands in.
struct Installed {
    gog_id: u64,
    platform: Platform,
    language: String,
    track: Track,
    products: Vec<Product>,
    /// Why nothing can be compared yet.
    unfinished_install: bool,
}

impl Installed {
    fn of(name: &str, state: &FolderState) -> Result<Self> {
        if let Some(committed) = &state.committed {
            return Ok(Self::committed(committed, false));
        }
        let Some(journal) = &state.journal else {
            // Callers pick states by their game id, which this state would
            // have to have, so this is only a guard.
            bail!("{name}'s GOG install hasn't started.");
        };
        let target = &journal.target;
        Ok(Self {
            gog_id: target.gog_id,
            platform: target.platform,
            language: target.language.clone(),
            track: target.track,
            products: Vec::new(),
            unfinished_install: true,
        })
    }

    fn committed(committed: &Committed, unfinished_install: bool) -> Self {
        Self {
            gog_id: committed.gog_id,
            platform: committed.platform,
            language: committed.language.clone(),
            track: committed.track,
            products: committed.products.clone(),
            unfinished_install,
        }
    }
}

/// What comparing a game's products found.
struct Compared {
    state: UpdateState,
    installed: Option<String>,
    available: Option<String>,
    why: Option<String>,
    dlc: Vec<DlcStatus>,
    /// What an installer-tracked game's update rebuilds: every product, with
    /// failed and unavailable DLC GOG offers now turned installed, and newly
    /// owned DLC added after them.
    products: Option<Vec<Product>>,
    /// Why each newly owned DLC can't be installed, for an update's review.
    notes: Vec<String>,
    /// What comparing couldn't find out, besides the state.
    warnings: Vec<String>,
    installed_name: Option<String>,
    available_name: Option<String>,
}

/// The ids the account owns, asked of GOG once for any number of games.
#[derive(Debug, Default)]
pub(super) struct Owned(Option<Result<HashSet<u64>, String>>);

impl Owned {
    fn get(&mut self, session: &mut Session) -> Result<&HashSet<u64>, String> {
        let owned = self.0.get_or_insert_with(|| {
            session
                .call(|client, token| client.owned_ids(token))
                .map(|ids| ids.into_iter().collect())
                .map_err(|error| error.to_string())
        });
        owned.as_ref().map_err(Clone::clone)
    }
}

/// A status and what updating the game would rebuild.
pub(super) struct Checked {
    pub(super) status: Status,
    /// See `Compared::products`. None on the Galaxy track and while an
    /// install hasn't finished.
    pub(super) products: Option<Vec<Product>>,
    /// Notes for an update's review: newly owned DLC that can't be
    /// installed, and why DLC ownership couldn't be told.
    pub(super) notes: Vec<String>,
}

fn status_of(
    session: &mut Session,
    paths: &Paths,
    name: &str,
    state: FolderState,
    how: Check,
    owned: &mut Owned,
) -> Result<Status> {
    let names = names::product_names(&state.folder);
    Ok(status_with(session, paths, name, state, how, &names, owned)?.status)
}

/// `status_of` with the folder's product names already read.
pub(super) fn status_with(
    session: &mut Session,
    paths: &Paths,
    name: &str,
    state: FolderState,
    how: Check,
    names: &HashMap<u64, String>,
    owned: &mut Owned,
) -> Result<Checked> {
    let mut installed = Installed::of(name, &state)?;
    let details = product::load_with(
        paths,
        session.client(),
        installed.gog_id,
        how.refresh,
        !how.read_only,
    )?;
    let mut warnings: Vec<String> = details.warning.into_iter().collect();
    let unfinished = state.journal.is_some();
    let galaxy_install = state.journal.as_ref().is_some_and(|journal| {
        journal.kind == folder::JournalKind::Install && journal.target.track == Track::Galaxy
    });

    if installed.platform == Platform::Windows
        && installed.track == Track::Galaxy
        && !unfinished
        // A folder no environment has claimed yet is only read.
        && state.owner.is_some()
        && let Some(adopted) = if how.read_only {
            seen_build(&state)
        } else {
            adopt_build(paths, &state)?
        }
    {
        if adopted.recorded {
            warnings.push(format!(
                "Recorded the build {}, which another tool installed.",
                base_build(&adopted.committed).unwrap_or_default()
            ));
        }
        installed = Installed::committed(&adopted.committed, false);
    }

    // Newly owned DLC are only looked for when GOG lists DLC the folder
    // doesn't have, so most games never ask. Without the list, nothing is
    // said about them, as when installing.
    let mut notes = Vec::new();
    let unlisted = details
        .product
        .dlc_ids
        .iter()
        .any(|id| !installed.products.iter().any(|product| product.id == *id));
    let owned = if !installed.unfinished_install && unlisted {
        match owned.get(session) {
            Ok(owned) => Some(owned),
            Err(error) => {
                let note = format!(
                    "Couldn't check which DLC you own, so DLC you bought since isn't listed: {error}"
                );
                warnings.push(note.clone());
                notes.push(note);
                None
            }
        }
    } else {
        None
    };
    let compared = if installed.unfinished_install {
        Compared {
            state: UpdateState::CantCompare,
            installed: None,
            available: None,
            why: Some("The install hasn't finished.".to_owned()),
            dlc: Vec::new(),
            products: None,
            notes: Vec::new(),
            warnings: Vec::new(),
            installed_name: None,
            available_name: None,
        }
    } else {
        match (installed.platform, installed.track) {
            (Platform::Linux | Platform::Windows, Track::Installer) => {
                by_installer(&installed, names, &details.product, owned)
            }
            (Platform::Windows, Track::Galaxy) => by_build(
                session,
                paths,
                name,
                &installed,
                names,
                &details.product,
                owned,
            )?,
            (Platform::Linux, Track::Galaxy) => cant(
                &installed,
                names,
                "GOG's Galaxy builds are only for Windows games.".to_owned(),
            ),
        }
    };

    let applicable = compared.state == UpdateState::Available;
    let reason = match compared.state {
        UpdateState::CantCompare => compared.why.map(Reason::CantCompare),
        UpdateState::OnGog => Some(Reason::Galaxy {
            name: name.to_owned(),
        }),
        UpdateState::Available | UpdateState::UpToDate => None,
    };
    notes.extend(compared.notes);
    warnings.extend(compared.warnings);
    let status = Status {
        environment: name.to_owned(),
        gog_id: installed.gog_id,
        title: details.product.title,
        folder: state.folder,
        platform: installed.platform,
        track: installed.track,
        language: installed.language,
        installed: compared.installed,
        available: compared.available,
        installed_name: compared.installed_name,
        available_name: compared.available_name,
        state: compared.state,
        applicable,
        reason,
        unfinished,
        galaxy_install,
        warnings,
        dlc: compared.dlc,
    };
    Ok(Checked {
        status,
        products: compared.products,
        notes,
    })
}

/// A game that can't be compared, with its DLC listed as they are.
fn cant(installed: &Installed, names: &HashMap<u64, String>, why: String) -> Compared {
    Compared {
        state: UpdateState::CantCompare,
        installed: base(installed).and_then(|base| base.version.clone().or(base.build.clone())),
        available: None,
        why: Some(why),
        dlc: dlc(installed, names).map(|(dlc, _)| dlc).collect(),
        products: None,
        notes: Vec::new(),
        warnings: Vec::new(),
        installed_name: None,
        available_name: None,
    }
}

fn base(installed: &Installed) -> Option<&Product> {
    installed
        .products
        .iter()
        .find(|product| product.id == installed.gog_id)
}

/// Each product after the base, with its listing before any comparison.
fn dlc<'a>(
    installed: &'a Installed,
    names: &'a HashMap<u64, String>,
) -> impl Iterator<Item = (DlcStatus, &'a Product)> {
    installed
        .products
        .iter()
        .filter(|product| product.id != installed.gog_id)
        .map(|product| {
            let state = match product.status {
                ProductStatus::Installed => DlcState::Unknown,
                ProductStatus::Failed => DlcState::Failed,
                ProductStatus::Unavailable => DlcState::Unavailable,
            };
            (
                DlcStatus {
                    id: product.id,
                    title: None,
                    name: names.get(&product.id).cloned(),
                    installed: product.version.clone().or_else(|| product.build.clone()),
                    available: None,
                    state,
                },
                product,
            )
        })
}

/// How one product's version stands against GOG's installer.
enum Verdict {
    Same,
    Differs,
    /// An installer exists but nothing records which version is installed.
    InstalledUnknown,
    /// Can't tell, with why.
    Cant(String),
}

/// GOG's `os` installer of a product for the recorded language: the one
/// GOG spells the same way, else the one for its primary subtag (`en-US`
/// matches `en`). `entry` is GOG's details for the product, if it lists
/// any. Anything else, none or more than one, is why it can't be told.
fn matching_installer<'a>(
    os: &str,
    label: &str,
    language: &str,
    entry: Option<&'a wineo_gog::Product>,
) -> Result<&'a wineo_gog::Download, String> {
    let shown = if os == "linux" { "Linux" } else { "Windows" };
    let candidates: Vec<&wineo_gog::Download> = entry
        .map(|entry| {
            entry
                .installers
                .iter()
                .filter(|installer| installer.os == os)
                .collect()
        })
        .unwrap_or_default();
    let exact: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|installer| installer.language == language)
        .collect();
    let matching = if exact.is_empty() {
        let primary = language.split(['-', '_']).next().unwrap_or(language);
        candidates
            .iter()
            .copied()
            .filter(|installer| primary != language && installer.language == primary)
            .collect()
    } else {
        exact
    };
    match matching.as_slice() {
        [] => Err(format!("{label} has no {shown} installer in {language}.")),
        [only] => Ok(*only),
        _ => Err(format!(
            "{label} has more than one {shown} installer in {language}."
        )),
    }
}

/// The version GOG lists for an installer, or why there's none.
fn installer_version(
    os: &str,
    label: &str,
    installer: &wineo_gog::Download,
) -> Result<String, String> {
    let shown = if os == "linux" { "Linux" } else { "Windows" };
    installer
        .version
        .clone()
        .ok_or_else(|| format!("GOG lists no version for {label}'s {shown} installer."))
}

/// Compares a product's installed version with GOG's `os` installer for the
/// recorded language. `entry` is GOG's details for it, if it lists any.
fn compare_one(
    os: &str,
    label: &str,
    language: &str,
    entry: Option<&wineo_gog::Product>,
    installed: Option<&str>,
) -> (Verdict, Option<String>) {
    let installer = match matching_installer(os, label, language, entry) {
        Ok(installer) => installer,
        Err(why) => return (Verdict::Cant(why), None),
    };
    // Nothing to compare with, so GOG's version, or the lack of one, can't
    // change what is said.
    let Some(version) = installed else {
        return (Verdict::InstalledUnknown, installer.version.clone());
    };
    let available = match installer_version(os, label, installer) {
        Ok(available) => available,
        Err(why) => return (Verdict::Cant(why), None),
    };
    let verdict = if version == available {
        Verdict::Same
    } else {
        Verdict::Differs
    };
    (verdict, Some(available))
}

/// The installer track: the base game and every installed DLC against GOG's
/// installers. Any product that can't be compared makes the game's state
/// Can't compare, since a rebuild would refuse it. Otherwise any difference
/// makes it Available, and so does a DLC the next update would install: a
/// failed or unavailable one GOG offers now, or one in `owned` that the
/// folder doesn't have.
fn by_installer(
    installed: &Installed,
    names: &HashMap<u64, String>,
    details: &wineo_gog::Product,
    owned: Option<&HashSet<u64>>,
) -> Compared {
    let os = match installed.platform {
        Platform::Linux => "linux",
        Platform::Windows => "windows",
    };
    let language = &installed.language;
    let mut tally = Tally::default();
    // Failed and unavailable DLC GOG offers now, to install in their place.
    let mut offered = HashMap::new();
    let mut notes = Vec::new();

    let (base_installed, base_available) = match base(installed) {
        Some(base) if base.status == ProductStatus::Installed => {
            let label = title_or(&details.title, || format!("GOG game {}", installed.gog_id));
            let (verdict, available) = compare_one(
                os,
                &label,
                base.language.as_deref().unwrap_or(language),
                Some(details),
                base.version.as_deref(),
            );
            match verdict {
                Verdict::Same => {}
                Verdict::Differs => tally.available(),
                Verdict::InstalledUnknown => {
                    tally.cant(format!("The installed version of {label} isn't known."));
                }
                Verdict::Cant(why) => tally.cant(why),
            }
            (base.version.clone(), available)
        }
        Some(_) | None => {
            tally.cant("The base game isn't installed.".to_owned());
            (None, None)
        }
    };

    let mut listing = Vec::new();
    for (mut entry, product) in dlc(installed, names) {
        let gog = details.dlcs.iter().find(|dlc| dlc.id == product.id);
        entry.title = gog.map(|gog| gog.title.clone()).filter(|t| !t.is_empty());
        if product.status != ProductStatus::Installed {
            // Recorded because it was owned, so only GOG's offer is checked.
            let label = entry.label(&details.title);
            if let Ok(offer) = dlc_offer(installed.platform, &label, language, gog) {
                entry.available = Some(offer.version.clone());
                if product.status == ProductStatus::Unavailable {
                    entry.state = DlcState::NotInstalled;
                }
                tally.available();
                offered.insert(product.id, offer.product(product.id));
            }
        } else {
            let label = entry.label(&details.title);
            let (verdict, available) = compare_one(
                os,
                &label,
                product.language.as_deref().unwrap_or(language),
                gog,
                product.version.as_deref(),
            );
            entry.available = available;
            match verdict {
                Verdict::Same => entry.state = DlcState::UpToDate,
                Verdict::Differs => {
                    entry.state = DlcState::Available;
                    tally.available();
                }
                // Nothing records a migrated DLC's version, so the game is
                // judged without it until a rebuild does.
                Verdict::InstalledUnknown => entry.state = DlcState::Unknown,
                Verdict::Cant(why) => {
                    entry.state = DlcState::CantCompare;
                    tally.cant(why);
                }
            }
        }
        listing.push(entry);
    }

    let mut products: Vec<Product> = installed
        .products
        .iter()
        .map(|product| {
            offered
                .remove(&product.id)
                .unwrap_or_else(|| product.clone())
        })
        .collect();
    // Owned DLC the folder has no record of, in GOG's order.
    let mut added = HashSet::new();
    for &id in &details.dlc_ids {
        if !owned.is_some_and(|owned| owned.contains(&id))
            || installed.products.iter().any(|product| product.id == id)
            || !added.insert(id)
        {
            continue;
        }
        let gog = details.dlcs.iter().find(|dlc| dlc.id == id);
        let mut entry = DlcStatus {
            id,
            title: gog.map(|gog| gog.title.clone()).filter(|t| !t.is_empty()),
            name: names.get(&id).cloned(),
            installed: None,
            available: None,
            state: DlcState::Unavailable,
        };
        let label = entry.label(&details.title);
        match dlc_offer(installed.platform, &label, language, gog) {
            Ok(offered) => {
                entry.available = Some(offered.version.clone());
                entry.state = DlcState::NotInstalled;
                tally.available();
                products.push(offered.product(id));
            }
            // Recorded as unavailable when an update runs anyway, as an
            // install records it, but no reason to run one.
            Err(why) => {
                notes.push(why);
                products.push(Product {
                    reported_language: None,
                    id,
                    version: None,
                    build: None,
                    language: None,
                    status: ProductStatus::Unavailable,
                });
            }
        }
        listing.push(entry);
    }

    Compared {
        state: tally.state,
        installed: base_installed,
        available: base_available,
        why: tally.why,
        dlc: listing,
        products: Some(products),
        notes,
        warnings: Vec::new(),
        installed_name: None,
        available_name: None,
    }
}

/// A DLC that isn't installed, as GOG offers it now.
struct DlcOffer {
    version: String,
    /// GOG's code for its installer's language when that isn't the folder's,
    /// which is how it's recorded.
    language: Option<String>,
}

impl DlcOffer {
    fn product(&self, id: u64) -> Product {
        Product {
            reported_language: None,
            id,
            version: Some(self.version.clone()),
            build: None,
            language: self.language.clone(),
            status: ProductStatus::Installed,
        }
    }
}

/// The installer a DLC that isn't installed would get, the way an install
/// picks one: in the folder's language, else English. A folder language
/// with a region, like a migrated game's `en-US`, is GOG's primary code when
/// GOG has nothing spelled the same way.
fn dlc_offer(
    platform: Platform,
    label: &str,
    language: &str,
    entry: Option<&wineo_gog::Product>,
) -> Result<DlcOffer, String> {
    let Some(entry) = entry else {
        return Err(format!("GOG lists no details for {label}."));
    };
    let os = match platform {
        Platform::Linux => "linux",
        Platform::Windows => "windows",
    };
    let primary = language.split(['-', '_']).next().unwrap_or(language);
    let code = if entry
        .installers
        .iter()
        .any(|installer| installer.os == os && installer.language == language)
    {
        language
    } else {
        primary
    };
    let picked = install::pick(entry, label, code, platform)?;
    // What an update matches it by: the folder's language, unless it fell
    // back to another.
    let recorded = (picked.language != language && picked.language != primary)
        .then(|| picked.language.clone());
    Ok(DlcOffer {
        version: picked.version,
        language: recorded,
    })
}

/// A game's state from its products: Can't compare beats Available beats Up
/// to date, and the first reason it can't be compared is kept.
struct Tally {
    state: UpdateState,
    why: Option<String>,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            state: UpdateState::UpToDate,
            why: None,
        }
    }
}

impl Tally {
    fn available(&mut self) {
        if self.state == UpdateState::UpToDate {
            self.state = UpdateState::Available;
        }
    }

    fn cant(&mut self, why: String) {
        if self.state != UpdateState::CantCompare {
            self.state = UpdateState::CantCompare;
            self.why = Some(why);
        }
    }
}

fn title_or(title: &str, fallback: impl FnOnce() -> String) -> String {
    if title.is_empty() {
        fallback()
    } else {
        title.to_owned()
    }
}

/// The Galaxy track: the base game's build against the latest generation 2
/// build GOG lists on the default branch. Any other installed build, one
/// that is unknown, unlisted, generation 1, or on another branch, is
/// updated onto it. Newly owned DLC and failed or unavailable DLC the build
/// has count as an update too, as `galaxy::update::plan` installs them.
fn by_build(
    session: &mut Session,
    paths: &Paths,
    name: &str,
    installed: &Installed,
    names: &HashMap<u64, String>,
    details: &wineo_gog::Product,
    owned: Option<&HashSet<u64>>,
) -> Result<Compared> {
    let label = title_or(&details.title, || format!("GOG game {}", installed.gog_id));
    let id = installed.gog_id;
    // A game GOG has no content system for answers 404, which leaves it to
    // its installers, as installing picks them.
    let builds = match session.call(|client, token| client.windows_builds(id, Some(token))) {
        Ok(builds) => builds,
        Err(CallError::Request(error)) if error.kind() == wineo_gog::ErrorKind::Status(404) => {
            wineo_gog::Builds {
                items: Vec::new(),
                has_private_branches: false,
            }
        }
        Err(error) => return Err(error.into()),
    };
    let latest = galaxy::latest(&builds);
    let current = base(installed).and_then(|base| base.build.clone());
    let version_name = |build: &str| {
        builds
            .items
            .iter()
            .find(|listed| listed.build_id == build)
            .and_then(|listed| listed.version_name.clone())
            .filter(|name| !name.is_empty())
    };
    let mut dlc: Vec<DlcStatus> = dlc(installed, names)
        .map(|(mut entry, _)| {
            entry.title = details
                .dlcs
                .iter()
                .find(|dlc| dlc.id == entry.id)
                .map(|dlc| dlc.title.clone())
                .filter(|title| !title.is_empty());
            entry
        })
        .collect();
    let mut warnings = Vec::new();
    let mut tally = Tally::default();
    match latest {
        None => {
            tally.state = UpdateState::OnGog;
        }
        Some(latest) => {
            if current.as_deref() != Some(latest.build_id.as_str()) {
                tally.available();
            }
            let committed = Committed {
                source: None,
                gog_id: id,
                platform: installed.platform,
                language: installed.language.clone(),
                track: installed.track,
                products: installed.products.clone(),
                files: Vec::new(),
            };
            // The build's own file list is only fetched when there is a DLC
            // to ask it about.
            let questions = installed
                .products
                .iter()
                .any(|product| product.status != ProductStatus::Installed)
                || details.dlc_ids.iter().any(|dlc| {
                    owned.is_some_and(|owned| owned.contains(dlc))
                        && !installed.products.iter().any(|product| product.id == *dlc)
                });
            if questions {
                match galaxy::update::selected_for_check(
                    session, paths, name, &label, details, names, &committed, latest, owned,
                ) {
                    Ok(products) => {
                        if galaxy::update::selection_changed(&committed, &products) {
                            tally.available();
                        }
                        for product in products
                            .iter()
                            .filter(|product| product.id != id)
                            .filter(|product| product.status == ProductStatus::Installed)
                        {
                            match dlc.iter_mut().find(|entry| entry.id == product.id) {
                                // Already installed.
                                Some(entry) if entry.state == DlcState::Unknown => {}
                                Some(entry) => {
                                    entry.available = Some(latest.build_id.clone());
                                    if entry.state == DlcState::Unavailable {
                                        entry.state = DlcState::NotInstalled;
                                    }
                                }
                                None => dlc.push(DlcStatus {
                                    id: product.id,
                                    title: details
                                        .dlcs
                                        .iter()
                                        .find(|dlc| dlc.id == product.id)
                                        .map(|dlc| dlc.title.clone())
                                        .filter(|title| !title.is_empty()),
                                    name: names.get(&product.id).cloned(),
                                    installed: None,
                                    available: Some(latest.build_id.clone()),
                                    state: DlcState::NotInstalled,
                                }),
                            }
                        }
                    }
                    Err(error) => warnings.push(format!(
                        "Couldn't tell which DLC of {label} an update would install: {error:#}"
                    )),
                }
            }
        }
    }
    Ok(Compared {
        state: tally.state,
        installed_name: current.as_deref().and_then(version_name),
        available_name: latest.and_then(|build| version_name(&build.build_id)),
        installed: current,
        available: latest.map(|build| build.build_id.clone()),
        why: None,
        dlc,
        products: None,
        notes: Vec::new(),
        warnings,
    })
}

/// A Galaxy game's committed state after looking for a build another tool
/// installed.
#[derive(Debug, PartialEq, Eq)]
struct Adopted {
    committed: Committed,
    /// Whether this check wrote the build.
    recorded: bool,
}

#[cfg(test)]
thread_local! {
    static BEFORE_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs a test's hook between reading the info file and taking the lock.
fn before_lock() {
    #[cfg(test)]
    if let Some(hook) = BEFORE_LOCK.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

/// Records the build `goggame-<id>.info` names when it differs from the
/// committed one, since another tool (GOG Galaxy, Heroic) updated the game.
/// Only a Galaxy-tracked Windows game with no journal is adopted, and only
/// when the file's `gameId` is the game's own. Returns the committed state
/// to use from here on, or None when `state` stands.
///
/// The file is read once to skip the lock when it has nothing new. Under the
/// exclusive state lock the state is read again and the file once more, and
/// the build is written only if the committed build is still the one `state`
/// had. A check that lost a race with another one therefore never writes an
/// older build over a newer one, and uses what is committed now.
fn adopt_build(paths: &Paths, state: &FolderState) -> Result<Option<Adopted>> {
    let (Some(committed), None) = (&state.committed, &state.journal) else {
        return Ok(None);
    };
    let seen = base_build(committed);
    let Some(build) = info_build(&state.folder, committed.gog_id) else {
        return Ok(None);
    };
    if seen == Some(build.as_str()) {
        return Ok(None);
    }
    before_lock();
    let lock = FolderLock::exclusive(paths, &state.folder, &|| false)?;
    let Some(current) = lock.read()? else {
        return Ok(None);
    };
    let Some(mut committed) = current.committed.clone() else {
        return Ok(None);
    };
    if current.journal.is_some()
        || current.owner != state.owner
        || committed.gog_id != state.gog_id().unwrap_or_default()
        || committed.track != Track::Galaxy
        || committed.platform != Platform::Windows
    {
        return Ok(None);
    }
    if base_build(&committed) != seen {
        return Ok(Some(Adopted {
            committed,
            recorded: false,
        }));
    }
    let gog_id = committed.gog_id;
    let Some(build) = info_build(&state.folder, gog_id) else {
        return Ok(None);
    };
    let Some(base) = committed
        .products
        .iter_mut()
        .find(|product| product.id == gog_id)
    else {
        return Ok(None);
    };
    if base.build.as_deref() == Some(&build) {
        return Ok(None);
    }
    base.build = Some(build);
    lock.commit(committed.clone())?;
    // The stored manifest was of the build just replaced. The info file's
    // hash would catch it too, but an older wineo that adopts a build
    // can't drop it, and this one can.
    if let Some(mut record) = galaxy::record::read(&lock)?
        && record.installed.take().is_some()
    {
        galaxy::record::write(&lock, &record)?;
    }
    Ok(Some(Adopted {
        committed,
        recorded: true,
    }))
}

/// What `adopt_build` would use, without the lock and without writing: the
/// committed state with the build `goggame-<id>.info` names.
fn seen_build(state: &FolderState) -> Option<Adopted> {
    let (Some(committed), None) = (&state.committed, &state.journal) else {
        return None;
    };
    let build = info_build(&state.folder, committed.gog_id)?;
    if base_build(committed) == Some(build.as_str()) {
        return None;
    }
    let mut committed = committed.clone();
    committed
        .products
        .iter_mut()
        .find(|product| product.id == committed.gog_id)?
        .build = Some(build);
    Some(Adopted {
        committed,
        recorded: false,
    })
}

fn base_build(committed: &Committed) -> Option<&str> {
    committed
        .products
        .iter()
        .find(|product| product.id == committed.gog_id)
        .and_then(|base| base.build.as_deref())
}

/// The `buildId` in the folder's `goggame-<id>.info`, when the file is for
/// this game and names one.
fn info_build(folder: &Path, gog_id: u64) -> Option<String> {
    let text = fs::read_to_string(folder.join(format!("goggame-{gog_id}.info"))).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    if id_text(root.get("gameId")?) != gog_id.to_string() {
        return None;
    }
    Some(id_text(root.get("buildId")?)).filter(|build| !build.is_empty())
}

mod apply;
pub mod galaxy;
pub mod install;
mod present;
mod publish;
pub mod windows;

pub use apply::{Options, Pending, Plan, PlannedProduct, Progress, Updated, apply, plan};
pub use present::{Reporter, ReviewLabel};
pub use publish::SavedAside;

#[cfg(test)]
mod tests;
