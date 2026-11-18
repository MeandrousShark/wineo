//! Add emulated games: pick an emulator, a folder or file of games, a
//! RetroArch core and a system when needed, then the games, like
//! `wineo import roms`.

mod view;
mod work;

use std::path::PathBuf;
use std::sync::Arc;

use iced::Task;
use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key};
use wineo_core::cancel::Token;
use wineo_core::download::Lazy;
use wineo_core::emulator::{Core, Emulator};
use wineo_core::paths::Paths;
use wineo_core::rom_import::{
    Batch, BatchReport, Emulating, Entry, EntryState, EntrySystem, Listing,
};

use super::Action;
use super::artwork::{self, Fetcher};
use super::import_grid::{GridKey, ImportGrid};
use crate::focus_button::ListRows;
use crate::task;
use work::{Chosen, install_label};
pub(crate) use work::{Found, Host, find};

const GRID_ID: &str = "emulated-grid";
const EMULATOR_LIST: &str = "emulated-emulators";
const CORE_LIST: &str = "emulated-cores";
const SYSTEM_LIST: &str = "emulated-systems";
const OTHER_ROW: &str = "emulated-other";
const CUSTOM_ROW: &str = "emulated-custom";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Emulator,
    /// A built-in emulator run from a file.
    Other,
    Custom,
    Folder,
    Core,
    System,
    Games,
    Review,
    Working,
    Finished,
}

/// How the emulator was chosen, so Back from the folder returns there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Found(usize),
    Other,
    Custom,
}

/// A built-in emulator as the "Other..." page's dropdown shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmulatorChoice(Emulator);

impl std::fmt::Display for EmulatorChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.name())
    }
}

#[derive(Debug, Clone, Default)]
struct CustomDraft {
    exe: String,
    args: String,
    extensions: String,
    system: String,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Found(Result<Found, String>),
    Install(usize),
    Other,
    Custom,
    OtherEmulator(EmulatorChoice),
    OtherFile(String),
    BrowseOtherFile,
    CustomExe(String),
    BrowseCustomExe,
    CustomArgs(String),
    CustomExtensions(String),
    CustomSystem(String),
    Folder(String),
    BrowseFolder,
    BrowseFile,
    Picked(u64, Result<Option<PathBuf>, String>),
    Next,
    Cores(u64, Result<Vec<Core>, String>),
    Core(usize),
    System(usize),
    Listed(u64, Result<Listing, String>),
    Toggle(usize),
    SelectAll,
    SelectNone,
    ImportSelected,
    Fullscreen(bool),
    FetchArt(bool),
    ProgressDue(u64),
    Planned(u64, Result<Batch, String>),
    Import,
    Cancel,
    Force,
    Progress(u64, String),
    Finished(u64, Result<BatchReport, String>),
    BackToLibrary,
}

