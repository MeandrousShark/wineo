//! `wineo import roms`: emulated games from a folder of ROMs, or one file,
//! each its own native environment.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap_complete::ArgValueCompleter;
use serde_json::{Value, json};
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::emulator::{self, Core, Emulator, Install};
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::rom_import::{
    self, Applying, Batch, BatchReport, Emulating, Entry, EntryState, EntrySystem, Listing, Plan,
    Scan,
};
use wineo_core::steamgriddb::Key;

use super::art::{Access, AutoFetch, KeyCheck, Pick};
use super::import::ReturnToSourceMenu;
use super::table::render_columns;
use super::transfer::Cancellation;
use crate::cancel;
use crate::picker;
use crate::prompt::{self, Canceled, Choice, NoInput, Prompt, Reply, Terminal, numbered};
use crate::style;
use crate::text;

/// Everything but `--json`, which `--emulators` takes alone.
const EMULATORS_CONFLICTS: [&str; 15] = [
    "path",
    "emulator",
    "exe",
    "custom",
    "args",
    "ext",
    "core",
    "system",
    "list",
    "game",
    "all",
    "yes",
    "no_fullscreen",
    "art",
    "dry_run",
];

#[derive(clap::Args, Default)]
#[command(group = clap::ArgGroup::new("json_mode").args(["list", "emulators"]).multiple(true))]
pub struct Args {
    /// A folder of games, or one game's file
    #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
    pub path: Option<String>,
    /// The emulator, by id, like dolphin or retroarch
    #[arg(
        long,
        value_name = "ID",
        add = ArgValueCompleter::new(crate::completion::emulators)
    )]
    pub emulator: Option<String>,
    /// With --emulator, run it from this file, like an AppImage
    #[arg(
        long,
        value_name = "PATH",
        requires = "emulator",
        value_hint = clap::ValueHint::FilePath
    )]
    pub exe: Option<String>,
    /// A custom emulator's command, a path or a name on PATH
    #[arg(
        long,
        value_name = "EXE",
        conflicts_with_all = ["emulator", "core"],
        requires_all = ["args", "ext"],
        value_hint = clap::ValueHint::CommandName
    )]
    pub custom: Option<String>,
    /// With --custom, its arguments in one string, with {rom} for the game
    #[arg(
        long,
        value_name = "ARGS",
        requires = "custom",
        allow_hyphen_values = true
    )]
    pub args: Option<String>,
    /// With --custom, its games' file types, like nsp,xci
    #[arg(long, value_name = "EXT", requires = "custom", value_delimiter = ',')]
    pub ext: Vec<String>,
    /// RetroArch's core, by id, name, file name, or path
    #[arg(
        long,
        value_name = "CORE",
        add = ArgValueCompleter::new(crate::completion::cores)
    )]
    pub core: Option<String>,
    /// The system for games whose file type doesn't tell, or a custom emulator's
    #[arg(
        long,
        value_name = "SYSTEM",
        add = ArgValueCompleter::new(crate::completion::systems)
    )]
    pub system: Option<String>,
    /// List the games without importing
    #[arg(long, conflicts_with_all = ["game", "all", "yes", "art", "dry_run"])]
    pub list: bool,
    /// Import one game, by its title, file name, or path
    #[arg(long, value_name = "NAME")]
    pub game: Option<String>,
    /// Import every game not imported yet
    #[arg(long, conflicts_with = "game")]
    pub all: bool,
    /// Skip the confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Don't start the games in fullscreen
    #[arg(long)]
    pub no_fullscreen: bool,
    /// Fetch art from SteamGridDB, using the top match for each title
    #[arg(long)]
    pub art: bool,
    /// Review the import without changing anything
    #[arg(long)]
    pub dry_run: bool,
    /// List the emulators wineo found, and RetroArch's cores
    #[arg(long, conflicts_with_all = EMULATORS_CONFLICTS)]
    pub emulators: bool,
    /// With --list or --emulators, print JSON
    #[arg(long, requires = "json_mode")]
    pub json: bool,
}

pub fn run(paths: &Paths, args: Args, first_question: bool) -> anyhow::Result<ExitCode> {
    // Core refuses too; this says so before anything is read.
    anyhow::ensure!(
        paths.portable_root().is_none(),
        "Emulated games can't be imported into a portable folder."
    );
    let global = GlobalConfig::load(paths)?;
    let host = Host::new(paths, &global);
    let result = if args.emulators {
        emulators(paths, &host, args.json)
    } else {
        Flow {
            paths,
            global: &global,
            host: &host,
            args: &args,
            prompt: None,
            first_question,
        }
        .run()
    };
    match result {
        Err(error) if canceled(&error) => {
            eprintln!("Import canceled.");
            Ok(cancel::exit_code())
        }
        other => other,
    }
}

fn canceled(error: &anyhow::Error) -> bool {
    cancel::prompted(error) || error.is::<wineo_core::import::Cancelled>() || cancel::core(error)
}

/// Where emulators and RetroArch's cores are looked for: the PATH a launch
/// would use, and the system folders.
pub(crate) struct Host {
    path_var: OsString,
    exports: Vec<PathBuf>,
    system_root: PathBuf,
}

