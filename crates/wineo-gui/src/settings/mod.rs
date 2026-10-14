//! The settings page, which replaces the library while open.

mod editor;
mod form;
mod global;
pub(crate) mod gog;
mod handle;
mod load;
mod runner_picker;
pub(crate) mod steam_key;
mod steamgriddb;
mod view;

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::components::Component;
use wineo_core::config::{Arch, ArtFetch};
use wineo_core::paths::Paths;

use crate::{App, Message};
pub(crate) use editor::{Editor, NewTarget};
use form::{Build, Choice, Field, List, Switch, Values};
pub(crate) use load::desktop_line;
use load::{ProgramChoice, Report, RunnerChoice, Scan, Snapshot, Texts};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Discard,
    KeepEditing,
    Save,
    /// Throws edits away for what's on disk, once the file changed under them.
    Reload,
    Revert,
    RunnerPicked(RunnerChoice),
    Switch(Switch, bool),
    Reset(Switch),
    Gamescope(bool),
    FieldText(Field, String),
    FieldChoice(Field, Choice),
    GrabCursor(bool),
    ArgsInput(String),
    ArgsAdd,
    ArgsRemove(usize),
    BuildPicked(Component, Build),
    BuildRemoved(Component),
    ListInput(List, String),
    ListAdd(List),
    ListRemove(List, usize),
    ProxiedTitle(String),
    ProxiedDesktop(bool),
    EnvKey(usize, String),
    EnvValue(usize, String),
    EnvRemove(usize),
    NewEnvKey(String),
    NewEnvValue(String),
    EnvAdd,
    Prefix(String),
    GameId(String),
    Store(String),
    GlobalSwitch(Switch, bool),
    GlobalRunner(RunnerChoice),
    GlobalArch(Arch),
    GlobalField(global::Field, String),
    GlobalArt(ArtFetch),
    GlobalArtKind(wineo_core::art::Kind, bool),
    GlobalExternalRunners(bool),
    /// Opens a folder picker for the Games folder.
    BrowseGamesRoot,
    /// The page's generation, the field when the picker opened, and the pick.
    PickedGamesRoot(u64, String, Result<Option<std::path::PathBuf>, String>),
    SteamGridDbKeySetOpen,
    SteamGridDbKeySetCancel,
    SteamGridDbKeyText(steamgriddb::Secret),
    SteamGridDbKeySet,
    SteamGridDbKeyRemove,
    SteamGridDbKeyStatusLoaded(u64, u64, Result<steamgriddb::Status, String>),
    SteamGridDbKeySaved(u64, u64, Option<u64>, Result<steamgriddb::Saved, String>),
    SteamGridDbKeyRemoved(u64, u64, Option<u64>, Result<steamgriddb::Removed, String>),
    SteamKeySetOpen,
    SteamKeySetCancel,
    SteamKeyText(steamgriddb::Secret),
    SteamKeySet,
    SteamKeyRemove,
    SteamKeyStatusLoaded(u64, u64, Result<steam_key::Status, String>),
    SteamKeySaved(u64, u64, Option<u64>, Result<steam_key::Saved, String>),
    SteamKeyRemoved(u64, u64, Option<u64>, Result<steam_key::Removed, String>),
    /// Where the Steam key row is, to scroll it into view.
    SteamKeyLocated(u64, crate::grid_motion::CardBounds),
    /// The GOG login row.
    Gog(gog::Event),
    /// An edit on one target's card.
    Target(String, TargetEdit),
    /// Opens or closes a target's card, which only changes the view.
    Card(String, bool),
    /// Backs out of removing a target.
    KeepTarget,
    /// Opens or closes the add target form, scanning the prefix on opening.
    AddOpen(bool),
    /// Shows programs held back as noise, like uninstallers.
    ShowHeldBack(bool),
    Scanned(u64, u64, Scan),
    ProgramPicked(ProgramChoice),
    NewTargetName(String),
    NewTargetPath(String),
    BrowsePath(Option<String>, bool),
    PickedPath(
        u64,
        u64,
        Option<String>,
        bool,
        String,
        Result<Option<std::path::PathBuf>, String>,
    ),
    TargetAdd,
    /// The page's generation, the check's sequence, and the typed exe as a
    /// target holds it.
    Resolved(u64, u64, Result<String, String>),
    MenuOpened,
    MenuClosed,
    OpenConfig,
    Opened(Result<(), String>),
    Loaded(u64, Snapshot),
    /// The page's generation and the check's sequence number.
    Checked(u64, u64, Texts),
    CheckFailed(u64, u64),
    Saved(u64, Result<Report, String>),
}

