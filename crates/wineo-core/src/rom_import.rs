//! Importing emulated games: a folder of ROMs, or one file, for one
//! emulator, each game its own native environment with `[emulation]` and a
//! target that runs it. Shaped like Steam's batch import: `listing`, then
//! `plan_batch` and `apply_batch`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use toml_edit::{Array, Item, Table, Value, value};

use crate::cancel::Token;
use crate::config::{self, EnvironmentConfig, GlobalConfig};
use crate::emulator::{self, Core, Emulator, ExtSystem, Install, TargetCommand};
use crate::environment::{self, Environment};
use crate::launch::{self, WINDOWS_EXTENSIONS};
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::winpath;

mod scan;
mod title;

/// Which emulator's games a listing looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scan {
    BuiltIn(Emulator),
    /// RetroArch with one core: the files its info lists, or every file for
    /// a core without info.
    RetroArch(Core),
    /// A custom emulator: its file extensions, with or without the dot and
    /// in any case, and the system the person gave it, if any.
    Custom {
        extensions: Vec<String>,
        system: Option<String>,
    },
}

/// What a listed game's system is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntrySystem {
    Known(String),
    /// The file type is played as more than one system, so the import asks.
    Ambiguous,
    /// No system, so the game gets no tag.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    New,
    /// Some environment's `[emulation] rom` is this ROM.
    AlreadyImported(String),
    /// Why it can't be imported.
    Unusable(String),
}

/// One game in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Absolute, as found: a file, or a folder game's folder.
    pub rom: PathBuf,
    pub file_name: String,
    pub title: String,
    /// The file name, when another game in the listing has the same title.
    pub note: Option<String>,
    pub system: EntrySystem,
    pub state: EntryState,
}

impl Entry {
    /// Whether `query` names this game, by its title or its file name,
    /// without regard to case.
    pub fn matches(&self, query: &str) -> bool {
        let query = name::fold_case(query.trim());
        name::fold_case(&self.title) == query || name::fold_case(&self.file_name) == query
    }
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// Sorted by title, then path.
    pub entries: Vec<Entry>,
    /// Subfolders that couldn't be read, which were left out.
    pub skipped: Vec<PathBuf>,
}

/// Refuses a custom emulator's command that can't run, the way a native
/// target's exe is checked: a bare name must be on `path_var`, and a path
/// must be absolute (`~/` counts) and an executable file, not a Windows
/// program.
pub fn check_custom_command(paths: &Paths, exe: &str, path_var: &OsStr) -> anyhow::Result<()> {
    let exe = exe.trim();
    if exe.is_empty() {
        anyhow::bail!("The emulator's command is empty.");
    }
    let lower = exe.to_ascii_lowercase();
    if winpath::is_windows_path(exe) || WINDOWS_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        anyhow::bail!("`{exe}` is a Windows program, not a native one.");
    }
    launch::resolve_native_exe(paths, exe, exe, path_var)?;
    Ok(())
}

/// A custom emulator's system as a tag-safe id. Empty means none.
pub fn custom_system(raw: &str) -> anyhow::Result<Option<String>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let id = name::from_title(raw);
    if id.is_empty() {
        anyhow::bail!("Invalid system: {raw}.");
    }
    Ok(Some(id))
}

