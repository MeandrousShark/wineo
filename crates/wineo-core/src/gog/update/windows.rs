//! GOG's Windows installers: Inno Setup programs run silently in the game's
//! Wine or Proton prefix, into the game folder.
//!
//! An Inno installer runs its own scripts and programs, so its package
//! doesn't say every file it writes. Its journal names only the folder,
//! nothing is ever deleted after a failed run, and finishing an interrupted
//! install or update runs the full installers again. A run is judged by its exit code;
//! afterwards the game's `goggame-<id>.info` says which language it
//! installed in and how it starts.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::Value;

use super::UpdateError;
use super::apply::{Located, free_space, remove_packages};
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::create::{self, CreateEvent, CreateRequest, Hooks, Step, Stepped};
use crate::environment::Environment;
use crate::gog::folder::{
    Committed, FolderLock, Platform, ProductStatus, Target, Track, failpoint,
};
use crate::gog::language;
use crate::gog::migrate;
use crate::gog::package::{self, PackageError, SpaceNeeded};
use crate::installer::{self, Installer, Outcome, Output};
use crate::operation::Guard;
use crate::paths::Paths;
use crate::supervisor::Watched;
use crate::targets::{self, Batch, Spec, Written};

/// The target a new game's environment gets, as a native one does.
pub(super) const TARGET: &str = "game";

/// Refusals and failures a frontend may want to tell apart. Every one after
/// the folder's journal is written leaves it, and the files the installer
/// wrote, for `wineo update` or installing again to finish.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("Installing a Windows game needs a graphical session. Run this from your desktop.")]
    NoGraphicalSession,
    /// GOG's installer for a product isn't one setup program and its parts.
    #[error(
        "{label}'s Windows installer has {count} setup programs, so wineo can't tell which to run."
    )]
    NoSetupProgram { label: String, count: usize },
    /// Windows installers don't list what they unpack to, so `needed` is an
    /// estimate.
    #[error(
        "Not enough space: needs about {} (an estimate, twice the installers' size), {} free on {place}.",
        crate::units::size(*needed),
        crate::units::size(*free)
    )]
    NoSpace {
        needed: u64,
        free: u64,
        place: String,
    },
    /// Exit code 1, 3, or 7: setup never started changing files. With
    /// `wizard`, running the installer's own wizard instead may get further.
    #[error("{label}'s installer stopped before installing anything (exit code {code}).")]
    NotStarted {
        product: u64,
        label: String,
        code: i32,
        wizard: bool,
    },
    /// Exit code 2: canceled before anything changed.
    #[error("{label}'s installer was canceled before installing anything.")]
    Canceled { product: u64, label: String },
    /// Any other end: it may have changed files.
    #[error("{label}'s installer stopped partway ({detail}). Finish it with wineo update {name}.")]
    Partway {
        product: u64,
        label: String,
        name: String,
        detail: String,
    },
}

/// How a setup run ended, read from Inno Setup's exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupExit {
    Success,
    /// 1, 3, or 7: setup failed to start, or stopped before it began
    /// installing, so it never changed a file.
    NotStarted(i32),
    /// 2: canceled before anything changed.
    Canceled,
    /// Anything else, or no code at all: it may have stopped partway.
    Partway(Option<i32>),
}

impl SetupExit {
    pub fn of(exit_code: Option<i32>) -> Self {
        match exit_code {
            Some(0) => Self::Success,
            Some(code @ (1 | 3 | 7)) => Self::NotStarted(code),
            Some(2) => Self::Canceled,
            other => Self::Partway(other),
        }
    }
}

/// What an install reports besides download progress.
#[derive(Debug)]
pub enum Event {
    /// Creating the environment and its prefix.
    Create(CreateEvent),
    /// A product's installer is starting. `interactive` when it shows its
    /// own wizard.
    Installing { product: u64, interactive: bool },
    /// The running installer's supervision notices.
    Watched(Watched),
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static GRAPHICAL_SESSION: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Makes Windows installs on this thread see a graphical session, or not,
/// whatever the environment says, so a frontend's tests needn't set
/// `DISPLAY`. None goes back to asking the environment.
#[cfg(any(test, feature = "test-support"))]
pub fn set_graphical_session(graphical: Option<bool>) {
    GRAPHICAL_SESSION.with(|cell| cell.set(graphical));
}

/// Whether a Windows installer has a session to show its windows in.
pub(super) fn check_graphical() -> Result<()> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(graphical) = GRAPHICAL_SESSION.with(std::cell::Cell::get) {
        return if graphical {
            Ok(())
        } else {
            Err(SetupError::NoGraphicalSession.into())
        };
    }
    #[cfg(test)]
    if let Some(graphical) = tests::graphical() {
        return if graphical {
            Ok(())
        } else {
            Err(SetupError::NoGraphicalSession.into())
        };
    }
    if crate::launch::graphical_session() {
        Ok(())
    } else {
        Err(SetupError::NoGraphicalSession.into())
    }
}