/// An edit to one target.
#[derive(Debug, Clone)]
pub(crate) enum TargetEdit {
    Exe(String),
    Cwd(String),
    Name(String),
    Desktop(bool),
    Default(bool),
    Switch(Switch, bool),
    Reset(Switch),
    Gamescope(bool),
    GamescopeReset,
    /// Copies the environment's gamescope fields in, to edit them here.
    GamescopeHere,
    FieldText(Field, String),
    FieldChoice(Field, Choice),
    GrabCursor(bool),
    GamescopeArgsInput(String),
    GamescopeArgsAdd,
    GamescopeArgsRemove(usize),
    ArgsInput(String),
    ArgsAdd,
    ArgsRemove(usize),
    /// Shows what goes with the target before removing it.
    AskRemove,
    Remove,
}

impl Event {
    fn steamgriddb_token(&self) -> Option<(u64, u64)> {
        match self {
            Event::SteamGridDbKeyStatusLoaded(generation, sequence, _)
            | Event::SteamGridDbKeySaved(generation, sequence, _, _)
            | Event::SteamGridDbKeyRemoved(generation, sequence, _, _) => {
                Some((*generation, *sequence))
            }
            _ => None,
        }
    }

    fn rebind_steamgriddb(&self, generation: u64) -> Option<Self> {
        match self {
            Event::SteamGridDbKeyStatusLoaded(_, sequence, result) => Some(
                Event::SteamGridDbKeyStatusLoaded(generation, *sequence, result.clone()),
            ),
            Event::SteamGridDbKeySaved(_, sequence, session, result) => Some(
                Event::SteamGridDbKeySaved(generation, *sequence, *session, result.clone()),
            ),
            Event::SteamGridDbKeyRemoved(_, sequence, session, result) => Some(
                Event::SteamGridDbKeyRemoved(generation, *sequence, *session, result.clone()),
            ),
            _ => None,
        }
    }

    /// The sequence of a finished Steam key save or remove.
    fn steam_key_operation(&self) -> Option<u64> {
        match self {
            Event::SteamKeySaved(_, sequence, _, _) | Event::SteamKeyRemoved(_, sequence, _, _) => {
                Some(*sequence)
            }
            _ => None,
        }
    }

    fn rebind_steam_key(self, generation: u64) -> Self {
        match self {
            Event::SteamKeySaved(_, sequence, session, result) => {
                Event::SteamKeySaved(generation, sequence, session, result)
            }
            Event::SteamKeyRemoved(_, sequence, session, result) => {
                Event::SteamKeyRemoved(generation, sequence, session, result)
            }
            other => other,
        }
    }

    fn steamgriddb_input_session(&self) -> Option<u64> {
        match self {
            Event::SteamGridDbKeySaved(_, _, session, _)
            | Event::SteamGridDbKeyRemoved(_, _, session, _) => *session,
            _ => None,
        }
    }