fn custom_extensions(extensions: &[String]) -> anyhow::Result<Vec<String>> {
    let cleaned: Vec<String> = extensions
        .iter()
        .map(|extension| {
            extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|extension| !extension.is_empty())
        .collect();
    if cleaned.is_empty() {
        anyhow::bail!("Give at least one file type for the emulator's games.");
    }
    Ok(cleaned)
}

/// Where a listed game's system comes from.
enum Systems {
    /// The emulator's table, by extension.
    BuiltIn(Emulator),
    /// The one the person gave a custom emulator, if any.
    Custom(Option<String>),
    /// A RetroArch core's, by its info and the file's extension.
    Core(Core),
}

/// The scan's rules and the system each file gets.
struct Rules {
    extensions: Vec<String>,
    any_file: bool,
    folder: Option<emulator::FolderGame>,
    systems: Systems,
}

impl Rules {
    fn new(scan: &Scan) -> anyhow::Result<Self> {
        Ok(match scan {
            Scan::BuiltIn(emulator) => Self {
                extensions: emulator
                    .extensions()
                    .iter()
                    .map(|(extension, _)| (*extension).to_owned())
                    .collect(),
                any_file: false,
                folder: emulator.folder_game(),
                systems: Systems::BuiltIn(*emulator),
            },
            Scan::RetroArch(core) => Self {
                extensions: core
                    .extensions()
                    .map(<[String]>::to_vec)
                    .unwrap_or_default(),
                any_file: core.extensions().is_none(),
                folder: None,
                systems: Systems::Core(core.clone()),
            },
            Scan::Custom { extensions, system } => Self {
                extensions: custom_extensions(extensions)?,
                any_file: false,
                folder: None,
                systems: Systems::Custom(match system {
                    Some(raw) => custom_system(raw)?,
                    None => None,
                }),
            },
        })
    }

    /// The system of the game at `path`. A folder game is its emulator's
    /// one system.
    fn system(&self, path: &Path, folder_game: bool) -> EntrySystem {
        let emulator = match &self.systems {
            Systems::BuiltIn(emulator) => *emulator,
            Systems::Custom(system) => {
                return system
                    .clone()
                    .map_or(EntrySystem::Unknown, EntrySystem::Known);
            }
            Systems::Core(core) => {
                return core
                    .file_system(scan::extension(path).as_deref())
                    .map_or(EntrySystem::Unknown, |system| {
                        EntrySystem::Known(system.to_owned())
                    });
            }
        };
        let found = if folder_game {
            emulator
                .systems()
                .first()
                .map(|system| ExtSystem::Is(system))
        } else {
            scan::extension(path).and_then(|extension| emulator.extension_system(&extension))
        };
        match found {
            Some(ExtSystem::Is(system)) => EntrySystem::Known(system.to_owned()),
            Some(ExtSystem::Ambiguous) => EntrySystem::Ambiguous,
            None => EntrySystem::Unknown,
        }
    }

    fn lists(&self, path: &Path) -> bool {
        self.any_file
            || scan::extension(path).is_some_and(|extension| self.extensions.contains(&extension))
    }

    fn scan_rules(&self) -> scan::Rules<'_> {
        scan::Rules {
            extensions: &self.extensions,
            any_file: self.any_file,
            folder: self.folder,
        }
    }
}

fn check_portable(paths: &Paths) -> anyhow::Result<()> {
    if paths.portable_root().is_some() {
        anyhow::bail!("Emulated games can't be imported into a portable folder.");
    }
    Ok(())
}

/// `path` with `~` expanded and made absolute. A `..` is resolved through
/// the disk, since a target can't name a ROM with one.
fn absolute(paths: &Paths, path: &Path) -> anyhow::Result<PathBuf> {
    let expanded = match path.to_str() {
        Some(text) => paths.expand(text),
        None => path.to_owned(),
    };
    let absolute = std::path::absolute(&expanded)?;
    if absolute
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return Ok(paths::resolve(&absolute));
    }
    Ok(absolute)
}

/// Each imported ROM, resolved, with the environment that has it.
#[derive(Default)]
struct Imported {
    roms: HashMap<PathBuf, String>,
    /// Every environment name read so far, loaded or not.
    names: HashSet<String>,
}

impl Imported {
    fn read(paths: &Paths) -> crate::Result<Self> {
        let mut imported = Self::default();
        imported.refresh(paths)?;
        Ok(imported)
    }