impl Host {
    pub(crate) fn new(paths: &Paths, global: &GlobalConfig) -> Self {
        let system_root = test_system_root().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            path_var: launch::native_path_var(&global.env),
            exports: emulator::flatpak_exports(paths, &system_root),
            system_root,
        }
    }

    pub(crate) fn installs(&self, emulator: Emulator) -> Vec<Install> {
        emulator::installs(emulator, &self.path_var, &self.exports)
    }

    pub(crate) fn pick_file(
        &self,
        paths: &Paths,
        emulator: Emulator,
        picked: &str,
    ) -> anyhow::Result<Install> {
        Ok(emulator::pick_file(paths, emulator, &self.exports, picked)?)
    }

    pub(crate) fn cores(&self, paths: &Paths, install: &Install) -> Vec<Core> {
        emulator::cores(&emulator::core_folders(paths, install, &self.system_root))
    }
}

/// Debug builds look for system folders under `WINEO_TEST_SYSTEM_ROOT`, so
/// CLI tests never read the host's emulators or cores. Release builds never
/// look at the variable.
fn test_system_root() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::var_os("WINEO_TEST_SYSTEM_ROOT").filter(|root| !root.is_empty()) {
        return Some(PathBuf::from(root));
    }
    None
}

/// A built-in emulator by its id. Unlike `[emulation]`, an id this wineo
/// doesn't know is an error.
pub(crate) fn emulator_by_id(id: &str) -> anyhow::Result<Emulator> {
    Emulator::BUILT_IN
        .into_iter()
        .find(|emulator| emulator.id() == id)
        .ok_or_else(|| {
            let known: Vec<&str> = Emulator::BUILT_IN.iter().map(|e| e.id()).collect();
            anyhow::anyhow!("Unknown emulator `{id}`. Known: {}.", known.join(", "))
        })
}

/// Whether `query` names `core` by its id, display name, or file name,
/// without regard to case, or is the path to its file.
fn core_matches(paths: &Paths, core: &Core, query: &str) -> bool {
    let query = query.trim();
    let folded = wineo_core::name::fold_case(query);
    // Display names can hold a `/`, so only something else is a path.
    [core.id(), core.name(), core.file_name()]
        .iter()
        .any(|name| wineo_core::name::fold_case(name) == folded)
        || (query.contains('/') && same_path(&paths.expand(query), &core.path))
}

