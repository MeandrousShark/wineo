//! The emulated games page's work off the UI thread: finding emulators and
//! cores, listing games, planning, and importing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::emulator::{self, Core, Emulator, Install};
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::rom_import::{self, Applying, Batch, BatchReport, Emulating, Entry, Listing, Scan};

use crate::flow::artwork::Fetcher;

pub(super) const PORTABLE_REFUSAL: &str =
    "Emulated games can't be imported into a portable folder.";

/// Where emulators and RetroArch's cores are looked for: the PATH a launch
/// would use, Flatpak's export folders, and the root the system folders
/// hang off. Tests pass their own, so they never look at the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Host {
    pub(crate) path_var: OsString,
    pub(crate) exports: Vec<PathBuf>,
    pub(crate) system_root: PathBuf,
}

impl Host {
    fn system(paths: &Paths, global: &GlobalConfig) -> Self {
        let system_root = PathBuf::from("/");
        Self {
            path_var: launch::native_path_var(&global.env),
            exports: emulator::flatpak_exports(paths, &system_root),
            system_root,
        }
    }

    fn cores(&self, paths: &Paths, install: &Install) -> Vec<Core> {
        emulator::cores(&emulator::core_folders(paths, install, &self.system_root))
    }
}

/// What runs the games, before a RetroArch core is picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Chosen {
    BuiltIn(Emulator, Install),
    Custom {
        exe: String,
        args: String,
        extensions: Vec<String>,
        system: Option<String>,
    },
}

impl Chosen {
    pub(super) fn retroarch(&self) -> Option<&Install> {
        match self {
            Self::BuiltIn(Emulator::RetroArch, install) => Some(install),
            _ => None,
        }
    }

    /// Whether the games can start in fullscreen. Dolphin has no flag for
    /// it, and a custom emulator's arguments are its own.
    pub(super) fn has_fullscreen(&self) -> bool {
        matches!(self, Self::BuiltIn(emulator, _) if emulator.has_fullscreen_flag())
    }

    /// The systems a game whose file could be either is chosen from.
    pub(super) fn systems(&self) -> &'static [&'static str] {
        match self {
            Self::BuiltIn(emulator, _) => emulator.systems(),
            Self::Custom { .. } => &[],
        }
    }

    /// The Source its games get.
    pub(super) fn source(&self) -> &'static str {
        match self {
            Self::BuiltIn(emulator, _) => emulator.name(),
            Self::Custom { .. } => Emulator::Other.name(),
        }
    }

    pub(super) fn scan(&self, core: Option<&Core>) -> Scan {
        match (self, core) {
            (Self::BuiltIn(Emulator::RetroArch, _), Some(core)) => Scan::RetroArch(core.clone()),
            (Self::BuiltIn(emulator, _), _) => Scan::BuiltIn(*emulator),
            (
                Self::Custom {
                    extensions, system, ..
                },
                _,
            ) => Scan::Custom {
                extensions: extensions.clone(),
                system: system.clone(),
            },
        }
    }

    pub(super) fn emulating(&self, core: Option<&Core>, fullscreen: bool) -> Option<Emulating> {
        Some(match self {
            Self::BuiltIn(Emulator::RetroArch, install) => Emulating::RetroArch {
                install: install.clone(),
                core: core?.clone(),
                fullscreen,
            },
            Self::BuiltIn(emulator, install) => Emulating::BuiltIn {
                emulator: *emulator,
                install: install.clone(),
                fullscreen,
            },
            Self::Custom { exe, args, .. } => Emulating::Custom {
                exe: exe.clone(),
                args: args.clone(),
            },
        })
    }
}

/// An install as the emulator page and the review name it.
pub(super) fn install_label(emulator: Emulator, install: &Install) -> String {
    match install {
        Install::Path { name, .. } => format!("{} ({name})", emulator.name()),
        Install::Flatpak { .. } => format!("{} (Flatpak)", emulator.name()),
        Install::File(path) => format!("{} ({})", emulator.name(), path.display()),
    }
}

/// Where an install is, under its label.
pub(super) fn install_detail(emulator: Emulator, install: &Install) -> String {
    match install {
        Install::Path { found, .. } => found.display().to_string(),
        Install::Flatpak { .. } => emulator.flatpak_id().unwrap_or_default().to_owned(),
        Install::File(path) => path.display().to_string(),
    }
}

fn core_folder(core: &Core) -> String {
    core.path
        .parent()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default()
}

/// A core as the core page shows it, with its folder when another core has
/// the same id, like a distro core and one RetroArch downloaded.
pub(super) fn core_label(core: &Core, cores: &[Core]) -> String {
    let label = if core.name() == core.file_name() {
        core.name().to_owned()
    } else {
        format!("{} ({})", core.name(), core.id())
    };
    let shared = cores.iter().filter(|other| other.id() == core.id()).count() > 1;
    if shared {
        format!("{label} in {}", core_folder(core))
    } else {
        label
    }
}

/// What the emulator page lists.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    pub(super) host: Host,
    /// Each install found, PATH before Flatpak, in table order.
    pub(super) installs: Vec<(Emulator, Install)>,
    /// Whether any kind is on under Art to fetch.
    pub(super) art_enabled: bool,
}

/// The emulators installed, looked for with `host`, or the host's own when
/// None.
pub(crate) fn find(paths: &Paths, host: Option<Host>) -> Result<Found, String> {
    if paths.portable_root().is_some() {
        return Err(PORTABLE_REFUSAL.to_owned());
    }
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let host = host.unwrap_or_else(|| Host::system(paths, &global));
    let installs = Emulator::BUILT_IN
        .into_iter()
        .flat_map(|emulator| {
            emulator::installs(emulator, &host.path_var, &host.exports)
                .into_iter()
                .map(move |install| (emulator, install))
        })
        .collect();
    Ok(Found {
        host,
        installs,
        art_enabled: !global.art.fetch.kinds().is_empty(),
    })
}