    /// Whether this changes the draft, as opposed to moving the page along.
    fn edits(&self) -> bool {
        matches!(
            self,
            Event::RunnerPicked(_)
                | Event::Switch(..)
                | Event::Reset(_)
                | Event::Gamescope(_)
                | Event::FieldText(..)
                | Event::FieldChoice(..)
                | Event::GrabCursor(_)
                | Event::ArgsInput(_)
                | Event::ArgsAdd
                | Event::ArgsRemove(_)
                | Event::BuildPicked(..)
                | Event::BuildRemoved(_)
                | Event::ListInput(..)
                | Event::ListAdd(_)
                | Event::ListRemove(..)
                | Event::ProxiedTitle(_)
                | Event::ProxiedDesktop(_)
                | Event::EnvKey(..)
                | Event::EnvValue(..)
                | Event::EnvRemove(_)
                | Event::NewEnvKey(_)
                | Event::NewEnvValue(_)
                | Event::EnvAdd
                | Event::Prefix(_)
                | Event::GameId(_)
                | Event::Store(_)
                | Event::GlobalSwitch(..)
                | Event::GlobalRunner(_)
                | Event::GlobalArch(_)
                | Event::GlobalField(..)
                | Event::GlobalArt(_)
                | Event::GlobalArtKind(_, _)
                | Event::GlobalExternalRunners(_)
                | Event::Target(..)
                | Event::ProgramPicked(_)
                | Event::NewTargetName(_)
                | Event::NewTargetPath(_)
                | Event::TargetAdd
        )
    }
    /// Edits to keys a native environment can't have.
    fn wine_only(&self) -> bool {
        matches!(
            self,
            Event::BuildPicked(..)
                | Event::BuildRemoved(_)
                | Event::Switch(Switch::Wayland, _)
                | Event::Reset(Switch::Wayland)
                | Event::ListInput(List::Winetricks, _)
                | Event::ListAdd(List::Winetricks)
                | Event::ListRemove(List::Winetricks, _)
                | Event::Prefix(_)
                | Event::GameId(_)
                | Event::Store(_)
                | Event::ProgramPicked(_)
                | Event::Target(
                    _,
                    TargetEdit::Switch(Switch::Wayland, _) | TargetEdit::Reset(Switch::Wayland)
                )
        )
    }
}

pub(crate) enum View {
    Loading,
    Form(Box<Editor>),
    Global(Box<global::Editor>),
    Error(String),
}

/// How a finished load lands on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The file becomes the page, unless it was edited while loading.
    Replace,
    /// What a successful save wrote becomes the page. The controls are
    /// locked until then, so there are no edits to keep.
    AfterSave,
    /// The file becomes the base the edits are compared with, and the edits
    /// stay. For a save that failed partway and for global config changes.
    Rebase,
}

struct Pending {
    mode: Mode,
    /// The draft when the load started, to tell edits made since.
    draft: Option<Values>,
    global_draft: Option<global::Values>,
    /// Follows a save, so the controls stay locked until it lands.
    after_save: bool,
}

pub(crate) struct Page {
    pub(crate) name: String,
    global: bool,
    /// Tells this page's worker results from those of a page closed earlier.
    generation: u64,
    pub(crate) view: View,
    /// The files the shown state was read from, `None` before the first load.
    disk: Option<Texts>,
    pending: Option<Pending>,
    checking: bool,
    /// Bumped by every check, load, and save, so a check that was reading
    /// while one of those ran can't land on top of it with stale text.
    check_sequence: u64,
    pub(crate) saving: bool,
    pub(crate) work_refusal: Option<String>,
    pub(crate) changed_on_disk: bool,
    pub(crate) leave_prompt: bool,
    /// A pick list's menu is open, so Escape belongs to it.
    menu_open: bool,
    /// What the last save did.
    pub(crate) notes: Vec<String>,
    pub(crate) error: Option<String>,
    /// Bumped by every typed exe check, so only the latest one lands.
    resolve_sequence: u64,
    /// Bumped by every scan, so an earlier prefix's result cannot replace it.
    scan_sequence: u64,
    steamgriddb_sequence: u64,
    steamgriddb_input_session_sequence: u64,
    steamgriddb_input_session: Option<u64>,
    steamgriddb_input_session_touched: bool,
    steamgriddb_busy_sequence: Option<u64>,
    steamgriddb: steamgriddb::Settings,
    /// Only the latest Steam key status lands. Saves and removes belong to
    /// the app (`SteamKeyWork`), which hands their results to this page.
    steam_key_sequence: u64,
    steam_key_busy_sequence: Option<u64>,
    steam_key_input_session: Option<u64>,
    steam_key: steam_key::Settings,
    gog: gog::Settings,
    /// Scrolls to the Steam key row once the page loads.
    pub(crate) reveal_steam_key: bool,
    /// The add target form as it was sent to be checked.
    resolving: Option<NewTarget>,
    pub(crate) open_add_after_load: bool,
}

/// The Steam key save or remove the app owns, so it outlives the page that
/// started it. Its result lands on whichever global page is open, or waits
/// for the next one, and no second operation starts while it runs.
#[derive(Default)]
pub(crate) struct SteamKeyWork {
    /// The highest sequence any page used, so pages never reuse one.
    sequence: u64,
    pending: Option<(u64, steamgriddb::Busy)>,
    late: Option<Event>,
}