/// Whether two paths name the same file, also through links. A relative
/// path is read from the working directory.
fn same_path(a: &Path, b: &Path) -> bool {
    let absolute = |path: &Path| std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    absolute(a) == absolute(b)
        || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// The ids that more than one of `cores` has, like a distro snes9x and
/// one RetroArch downloaded.
fn shared_ids(cores: &[Core]) -> HashSet<&str> {
    let mut seen = HashSet::new();
    cores
        .iter()
        .map(Core::id)
        .filter(|id| !seen.insert(*id))
        .collect()
}

fn core_folder(core: &Core) -> String {
    core.path
        .parent()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default()
}

/// How `--core` names each of `cores`: its id, or its path when another
/// has the same id.
fn core_flags(cores: &[Core]) -> Vec<String> {
    let shared = shared_ids(cores);
    cores
        .iter()
        .map(|core| {
            if shared.contains(core.id()) {
                core.path.display().to_string()
            } else {
                core.id().to_owned()
            }
        })
        .collect()
}

fn install_label(emulator: Emulator, install: &Install) -> String {
    match install {
        Install::Path { name, .. } => format!("{} ({name})", emulator.name()),
        Install::Flatpak { .. } => format!("{} (Flatpak)", emulator.name()),
        Install::File(path) => format!("{} ({})", emulator.name(), path.display()),
    }
}

fn install_path(install: &Install) -> &Path {
    match install {
        Install::Path { found, .. } => found,
        Install::Flatpak { script } => script,
        Install::File(path) => path,
    }
}

fn emulators(paths: &Paths, host: &Host, json: bool) -> anyhow::Result<ExitCode> {
    let found: Vec<(Emulator, Vec<Install>)> = Emulator::BUILT_IN
        .into_iter()
        .map(|emulator| (emulator, host.installs(emulator)))
        .filter(|(_, installs)| !installs.is_empty())
        .collect();
    let cores = |install: &Install| host.cores(paths, install);
    if json {
        let emulators: Vec<Value> = found
            .iter()
            .map(|(emulator, installs)| {
                let installs: Vec<Value> = installs
                    .iter()
                    .map(|install| {
                        let mut value = json!({
                            "kind": match install {
                                Install::Path { .. } => "path",
                                Install::Flatpak { .. } => "flatpak",
                                Install::File(_) => "file",
                            },
                            "exe": match install {
                                Install::Path { name, .. } => Some(*name),
                                _ => None,
                            },
                            "path": install_path(install).to_string_lossy(),
                            "app": match install {
                                Install::Flatpak { .. } => emulator.flatpak_id(),
                                _ => None,
                            },
                        });
                        if *emulator == Emulator::RetroArch {
                            value["cores"] = cores(install).iter().map(core_json).collect();
                        }
                        value
                    })
                    .collect();
                json!({
                    "id": emulator.id(),
                    "name": emulator.name(),
                    "installs": installs,
                })
            })
            .collect();
        super::json::print(&json!({ "emulators": emulators }))?;
        return Ok(ExitCode::SUCCESS);
    }
    if found.is_empty() {
        println!("wineo found no emulators.");
        return Ok(ExitCode::SUCCESS);
    }
    let mut rows = vec![["ID", "NAME", "FOUND"].map(str::to_owned)];
    for (emulator, installs) in &found {
        for install in installs {
            let place = match install {
                Install::Flatpak { .. } => {
                    format!("Flatpak {}", emulator.flatpak_id().unwrap_or_default())
                }
                other => install_path(other).display().to_string(),
            };
            rows.push([emulator.id().to_owned(), emulator.name().to_owned(), place]);
        }
    }
    for line in render_columns(&rows) {
        println!("{line}");
    }
    for (emulator, installs) in &found {
        if *emulator != Emulator::RetroArch {
            continue;
        }
        for install in installs {
            println!();
            println!("Cores for {}:", install_label(*emulator, install));
            let cores = cores(install);
            if cores.is_empty() {
                println!("  none found");
                continue;
            }
            let row = |core: &Core| {
                [
                    core.id().to_owned(),
                    core.name().to_owned(),
                    core.system().unwrap_or("-").to_owned(),
                ]
            };
            // The folder tells apart cores with one id.
            let lines = if shared_ids(&cores).is_empty() {
                let mut rows = vec![["CORE", "NAME", "SYSTEM"].map(str::to_owned)];
                rows.extend(cores.iter().map(row));
                render_columns(&rows)
            } else {
                let mut rows = vec![["CORE", "NAME", "SYSTEM", "FOLDER"].map(str::to_owned)];
                rows.extend(cores.iter().map(|core| {
                    let [id, name, system] = row(core);
                    [id, name, system, core_folder(core)]
                }));
                render_columns(&rows)
            };
            for line in lines {
                println!("  {line}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn core_json(core: &Core) -> Value {
    json!({
        "id": core.id(),
        "name": core.name(),
        "file": core.path.to_string_lossy(),
        "system": core.system(),
        "extensions": core.extensions(),
    })
}

/// What runs the games, before a RetroArch core is picked.
#[derive(Clone)]
enum Chosen {
    BuiltIn(Emulator, Install),
    Custom {
        exe: String,
        args: String,
        ext: Vec<String>,
    },
}

impl Chosen {
    fn retroarch(&self) -> Option<&Install> {
        match self {
            Self::BuiltIn(Emulator::RetroArch, install) => Some(install),
            _ => None,
        }
    }

    /// The systems a game whose file could be either is chosen from.
    fn systems(&self) -> &'static [&'static str] {
        match self {
            Self::BuiltIn(emulator, _) => emulator.systems(),
            Self::Custom { .. } => &[],
        }
    }
}

/// Which games were picked.
#[derive(Clone)]
enum Picked {
    One(Entry),
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Emulator,
    Folder,
    Core,
    System,
    Game,
    Confirm,
}

type Ask = Prompt<Terminal, crate::output::Stream<io::Stdout>>;

struct Flow<'a> {
    paths: &'a Paths,
    global: &'a GlobalConfig,
    host: &'a Host,
    args: &'a Args,
    prompt: Option<Ask>,
    first_question: bool,
}

/// Said when a question needs a terminal that isn't there.
const NEEDS_TERMINAL: &str = "guided ROM import needs a terminal";

impl Flow<'_> {
    fn interactive(&self) -> bool {
        !self.args.dry_run && !self.args.list && prompt::interactive()
    }

    /// The terminal, opened on the first question. `what` names the flags
    /// that answer it instead.
    fn ask(&mut self, what: &str) -> anyhow::Result<&mut Ask> {
        if !self.interactive() {
            // These never ask, so a terminal wouldn't help.
            if prompt::interactive() && (self.args.list || self.args.dry_run) {
                let flag = if self.args.list {
                    "--list"
                } else {
                    "--dry-run"
                };
                anyhow::bail!("{flag} needs {what}");
            }
            anyhow::bail!("{NEEDS_TERMINAL}; pass {what}");
        }
        if self.prompt.is_none() {
            self.prompt = Some(
                Prompt::new(Terminal::open()?, crate::output::stdout())
                    .with_first_question(self.first_question),
            );
        }
        Ok(self.prompt.as_mut().expect("opened above"))
    }

    fn run(mut self) -> anyhow::Result<ExitCode> {
        let args = self.args;
        let mut chosen = self.fixed_emulator()?;
        let emulator_fixed = chosen.is_some();
        let mut folder = args.path.clone();
        let mut core: Option<Core> = None;
        let mut system = args.system.clone();
        let mut picked: Option<Picked> = None;
        let mut listing = Listing::default();
        let mut single = false;
        // The questions asked so far, for going back.
        let mut asked: Vec<Step> = Vec::new();
        let mut step = Step::Emulator;
        loop {
            let first = asked.is_empty();
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.set_first_question(first && self.first_question);
            }
            let reply: Reply<()> = match step {
                Step::Emulator => {
                    if emulator_fixed {
                        step = Step::Folder;
                        continue;
                    }
                    match self.ask_emulator()? {
                        Reply::Value(value) => {
                            chosen = Some(value);
                            core = None;
                            system = args.system.clone();
                            Reply::Value(())
                        }
                        Reply::Back => Reply::Back,
                    }
                }
                Step::Folder => {
                    if args.path.is_some() {
                        step = Step::Core;
                        continue;
                    }
                    match self.ask_folder(folder.as_deref())? {
                        Reply::Value(value) => {
                            folder = Some(value);
                            core = None;
                            system = args.system.clone();
                            Reply::Value(())
                        }
                        Reply::Back => Reply::Back,
                    }
                }
                Step::Core => {
                    let chosen = chosen.as_ref().expect("asked first");
                    let folder = folder.as_deref().expect("asked first");
                    let Some(install) = chosen.retroarch() else {
                        anyhow::ensure!(args.core.is_none(), "--core is only for RetroArch.");
                        listing = self.listing(chosen, None, folder)?;
                        single = is_single(self.paths, folder, &listing);
                        step = Step::System;
                        continue;
                    };
                    let cores = self.host.cores(self.paths, install);
                    let candidates = match &args.core {
                        Some(query) => {
                            let found = named_cores(self.paths, &cores, query)?;
                            if found.len() > 1 && !self.interactive() {
                                anyhow::bail!(
                                    "`{query}` matches more than one RetroArch core: {}. Pass --core with its path.",
                                    found
                                        .iter()
                                        .map(|core| core.path.display().to_string())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                );
                            }
                            found
                        }
                        None => {
                            let matching =
                                rom_import::matching_cores(self.paths, &cores, Path::new(folder))?;
                            if matching.is_empty() {
                                if cores.is_empty() {
                                    anyhow::bail!("No RetroArch cores were found.");
                                }
                                anyhow::bail!(
                                    "No installed RetroArch core plays the files in {folder}."
                                );
                            }
                            matching
                        }
                    };
                    let fixed = match candidates.as_slice() {
                        [one] => Some(one.clone()),
                        many => {
                            let what =
                                format!("--core with one of: {}", core_flags(many).join(", "));
                            let shared = shared_ids(many);
                            let choices = numbered(many.iter().map(|core| {
                                (core_label(core, shared.contains(core.id())), core.clone())
                            }));
                            let prompt = self.ask(&what)?;
                            match prompt.choose("Which core?", "Core", &choices, None, true)? {
                                Reply::Value(value) => {
                                    core = Some(value);
                                    None
                                }
                                Reply::Back => {
                                    match asked.pop() {
                                        Some(previous) => step = previous,
                                        None => return Err(self.leave()),
                                    }
                                    continue;
                                }
                            }
                        }
                    };
                    let asked_core = fixed.is_none();
                    if let Some(fixed) = fixed {
                        core = Some(fixed);
                    }
                    listing = self.listing(chosen, core.as_ref(), folder)?;
                    single = is_single(self.paths, folder, &listing);
                    if asked_core {
                        Reply::Value(())
                    } else {
                        step = Step::System;
                        continue;
                    }
                }
                Step::System => {
                    let chosen = chosen.as_ref().expect("asked first");
                    let systems = chosen.systems();
                    let custom = matches!(chosen, Chosen::Custom { .. });
                    // Only the games that would be imported can need it.
                    let root = root(self.paths, folder.as_deref().expect("asked first"));
                    let relevant: Vec<&Entry> = match (&args.game, single) {
                        (Some(query), false) => listing
                            .entries
                            .iter()
                            .filter(|entry| game_matches(self.paths, &root, entry, query))
                            .collect(),
                        _ => listing.entries.iter().collect(),
                    };
                    let needed = !custom
                        && relevant.iter().any(|entry| {
                            entry.state == EntryState::New && entry.system == EntrySystem::Ambiguous
                        });
                    if !needed || args.system.is_some() || args.list {
                        step = Step::Game;
                        continue;
                    }
                    let what = format!(
                        "--system {} for the games whose files could be either",
                        systems.join(" or --system ")
                    );
                    let choices = numbered(
                        systems
                            .iter()
                            .map(|id| ((*id).to_owned(), (*id).to_owned())),
                    );
                    let default = system
                        .as_deref()
                        .and_then(|current| systems.iter().position(|id| *id == current))
                        .map(|index| (index + 1).to_string());
                    let prompt = self.ask(&what)?;
                    match prompt.choose(
                        "Which system are the games whose files could be either?",
                        "System",
                        &choices,
                        default.as_deref(),
                        true,
                    )? {
                        Reply::Value(value) => {
                            system = Some(value);
                            Reply::Value(())
                        }
                        Reply::Back => Reply::Back,
                    }
                }
                Step::Game => {
                    let folder = folder.as_deref().expect("asked first");
                    if args.list {
                        return self.list(
                            chosen.as_ref(),
                            core.as_ref(),
                            &listing,
                            folder,
                            system.as_deref(),
                        );
                    }
                    let root = root(self.paths, folder);
                    let fixed = if single {
                        let entry = &listing.entries[0];
                        anyhow::ensure!(!args.all, "--all needs a folder of games, not one game.");
                        if let Some(query) = &args.game {
                            anyhow::ensure!(
                                game_matches(self.paths, &root, entry, query),
                                "--game `{query}` doesn't match {folder}."
                            );
                        }
                        Some(Picked::One(entry.clone()))
                    } else if args.all {
                        Some(Picked::All)
                    } else if let Some(query) = &args.game {
                        Some(Picked::One(find_game(
                            self.paths, &root, &listing, query, folder,
                        )?))
                    } else {
                        None
                    };
                    if let Some(fixed) = fixed {
                        picked = Some(fixed);
                        step = Step::Confirm;
                        continue;
                    }
                    match self.ask_game(&listing, folder)? {
                        Reply::Value(value) => {
                            picked = Some(value);
                            Reply::Value(())
                        }
                        Reply::Back => Reply::Back,
                    }
                }
                Step::Confirm => {
                    let chosen = chosen.as_ref().expect("asked first");
                    let picked = picked.clone().expect("asked first");
                    let guided = !asked.is_empty();
                    match self.confirm(
                        chosen,
                        core.as_ref(),
                        &listing,
                        picked,
                        system.as_deref(),
                        guided,
                    )? {
                        Some(code) => return Ok(code),
                        None => Reply::Back,
                    }
                }
            };
            match reply {
                Reply::Value(()) => {
                    asked.push(step);
                    step = next(step);
                }
                Reply::Back => match asked.pop() {
                    Some(previous) => step = previous,
                    None => return Err(self.leave()),
                },
            }
        }
    }

    /// Esc at the first question ends the import, or goes back to
    /// `import`'s menu when that asked first.
    fn leave(&self) -> anyhow::Error {
        if self.first_question {
            Canceled.into()
        } else {
            ReturnToSourceMenu.into()
        }
    }

    /// The emulator the flags name, if they name one.
    fn fixed_emulator(&self) -> anyhow::Result<Option<Chosen>> {
        let args = self.args;
        if let Some(exe) = &args.custom {
            return Ok(Some(Chosen::Custom {
                exe: exe.clone(),
                args: args.args.clone().unwrap_or_default(),
                ext: args.ext.clone(),
            }));
        }
        let Some(id) = &args.emulator else {
            return Ok(None);
        };
        let emulator = emulator_by_id(id)?;
        let install = match &args.exe {
            Some(exe) => self.host.pick_file(self.paths, emulator, exe)?,
            None => self
                .host
                .installs(emulator)
                .into_iter()
                .next()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{} wasn't found. Install it, or pass --exe PATH.",
                        emulator.name()
                    )
                })?,
        };
        Ok(Some(Chosen::BuiltIn(emulator, install)))
    }

    fn ask_emulator(&mut self) -> anyhow::Result<Reply<Chosen>> {
        // --core only means something to RetroArch.
        let retroarch_only = self.args.core.is_some();
        let found: Vec<(String, Chosen)> = Emulator::BUILT_IN
            .into_iter()
            .filter(|emulator| !retroarch_only || *emulator == Emulator::RetroArch)
            .flat_map(|emulator| {
                self.host
                    .installs(emulator)
                    .into_iter()
                    .map(move |install| {
                        (
                            install_label(emulator, &install),
                            Chosen::BuiltIn(emulator, install),
                        )
                    })
            })
            .collect();
        if found.is_empty() && retroarch_only {
            anyhow::bail!("--core is only for RetroArch, and wineo found no RetroArch.");
        }
        if found.is_empty() {
            anyhow::bail!(
                "wineo found no emulators; pass --emulator ID --exe PATH or --custom EXE"
            );
        }
        let choices = numbered(found);
        let prompt = self.ask("--emulator ID or --custom EXE")?;
        prompt.choose("Which emulator?", "Emulator", &choices, None, true)
    }

    fn ask_folder(&mut self, current: Option<&str>) -> anyhow::Result<Reply<String>> {
        let paths = self.paths;
        let prompt = self.ask("a folder or file of games")?;
        prompt.text("Folder or file of games", current, |text| {
            let path = paths
                .absolute(&picker::clean_path(text))
                .map_err(|error| error.to_string())?;
            if path.exists() {
                Ok(path.display().to_string())
            } else {
                Err(format!("{} doesn't exist", path.display()))
            }
        })
    }

    fn listing(
        &self,
        chosen: &Chosen,
        core: Option<&Core>,
        folder: &str,
    ) -> anyhow::Result<Listing> {
        let scan = match (chosen, core) {
            (Chosen::BuiltIn(Emulator::RetroArch, _), Some(core)) => Scan::RetroArch(core.clone()),
            (Chosen::BuiltIn(emulator, _), _) => Scan::BuiltIn(*emulator),
            (Chosen::Custom { ext, .. }, _) => Scan::Custom {
                extensions: ext.clone(),
                system: self.args.system.clone(),
            },
        };
        rom_import::listing(self.paths, &scan, Path::new(folder))
    }

    /// The game to import. Like the other imports' pickers, having nothing
    /// to pick is an error.
    fn ask_game(&mut self, listing: &Listing, folder: &str) -> anyhow::Result<Reply<Picked>> {
        // --list can't be combined with --dry-run, so don't suggest it there.
        let what = if self.args.dry_run {
            "--game NAME or --all"
        } else {
            "--game NAME, --all, or --list"
        };
        self.ask(what)?;
        let new: Vec<&Entry> = listing
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::New)
            .collect();
        if new.is_empty() {
            print_skipped(listing);
            if listing.entries.is_empty() {
                anyhow::bail!("No games found in {folder}.");
            }
            let imported = listing
                .entries
                .iter()
                .filter(|entry| matches!(entry.state, EntryState::AlreadyImported(_)))
                .count();
            let unusable = listing.entries.len() - imported;
            let mut parts = Vec::new();
            if imported > 0 {
                parts.push(format!("{imported} already imported"));
            }
            if unusable > 0 {
                parts.push(format!("{unusable} can't be imported"));
            }
            anyhow::bail!("No games to import in {folder}: {}.", parts.join(", "));
        }
        let mut choices: Vec<Choice<Picked>> = numbered(
            new.iter()
                .map(|entry| (entry_label(entry), Picked::One((*entry).clone()))),
        );
        choices.push(Choice::letter(
            'a',
            format!("All {}", text::counted(new.len(), "game", "games")),
            Picked::All,
        ));
        let prompt = self.ask(what)?;
        prompt.choose("Which game?", "Game", &choices, None, true)
    }

    fn list(
        &self,
        chosen: Option<&Chosen>,
        core: Option<&Core>,
        listing: &Listing,
        folder: &str,
        system: Option<&str>,
    ) -> anyhow::Result<ExitCode> {
        let emulator = match chosen {
            Some(Chosen::BuiltIn(emulator, _)) => emulator.id(),
            _ => emulator::CUSTOM,
        };
        let systems = chosen.map(Chosen::systems).unwrap_or_default();
        // A custom emulator's system is its games' tag, not a choice.
        if let (Some(chosen @ Chosen::BuiltIn(..)), Some(system)) = (chosen, system) {
            rom_import::check_system(&self.emulating(chosen, core), system)?;
        }
        // --system answers the files that could be either, as the import would.
        let entry_system = |entry: &Entry| -> Option<String> {
            match &entry.system {
                EntrySystem::Known(system) => Some(system.clone()),
                EntrySystem::Ambiguous => system.map(str::to_owned),
                EntrySystem::Unknown => None,
            }
        };
        if self.args.json {
            let games: Vec<Value> = listing
                .entries
                .iter()
                .map(|entry| {
                    let (state, environment, reason) = match &entry.state {
                        EntryState::New => ("new", None, None),
                        EntryState::AlreadyImported(name) => ("imported", Some(name), None),
                        EntryState::Unusable(reason) => ("unusable", None, Some(reason)),
                    };
                    json!({
                        "title": entry.title,
                        "file": entry.file_name,
                        "rom": entry.rom.to_string_lossy(),
                        "system": entry_system(entry),
                        "ambiguous": entry.system == EntrySystem::Ambiguous,
                        "state": state,
                        "environment": environment,
                        "reason": reason,
                    })
                })
                .collect();
            super::json::print(&json!({
                "emulator": emulator,
                "core": core.map(Core::id),
                "games": games,
                "skipped": listing
                    .skipped
                    .iter()
                    .map(|dir| dir.to_string_lossy())
                    .collect::<Vec<_>>(),
            }))?;
            return Ok(ExitCode::SUCCESS);
        }
        if listing.entries.is_empty() {
            println!("No games found in {folder}.");
        } else {
            let mut rows = vec![["TITLE", "SYSTEM", "FILE", "IMPORTED"].map(str::to_owned)];
            rows.extend(listing.entries.iter().map(|entry| {
                let system = entry_system(entry).unwrap_or_else(|| {
                    if entry.system == EntrySystem::Ambiguous {
                        systems.join(" or ")
                    } else {
                        "-".to_owned()
                    }
                });
                let imported = match &entry.state {
                    EntryState::New => String::new(),
                    EntryState::AlreadyImported(name) => name.clone(),
                    EntryState::Unusable(reason) => format!("can't import: {reason}"),
                };
                [
                    entry.title.clone(),
                    system,
                    entry.file_name.clone(),
                    imported,
                ]
            }));
            for line in render_columns(&rows) {
                println!("{line}");
            }
        }
        print_skipped(listing);
        Ok(ExitCode::SUCCESS)
    }

    fn emulating(&self, chosen: &Chosen, core: Option<&Core>) -> Emulating {
        let fullscreen = !self.args.no_fullscreen;
        match (chosen, core) {
            (Chosen::BuiltIn(Emulator::RetroArch, install), Some(core)) => Emulating::RetroArch {
                install: install.clone(),
                core: core.clone(),
                fullscreen,
            },
            (Chosen::BuiltIn(emulator, install), _) => Emulating::BuiltIn {
                emulator: *emulator,
                install: install.clone(),
                fullscreen,
            },
            (Chosen::Custom { exe, args, .. }, _) => Emulating::Custom {
                exe: exe.clone(),
                args: args.clone(),
            },
        }
    }

    /// Plans, reviews, and imports the picked games. None goes back to the
    /// game question.
    fn confirm(
        &mut self,
        chosen: &Chosen,
        core: Option<&Core>,
        listing: &Listing,
        picked: Picked,
        system: Option<&str>,
        guided: bool,
    ) -> anyhow::Result<Option<ExitCode>> {
        let args = self.args;
        let all = matches!(picked, Picked::All);
        let entries: Vec<Entry> = match picked {
            Picked::One(entry) => vec![entry],
            Picked::All => listing.entries.clone(),
        };
        if all && !args.dry_run && !args.yes && !guided && !self.interactive() {
            anyhow::bail!("bulk ROM import needs a terminal; pass --yes to skip the confirmation");
        }
        let custom = matches!(chosen, Chosen::Custom { .. });
        let emulating = self.emulating(chosen, core);
        let ambiguous = if custom { None } else { system };
        let batch =
            rom_import::plan_batch(self.paths, self.global, &emulating, entries, ambiguous)?;
        if !all && batch.ready.is_empty() {
            if let Some(already) = batch.already.first() {
                println!("Already imported as {}.", already.name);
                if args.dry_run {
                    println!("Dry run, nothing was imported.");
                }
                return Ok(Some(ExitCode::SUCCESS));
            }
            if let Some(failure) = batch.failures.first() {
                anyhow::bail!("Can't import {}: {}", failure.title, failure.reason);
            }
        }
        print_skipped(listing);
        // Checked before the review, so it says what will happen.
        let key = if args.art {
            art_key(self.paths, self.global)
        } else {
            None
        };
        if all {
            review_batch(&batch, chosen, core, key.is_some());
        } else if let Some(plan) = batch.ready.first() {
            review(self.paths, plan, chosen, core, key.is_some());
        }
        if args.dry_run {
            for failure in &batch.failures {
                eprintln!("Failed {}: {}", failure.title, failure.reason);
            }
            println!("Dry run, nothing was imported.");
            return Ok(Some(if batch.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }));
        }
        let count = batch.ready.len();
        if count > 0 && !args.yes && (guided || all) {
            let label = if all {
                format!("Import {}?", text::counted(count, "game", "games"))
            } else {
                "Import it?".to_owned()
            };
            let prompt = self.ask("--yes to skip the confirmation")?;
            prompt.confirm_hint(guided)?;
            match prompt.confirm(&label, true)? {
                Reply::Value(true) => {}
                Reply::Value(false) => return Err(Canceled.into()),
                Reply::Back if guided => return Ok(None),
                Reply::Back => return Err(Canceled.into()),
            }
        }
        let report = apply(self.paths, self.global, batch, key.as_ref())?;
        Ok(Some(finish(&report, all)))
    }
}