/// `path` as Wine sees it through its `Z:` drive, fully qualified.
fn z_path(path: &Path) -> Result<String> {
    let Some(text) = path.to_str() else {
        bail!("{} isn't a path Wine can be given", path.display());
    };
    Ok(format!("Z:{}", text.replace('/', "\\")))
}

/// The arguments and variables an Inno installer runs with. Each argument is
/// passed whole; Wine quotes one with spaces when it builds the Windows
/// command line, which Inno reads like `/DIR="Z:\..."`.
///
/// `code` is GOG's code for the installer's language and `tag` the locale
/// tag asked for. `/LANG=` picks one of the installer's own languages when
/// the table knows its usual name, and the locale is what Inno falls back
/// to when it doesn't have that name. `interactive` leaves out the flags
/// that hide the wizard and its messages, for running its own wizard.
pub(super) fn setup_args(
    folder: &Path,
    log: &Path,
    code: &str,
    tag: &str,
    interactive: bool,
) -> Result<installer::Args> {
    let mut args: Vec<String> = Vec::new();
    if !interactive {
        args.extend(
            ["/SP-", "/SILENT", "/SUPPRESSMSGBOXES"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    args.push("/NORESTART".to_owned());
    if !interactive {
        args.push("/NOCANCEL".to_owned());
    }
    args.push(format!("/LOG={}", z_path(log)?));
    args.push(format!("/DIR={}", z_path(folder)?));
    if let Some(name) = language::inno_language(code) {
        args.push(format!("/LANG={name}"));
    }
    let mut env = BTreeMap::new();
    if let Some(locale) = language::posix_locale(tag, code) {
        env.insert("LANG".to_owned(), locale.clone());
        env.insert("LC_ALL".to_owned(), locale);
    }
    Ok(installer::Args { args, env })
}

/// Each product's setup program among its downloaded files: the one `.exe`,
/// with every part beside it under its own name, which is how it finds them.
pub(super) fn setup_programs(
    located: &[Located],
) -> BTreeMap<u64, Result<PathBuf, (String, usize)>> {
    let mut products: BTreeMap<u64, Vec<&Located>> = BTreeMap::new();
    for entry in located {
        products.entry(entry.0.product).or_default().push(entry);
    }
    products
        .into_iter()
        .map(|(product, files)| {
            let label = files[0].0.label.clone();
            let exes: Vec<&PathBuf> = files
                .iter()
                .map(|(_, path)| path)
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                })
                .collect();
            let found = match exes.as_slice() {
                [exe] if files.iter().all(|(_, path)| path.parent() == exe.parent()) => {
                    Ok((*exe).clone())
                }
                // The cache keeps an installer's files in one folder, so a
                // part elsewhere means it isn't one installer.
                [_] => Err((label, 0)),
                others => Err((label, others.len())),
            };
            (product, found)
        })
        .collect()
}

/// An estimate of the space a Windows install takes: twice its installers'
/// size, since they don't list what they unpack to.
pub(super) fn estimate(installers: u64) -> u64 {
    installers.saturating_mul(2)
}

/// Refuses packages whose estimated installed size doesn't fit on the game
/// folder's filesystem.
pub(super) fn check_space(paths: &Paths, folder: &Path, located: &[Located]) -> Result<()> {
    let total: u64 = located.iter().map(|(file, _)| file.size).sum();
    check_estimate(paths, folder, estimate(total))
}

/// Refuses an estimate that doesn't fit, saying it's one.
pub(super) fn check_estimate(paths: &Paths, folder: &Path, needed: u64) -> Result<()> {
    let needed = SpaceNeeded {
        unpacked: needed,
        assembled: 0,
    };
    match package::check_space_with(paths, folder, needed, free_space(folder)?) {
        Ok(()) => Ok(()),
        Err(PackageError::NoSpace {
            needed,
            free,
            place,
        }) => Err(SetupError::NoSpace {
            needed,
            free,
            place,
        }
        .into()),
        Err(error) => Err(error.into()),
    }
}

/// One Windows install's runs, as the install or a recovery hands it over:
/// the folder's state lock held and its journal written.
pub(super) struct Job<'a> {
    pub(super) name: &'a str,
    pub(super) title: &'a str,
    /// Updated with each DLC that fails before changing anything.
    pub(super) target: &'a mut Target,
    /// The setup program of each product that runs, in the target's order.
    /// Other products are left as they are.
    pub(super) setups: &'a BTreeMap<u64, PathBuf>,
    pub(super) label: &'a dyn Fn(u64) -> String,
    /// The environment's operation lock, holding `prefix` too, taken before
    /// the folder's and kept to the commit.
    pub(super) guard: Guard,
    /// The prefix, resolved, that `guard` holds.
    pub(super) prefix: &'a Path,
    /// The environment to create first, when it has no config yet.
    pub(super) create: Option<CreateRequest>,
    /// The locale tag asked for, for the installers' locale.
    pub(super) tag: &'a str,
    /// What the review calls the language asked for, like `English`.
    pub(super) requested: &'a str,
    /// Run the base game's installer as its own wizard.
    pub(super) wizard: bool,
    /// Whether a default target gets a desktop entry.
    pub(super) desktop: bool,
    /// What an update or reinstall starts from. A DLC whose installer
    /// stopped before installing anything keeps its entry here.
    pub(super) committed: Option<&'a Committed>,
    /// Whether this attempt wrote the journal, so nothing ran before it.
    /// Then an update whose game installer stopped before installing
    /// anything drops the journal, since the game is as committed, and a DLC
    /// whose installer did is failed or keeps its old entry. Otherwise an
    /// earlier attempt may have changed files, so either keeps the journal.
    pub(super) fresh: bool,
}