    /// Catches up with the environments on disk. Only names it hasn't seen
    /// are loaded: an import or a clone always makes a new name, so this
    /// stays cheap across a batch. Names that are gone are dropped.
    fn refresh(&mut self, paths: &Paths) -> crate::Result<()> {
        let names: HashSet<String> = environment::names(paths)?.into_iter().collect();
        self.roms.retain(|_, name| names.contains(name));
        self.names.retain(|name| names.contains(name));
        let mut pending = None;
        let mut new: Vec<&String> = names
            .iter()
            .filter(|name| !self.names.contains(*name))
            .collect();
        new.sort();
        for name in new {
            let pending = pending.get_or_insert_with(|| crate::trash::PendingScan::new(paths));
            let Ok(environment) = Environment::load_with_pending(paths, name, pending) else {
                continue;
            };
            if let Some(emulation) = &environment.config.emulation
                && let Ok(rom) = paths.config_path(&emulation.rom)
            {
                self.roms
                    .entry(paths::resolve(&rom))
                    .or_insert_with(|| name.clone());
            }
        }
        self.names = names;
        Ok(())
    }

    fn get(&self, rom: &Path) -> Option<&String> {
        self.roms.get(&paths::resolve(rom))
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The title a game's file or folder name gives. A PSP or PS1 `EBOOT.PBP`
/// is named after its folder, and a compressed image loses its inner
/// extension too, so `Game.iso.gz` is `Game`.
fn title_for(path: &Path, folder_game: bool, extensions: &[String]) -> String {
    let eboot = !folder_game
        && path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("EBOOT.PBP"));
    let mut stem = if folder_game {
        file_name(path)
    } else if eboot {
        path.parent().map(file_name).unwrap_or_default()
    } else {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if !folder_game && scan::extension(path).as_deref() == Some("gz") {
        let inner = Path::new(&stem);
        if let Some(extension) = scan::extension(inner)
            && extension != "gz"
            && extensions.contains(&extension)
            && let Some(inner_stem) = inner.file_stem()
        {
            stem = inner_stem.to_string_lossy().into_owned();
        }
    }
    title::clean(&stem)
}

/// The games at `path`, a folder or one file, that `scan` finds, with
/// what each would be imported as. Nothing is written.
pub fn listing(paths: &Paths, scan: &Scan, path: &Path) -> anyhow::Result<Listing> {
    check_portable(paths)?;
    let rules = Rules::new(scan)?;
    let root = absolute(paths, path)?;
    let metadata = std::fs::metadata(&root)
        .map_err(|error| anyhow::anyhow!("Can't read {}: {error}.", root.display()))?;
    let mut found: Vec<(PathBuf, bool)> = Vec::new();
    let mut skipped = Vec::new();
    if metadata.is_dir() {
        if rules
            .folder
            .is_some_and(|kind| scan::is_folder_game(&root, kind))
        {
            found.push((root, true));
        } else {
            std::fs::read_dir(&root)
                .map_err(|error| anyhow::anyhow!("Can't read {}: {error}.", root.display()))?;
            let scanned = scan::scan(&root, &rules.scan_rules());
            skipped = scanned.skipped;
            found.extend(scanned.games.into_iter().map(|game| {
                let folder_game = game.is_dir();
                (game, folder_game)
            }));
        }
    } else if rules.lists(&root) {
        found.push((root, false));
    } else {
        match scan {
            Scan::BuiltIn(emulator) => anyhow::bail!(
                "`{}` isn't a file {} plays.",
                root.display(),
                emulator.name()
            ),
            Scan::RetroArch(core) => {
                anyhow::bail!("`{}` isn't a file {} plays.", root.display(), core.name())
            }
            Scan::Custom { .. } => {
                anyhow::bail!("`{}` isn't one of the file types given.", root.display())
            }
        }
    }
    let imported = Imported::read(paths)?;
    let mut entries: Vec<Entry> = found
        .into_iter()
        .map(|(rom, folder_game)| {
            let state = if rom.to_str().is_none() {
                EntryState::Unusable(format!("{} isn't valid UTF-8.", rom.display()))
            } else if let Some(name) = imported.get(&rom) {
                EntryState::AlreadyImported(name.clone())
            } else {
                EntryState::New
            };
            Entry {
                file_name: file_name(&rom),
                title: title_for(&rom, folder_game, &rules.extensions),
                note: None,
                system: rules.system(&rom, folder_game),
                state,
                rom,
            }
        })
        .collect();
    let mut titles: HashMap<String, usize> = HashMap::new();
    for entry in &entries {
        *titles.entry(name::fold_case(&entry.title)).or_default() += 1;
    }
    for entry in &mut entries {
        if titles[&name::fold_case(&entry.title)] > 1 {
            entry.note = Some(entry.file_name.clone());
        }
    }
    entries.sort_by_cached_key(|entry| (name::fold_case(&entry.title), entry.rom.clone()));
    skipped.sort();
    Ok(Listing { entries, skipped })
}

/// The cores in `cores` that play something at `path`, a folder or one
/// file, in the order given. The folder comes first, then its cores: a core
/// is offered when its info lists the extension of a file there, and a
/// core without info whenever there is a file at all.
pub fn matching_cores(paths: &Paths, cores: &[Core], path: &Path) -> anyhow::Result<Vec<Core>> {
    check_portable(paths)?;
    let root = absolute(paths, path)?;
    let metadata = std::fs::metadata(&root)
        .map_err(|error| anyhow::anyhow!("Can't read {}: {error}.", root.display()))?;
    let files = if metadata.is_dir() {
        std::fs::read_dir(&root)
            .map_err(|error| anyhow::anyhow!("Can't read {}: {error}.", root.display()))?;
        scan::all_files(&root)
    } else {
        vec![root]
    };
    let extensions: HashSet<Option<String>> =
        files.iter().map(|file| scan::extension(file)).collect();
    Ok(cores
        .iter()
        .filter(|core| {
            extensions
                .iter()
                .any(|extension| core.plays(extension.as_deref()))
        })
        .cloned()
        .collect())
}

/// What runs the games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Emulating {
    BuiltIn {
        emulator: Emulator,
        install: Install,
        /// Ignored for an emulator with no fullscreen flag.
        fullscreen: bool,
    },
    /// RetroArch with the core that plays the games.
    RetroArch {
        install: Install,
        core: Core,
        fullscreen: bool,
    },
    /// A custom emulator: its command, a path or a bare name on PATH, and
    /// its arguments in one string with `{rom}` for the game.
    Custom { exe: String, args: String },
}

impl Emulating {
    /// The `[emulation] emulator` its games get.
    fn id(&self) -> &str {
        match self {
            Self::BuiltIn { emulator, .. } => emulator.id(),
            Self::RetroArch { .. } => Emulator::RetroArch.id(),
            Self::Custom { .. } => emulator::CUSTOM,
        }
    }

