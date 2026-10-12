//! Whether a GOG game has an update on GOG, and, in `apply` and `publish`,
//! applying a native game's update.
//!
//! Status is computed from a folder's `[committed]` state, even while a
//! journal is present, so a half-finished update never hides what is
//! installed. Checking never touches the game's files. Its one write is
//! recording a build another tool installed (see `adopt_build`).
//!
//! State files are read without their lock: they are replaced atomically,
//! so a read is always a whole file, and a check mustn't leave lock files
//! behind.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

use super::account::Session;
use super::folder::{
    self, Committed, FolderError, FolderLock, FolderState, Platform, Product, ProductStatus, Track,
};
use super::migrate::{host_only, id_text};
use super::product;
use crate::environment::{self, Environment};
use crate::paths::Paths;

/// Whether a game is behind GOG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// JSON `none`.
    UpToDate,
    /// JSON `available`.
    Available,
    /// A newer Galaxy build exists, applied by plan 0051. JSON `galaxy`.
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
    /// An installer-tracked Windows game, applied by plan 0050.
    WindowsInstaller,
    /// A Galaxy-tracked game, applied by plan 0051.
    Galaxy,
    /// Why the game can't be compared, as a sentence.
    CantCompare(String),
}

impl Reason {
    /// The machine word for the first two, else the sentence.
    pub fn as_str(&self) -> &str {
        match self {
            Self::WindowsInstaller => "windows_installer",
            Self::Galaxy => "galaxy",
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
    /// Plan 0050 only.
    Failed,
    /// Plan 0050 only.
    Unavailable,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlcStatus {
    pub id: u64,
    /// None when GOG's details don't list this DLC.
    pub title: Option<String>,
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
    /// The base game's installed version, or build on the Galaxy track.
    pub installed: Option<String>,
    /// The base game's current installer version, or latest default-branch
    /// build, when it could be told.
    pub available: Option<String>,
    pub state: UpdateState,
    /// Whether this wineo can apply the update: a native game with one.
    pub applicable: bool,
    /// Set when the state is `Available` or `OnGog` for a game this wineo
    /// can't update, and when it is `CantCompare`.
    pub reason: Option<Reason>,
    /// A journal is present: the owner has an update to finish.
    pub unfinished: bool,
    pub warnings: Vec<String>,
    /// Every product after the base, in the order they were added.
    pub dlc: Vec<DlcStatus>,
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
    /// The same sentence `update --check` gives for these games.
    #[error("Updating Windows installer games comes in a later release.")]
    WindowsInstaller { name: String },
    #[error("Updating Galaxy builds comes in a later release.")]
    Galaxy { name: String },
    /// A journal of plan 0050's kinds, `install` or `repair`.
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

/// Checks the GOG game environment `name` runs. Needs a login for Galaxy
/// builds only; product details are public. With `refresh`, cached details
/// are fetched again.
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
    status_of(session, &paths, name, state, how)
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
        match status_of(session, &paths, &owner, state, how) {
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
                match status_of(session, &paths, &name, state, how) {
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

/// The `gog_id` an environment's config records, read as plain TOML. None
/// when the config is gone or records none.
pub fn recorded_gog_id(paths: &Paths, name: &str) -> Result<Option<String>, UpdateError> {
    let unloadable = |detail: String| UpdateError::Unloadable {
        name: name.to_owned(),
        detail,
    };
    let text = match fs::read_to_string(paths.environment_config(name)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unloadable(error.to_string())),
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
}

fn status_of(
    session: &mut Session,
    paths: &Paths,
    name: &str,
    state: FolderState,
    how: Check,
) -> Result<Status> {
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

    let compared = if installed.unfinished_install {
        Compared {
            state: UpdateState::CantCompare,
            installed: None,
            available: None,
            why: Some("The install hasn't finished.".to_owned()),
            dlc: Vec::new(),
        }
    } else {
        match (installed.platform, installed.track) {
            (Platform::Linux, Track::Installer) => {
                by_installer("linux", &installed, &details.product)
            }
            (Platform::Windows, Track::Installer) => {
                by_installer("windows", &installed, &details.product)
            }
            (Platform::Windows, Track::Galaxy) => by_build(session, &installed, &details.product)?,
            (Platform::Linux, Track::Galaxy) => cant(
                &installed,
                "GOG's Galaxy builds are only for Windows games.".to_owned(),
            ),
        }
    };

    let applicable =
        compared.state == UpdateState::Available && installed.platform == Platform::Linux;
    let reason = match compared.state {
        UpdateState::CantCompare => compared.why.map(Reason::CantCompare),
        UpdateState::OnGog => Some(Reason::Galaxy),
        UpdateState::Available if !applicable => Some(Reason::WindowsInstaller),
        UpdateState::Available | UpdateState::UpToDate => None,
    };
    Ok(Status {
        environment: name.to_owned(),
        gog_id: installed.gog_id,
        title: details.product.title,
        folder: state.folder,
        platform: installed.platform,
        track: installed.track,
        language: installed.language,
        installed: compared.installed,
        available: compared.available,
        state: compared.state,
        applicable,
        reason,
        unfinished,
        warnings,
        dlc: compared.dlc,
    })
}

/// A game that can't be compared, with its DLC listed as they are.
fn cant(installed: &Installed, why: String) -> Compared {
    Compared {
        state: UpdateState::CantCompare,
        installed: base(installed).and_then(|base| base.version.clone().or(base.build.clone())),
        available: None,
        why: Some(why),
        dlc: dlc(installed).map(|(dlc, _)| dlc).collect(),
    }
}

fn base(installed: &Installed) -> Option<&Product> {
    installed
        .products
        .iter()
        .find(|product| product.id == installed.gog_id)
}

/// Each product after the base, with its listing before any comparison.
fn dlc(installed: &Installed) -> impl Iterator<Item = (DlcStatus, &Product)> {
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
/// makes it Available.
fn by_installer(os: &str, installed: &Installed, details: &wineo_gog::Product) -> Compared {
    let language = &installed.language;
    let mut tally = Tally::default();

    let (base_installed, base_available) = match base(installed) {
        Some(base) if base.status == ProductStatus::Installed => {
            let label = title_or(&details.title, || format!("GOG game {}", installed.gog_id));
            let (verdict, available) =
                compare_one(os, &label, language, Some(details), base.version.as_deref());
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
    for (mut entry, product) in dlc(installed) {
        let gog = details.dlcs.iter().find(|dlc| dlc.id == product.id);
        entry.title = gog.map(|gog| gog.title.clone()).filter(|t| !t.is_empty());
        if product.status == ProductStatus::Installed {
            let label = title_or(entry.title.as_deref().unwrap_or(""), || {
                format!("DLC {}", product.id)
            });
            let (verdict, available) =
                compare_one(os, &label, language, gog, product.version.as_deref());
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

    Compared {
        state: tally.state,
        installed: base_installed,
        available: base_available,
        why: tally.why,
        dlc: listing,
    }
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

/// The Galaxy track: the base game's build against GOG's build list. The
/// latest default-branch build is the first one GOG lists with no branch,
/// since GOG lists newest first.
fn by_build(
    session: &mut Session,
    installed: &Installed,
    details: &wineo_gog::Product,
) -> Result<Compared> {
    let label = title_or(&details.title, || format!("GOG game {}", installed.gog_id));
    let id = installed.gog_id;
    let builds = session.call(|client, token| client.windows_builds(id, Some(token)))?;
    let latest = builds.items.iter().find(|build| build.branch.is_none());
    let current = base(installed).and_then(|base| base.build.clone());
    let (state, why) = match (&current, latest) {
        (None, _) => (
            UpdateState::CantCompare,
            Some(format!("The installed build of {label} isn't known.")),
        ),
        (_, None) => (
            UpdateState::CantCompare,
            Some(format!(
                "GOG lists no build of {label} on its default branch."
            )),
        ),
        (Some(current), Some(latest)) if *current == latest.build_id => {
            (UpdateState::UpToDate, None)
        }
        (Some(current), Some(_)) => {
            match builds.items.iter().find(|build| build.build_id == *current) {
                Some(build) if build.branch.is_none() => (UpdateState::OnGog, None),
                Some(build) => (
                    UpdateState::CantCompare,
                    Some(format!(
                        "Build {current} is on GOG's {} branch, not the default one.",
                        build.branch.as_deref().unwrap_or_default()
                    )),
                ),
                None => (
                    UpdateState::CantCompare,
                    Some(format!("Build {current} isn't one GOG lists.")),
                ),
            }
        }
    };
    let dlc = dlc(installed)
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
    Ok(Compared {
        state,
        installed: current,
        available: latest.map(|build| build.build_id.clone()),
        why,
        dlc,
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
mod publish;

pub use apply::{Options, Pending, Plan, PlannedProduct, Progress, Updated, apply, plan};
pub use publish::SavedAside;

#[cfg(test)]
mod tests;