/// What a Windows install found once its installers ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Done {
    /// The exe of the default target it added from the game's info file.
    pub(super) target: Option<PathBuf>,
    /// GOG's code for the language the game's info file reports.
    pub(super) reported_language: Option<String>,
}

/// Runs every installer in `job`, reads the game's info file, adds a default
/// target from it, and commits. Each run takes the environment's own locks,
/// so the caller holds none but the folder's.
pub(super) fn install(
    paths: &Paths,
    lock: FolderLock,
    job: Job,
    token: &Token,
    events: &mut dyn FnMut(Event),
    warnings: &mut Vec<String>,
) -> Result<Done> {
    let Job {
        name,
        title,
        target,
        setups,
        label,
        guard,
        prefix,
        mut create,
        tag,
        requested,
        wizard,
        desktop,
        committed,
        fresh,
    } = job;
    // Passed through creation and each installer and back, so the
    // environment and its prefix stay reserved until the commit.
    let mut guard = Some(guard);
    let folder = lock.folder().to_owned();
    let global = GlobalConfig::load(paths)?;
    let logs = paths.runtime_dir(name);
    let output = logs.join("gog-install.log");
    let stop = Duration::from_secs(global.stop_timeout.into());
    let gog_id = target.gog_id;
    let order: Vec<u64> = target
        .products
        .iter()
        .filter(|product| {
            product.status == ProductStatus::Installed && setups.contains_key(&product.id)
        })
        .map(|product| product.id)
        .collect();
    for id in order {
        if token.is_canceled() {
            return Err(UpdateError::Canceled { journal: true }.into());
        }
        let exe = &setups[&id];
        let exe_text = exe.to_string_lossy().into_owned();
        let base = id == gog_id;
        let interactive = base && wizard;
        let code = target
            .products
            .iter()
            .find(|product| product.id == id)
            .and_then(|product| product.language.clone())
            .unwrap_or_else(|| target.language.clone());
        let args = setup_args(
            &folder,
            &logs.join(format!("gog-setup-{id}.log")),
            &code,
            tag,
            interactive,
        )?;
        let installing = || Event::Installing {
            product: id,
            interactive,
        };
        let created = if base { create.take() } else { None };
        // Creating the environment comes first, so it says so after.
        if created.is_none() {
            events(installing());
        }
        let held = guard.take().expect("each run hands the locks back");
        let (outcome, resolved) = match created {
            Some(request) => {
                let (outcome, resolved, back) = run_created(
                    paths,
                    request,
                    held,
                    (&exe_text, &args),
                    &output,
                    token,
                    events,
                    installing(),
                )?;
                guard = Some(back);
                (outcome?, resolved)
            }
            None => {
                let installer = Installer::prepare_args_with(
                    held, paths, &global, name, &exe_text, &args, prefix,
                )?;
                let (outcome, back) =
                    installer.run_keeping(token, stop, Output::Log(&output), |watched| {
                        events(Event::Watched(watched))
                    });
                guard = Some(back);
                (outcome?, true)
            }
        };
        let partway = |detail: String| SetupError::Partway {
            product: id,
            label: label(id),
            name: name.to_owned(),
            detail,
        };
        if !resolved {
            return Err(partway(format!(
                "something it started may still be running; stop it with wineo stop {name}"
            ))
            .into());
        }
        let exit = match outcome {
            Outcome::Canceled { .. } => return Err(UpdateError::Canceled { journal: true }.into()),
            Outcome::Unresolved { error, .. } => return Err(partway(error.to_string()).into()),
            Outcome::Finished { exit_code } => SetupExit::of(exit_code),
        };
        match exit {
            SetupExit::Success => {}
            SetupExit::Partway(code) => {
                let detail = match code {
                    Some(code) => format!("exit code {code}"),
                    None => "no exit code".to_owned(),
                };
                return Err(partway(detail).into());
            }
            SetupExit::NotStarted(code) if base => {
                give_up(&lock, committed.filter(|_| fresh))?;
                return Err(SetupError::NotStarted {
                    product: id,
                    label: label(id),
                    code,
                    wizard: !wizard,
                }
                .into());
            }
            SetupExit::Canceled if base => {
                give_up(&lock, committed.filter(|_| fresh))?;
                return Err(SetupError::Canceled {
                    product: id,
                    label: label(id),
                }
                .into());
            }
            // An earlier attempt may have run this DLC's installer partway,
            // so it isn't known to be as it was.
            SetupExit::NotStarted(code) if !fresh => {
                return Err(SetupError::NotStarted {
                    product: id,
                    label: label(id),
                    code,
                    wizard: false,
                }
                .into());
            }
            SetupExit::Canceled if !fresh => {
                return Err(SetupError::Canceled {
                    product: id,
                    label: label(id),
                }
                .into());
            }
            // A DLC that changed nothing leaves the game launchable without
            // it.
            SetupExit::NotStarted(_) | SetupExit::Canceled => {
                let why = match exit {
                    SetupExit::NotStarted(code) => {
                        format!(
                            "its installer stopped before installing anything (exit code {code})"
                        )
                    }
                    _ => "its installer was canceled before installing anything".to_owned(),
                };
                let product = target
                    .products
                    .iter_mut()
                    .find(|product| product.id == id)
                    .expect("in the target");
                // A DLC already installed is still there as it was.
                match committed.and_then(|committed| {
                    committed
                        .products
                        .iter()
                        .find(|old| old.id == id && old.status == ProductStatus::Installed)
                }) {
                    Some(old) => {
                        warnings.push(format!("{} wasn't updated: {why}.", label(id)));
                        *product = old.clone();
                    }
                    None => {
                        warnings.push(format!("{} failed and wasn't installed: {why}.", label(id)));
                        product.status = ProductStatus::Failed;
                        product.version = None;
                    }
                }
            }
        }
        failpoint(if base {
            "install.windows.base.after"
        } else {
            "install.windows.dlc.after"
        })?;
    }

    // Each product's info file says which language it installed in. One
    // that isn't the language asked for is recorded and said, without
    // guessing why.
    let folder_language = target.language.clone();
    for product in &mut target.products {
        product.reported_language = None;
        if product.status != ProductStatus::Installed {
            continue;
        }
        let Some((code, shown)) = read_info(&folder, product.id)
            .as_ref()
            .and_then(reported_language)
        else {
            continue;
        };
        let asked = product.language.as_deref().unwrap_or(&folder_language);
        if code == asked {
            continue;
        }
        let asked_shown = if asked == folder_language {
            requested
        } else {
            asked
        };
        warnings.push(if product.id == gog_id {
            format!("Requested {asked_shown}; the installed game reports {shown}.")
        } else {
            format!(
                "Requested {asked_shown}; {} reports {shown}.",
                label(product.id)
            )
        });
        product.reported_language = Some(code);
    }
    let info = read_info(&folder, gog_id);
    let reported = info.as_ref().and_then(reported_language);
    let launch = info.as_ref().and_then(|root| {
        migrate::windows_launch(&folder, folder.join(format!("goggame-{gog_id}.info")), root).ok()
    });
    // The last moment to stop before the config changes, and then before
    // the commit, under the folder's and the environment's locks.
    #[cfg(test)]
    tests::before_target();
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: true }.into());
    }
    // An update leaves the targets you have as they are.
    let added = match (&launch, committed) {
        (Some(launch), None) => add_target(paths, name, title, launch, desktop)?,
        _ => None,
    };
    // What the installers wrote reaches the disk before a state saying it's
    // installed does; a failure here leaves the journal.
    crate::import::sync_filesystem(&folder)?;
    crate::import::sync_filesystem(prefix)?;
    if token.is_canceled() {
        return Err(UpdateError::Canceled { journal: true }.into());
    }
    failpoint("install.windows.commit.before")?;
    lock.commit(Committed {
        source: None,
        gog_id,
        platform: Platform::Windows,
        language: target.language.clone(),
        track: Track::Installer,
        products: target.products.clone(),
        files: Vec::new(),
    })?;
    failpoint("install.windows.commit.after")?;
    drop(lock);
    drop(guard);
    let products: Vec<u64> = target.products.iter().map(|product| product.id).collect();
    remove_packages(paths, &folder, &products, warnings);
    Ok(Done {
        target: added,
        reported_language: reported.map(|(code, _)| code),
    })
}