fn next(step: Step) -> Step {
    match step {
        Step::Emulator => Step::Folder,
        Step::Folder => Step::Core,
        Step::Core => Step::System,
        Step::System => Step::Game,
        Step::Game | Step::Confirm => Step::Confirm,
    }
}

/// The folder or file the games were listed from, as `listing` reads it:
/// `~` expanded and absolute.
fn root(paths: &Paths, folder: &str) -> PathBuf {
    let expanded = paths.expand(folder);
    std::path::absolute(&expanded).unwrap_or(expanded)
}

/// Whether `folder` is one game: a file, or a folder game such as a Cemu
/// title folder.
fn is_single(paths: &Paths, folder: &str, listing: &Listing) -> bool {
    let [entry] = listing.entries.as_slice() else {
        return false;
    };
    let given = root(paths, folder);
    if std::fs::metadata(&given).is_ok_and(|metadata| metadata.is_file()) {
        return true;
    }
    same_path(&entry.rom, &given)
}

/// Every core `query` names. Several cores can share an id.
fn named_cores(paths: &Paths, cores: &[Core], query: &str) -> anyhow::Result<Vec<Core>> {
    if cores.is_empty() {
        anyhow::bail!("No RetroArch cores were found.");
    }
    let found: Vec<Core> = cores
        .iter()
        .filter(|core| core_matches(paths, core, query))
        .cloned()
        .collect();
    if found.is_empty() {
        anyhow::bail!(
            "No RetroArch core named `{query}`. Installed: {}.",
            core_flags(cores).join(", ")
        );
    }
    Ok(found)
}