    /// Refuses a custom emulator's command that can't run, with a bare name
    /// looked for on the global PATH.
    fn check(&self, paths: &Paths, global: &GlobalConfig) -> anyhow::Result<()> {
        let Self::Custom { exe, .. } = self else {
            return Ok(());
        };
        let path_var = launch::path_or_inherited(global.env.get("PATH"));
        check_custom_command(paths, exe, &path_var)
    }

    fn target(&self, paths: &Paths, rom: &Path) -> anyhow::Result<TargetCommand> {
        Ok(match self {
            Self::BuiltIn {
                emulator,
                install,
                fullscreen,
            } => {
                let mut command = emulator::build_target(*emulator, install, rom, *fullscreen)?;
                if let Install::File(path) = install {
                    command.exe = paths.store(path)?;
                }
                command
            }
            Self::RetroArch {
                install,
                core,
                fullscreen,
            } => {
                let mut command = emulator::retroarch_target(
                    install,
                    core,
                    rom,
                    *fullscreen,
                    &emulator::quit_file(paths),
                )?;
                if let Install::File(path) = install {
                    command.exe = paths.store(path)?;
                }
                command
            }
            Self::Custom { exe, args } => {
                // Trimmed as `check` sees it, so what was checked is stored.
                let exe = exe.trim();
                let mut command = emulator::custom_target(exe, args, rom)?;
                // A bare name stays one, for PATH.
                if exe.contains('/') || exe.starts_with('~') {
                    let path = paths.expand(exe);
                    if path.is_absolute() {
                        command.exe = paths.store(&path)?;
                    }
                }
                command
            }
        })
    }
}

/// One game ready to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub rom: PathBuf,
    pub title: String,
    /// The environment's name, which is also its target's.
    pub name: String,
    pub system: Option<String>,
    /// `[emulation] emulator`: a built-in's id or `custom`.
    pub emulator: String,
    pub target: TargetCommand,
}