/// The locale tag installers in GOG's language `code` run with when nobody
/// is asking for one, as when updating, and how warnings name the language:
/// the `gog.language` setting when it is that language, else the code
/// itself, named by GOG's `full` name for it when known.
pub(super) fn asked_for(paths: &Paths, code: &str, full: Option<&str>) -> Result<(String, String)> {
    let global = GlobalConfig::load(paths)?;
    let setting = global.gog_language();
    let tag = if language::installer_code(setting).as_deref() == Ok(code) {
        setting.to_owned()
    } else {
        code.to_owned()
    };
    let name = full
        .filter(|full| !full.trim().is_empty())
        .map_or_else(|| tag.clone(), str::to_owned);
    Ok((tag, name))
}

/// After an update's game installer stopped before installing anything,
/// drops the journal this attempt wrote, leaving the game as `committed`
/// says and launchable. Without `committed` the journal stays for `wineo
/// update` or installing again.
fn give_up(lock: &FolderLock, committed: Option<&Committed>) -> Result<()> {
    let Some(committed) = committed else {
        return Ok(());
    };
    let Some(mut state) = lock.read()? else {
        return Ok(());
    };
    if state.committed.as_ref() != Some(committed) {
        return Ok(());
    }
    state.journal = None;
    lock.write(&state)
}