/// A core as the "Which core?" menu shows it, with its folder when another
/// core has the same id.
fn core_label(core: &Core, shared: bool) -> String {
    let label = if core.name() == core.file_name() {
        core.name().to_owned()
    } else {
        format!("{} ({})", core.name(), core.id())
    };
    if shared {
        format!("{label} in {}", core_folder(core))
    } else {
        label
    }
}

fn entry_label(entry: &Entry) -> String {
    match &entry.note {
        Some(note) => format!("{} ({note})", entry.title),
        None => entry.title.clone(),
    }
}

/// A game's path under the folder it was listed from, which `--game` takes.
fn relative(root: &Path, entry: &Entry) -> String {
    let real = root.canonicalize().ok();
    entry
        .rom
        .strip_prefix(root)
        .ok()
        .or_else(|| {
            real.as_deref()
                .and_then(|real| entry.rom.strip_prefix(real).ok())
        })
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.file_name.clone())
}

/// Whether `query` names the game: its title or file name, its path under
/// the folder, or its full path.
fn game_matches(paths: &Paths, root: &Path, entry: &Entry, query: &str) -> bool {
    entry.matches(query)
        || relative(root, entry) == query
        || (query.contains('/') && same_path(&paths.expand(query), &entry.rom))
}

fn find_game(
    paths: &Paths,
    root: &Path,
    listing: &Listing,
    query: &str,
    folder: &str,
) -> anyhow::Result<Entry> {
    let found: Vec<&Entry> = listing
        .entries
        .iter()
        .filter(|entry| game_matches(paths, root, entry, query))
        .collect();
    match found.as_slice() {
        [] => anyhow::bail!("No game in {folder} matches `{query}`."),
        [one] => Ok((*one).clone()),
        many => {
            let files: Vec<String> = many.iter().map(|entry| relative(root, entry)).collect();
            anyhow::bail!(
                "`{query}` matches more than one game: {}. Pass one of those paths.",
                files.join(", ")
            )
        }
    }
}