/// A game a batch couldn't plan or import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub rom: PathBuf,
    pub title: String,
    /// The name it was given, when planning got that far.
    pub name: Option<String>,
    pub reason: String,
}

/// A game some environment already has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Already {
    pub rom: PathBuf,
    pub title: String,
    pub name: String,
}

/// A batch planned for review, before anything is written.
#[derive(Debug, Clone, Default)]
pub struct Batch {
    pub ready: Vec<Plan>,
    pub already: Vec<Already>,
    pub failures: Vec<Failure>,
}

/// Plans each chosen game in order. `ambiguous` is the system for every
/// game whose file type doesn't tell, and is required when there is one.
/// Each game gets its title as a name; one already taken, by an environment
/// or an earlier game here, gets its system added, then a number.
pub fn plan_batch(
    paths: &Paths,
    global: &GlobalConfig,
    emulating: &Emulating,
    entries: impl IntoIterator<Item = Entry>,
    ambiguous: Option<&str>,
) -> anyhow::Result<Batch> {
    check_portable(paths)?;
    emulating.check(paths, global)?;
    let entries: Vec<Entry> = entries.into_iter().collect();
    let ambiguous = ambiguous_system(emulating, &entries, ambiguous)?;
    let imported = Imported::read(paths)?;
    let mut batch = Batch::default();
    let mut names: HashSet<String> = HashSet::new();
    for entry in entries {
        let existing = match &entry.state {
            EntryState::AlreadyImported(name) => Some(name.clone()),
            _ => imported.get(&entry.rom).cloned(),
        };
        if let Some(name) = existing {
            batch.already.push(Already {
                rom: entry.rom,
                title: entry.title,
                name,
            });
            continue;
        }
        if let EntryState::Unusable(reason) = entry.state {
            batch.failures.push(Failure {
                rom: entry.rom,
                title: entry.title,
                name: None,
                reason,
            });
            continue;
        }
        let system = match &entry.system {
            EntrySystem::Known(system) => Some(system.clone()),
            EntrySystem::Ambiguous => ambiguous.clone(),
            EntrySystem::Unknown => None,
        };
        let name = free_name(paths, &names, &entry.title, system.as_deref());
        match emulating.target(paths, &entry.rom) {
            Ok(target) => {
                names.insert(name.clone());
                batch.ready.push(Plan {
                    rom: entry.rom,
                    title: entry.title,
                    name,
                    system,
                    emulator: emulating.id().to_owned(),
                    target,
                });
            }
            Err(error) => batch.failures.push(Failure {
                rom: entry.rom,
                title: entry.title,
                name: Some(name),
                reason: format!("{error:#}"),
            }),
        }
    }
    Ok(batch)
}

/// The system for the games whose files could be more than one, checked
/// against what the emulator plays. None when no chosen game needs it.
fn ambiguous_system(
    emulating: &Emulating,
    entries: &[Entry],
    chosen: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let needed = entries
        .iter()
        .any(|entry| entry.state == EntryState::New && entry.system == EntrySystem::Ambiguous);
    let systems = match emulating {
        Emulating::BuiltIn { emulator, .. } => emulator.systems(),
        Emulating::RetroArch { .. } | Emulating::Custom { .. } => &[],
    };
    let Some(chosen) = chosen else {
        if needed {
            anyhow::bail!(
                "Choose {} for the games whose files could be either.",
                systems.join(" or ")
            );
        }
        return Ok(None);
    };
    check_system(emulating, chosen)?;
    Ok(Some(chosen.to_owned()))
}