/// Creates the environment and runs the base game's installer in it under
/// the same lock, returning how it ended and whether everything it started
/// is known to have stopped. `installing` is reported once the environment
/// exists.
#[allow(clippy::too_many_arguments)]
fn run_created(
    paths: &Paths,
    request: CreateRequest,
    guard: Guard,
    (exe, args): (&str, &installer::Args),
    output: &Path,
    token: &Token,
    events: &mut dyn FnMut(Event),
    installing: Event,
) -> Result<(crate::Result<Outcome>, bool, Guard)> {
    let step = Step::GogInstaller {
        program: exe,
        args,
        output: Output::Log(output),
        token,
    };
    let mut hooks = Forward {
        events,
        installing: Some(installing),
    };
    let installed =
        create::create_and_install_held(paths, request, Some(step), guard, token, &mut hooks)?;
    let outcome = match installed.step {
        Stepped::Ran { outcome, resolved } => (outcome, resolved),
        Stepped::NotPrepared(error) => return Err(error.into()),
        Stepped::None => bail!("the installer wasn't run"),
        Stepped::Stopped(never) => match never {},
    };
    let guard = installed
        .guard
        .ok_or_else(|| anyhow::anyhow!("the installer let go of the environment's locks"))?;
    Ok((outcome.0, outcome.1, guard))
}

/// Passes creation's events on.
struct Forward<'a, 'b> {
    events: &'a mut (dyn FnMut(Event) + 'b),
    installing: Option<Event>,
}

impl Hooks for Forward<'_, '_> {
    type Stop = Infallible;

    fn event(&mut self, event: CreateEvent) {
        (self.events)(Event::Create(event));
    }

    fn created(&mut self, _created: &create::Created) -> Result<(), Self::Stop> {
        if let Some(installing) = self.installing.take() {
            (self.events)(installing);
        }
        Ok(())
    }

    fn watched(&mut self, watched: Watched) {
        (self.events)(Event::Watched(watched));
    }
}