fn print_skipped(listing: &Listing) {
    for dir in &listing.skipped {
        eprintln!(
            "{}",
            style::warning_stderr(format!("warning: couldn't read {}, skipped", dir.display()))
        );
    }
}

fn emulator_label(chosen: &Chosen, core: Option<&Core>) -> String {
    match (chosen, core) {
        (Chosen::BuiltIn(emulator, install), Some(core)) => {
            format!(
                "{}, core {}",
                install_label(*emulator, install),
                core.name()
            )
        }
        (Chosen::BuiltIn(emulator, install), None) => install_label(*emulator, install),
        (Chosen::Custom { .. }, _) => "custom".to_owned(),
    }
}

fn review(paths: &Paths, plan: &Plan, chosen: &Chosen, core: Option<&Core>, art: bool) {
    let words: Vec<&str> = std::iter::once(plan.target.exe.as_str())
        .chain(plan.target.args.iter().map(String::as_str))
        .collect();
    // Quoted as a shell would need it, so a path with spaces reads as one.
    let command = shlex::try_join(words.iter().copied()).unwrap_or_else(|_| words.join(" "));
    println!("Ready to import {}:", plan.name);
    println!("  game      {}", plan.title);
    println!("  rom       {}", style::path(paths, &plan.rom));
    println!("  emulator  {}", emulator_label(chosen, core));
    println!("  command   {command}");
    println!("  tags      {}", plan.system.as_deref().unwrap_or("none"));
    println!("  desktop   off");
    println!(
        "  art       {}",
        if art {
            "SteamGridDB, top match"
        } else {
            "none"
        }
    );
}