/// A picked file as the built-in `emulator`.
pub(super) fn pick_file(
    paths: &Paths,
    host: &Host,
    emulator: Emulator,
    file: &str,
) -> Result<Chosen, String> {
    let file = file.trim();
    if file.is_empty() {
        return Err("Enter the emulator's file.".to_owned());
    }
    emulator::pick_file(paths, emulator, &host.exports, file)
        .map(|install| Chosen::BuiltIn(emulator, install))
        .map_err(|error| error.to_string())
}

/// What the Custom page's fields make, checked the way the import will
/// check them. The command is checked again when the games are planned.
pub(super) fn custom(
    paths: &Paths,
    host: &Host,
    exe: &str,
    args: &str,
    extensions: &str,
    system: &str,
) -> Result<Chosen, String> {
    rom_import::check_custom_command(paths, exe, &host.path_var).map_err(|e| format!("{e:#}"))?;
    let exe = exe.trim();
    emulator::custom_target(exe, args, Path::new("/game")).map_err(|e| e.to_string())?;
    let extensions: Vec<String> = extensions
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|extension| extension.trim().trim_start_matches('.').to_owned())
        .filter(|extension| !extension.is_empty())
        .collect();
    if extensions.is_empty() {
        return Err("Give at least one file type for the emulator's games.".to_owned());
    }
    let system = rom_import::custom_system(system).map_err(|e| e.to_string())?;
    Ok(Chosen::Custom {
        exe: exe.to_owned(),
        args: args.to_owned(),
        extensions,
        system,
    })
}

/// The folder or file typed, with `~` expanded, if it's there.
pub(super) fn folder(paths: &Paths, typed: &str) -> Result<PathBuf, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("Enter a folder or file of games.".to_owned());
    }
    let path = paths.absolute(typed).map_err(|error| error.to_string())?;
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("{} doesn't exist.", path.display()))
    }
}

/// RetroArch's cores, for `install`, that play something at `folder`.
pub(super) fn matching_cores(
    paths: &Paths,
    host: &Host,
    install: &Install,
    folder: &Path,
) -> Result<Vec<Core>, String> {
    let cores = host.cores(paths, install);
    if cores.is_empty() {
        return Err("No RetroArch cores were found.".to_owned());
    }
    let matching =
        rom_import::matching_cores(paths, &cores, folder).map_err(|e| format!("{e:#}"))?;
    if matching.is_empty() {
        return Err(format!(
            "No installed RetroArch core plays the files in {}.",
            folder.display()
        ));
    }
    Ok(matching)
}

pub(super) fn listing(paths: &Paths, scan: &Scan, folder: &Path) -> Result<Listing, String> {
    rom_import::listing(paths, scan, folder).map_err(|error| format!("{error:#}"))
}

pub(super) fn plan(
    paths: &Paths,
    emulating: &Emulating,
    entries: Vec<Entry>,
    system: Option<&str>,
) -> Result<Batch, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    rom_import::plan_batch(paths, &global, emulating, entries, system)
        .map_err(|error| format!("{error:#}"))
}

/// Imports the batch, fetching each new game's art with `fetch` when given.
/// Art never undoes an import, so its problems are warnings.
pub(super) fn run_import(
    paths: &Paths,
    batch: Batch,
    token: &Token,
    fetch: Option<&Fetcher>,
    mut progress: impl FnMut(String),
) -> Result<BatchReport, String> {
    let count = batch.ready.len();
    let mut position = 0;
    let mut notes = Vec::new();
    let mut report = rom_import::apply_batch(paths, batch, token, |applying| {
        match applying {
            Applying::Starting(plan) => {
                position += 1;
                progress(if count > 1 {
                    format!("({position} of {count}) Importing {}...", plan.title)
                } else {
                    format!("Importing {}...", plan.title)
                });
            }
            Applying::Imported(plan, applied) => {
                if let Some(fetch) = fetch {
                    let name = &applied.environment.name;
                    let named = |note: String| format!("{name}: {note}");
                    match fetch(paths, name, &plan.title, applied.identity, token).art {
                        Ok(fetched) => notes.extend(fetched.into_iter().map(named)),
                        Err(_) if token.is_canceled() => {
                            notes.push(named("Artwork canceled".to_owned()))
                        }
                        Err(error) if error.names_environment() => notes.push(error.note()),
                        Err(error) => notes.push(named(error.note())),
                    }
                }
            }
            Applying::AlreadyImported(..) => {}
        }
        Ok(())
    })
    .map_err(|error| format!("{error:#}"))?;
    report.warnings.extend(notes);
    Ok(report)
}

/// The finish page's lines, counted like `wineo import roms --all`.
pub(super) fn report_lines(report: &BatchReport) -> Vec<String> {
    let noun = if report.total == 1 { "game" } else { "games" };
    let mut lines = vec![format!(
        "Imported {} of {} {noun}.",
        report.done.len(),
        report.total
    )];
    if !report.already.is_empty() {
        lines.push(format!(
            "{} already imported: {}",
            report.already.len(),
            report.already.join(", ")
        ));
    }
    if !report.failures.is_empty() {
        let names: Vec<&str> = report
            .failures
            .iter()
            .map(|failure| failure.name.as_deref().unwrap_or(&failure.title))
            .collect();
        lines.push(format!(
            "{} failed: {}",
            report.failures.len(),
            names.join(", ")
        ));
    }
    lines
}