pub(crate) struct Page {
    paths: Paths,
    found: Option<Found>,
    step: Step,
    generation: u64,
    via: Option<Via>,
    chosen: Option<Chosen>,
    other_emulator: Option<Emulator>,
    other_file: String,
    custom: CustomDraft,
    folder: String,
    /// The folder the cores and games were found in.
    listed_folder: Option<PathBuf>,
    cores: Option<Vec<Core>>,
    core: Option<Core>,
    system: Option<String>,
    listing: Option<Listing>,
    /// Picks ROM paths, by `rom_key`.
    grid: ImportGrid<Event>,
    fullscreen: bool,
    fetch_art: bool,
    review: Option<Batch>,
    report: Option<BatchReport>,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    action: Option<Action>,
    picker_sequence: u64,
    /// Fetches art after import. Tests swap in their own.
    fetcher: Option<Fetcher>,
    /// Whether a SteamGridDB key was there at the last update, for keys.
    key_available: bool,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        Self::start(paths, None)
    }

    /// The page looking for emulators with `host` instead of the host's own.
    #[cfg(test)]
    pub(crate) fn with_host(paths: Paths, host: Host) -> (Self, Task<Event>) {
        Self::start(paths, Some(host))
    }

    fn start(paths: Paths, host_override: Option<Host>) -> (Self, Task<Event>) {
        let page = Self {
            paths: paths.clone(),
            found: None,
            step: Step::Emulator,
            generation: 0,
            via: None,
            chosen: None,
            other_emulator: None,
            other_file: String::new(),
            custom: CustomDraft::default(),
            folder: String::new(),
            listed_folder: None,
            cores: None,
            core: None,
            system: None,
            listing: None,
            grid: ImportGrid::new(GRID_ID, Event::ProgressDue, 0),
            fullscreen: true,
            fetch_art: false,
            review: None,
            report: None,
            token: None,
            status: "Looking for emulators...".to_owned(),
            error: None,
            action: None,
            picker_sequence: 0,
            fetcher: None,
            key_available: false,
        };
        let task = task::blocking(move || find(&paths, host_override), Event::Found);
        (page, task)
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
    }

    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = "Force stop requested...".to_owned();
            } else {
                token.cancel();
                self.status = "Canceling...".to_owned();
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    fn host(&self) -> Option<&Host> {
        self.found.as_ref().map(|found| &found.host)
    }

    pub(crate) fn submit(&mut self, key_available: bool) -> Task<Event> {
        if self.busy() || self.waiting() {
            return Task::none();
        }
        match self.step {
            Step::Other | Step::Custom | Step::Folder => self.next(),
            Step::Games => self.import_selected(key_available),
            Step::Review => self.start_import(),
            Step::Finished => self.update(Event::BackToLibrary, key_available),
            Step::Emulator | Step::Core | Step::System | Step::Working => Task::none(),
        }
    }

    /// Whether the core page was shown, which only happens with a choice.
    fn core_asked(&self) -> bool {
        self.cores.as_ref().is_some_and(|cores| cores.len() > 1)
    }

    /// Whether some game's file could be more than one system, so the
    /// system page asks.
    fn system_needed(&self) -> bool {
        matches!(self.chosen, Some(Chosen::BuiltIn(..)))
            && self.listing.as_ref().is_some_and(|listing| {
                listing.entries.iter().any(|entry| {
                    entry.state == EntryState::New && entry.system == EntrySystem::Ambiguous
                })
            })
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        if self.step != Step::Emulator || self.found.is_some() {
            self.status.clear();
        }
        self.error = None;
        self.step = match self.step {
            Step::Emulator => {
                self.action = Some(Action::Choose);
                Step::Emulator
            }
            Step::Other | Step::Custom => Step::Emulator,
            Step::Folder => match self.via {
                Some(Via::Other) => Step::Other,
                Some(Via::Custom) => Step::Custom,
                _ => Step::Emulator,
            },
            Step::Core => Step::Folder,
            Step::System if self.core_asked() => Step::Core,
            Step::System => Step::Folder,
            Step::Games if self.system_needed() => Step::System,
            Step::Games if self.core_asked() => Step::Core,
            Step::Games => Step::Folder,
            Step::Review => Step::Games,
            Step::Working => Step::Working,
            Step::Finished => {
                self.action = Some(Action::Library {
                    name: None,
                    launch: false,
                });
                Step::Finished
            }
        };
        Task::none()
    }

    /// Forgets the answers after the emulator, which depend on it.
    fn forget_folder_answers(&mut self) {
        self.listed_folder = None;
        self.cores = None;
        self.core = None;
        self.forget_core_answers();
    }

    /// Forgets the answers after the core: the games and their system.
    fn forget_core_answers(&mut self) {
        self.system = None;
        self.listing = None;
        self.grid.clear();
        self.review = None;
    }

    fn choose(&mut self, chosen: Chosen, via: Via) {
        if self.chosen.as_ref() != Some(&chosen) {
            self.forget_folder_answers();
        }
        self.chosen = Some(chosen);
        self.via = Some(via);
        self.error = None;
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        self.step = Step::Folder;
    }

    fn next(&mut self) -> Task<Event> {
        let Some(host) = self.host().cloned() else {
            return Task::none();
        };
        self.error = None;
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        match self.step {
            Step::Other => {
                let Some(emulator) = self.other_emulator else {
                    self.error = Some("Choose which emulator it is.".to_owned());
                    return Task::none();
                };
                match work::pick_file(&self.paths, &host, emulator, &self.other_file) {
                    Ok(chosen) => self.choose(chosen, Via::Other),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Step::Custom => {
                let draft = &self.custom;
                match work::custom(
                    &self.paths,
                    &host,
                    &draft.exe,
                    &draft.args,
                    &draft.extensions,
                    &draft.system,
                ) {
                    Ok(chosen) => self.choose(chosen, Via::Custom),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Step::Folder => self.find_games(host),
            _ => Task::none(),
        }
    }

    /// Looks at the folder: RetroArch's cores first, then the games.
    fn find_games(&mut self, host: Host) -> Task<Event> {
        let Some(chosen) = self.chosen.clone() else {
            return Task::none();
        };
        let folder = match work::folder(&self.paths, &self.folder) {
            Ok(folder) => folder,
            Err(error) => {
                self.error = Some(error);
                return Task::none();
            }
        };
        if self.listed_folder.as_ref() != Some(&folder) {
            self.forget_folder_answers();
            self.listed_folder = Some(folder.clone());
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        match chosen.retroarch() {
            Some(install) => {
                self.status = "Looking for RetroArch cores...".to_owned();
                let install = install.clone();
                task::blocking(
                    move || work::matching_cores(&paths, &host, &install, &folder),
                    move |result| Event::Cores(generation, result),
                )
            }
            None => self.list(),
        }
    }

    fn list(&mut self) -> Task<Event> {
        let (Some(chosen), Some(folder)) = (&self.chosen, self.listed_folder.clone()) else {
            return Task::none();
        };
        let scan = chosen.scan(self.core.as_ref());
        self.status = "Looking for games...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        task::blocking(
            move || work::listing(&paths, &scan, &folder),
            move |result| Event::Listed(generation, result),
        )
    }

    fn pick_core(&mut self, core: Core) -> Task<Event> {
        if self.core.as_ref() != Some(&core) {
            self.forget_core_answers();
        }
        self.core = Some(core);
        self.list()
    }

    fn listed(&mut self, listing: Listing) {
        // Picks of games no longer listed, or no longer new, are dropped.
        self.grid.chosen.retain(|rom| {
            listing
                .entries
                .iter()
                .any(|entry| entry.state == EntryState::New && rom_key(&entry.rom) == *rom)
        });
        self.grid.focus = self
            .grid
            .focus
            .filter(|index| *index < listing.entries.len());
        let systems = self.chosen.as_ref().map(Chosen::systems).unwrap_or(&[]);
        if self
            .system
            .as_ref()
            .is_some_and(|system| !systems.contains(&system.as_str()))
        {
            self.system = None;
        }
        self.listing = Some(listing);
        self.step = if self.system_needed() {
            Step::System
        } else {
            Step::Games
        };
    }

    fn eligible(&self) -> impl Iterator<Item = String> + '_ {
        self.listing
            .iter()
            .flat_map(|listing| listing.entries.iter())
            .filter(|entry| entry.state == EntryState::New)
            .map(|entry| rom_key(&entry.rom))
    }

    fn toggle(&mut self, index: usize) {
        let Some(entry) = self
            .listing
            .as_ref()
            .and_then(|listing| listing.entries.get(index))
        else {
            return;
        };
        if entry.state != EntryState::New {
            return;
        }
        let rom = rom_key(&entry.rom);
        self.grid.toggle(&rom);
    }

    /// What planning the chosen games takes: how they run, the games, and
    /// the system for files that could be either.
    fn plan_inputs(&self) -> Option<(Emulating, Vec<Entry>, Option<String>)> {
        let (chosen, listing) = (self.chosen.as_ref()?, self.listing.as_ref()?);
        let emulating = chosen.emulating(self.core.as_ref(), self.fullscreen)?;
        let entries = listing
            .entries
            .iter()
            .filter(|entry| self.grid.chosen.contains(&rom_key(&entry.rom)))
            .cloned()
            .collect();
        // A custom emulator's system is its games' tag, not a choice.
        let system = match chosen {
            Chosen::Custom { .. } => None,
            Chosen::BuiltIn(..) => self.system.clone(),
        };
        Some((emulating, entries, system))
    }

    fn import_selected(&mut self, key_available: bool) -> Task<Event> {
        if self.waiting() || self.grid.chosen.is_empty() {
            return Task::none();
        }
        let Some((emulating, entries, system)) = self.plan_inputs() else {
            return Task::none();
        };
        if !(self.art_enabled() && key_available) {
            self.fetch_art = false;
        }
        self.error = None;
        self.status = "Checking games...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        task::blocking(
            move || work::plan(&paths, &emulating, entries, system.as_deref()),
            move |result| Event::Planned(generation, result),
        )
    }

    fn art_enabled(&self) -> bool {
        self.found.as_ref().is_some_and(|found| found.art_enabled)
    }

    fn start_import(&mut self) -> Task<Event> {
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        if self.busy() || self.waiting() || review.ready.is_empty() {
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.status = "Importing games...".to_owned();
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let fetch = self.fetch_art.then(|| {
            self.fetcher.clone().unwrap_or_else(|| {
                // One client for every game's art, not one per game.
                // The games are fetched one at a time, so the lock never waits.
                let https = std::sync::Mutex::new(Lazy::new());
                let fetcher: Fetcher = Arc::new(
                    move |paths: &Paths, name: &str, title: &str, token: &Token| {
                        let https = https.lock().unwrap_or_else(|poison| poison.into_inner());
                        artwork::fetch_default(paths, name, title, &https, token)
                    },
                );
                fetcher
            })
        });
        let (sender, receiver) = unbounded();
        let work = task::blocking(
            move || {
                work::run_import(&paths, review, &token, fetch.as_ref(), |message| {
                    let _ = sender.unbounded_send(Event::Progress(generation, message));
                })
            },
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn browse(&mut self, title: &str, directory: bool) -> Task<Event> {
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        let sequence = self.picker_sequence;
        Task::perform(
            crate::picker::pick(title.to_owned(), directory),
            move |result| Event::Picked(sequence, result),
        )
    }

    pub(crate) fn update(&mut self, event: Event, key_available: bool) -> Task<Event> {
        self.key_available = key_available;
        match event {
            Event::Back => {
                // The Back button is a click, so the grid it returns to shows no ring.
                self.grid.focus_visible = false;
                self.back()
            }
            Event::Found(result) => {
                if self.step == Step::Emulator {
                    self.status.clear();
                }
                match result {
                    Ok(found) => self.found = Some(found),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Install(index) if self.step == Step::Emulator => {
                if let Some((emulator, install)) = self
                    .found
                    .as_ref()
                    .and_then(|found| found.installs.get(index))
                    .cloned()
                {
                    self.choose(Chosen::BuiltIn(emulator, install), Via::Found(index));
                }
                Task::none()
            }
            Event::Other if self.step == Step::Emulator && self.found.is_some() => {
                self.error = None;
                self.step = Step::Other;
                Task::none()
            }
            Event::Custom if self.step == Step::Emulator && self.found.is_some() => {
                self.error = None;
                self.step = Step::Custom;
                Task::none()
            }
            Event::OtherEmulator(choice) => {
                self.other_emulator = Some(choice.0);
                Task::none()
            }
            Event::OtherFile(value) => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                self.other_file = value;
                Task::none()
            }
            Event::CustomExe(value) => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                self.custom.exe = value;
                Task::none()
            }
            Event::CustomArgs(value) => {
                self.custom.args = value;
                Task::none()
            }
            Event::CustomExtensions(value) => {
                self.custom.extensions = value;
                Task::none()
            }
            Event::CustomSystem(value) => {
                self.custom.system = value;
                Task::none()
            }
            Event::Folder(value) => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                // A lookup of the folder as it was can't land.
                if self.step == Step::Folder && self.waiting() {
                    self.generation = self.generation.wrapping_add(1);
                    self.status.clear();
                }
                self.folder = value;
                Task::none()
            }
            Event::BrowseOtherFile if self.step == Step::Other => {
                self.browse("Emulator file", false)
            }
            Event::BrowseCustomExe if self.step == Step::Custom => {
                self.browse("Emulator program", false)
            }
            Event::BrowseFolder if self.step == Step::Folder => {
                self.browse("Folder of games", true)
            }
            Event::BrowseFile if self.step == Step::Folder => self.browse("Game file", false),
            Event::Picked(sequence, result) if sequence == self.picker_sequence => {
                match result {
                    Ok(Some(path)) => {
                        let path = path.display().to_string();
                        match self.step {
                            Step::Other => self.other_file = path,
                            Step::Custom => self.custom.exe = path,
                            Step::Folder => self.folder = path,
                            _ => {}
                        }
                    }
                    Ok(None) => {}
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Next if matches!(self.step, Step::Other | Step::Custom | Step::Folder) => {
                if self.waiting() {
                    return Task::none();
                }
                self.next()
            }
            Event::Cores(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(cores) => {
                        let kept = self
                            .core
                            .clone()
                            .filter(|core| cores.iter().any(|each| each == core));
                        self.cores = Some(cores.clone());
                        match (cores.as_slice(), kept) {
                            ([one], _) => self.pick_core(one.clone()),
                            (_, kept) => {
                                self.core = kept;
                                self.step = Step::Core;
                                Task::none()
                            }
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Core(index) if self.step == Step::Core && !self.waiting() => {
                match self.cores.as_ref().and_then(|cores| cores.get(index)) {
                    Some(core) => {
                        let core = core.clone();
                        self.pick_core(core)
                    }
                    None => Task::none(),
                }
            }
            Event::Listed(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(listing) => self.listed(listing),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::System(index) if self.step == Step::System => {
                let systems = self.chosen.as_ref().map(Chosen::systems).unwrap_or(&[]);
                if let Some(system) = systems.get(index) {
                    self.system = Some((*system).to_owned());
                    self.review = None;
                    self.step = Step::Games;
                }
                Task::none()
            }
            Event::Toggle(index) if self.step == Step::Games && !self.waiting() => {
                self.grid.click(index);
                self.toggle(index);
                Task::none()
            }
            Event::SelectAll if self.step == Step::Games && !self.waiting() => {
                self.grid.chosen = self.eligible().collect();
                Task::none()
            }
            Event::SelectNone if self.step == Step::Games && !self.waiting() => {
                self.grid.chosen.clear();
                Task::none()
            }
            Event::ImportSelected if self.step == Step::Games => {
                self.import_selected(key_available)
            }
            Event::Fullscreen(on) if self.step == Step::Games && !self.waiting() => {
                self.fullscreen = on;
                Task::none()
            }
            Event::FetchArt(on) if self.step == Step::Games && !self.waiting() => {
                self.fetch_art = on && self.art_enabled() && key_available;
                Task::none()
            }
            Event::ProgressDue(generation) => {
                self.grid.progress.reveal(generation);
                Task::none()
            }
            Event::Planned(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(review) => {
                        self.review = Some(review);
                        self.step = Step::Review;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Import if self.step == Step::Review => self.start_import(),
            Event::Cancel if self.step == Step::Working => {
                self.cancel(false);
                Task::none()
            }
            Event::Force if self.step == Step::Working => {
                self.cancel(true);
                Task::none()
            }
            Event::Progress(generation, message)
                if generation == self.generation && self.step == Step::Working =>
            {
                if !self.token.as_ref().is_some_and(Token::is_canceled) {
                    self.status = message;
                }
                Task::none()
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.step = Step::Finished;
                match result {
                    Ok(report) => self.report = Some(report),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::BackToLibrary if self.step == Step::Finished => {
                self.action = Some(Action::Library {
                    name: None,
                    launch: false,
                });
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub(crate) fn key(
        &mut self,
        key: &Key,
        modifiers: keyboard::Modifiers,
        width: f32,
    ) -> Option<Task<Event>> {
        let key_available = self.key_available;
        if self.step != Step::Games || self.waiting() {
            return None;
        }
        let count = self.listing.as_ref()?.entries.len();
        Some(match self.grid.key(key, modifiers, width, count)? {
            GridKey::Moved(task) => task,
            GridKey::Toggle(focus) => {
                if let Some(index) = focus {
                    let _ = self.update(Event::Toggle(index), key_available);
                    self.grid.focus_visible = true;
                }
                Task::none()
            }
            GridKey::SelectAll => self.update(Event::SelectAll, key_available),
            GridKey::Import => self.import_selected(key_available),
        })
    }

    fn install_id(&self, index: usize) -> String {
        let (emulator, install) = &self.found.as_ref().expect("listed").installs[index];
        format!(
            "emulated-install:{}:{}",
            emulator.id(),
            work::install_detail(*emulator, install)
        )
    }

    fn core_id(core: &Core) -> String {
        format!("emulated-core:{}", core.path.display())
    }

    fn system_id(system: &str) -> String {
        format!("emulated-system:{system}")
    }

    /// The choice list the page shows, if any, for where focus lands.
    pub(crate) fn rows(&self) -> Option<ListRows> {
        match self.step {
            Step::Emulator => {
                let found = self.found.as_ref()?;
                let mut ids: Vec<String> = (0..found.installs.len())
                    .map(|index| self.install_id(index))
                    .collect();
                ids.extend([OTHER_ROW.to_owned(), CUSTOM_ROW.to_owned()]);
                let focus = self.via.map(|via| match via {
                    Via::Found(index) if index < found.installs.len() => self.install_id(index),
                    Via::Found(_) | Via::Other => OTHER_ROW.to_owned(),
                    Via::Custom => CUSTOM_ROW.to_owned(),
                });
                Some(ListRows {
                    list: EMULATOR_LIST,
                    ids,
                    focus,
                })
            }
            Step::Core => {
                let cores = self.cores.as_ref()?;
                Some(ListRows {
                    list: CORE_LIST,
                    ids: cores.iter().map(Self::core_id).collect(),
                    focus: self.core.as_ref().map(Self::core_id),
                })
            }
            Step::System => Some(ListRows {
                list: SYSTEM_LIST,
                ids: self
                    .chosen
                    .as_ref()
                    .map(Chosen::systems)
                    .unwrap_or(&[])
                    .iter()
                    .map(|system| Self::system_id(system))
                    .collect(),
                focus: self.system.as_deref().map(Self::system_id),
            }),
            _ => None,
        }
    }

    /// The emulator and core the review names.
    fn emulator_label(&self) -> String {
        match (&self.chosen, &self.core) {
            (Some(Chosen::BuiltIn(emulator, install)), Some(core)) => {
                format!(
                    "{}, core {}",
                    install_label(*emulator, install),
                    core.name()
                )
            }
            (Some(Chosen::BuiltIn(emulator, install)), None) => install_label(*emulator, install),
            (Some(Chosen::Custom { exe, .. }), _) => format!("Custom ({exe})"),
            (None, _) => String::new(),
        }
    }
}

/// A ROM's key among the picks: its path's bytes in hex, so names that
/// aren't UTF-8 stay apart.
fn rom_key(rom: &std::path::Path) -> String {
    use std::fmt::Write;
    rom.as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(String::new(), |mut key, byte| {
            let _ = write!(key, "{byte:02x}");
            key
        })
}

#[cfg(test)]
mod tests;