impl SteamKeyWork {
    #[cfg(test)]
    pub(crate) fn pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// The GOG login or logout the app owns, like `SteamKeyWork`.
#[derive(Default)]
pub(crate) struct GogWork {
    sequence: u64,
    pending: Option<(u64, gog::Busy)>,
    late: Option<gog::Event>,
}

impl GogWork {
    #[cfg(test)]
    pub(crate) fn pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// What the app does after the page handles an event.
pub(crate) struct Step {
    task: Task<Message>,
    close: bool,
}

impl Step {
    fn none() -> Self {
        Self {
            task: Task::none(),
            close: false,
        }
    }
    fn task(task: Task<Message>) -> Self {
        Self { task, close: false }
    }
    fn close() -> Self {
        Self {
            task: Task::none(),
            close: true,
        }
    }
}

fn unfocus() -> Task<Message> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus::<()>())
        .discard()
}

impl Page {
    fn new(name: String, generation: u64) -> Self {
        Self {
            name,
            global: false,
            generation,
            view: View::Loading,
            disk: None,
            pending: None,
            checking: false,
            check_sequence: 0,
            saving: false,
            work_refusal: None,
            changed_on_disk: false,
            leave_prompt: false,
            menu_open: false,
            notes: Vec::new(),
            error: None,
            resolve_sequence: 0,
            scan_sequence: 0,
            steamgriddb_sequence: 0,
            steamgriddb_input_session_sequence: 0,
            steamgriddb_input_session: None,
            steamgriddb_input_session_touched: false,
            steamgriddb_busy_sequence: None,
            steamgriddb: steamgriddb::Settings::default(),
            steam_key_sequence: 0,
            steam_key_busy_sequence: None,
            steam_key_input_session: None,
            steam_key: steam_key::Settings::default(),
            gog: gog::Settings::default(),
            reveal_steam_key: false,
            resolving: None,
            open_add_after_load: false,
        }
    }
    fn new_global(generation: u64) -> Self {
        let mut page = Self::new("Global".to_owned(), generation);
        page.global = true;
        page
    }
    pub(crate) fn is_global(&self) -> bool {
        self.global
    }
    pub(crate) fn resolving(&self) -> bool {
        self.resolving.is_some()
    }
    fn editor(&self) -> Option<&Editor> {
        match &self.view {
            View::Form(editor) => Some(editor),
            _ => None,
        }
    }
    fn editor_mut(&mut self) -> Option<&mut Editor> {
        match &mut self.view {
            View::Form(editor) => Some(editor),
            _ => None,
        }
    }
    pub(crate) fn loading(&self) -> bool {
        self.pending.is_some()
    }
    /// A save and the reload after it are in flight, so the controls take no
    /// edits: the reload would otherwise count them as already saved.
    pub(crate) fn locked(&self) -> bool {
        self.saving
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.after_save)
    }
    /// Any check reading now is stale by the time it lands.
    fn drop_checks(&mut self) {
        self.check_sequence = self.check_sequence.wrapping_add(1);
        self.checking = false;
    }
    fn bump_steamgriddb_sequence(&mut self) -> u64 {
        self.steamgriddb_sequence = self
            .steamgriddb_sequence
            .checked_add(1)
            .expect("SteamGridDB sequence exhausted");
        self.steamgriddb_sequence
    }
    fn bump_steamgriddb_input_session(&mut self) -> u64 {
        self.steamgriddb_input_session_sequence = self
            .steamgriddb_input_session_sequence
            .checked_add(1)
            .expect("SteamGridDB input session sequence exhausted");
        self.steamgriddb_input_session_sequence
    }
    fn refresh_steamgriddb(&mut self, paths: &Paths) -> Task<Message> {
        if !self.global || !matches!(self.view, View::Global(_)) {
            return Task::none();
        }
        let sequence = self.bump_steamgriddb_sequence();
        steamgriddb::status_task(paths.clone(), self.generation, sequence)
    }
    fn bump_steam_key_sequence(&mut self) -> u64 {
        self.steam_key_sequence = self.steam_key_sequence.wrapping_add(1);
        self.steam_key_sequence
    }
    fn refresh_steam_key(&mut self, paths: &Paths) -> Task<Message> {
        if !self.global || !matches!(self.view, View::Global(_)) {
            return Task::none();
        }
        let sequence = self.bump_steam_key_sequence();
        steam_key::status_task(paths.clone(), self.generation, sequence)
    }
    /// A key is being checked, saved, or removed, which spins the spinner.
    #[cfg(test)]
    pub(crate) fn key_busy(&self) -> bool {
        self.steamgriddb.busy.is_some() || self.steam_key.busy.is_some() || self.gog.busy.is_some()
    }
    fn refresh_gog(&mut self, paths: &Paths) -> Task<Message> {
        if !self.global || !matches!(self.view, View::Global(_)) {
            return Task::none();
        }
        self.gog.refresh(paths, self.generation)
    }
    fn gog_context(&self) -> gog::Context {
        gog::Context {
            generation: self.generation,
            idle: self.global && !self.locked() && !self.loading(),
        }
    }
    fn steam_key_idle(&self) -> bool {
        self.global && !self.locked() && !self.loading() && self.steam_key.busy.is_none()
    }
    pub(crate) fn is_dirty(&self) -> bool {
        match &self.view {
            View::Form(editor) => editor.form.is_dirty(),
            View::Global(editor) => editor.form.is_dirty(),
            _ => false,
        }
    }
    pub(crate) fn navigation_blocked(&self) -> bool {
        self.leave_prompt || self.menu_open || self.locked()
    }
    pub(crate) fn changes(&self) -> usize {
        match &self.view {
            View::Form(editor) => editor.form.changes(),
            View::Global(editor) => editor.form.changes(),
            _ => 0,
        }
    }
    pub(crate) fn has_file(&self) -> bool {
        self.disk.as_ref().is_some_and(|texts| {
            if self.global {
                texts.global.is_some()
            } else {
                texts.environment.is_some()
            }
        })
    }
    /// Why Save is off even though there are changes.
    pub(crate) fn problem(&self) -> Option<String> {
        if let Some(reason) = &self.work_refusal {
            return Some(reason.clone());
        }
        if let View::Global(editor) = &self.view {
            return editor.form.problem();
        }
        let editor = self.editor()?;
        // On Proton the builds can only be removed, so that reason comes
        // before one that points at a disabled picker.
        Component::ALL
            .into_iter()
            .filter(|component| editor.form.draft.build(*component).is_some())
            .find_map(|component| editor.build_refusal(&self.name, component))
            .or_else(|| {
                (!editor.proton())
                    .then(|| editor.form.protonfixes_problem())
                    .flatten()
                    .map(str::to_owned)
            })
            .or_else(|| editor.form.problem(editor.draft_runner().is_some()))
            .or_else(|| editor.form.targets_problem(editor.info.portable))
    }
    pub(crate) fn can_save(&self) -> bool {
        !self.saving
            && !self.loading()
            && !self.changed_on_disk
            && self.is_dirty()
            && self.problem().is_none()
    }
    pub(crate) fn can_reload(&self) -> bool {
        !self.saving && !self.loading() && self.changed_on_disk
    }
    pub(crate) fn global_warning(&self) -> Option<&str> {
        self.editor()
            .and_then(|editor| editor.info.global_warning.as_deref())
    }
    fn reload(&mut self, paths: &Paths, mode: Mode) -> Task<Message> {
        self.reload_after(paths, mode, false)
    }
    fn reload_after(&mut self, paths: &Paths, mode: Mode, after_save: bool) -> Task<Message> {
        if self.loading() {
            return Task::none();
        }
        self.drop_checks();
        self.menu_open = false;
        self.pending = Some(Pending {
            mode,
            draft: self.editor().map(|editor| editor.form.draft.clone()),
            global_draft: match &self.view {
                View::Global(editor) => Some(editor.form.draft.clone()),
                _ => None,
            },
            after_save,
        });
        load::load_task(
            paths.clone(),
            self.name.clone(),
            self.global,
            self.generation,
        )
    }
    /// A stale save report would describe edits that are no longer there.
    fn edited(&mut self) {
        self.notes.clear();
        self.error = None;
    }
    fn draft(&mut self) -> Option<&mut Values> {
        self.editor_mut().map(|editor| &mut editor.form.draft)
    }
    fn tick(&mut self, paths: &Paths) -> Task<Message> {
        if self.loading() || self.saving || self.checking || self.disk.is_none() {
            return Task::none();
        }
        self.check_sequence = self.check_sequence.wrapping_add(1);
        self.checking = true;
        load::check_task(
            paths.clone(),
            self.name.clone(),
            self.global,
            self.generation,
            self.check_sequence,
        )
    }
    fn update(&mut self, paths: &Paths, event: Event) -> Step {
        let step = self.handle(paths, event);
        if !self.is_dirty() {
            self.leave_prompt = false;
        }
        step
    }
}