/// Refuses `chosen` as the system for games whose files could be either,
/// unless it's one `emulating` plays.
pub fn check_system(emulating: &Emulating, chosen: &str) -> anyhow::Result<()> {
    let systems = match emulating {
        Emulating::BuiltIn { emulator, .. } => emulator.systems(),
        Emulating::RetroArch { .. } | Emulating::Custom { .. } => &[],
    };
    if !systems.contains(&chosen) {
        let name = match emulating {
            Emulating::BuiltIn { emulator, .. } => emulator.name(),
            Emulating::RetroArch { .. } => Emulator::RetroArch.name(),
            Emulating::Custom { .. } => "A custom emulator",
        };
        if systems.is_empty() {
            anyhow::bail!("{name} has no system to choose.");
        }
        anyhow::bail!(
            "`{chosen}` isn't a system {name} plays: {}.",
            systems.join(", ")
        );
    }
    Ok(())
}

/// The name a title makes, or `game` when it has nothing a name can use.
fn base_name(title: &str) -> String {
    let name = name::from_title(title);
    if name.is_empty() {
        "game".to_owned()
    } else {
        name
    }
}

/// The game's title as a name, with its system and then a number added
/// when that is taken.
fn free_name(paths: &Paths, batch: &HashSet<String>, title: &str, system: Option<&str>) -> String {
    let taken = |name: &str| batch.contains(name) || paths.environment_config(name).exists();
    let base = base_name(title);
    if !taken(&base) {
        return base;
    }
    let with_system = match system {
        Some(system) => format!("{base}-{system}"),
        None => base,
    };
    crate::targets::free_name(&with_system, taken)
}

/// One game's import.
#[derive(Debug)]
pub struct Applied {
    pub environment: Environment,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ApplyOutcome {
    Imported(Applied),
    /// Something imported this ROM since it was planned, as this name.
    AlreadyImported(String),
}

/// The config `plan` writes as `name`, checked as a config would be on load.
fn config_text(paths: &Paths, plan: &Plan, name: &str) -> anyhow::Result<String> {
    let mut document = config::new_document();
    document["kind"] = value("native");
    document["title"] = value(&plan.title);
    if let Some(system) = &plan.system {
        let mut tags = Array::new();
        tags.push(system.as_str());
        document["tags"] = value(tags);
    }
    let mut emulation = Table::new();
    emulation["emulator"] = value(&plan.emulator);
    if let Some(system) = &plan.system {
        emulation["system"] = value(system);
    }
    emulation["rom"] = value(paths.store(&plan.rom)?);
    document["emulation"] = Item::Table(emulation);
    let mut target = Table::new();
    target["exe"] = value(&plan.target.exe);
    let args: Array = plan.target.args.iter().map(String::as_str).collect();
    target["args"] = Value::Array(args).into();
    target["default"] = value(true);
    target["name"] = value(&plan.title);
    let mut targets = Table::new();
    targets.set_implicit(true);
    targets[name] = Item::Table(target);
    document["targets"] = Item::Table(targets);
    let text = document.to_string();
    EnvironmentConfig::parse(Path::new("<import>"), &text)?;
    Ok(text)
}

/// Creates the game's environment, unless its ROM was imported since it was
/// planned. The check and the write hold one lock, so two imports of the
/// same ROM can't both write. A name taken since planning is picked again
/// the way planning picks one.
pub fn apply(paths: &Paths, plan: &Plan, cancel: &Token) -> anyhow::Result<ApplyOutcome> {
    check_portable(paths)?;
    let mut outcome = apply_with(paths, plan, cancel, &mut Imported::default())?;
    if let ApplyOutcome::Imported(applied) = &mut outcome {
        applied
            .warnings
            .extend(retroarch_quit_file(paths, std::iter::once(plan)));
    }
    Ok(outcome)
}

/// Writes RetroArch's quit file for the global setting when `plans` has a
/// RetroArch game, so its target's `--appendconfig` finds it. A failure is
/// a warning, since the games import and run either way.
fn retroarch_quit_file<'a>(
    paths: &Paths,
    mut plans: impl Iterator<Item = &'a Plan>,
) -> Option<String> {
    if !plans.any(|plan| plan.emulator == Emulator::RetroArch.id()) {
        return None;
    }
    match GlobalConfig::load(paths) {
        Ok(global) => emulator::refresh_quit_file(paths, global.retroarch.quit_with_game()),
        Err(error) => Some(format!(
            "Can't write RetroArch's quit file: {}.",
            crate::error::chain(&error)
        )),
    }
}