fn review_batch(batch: &Batch, chosen: &Chosen, core: Option<&Core>, art: bool) {
    println!("Emulator: {}", emulator_label(chosen, core));
    if art {
        println!("Art: SteamGridDB, top match");
    }
    if !batch.ready.is_empty() {
        let mut rows = vec![["NAME", "TITLE", "SYSTEM", "FILE"].map(str::to_owned)];
        rows.extend(batch.ready.iter().map(|plan| {
            [
                plan.name.clone(),
                plan.title.clone(),
                plan.system.clone().unwrap_or_else(|| "-".to_owned()),
                plan.rom
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ]
        }));
        for line in render_columns(&rows) {
            println!("{line}");
        }
    }
    if !batch.already.is_empty() {
        let names: Vec<&str> = batch
            .already
            .iter()
            .map(|game| game.name.as_str())
            .collect();
        println!(
            "{} already imported: {}",
            batch.already.len(),
            names.join(", ")
        );
    }
    if batch.ready.is_empty() {
        println!("No new games to import.");
    }
}

fn art_key(paths: &Paths, global: &GlobalConfig) -> Option<Key> {
    if global.art.fetch.kinds().is_empty() {
        return None;
    }
    match super::art::check_key(paths) {
        KeyCheck::Found(key) => Some(key),
        check => {
            check.print_skip();
            None
        }
    }
}

fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    batch: Batch,
    key: Option<&Key>,
) -> anyhow::Result<BatchReport> {
    if batch.ready.is_empty() {
        return Ok(BatchReport::from(batch));
    }
    let cancellation = Cancellation::new()?;
    let cancel: &Token = &cancellation.token;
    let access = Access::steamgriddb(paths);
    let report = rom_import::apply_batch(paths, batch, cancel, |applying| {
        match applying {
            Applying::Starting(_) => {}
            Applying::Imported(plan, applied) => {
                // Its warnings are in the report, printed at the end.
                println!("Imported {}.", applied.environment.name);
                if let Some(key) = key {
                    let (mut out, mut err) = (Vec::new(), Vec::new());
                    let request = AutoFetch {
                        env: &applied.environment.name,
                        term: &plan.title,
                        kinds: global.art.fetch,
                    };
                    // A top match can't be canceled.
                    let _ = access.fetch(
                        paths,
                        key,
                        &request,
                        Pick::<NoInput, Vec<u8>>::Top(&mut out),
                        &mut err,
                    );
                    crate::output::stdout().write_all(&out)?;
                    crate::output::stderr().write_all(&err)?;
                }
            }
            Applying::AlreadyImported(_, name) => println!("Already imported as {name}."),
        }
        Ok(())
    });
    let report = cancellation.result(report)?;
    if report.canceled {
        return Err(Canceled.into());
    }
    Ok(report)
}

fn finish(report: &BatchReport, all: bool) -> ExitCode {
    for warning in &report.warnings {
        eprintln!("{}", style::warning_stderr(format!("warning: {warning}")));
    }
    for failure in &report.failures {
        eprintln!("Failed {}: {}", failure.title, failure.reason);
    }
    if all && (report.total > 0 || !report.failures.is_empty()) {
        println!(
            "{}",
            text::games_summary("Imported", report.done.len(), report.total)
        );
        if !report.already.is_empty() {
            println!(
                "{} already imported: {}",
                report.already.len(),
                report.already.join(", ")
            );
        }
        if !report.failures.is_empty() {
            let names: Vec<&str> = report
                .failures
                .iter()
                .map(|failure| failure.name.as_deref().unwrap_or(&failure.title))
                .collect();
            println!("{} failed: {}", report.failures.len(), names.join(", "));
        }
    }
    if report.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