/// The folder's `goggame-<id>.info`, when it is there and for this game.
pub(super) fn read_info(folder: &Path, gog_id: u64) -> Option<Value> {
    crate::gog::names::read_info(&folder.join(format!("goggame-{gog_id}.info")), gog_id)
}

/// GOG's code for the language an info file reports, from the locale tags it
/// lists, and how the file names it, like `French`.
fn reported_language(root: &Value) -> Option<(String, String)> {
    let tag = root
        .get("languages")
        .and_then(Value::as_array)?
        .iter()
        .find_map(Value::as_str)?;
    let code = language::installer_code(tag).ok()?;
    let shown = root
        .get("language")
        .and_then(Value::as_str)
        .filter(|shown| !shown.is_empty())
        .unwrap_or(tag)
        .to_owned();
    Some((code, shown))
}

/// Adds the game's play task as the default target, unless the environment
/// already has one by that name from an earlier attempt.
fn add_target(
    paths: &Paths,
    name: &str,
    title: &str,
    launch: &migrate::Launch,
    desktop: bool,
) -> Result<Option<PathBuf>> {
    let environment = Environment::load(paths, name)?;
    if environment.config.targets.contains_key(TARGET) {
        return Ok(None);
    }
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let mut batch = Batch::new(environment.config.targets.clone());
    batch.add(
        TARGET,
        Spec {
            exe: text(&launch.exe),
            args: launch.args.clone(),
            cwd: launch.cwd.as_deref().map(text),
            name: desktop.then(|| title.to_owned()),
            desktop,
        },
    )?;
    batch.set_default(Some(TARGET));
    match targets::write(paths, name, &batch)? {
        Written::Saved => Ok(Some(launch.exe.clone())),
        Written::Changed(_) => bail!("{name}'s config changed while its target was being added"),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::Cell;

    use super::*;

    thread_local! {
        static GRAPHICAL: Cell<Option<bool>> = const { Cell::new(None) };
        static BEFORE_TARGET: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Runs a test's hook once, after the last installer and before the
    /// default target is written.
    pub(in crate::gog::update) fn before_target() {
        if let Some(hook) = BEFORE_TARGET.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
    }

    pub(in crate::gog::update) fn set_before_target(hook: impl FnOnce() + 'static) {
        BEFORE_TARGET.with(|cell| *cell.borrow_mut() = Some(Box::new(hook)));
    }

    /// Whether a test says there is a graphical session. Tests run as if
    /// there were one unless they say otherwise.
    pub(in crate::gog::update) fn graphical() -> Option<bool> {
        Some(GRAPHICAL.with(Cell::get).unwrap_or(true))
    }

    pub(in crate::gog::update) fn set_graphical(graphical: bool) {
        GRAPHICAL.with(|cell| cell.set(Some(graphical)));
    }

    #[test]
    fn exit_codes_read_like_inno_setup_documents_them() {
        assert_eq!(SetupExit::of(Some(0)), SetupExit::Success);
        for code in [1, 3, 7] {
            assert_eq!(SetupExit::of(Some(code)), SetupExit::NotStarted(code));
        }
        assert_eq!(SetupExit::of(Some(2)), SetupExit::Canceled);
        for code in [4, 5, 6, 8, 9, -1] {
            assert_eq!(SetupExit::of(Some(code)), SetupExit::Partway(Some(code)));
        }
        assert_eq!(SetupExit::of(None), SetupExit::Partway(None));
    }

    #[test]
    fn a_silent_run_has_inno_flags_a_wine_path_and_the_locale() {
        let args = setup_args(
            Path::new("/home/me/Games/My Game"),
            Path::new("/state/rimworld/gog-setup-1.log"),
            "fr",
            "fr-CA",
            false,
        )
        .unwrap();
        assert_eq!(
            args.args,
            [
                "/SP-",
                "/SILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/NOCANCEL",
                "/LOG=Z:\\state\\rimworld\\gog-setup-1.log",
                "/DIR=Z:\\home\\me\\Games\\My Game",
                "/LANG=french",
            ]
        );
        assert_eq!(args.env["LANG"], "fr_CA.UTF-8");
        assert_eq!(args.env["LC_ALL"], "fr_CA.UTF-8");

        // The wizard keeps the folder, language, and log, and shows itself.
        let wizard = setup_args(Path::new("/g"), Path::new("/l"), "xx", "xx", true).unwrap();
        assert_eq!(wizard.args, ["/NORESTART", "/LOG=Z:\\l", "/DIR=Z:\\g"]);
        assert!(wizard.env.is_empty());
    }
}