/// `apply` with what is known to be imported so far, which it brings up to
/// date under the lock.
fn apply_with(
    paths: &Paths,
    plan: &Plan,
    cancel: &Token,
    imported: &mut Imported,
) -> anyhow::Result<ApplyOutcome> {
    crate::import::check_cancelled(cancel)?;
    let locks = Locks::new(paths);
    let _roms = locks.emulated_roms()?;
    imported.refresh(paths)?;
    if let Some(name) = imported.get(&plan.rom) {
        return Ok(ApplyOutcome::AlreadyImported(name.clone()));
    }
    let name = if paths.environment_config(&plan.name).exists() {
        free_name(paths, &HashSet::new(), &plan.title, plan.system.as_deref())
    } else {
        plan.name.clone()
    };
    environment::check_new_name(paths, &name)?;
    let text = config_text(paths, plan, &name)?;
    let _operation = locks.acquire(Operation::Create, &name, &[])?;
    crate::import::check_cancelled(cancel)?;
    let environment = crate::import::write_native_config(paths, &name, &text)?;
    let warnings = crate::library_lifecycle::fresh_warning(paths, &name)
        .into_iter()
        .collect();
    Ok(ApplyOutcome::Imported(Applied {
        environment,
        warnings,
    }))
}

/// What `apply_batch` is doing with one game, as it goes.
pub enum Applying<'a> {
    Starting(&'a Plan),
    Imported(&'a Plan, &'a Applied),
    /// Something else imported its ROM since it was planned, as this name.
    AlreadyImported(&'a Plan, &'a str),
}

/// How a batch went. Games found already imported while applying leave the
/// total, so it only counts what this batch could have imported.
#[derive(Debug, Clone, Default)]
pub struct BatchReport {
    pub done: Vec<String>,
    pub total: usize,
    pub already: Vec<String>,
    pub failures: Vec<Failure>,
    pub warnings: Vec<String>,
    pub canceled: bool,
}

impl From<Batch> for BatchReport {
    /// A batch with nothing applied yet, keeping its planning failures.
    fn from(batch: Batch) -> Self {
        Self {
            total: batch.ready.len(),
            failures: batch.failures,
            ..Self::default()
        }
    }
}

/// Imports each ready game in order, calling `each` as it goes. A game that
/// fails is reported and the rest go on. Cancellation stops at the next
/// game and reports what finished. An error from `each` stops the batch and
/// is returned instead of a report.
pub fn apply_batch(
    paths: &Paths,
    mut batch: Batch,
    cancel: &Token,
    mut each: impl FnMut(Applying<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<BatchReport> {
    check_portable(paths)?;
    let mut imported = Imported::read(paths)?;
    let ready = std::mem::take(&mut batch.ready);
    let mut report = BatchReport {
        total: ready.len(),
        ..batch.into()
    };
    report
        .warnings
        .extend(retroarch_quit_file(paths, ready.iter()));
    for plan in ready {
        if cancel.is_canceled() {
            report.canceled = true;
            break;
        }
        each(Applying::Starting(&plan))?;
        match apply_with(paths, &plan, cancel, &mut imported) {
            Ok(ApplyOutcome::Imported(applied)) => {
                each(Applying::Imported(&plan, &applied))?;
                report.done.push(applied.environment.name);
                report.warnings.extend(applied.warnings);
            }
            Ok(ApplyOutcome::AlreadyImported(name)) => {
                each(Applying::AlreadyImported(&plan, &name))?;
                report.total -= 1;
                report.already.push(name);
            }
            Err(_) if cancel.is_canceled() => {
                report.canceled = true;
                break;
            }
            Err(error) => report.failures.push(Failure {
                rom: plan.rom,
                title: plan.title,
                name: Some(plan.name),
                reason: format!("{error:#}"),
            }),
        }
    }
    if cancel.is_canceled() {
        report.canceled = true;
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