impl App {
    fn track_steamgriddb(&mut self, generation: u64, sequence: u64) {
        self.steamgriddb_pending.insert((generation, sequence));
    }

    pub(crate) fn open_settings(&mut self) -> Task<Message> {
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        self.leave_art();
        self.tool_page = None;
        self.portable_page = None;
        self.trash_page.cancel_review();
        self.downloads_page.cancel_review();
        self.section = crate::sidebar::Section::Library;
        self.settings_generation = self.settings_generation.wrapping_add(1);
        let mut page = Page::new(name, self.settings_generation);
        let task = page.reload(&self.paths, Mode::Replace);
        self.settings = Some(page);
        task
    }
    pub(crate) fn open_global_settings(&mut self) -> Task<Message> {
        self.leave_art();
        self.tool_page = None;
        self.portable_page = None;
        self.trash_page.cancel_review();
        self.downloads_page.cancel_review();
        self.section = crate::sidebar::Section::Library;
        self.settings_generation = self.settings_generation.wrapping_add(1);
        let mut page = Page::new_global(self.settings_generation);
        page.steamgriddb_sequence = self.steamgriddb_sequence;
        page.steamgriddb_input_session_sequence = self.steamgriddb_input_session_sequence;
        let mut task = page.reload(&self.paths, Mode::Replace);
        page.steam_key_sequence = self.steam_key_work.sequence;
        if let Some((sequence, busy)) = self.steam_key_work.pending {
            page.steam_key.busy = Some(busy);
            page.steam_key_busy_sequence = Some(sequence);
        }
        if let Some(event) = self.steam_key_work.late.take()
            && let Some(sequence) = event.steam_key_operation()
        {
            page.steam_key_busy_sequence = Some(sequence);
            let step = page.update(&self.paths, event.rebind_steam_key(page.generation));
            task = Task::batch([task, step.task]);
        }
        self.steam_key_work.sequence = self.steam_key_work.sequence.max(page.steam_key_sequence);
        page.gog.sequence = self.gog_work.sequence;
        if let Some((sequence, busy)) = self.gog_work.pending {
            page.gog.busy = Some(busy);
            page.gog.busy_sequence = Some(sequence);
        }
        if let Some(event) = self.gog_work.late.take()
            && let Some(sequence) = event.operation()
        {
            page.gog.busy_sequence = Some(sequence);
            let step = page.update(&self.paths, Event::Gog(event.rebind(page.generation)));
            task = Task::batch([task, step.task]);
        }
        self.gog_work.sequence = self.gog_work.sequence.max(page.gog.sequence);
        if let Some(event) = self.steamgriddb_late_result.take() {
            if !page.steamgriddb_input_session_touched
                && let Some(session) = event.steamgriddb_input_session()
            {
                page.steamgriddb_input_session = Some(session);
                page.steamgriddb_input_session_touched = true;
            }
            if let Some(event) = event.rebind_steamgriddb(page.generation) {
                let step = page.update(&self.paths, event);
                task = Task::batch([task, step.task]);
            }
        }
        self.steamgriddb_sequence = self.steamgriddb_sequence.max(page.steamgriddb_sequence);
        self.steamgriddb_input_session_sequence = self
            .steamgriddb_input_session_sequence
            .max(page.steamgriddb_input_session_sequence);
        self.settings = Some(page);
        task
    }
    /// Global settings, scrolled to the Steam Web API key row once loaded.
    pub(crate) fn open_steam_key_settings(&mut self) -> Task<Message> {
        let task = self.navigate_section(crate::sidebar::Section::Settings);
        if let Some(page) = self.settings.as_mut().filter(|page| page.is_global()) {
            page.reveal_steam_key = true;
        }
        task
    }
    pub(crate) fn settings_update(&mut self, event: Event) -> Task<Message> {
        self.update_settings_work_refusal();
        if matches!(event, Event::SteamKeySet | Event::SteamKeyRemove)
            && self.steam_key_work.pending.is_some()
        {
            return Task::none();
        }
        if let Event::Gog(gog_event) = &event {
            if matches!(gog_event, gog::Event::Login | gog::Event::Logout)
                && self.gog_work.pending.is_some()
            {
                return Task::none();
            }
            if let Some(sequence) = gog_event.operation() {
                if self
                    .gog_work
                    .pending
                    .is_some_and(|(pending, _)| pending == sequence)
                {
                    self.gog_work.pending = None;
                }
                let Event::Gog(gog_event) = event else {
                    unreachable!()
                };
                return match self.settings.as_mut().filter(|page| page.is_global()) {
                    Some(page) => {
                        let event = Event::Gog(gog_event.rebind(page.generation));
                        let step = page.update(&self.paths, event);
                        self.gog_work.sequence = self.gog_work.sequence.max(page.gog.sequence);
                        step.task
                    }
                    None => {
                        self.gog_work.late = Some(gog_event);
                        Task::none()
                    }
                };
            }
        }
        if let Some(sequence) = event.steam_key_operation() {
            if self
                .steam_key_work
                .pending
                .is_some_and(|(pending, _)| pending == sequence)
            {
                self.steam_key_work.pending = None;
            }
            match self.settings.as_mut().filter(|page| page.is_global()) {
                Some(page) => {
                    let event = event.rebind_steam_key(page.generation);
                    let step = page.update(&self.paths, event);
                    self.steam_key_work.sequence =
                        self.steam_key_work.sequence.max(page.steam_key_sequence);
                    return step.task;
                }
                None => {
                    self.steam_key_work.late = Some(event);
                    return Task::none();
                }
            }
        }
        if let Some((generation, sequence)) = event.steamgriddb_token() {
            let same_generation = self
                .settings
                .as_ref()
                .is_some_and(|page| page.generation == generation);
            let status_result = matches!(event, Event::SteamGridDbKeyStatusLoaded(..));
            if !same_generation && !self.steamgriddb_pending.remove(&(generation, sequence)) {
                return Task::none();
            }
            if same_generation {
                self.steamgriddb_pending.remove(&(generation, sequence));
            }
            if status_result {
                if sequence <= self.steamgriddb_completed_sequence {
                    return Task::none();
                }
                self.steamgriddb_completed_sequence = sequence;
            } else {
                self.steamgriddb_completed_sequence =
                    self.steamgriddb_completed_sequence.max(sequence);
            }
            if self
                .steamgriddb_late_result
                .as_ref()
                .and_then(Event::steamgriddb_token)
                .is_some_and(|(_, late_sequence)| late_sequence < sequence)
            {
                self.steamgriddb_late_result = None;
            }
            if !same_generation {
                if let Some(page) = self.settings.as_mut().filter(|page| page.is_global()) {
                    if !page.steamgriddb_input_session_touched
                        && let Some(session) = event.steamgriddb_input_session()
                    {
                        page.steamgriddb_input_session = Some(session);
                        page.steamgriddb_input_session_touched = true;
                    }
                    if let Some(rebound) = event.rebind_steamgriddb(page.generation) {
                        let previous_sequence = page.steamgriddb_sequence;
                        let step = page.update(&self.paths, rebound);
                        let page_generation = page.generation;
                        let current_sequence = page.steamgriddb_sequence;
                        self.steamgriddb_sequence = self.steamgriddb_sequence.max(current_sequence);
                        self.steamgriddb_input_session_sequence = self
                            .steamgriddb_input_session_sequence
                            .max(page.steamgriddb_input_session_sequence);
                        if current_sequence != previous_sequence {
                            self.track_steamgriddb(page_generation, current_sequence);
                        }
                        return step.task;
                    }
                }
                self.steamgriddb_late_result = Some(event.clone());
                if status_result {
                    let available = match &event {
                        Event::SteamGridDbKeyStatusLoaded(_, _, Ok(status)) => status.available(),
                        Event::SteamGridDbKeyStatusLoaded(_, _, Err(_)) => false,
                        _ => unreachable!("status result was checked above"),
                    };
                    return Task::done(Message::SteamGridDbKeyChangedAt(sequence, available));
                }
                self.steamgriddb_sequence = self
                    .steamgriddb_sequence
                    .checked_add(1)
                    .expect("SteamGridDB sequence exhausted");
                return key_presence_task(self.paths.clone(), self.steamgriddb_sequence);
            }
        }
        let (step, generation, previous_sequence, current_sequence, leave_prompt) = {
            let Some(page) = &mut self.settings else {
                return Task::none();
            };
            let previous_sequence = page.steamgriddb_sequence;
            let generation = page.generation;
            let step = page.update(&self.paths, event);
            (
                step,
                generation,
                previous_sequence,
                page.steamgriddb_sequence,
                page.leave_prompt,
            )
        };
        if current_sequence != previous_sequence {
            self.steamgriddb_sequence = self.steamgriddb_sequence.max(current_sequence);
            self.track_steamgriddb(generation, current_sequence);
        }
        if let Some(page) = self.settings.as_ref().filter(|page| page.is_global()) {
            self.steam_key_work.sequence =
                self.steam_key_work.sequence.max(page.steam_key_sequence);
            if let (Some(sequence), Some(busy)) =
                (page.steam_key_busy_sequence, page.steam_key.busy)
            {
                self.steam_key_work.pending = Some((sequence, busy));
            }
            self.gog_work.sequence = self.gog_work.sequence.max(page.gog.sequence);
            // Opening the login page ends with the page, so only logging
            // in or out outlives it.
            if let (Some(sequence), Some(busy @ (gog::Busy::LoggingIn | gog::Busy::LoggingOut))) =
                (page.gog.busy_sequence, page.gog.busy)
            {
                self.gog_work.pending = Some((sequence, busy));
            }
        }
        if let Some(page) = &self.settings {
            self.steamgriddb_input_session_sequence = self
                .steamgriddb_input_session_sequence
                .max(page.steamgriddb_input_session_sequence);
        }
        if step.close {
            self.settings = None;
            if let Some(name) = self.pending_checkup_environment.take() {
                self.pending_section = None;
                return Task::batch([
                    step.task,
                    crate::checkup::update(self, crate::checkup::Event::Environment(name)),
                ]);
            }
            if let Some(section) = self.pending_section.take() {
                return Task::batch([step.task, self.switch_section(section)]);
            }
        } else if !leave_prompt {
            self.pending_section = None;
            self.pending_checkup_environment = None;
        }
        step.task
    }
    pub(crate) fn settings_tick(&mut self) -> Task<Message> {
        self.update_settings_work_refusal();
        match &mut self.settings {
            Some(page) => page.tick(&self.paths),
            None => Task::none(),
        }
    }
    pub(crate) fn settings_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        let Some(page) = &self.settings else {
            return Task::none();
        };
        let (prompt, menu_open, global) = (page.leave_prompt, page.menu_open, page.is_global());
        match key.as_ref() {
            Key::Character(character)
                if modifiers.control() && character.eq_ignore_ascii_case("s") =>
            {
                self.settings_update(Event::Save)
            }
            Key::Named(Named::Tab) if prompt => Task::none(),
            // Only text inputs take focus in iced, so Tab moves between those.
            Key::Named(Named::Tab) if modifiers.shift() => {
                iced::widget::operation::focus_previous()
            }
            Key::Named(Named::Tab) => iced::widget::operation::focus_next(),
            // A focused field uses Escape to let go of focus first, and pick
            // list menus don't close on it, so it isn't taken as leaving.
            Key::Named(Named::Escape) if !captured && !menu_open => {
                if prompt {
                    self.settings_update(Event::KeepEditing)
                } else if global {
                    // Like Trash and Checkup, a sidebar section isn't left with Escape.
                    Task::none()
                } else {
                    self.settings_update(Event::Back)
                }
            }
            _ => Task::none(),
        }
    }
}

pub(crate) fn key_presence_task(paths: Paths, sequence: u64) -> Task<Message> {
    steamgriddb::key_presence_task(paths, sequence)
}

#[cfg(test)]
mod tests;
