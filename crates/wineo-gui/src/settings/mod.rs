//! The settings page, which replaces the library while open.

mod form;
mod global;
mod load;
mod runner_picker;
mod steamgriddb;
mod view;

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::components::Component;
use wineo_core::config::{Arch, ArtFetch};
use wineo_core::paths::Paths;

use std::collections::{BTreeMap, BTreeSet};

use crate::{App, Message};
use form::{Build, Choice, EnvRow, Field, Form, List, Switch, TargetValues, Values};
pub(crate) use load::desktop_line;
use load::{Content, Current, Loaded, ProgramChoice, Report, RunnerChoice, Scan, Snapshot, Texts};

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
    SteamGridDbKeySetOpen,
    SteamGridDbKeySetCancel,
    SteamGridDbKeyText(steamgriddb::Secret),
    SteamGridDbKeySet,
    SteamGridDbKeyRemove,
    SteamGridDbKeyStatusLoaded(u64, u64, Result<steamgriddb::Status, String>),
    SteamGridDbKeySaved(u64, u64, Option<u64>, Result<steamgriddb::Saved, String>),
    SteamGridDbKeyRemoved(u64, u64, Option<u64>, Result<steamgriddb::Removed, String>),
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

/// The add target form's fields, typed but not added yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewTarget {
    pub(crate) name: String,
    pub(crate) path: String,
    /// The picked program's name, which titles its desktop entry.
    pub(crate) label: Option<String>,
}

pub(crate) struct Editor {
    pub(crate) info: Loaded,
    pub(crate) form: Form,
    /// Why the last runner picked was refused.
    pub(crate) refusal: Option<String>,
    /// An extra gamescope arg typed but not added yet.
    pub(crate) args_input: String,
    /// Why the last build picked was refused.
    pub(crate) build_refusal: Option<String>,
    /// Chips and a variable typed but not added yet, which a save leaves out.
    pub(crate) winetricks_input: String,
    pub(crate) tags_input: String,
    pub(crate) new_env: EnvRow,
    /// Per target: an arg and a gamescope arg typed but not added yet.
    pub(crate) target_args_input: BTreeMap<String, String>,
    pub(crate) target_gamescope_args_input: BTreeMap<String, String>,
    pub(crate) new_target: NewTarget,
    pub(crate) candidate_exe: Option<String>,
    /// Why the last target add was refused.
    pub(crate) add_error: Option<String>,
    /// The target asking to be removed.
    pub(crate) removing: Option<String>,
    /// View state, kept across every reload.
    pub(crate) open_cards: BTreeSet<String>,
    pub(crate) add_open: bool,
    pub(crate) scan: Option<Scan>,
    pub(crate) show_held_back: bool,
}

impl Editor {
    fn new(info: Loaded, form: Form) -> Self {
        Self {
            info,
            form,
            refusal: None,
            args_input: String::new(),
            build_refusal: None,
            winetricks_input: String::new(),
            tags_input: String::new(),
            new_env: EnvRow::default(),
            target_args_input: BTreeMap::new(),
            target_gamescope_args_input: BTreeMap::new(),
            new_target: NewTarget::default(),
            candidate_exe: None,
            add_error: None,
            removing: None,
            open_cards: BTreeSet::new(),
            add_open: false,
            scan: None,
            show_held_back: false,
        }
    }
    /// Text typed into an add field but not added yet.
    fn carry_inputs(&mut self, from: &Editor) {
        self.args_input.clone_from(&from.args_input);
        self.winetricks_input.clone_from(&from.winetricks_input);
        self.tags_input.clone_from(&from.tags_input);
        self.new_env.clone_from(&from.new_env);
        self.target_args_input.clone_from(&from.target_args_input);
        self.target_gamescope_args_input
            .clone_from(&from.target_gamescope_args_input);
        self.new_target.clone_from(&from.new_target);
        self.candidate_exe.clone_from(&from.candidate_exe);
    }
    fn clear_inputs(&mut self) {
        self.args_input.clear();
        self.winetricks_input.clear();
        self.tags_input.clear();
        self.new_env = EnvRow::default();
        self.target_args_input.clear();
        self.target_gamescope_args_input.clear();
        self.new_target = NewTarget::default();
        self.candidate_exe = None;
        self.add_error = None;
        self.removing = None;
    }
    /// What's open on the page, which no reload should close.
    fn carry_view(&mut self, from: &Editor) {
        self.open_cards.clone_from(&from.open_cards);
        self.add_open = from.add_open;
        self.scan.clone_from(&from.scan);
        self.show_held_back = from.show_held_back;
    }
    pub(crate) fn target_args_input(&self, target: &str) -> &str {
        self.target_args_input
            .get(target)
            .map_or("", String::as_str)
    }
    pub(crate) fn target_gamescope_args_input(&self, target: &str) -> &str {
        self.target_gamescope_args_input
            .get(target)
            .map_or("", String::as_str)
    }
    /// Whether the typed name and path can be added as they stand.
    pub(crate) fn can_add_target(&self) -> bool {
        !self.new_target.path.trim().is_empty()
            && self
                .form
                .new_target_problem(&self.new_target.name)
                .is_none()
    }
    pub(crate) fn duplicate_target(&self) -> Option<&str> {
        self.candidate_exe
            .as_deref()
            .and_then(|exe| self.form.using_exe(exe))
    }
    /// Why a target's desktop entry can't be turned on, if it can't.
    pub(crate) fn desktop_refusal(&self) -> Option<String> {
        self.info
            .portable
            .then(|| wineo_core::error::Error::PortableDesktop.to_string())
    }
    pub(crate) fn list_input(&self, list: List) -> &str {
        match list {
            List::Winetricks => &self.winetricks_input,
            List::Tags => &self.tags_input,
        }
    }
    fn list_input_mut(&mut self, list: List) -> &mut String {
        match list {
            List::Winetricks => &mut self.winetricks_input,
            List::Tags => &mut self.tags_input,
        }
    }
    /// The runner the draft names and its kind, from what the page already
    /// resolved. `None` when it doesn't resolve, which core doesn't check.
    pub(crate) fn draft_runner(&self) -> Option<Current> {
        let runner = self.form.draft.runner.as_ref()?;
        if Some(runner) == self.form.loaded.runner.as_ref() {
            return self.info.current.clone();
        }
        let choice = self
            .info
            .runners
            .iter()
            .find(|choice| &choice.name == runner)?;
        Some(Current {
            name: choice.name.clone(),
            proton: choice.proton?,
        })
    }
    pub(crate) fn proton(&self) -> bool {
        self.draft_runner().is_some_and(|runner| runner.proton)
    }
    /// gameid and store are Proton's, but a value already there stays
    /// editable so it can be cleared.
    pub(crate) fn protonfixes_shown(&self) -> bool {
        let loaded = &self.form.loaded;
        let draft = &self.form.draft;
        !self.info.native
            && (self.proton()
                || !loaded.gameid.is_empty()
                || !loaded.store.is_empty()
                || !draft.gameid.is_empty()
                || !draft.store.is_empty())
    }
    pub(crate) fn build_refusal(&self, name: &str, component: Component) -> Option<String> {
        load::component_refusal(name, self.draft_runner().as_ref(), component)
    }
    /// Whether a component's pick list takes a pick.
    pub(crate) fn build_enabled(&self, name: &str, component: Component) -> bool {
        !self.info.native
            && self.build_refusal(name, component).is_none()
            && (component == Component::Dxvk || self.form.vkd3d_enabled())
    }
    /// Whether this edit reaches a control the page shows.
    fn accepts(&self, name: &str, event: &Event) -> bool {
        if self.info.native && event.wine_only() {
            return false;
        }
        match event {
            Event::BuildPicked(component, _) => self.build_enabled(name, *component),
            Event::BuildRemoved(component) => self.form.draft.build(*component).is_some(),
            Event::GameId(_) | Event::Store(_) => self.protonfixes_shown(),
            // Turning one off stays possible, so a hand-written one can go.
            Event::Target(_, TargetEdit::Desktop(true)) => self.desktop_refusal().is_none(),
            _ => true,
        }
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
    /// The add target form as it was sent to be checked.
    resolving: Option<NewTarget>,
    pub(crate) open_add_after_load: bool,
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
    fn handle(&mut self, paths: &Paths, event: Event) -> Step {
        if self.locked() && event.edits() {
            return Step::none();
        }
        if self.global
            && event.edits()
            && !matches!(
                event,
                Event::GlobalSwitch(..)
                    | Event::GlobalRunner(_)
                    | Event::GlobalArch(_)
                    | Event::GlobalField(..)
                    | Event::GlobalArt(_)
                    | Event::GlobalArtKind(_, _)
                    | Event::GlobalExternalRunners(_)
                    | Event::EnvKey(..)
                    | Event::EnvValue(..)
                    | Event::EnvRemove(_)
                    | Event::NewEnvKey(_)
                    | Event::NewEnvValue(_)
                    | Event::EnvAdd
            )
        {
            return Step::none();
        }
        if !self.global
            && matches!(
                event,
                Event::GlobalSwitch(..)
                    | Event::GlobalRunner(_)
                    | Event::GlobalArch(_)
                    | Event::GlobalField(..)
                    | Event::GlobalArt(_)
                    | Event::GlobalArtKind(_, _)
                    | Event::GlobalExternalRunners(_)
            )
        {
            return Step::none();
        }
        if let Some(editor) = self.editor()
            && !editor.accepts(&self.name, &event)
        {
            return Step::none();
        }
        match event {
            Event::BrowsePath(target, cwd) => {
                if self.locked() || self.global {
                    return Step::none();
                }
                let Some(editor) = self.editor() else {
                    return Step::none();
                };
                let previous = match &target {
                    Some(target) => match editor.form.draft.targets.get(target) {
                        Some(values) if cwd => values.cwd.clone(),
                        Some(values) => values.exe.clone(),
                        None => return Step::none(),
                    },
                    None => editor.new_target.path.clone(),
                };
                self.resolve_sequence = self.resolve_sequence.wrapping_add(1);
                let sequence = self.resolve_sequence;
                let generation = self.generation;
                Step::task(Task::perform(
                    crate::picker::pick(
                        if cwd {
                            "Select working directory"
                        } else {
                            "Select executable"
                        }
                        .to_owned(),
                        cwd,
                    ),
                    move |result| {
                        Message::Settings(Event::PickedPath(
                            generation,
                            sequence,
                            target.clone(),
                            cwd,
                            previous.clone(),
                            result,
                        ))
                    },
                ))
            }
            Event::PickedPath(generation, sequence, target, cwd, previous, result) => {
                if generation != self.generation
                    || sequence != self.resolve_sequence
                    || self.locked()
                {
                    return Step::none();
                }
                let Some(editor) = self.editor() else {
                    return Step::none();
                };
                let current = match &target {
                    Some(target) => match editor.form.draft.targets.get(target) {
                        Some(values) if cwd => &values.cwd,
                        Some(values) => &values.exe,
                        None => return Step::none(),
                    },
                    None => &editor.new_target.path,
                };
                if *current != previous {
                    return Step::none();
                }
                match result {
                    Ok(Some(path)) => {
                        let value = path.to_string_lossy().into_owned();
                        let edit = match target {
                            Some(target) if cwd => Event::Target(target, TargetEdit::Cwd(value)),
                            Some(target) => Event::Target(target, TargetEdit::Exe(value)),
                            None => Event::NewTargetPath(value),
                        };
                        self.handle(paths, edit)
                    }
                    Ok(None) => Step::none(),
                    Err(error) => {
                        self.error = Some(error);
                        Step::none()
                    }
                }
            }
            Event::Back => {
                if self.saving {
                    Step::none()
                } else if self.is_dirty() {
                    self.leave_prompt = true;
                    // Typing would otherwise reach a field behind the prompt.
                    Step::task(unfocus())
                } else {
                    Step::close()
                }
            }
            Event::Discard if self.saving => Step::none(),
            Event::Discard => Step::close(),
            Event::KeepEditing => {
                self.leave_prompt = false;
                Step::none()
            }
            Event::Save => {
                if !self.can_save() {
                    return Step::none();
                }
                let edits = match &self.view {
                    View::Form(editor) => editor.form.edits(),
                    View::Global(editor) => editor.form.edits(),
                    _ => return Step::none(),
                };
                if edits.is_empty() {
                    return Step::none();
                }
                self.saving = true;
                self.drop_checks();
                self.leave_prompt = false;
                self.edited();
                Step::task(load::save_task(
                    paths.clone(),
                    self.name.clone(),
                    self.global,
                    edits,
                    self.generation,
                ))
            }
            Event::Reload => {
                if !self.can_reload() {
                    return Step::none();
                }
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.clear_inputs();
                }
                if let View::Global(editor) = &mut self.view {
                    editor.new_env = EnvRow::default();
                }
                Step::task(self.reload(paths, Mode::Replace))
            }
            Event::Revert => {
                if self.locked() {
                    return Step::none();
                }
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.form.revert();
                    editor.clear_inputs();
                    editor.refusal = None;
                    editor.build_refusal = None;
                }
                if let View::Global(editor) = &mut self.view {
                    editor.form.revert();
                    editor.new_env = EnvRow::default();
                }
                if self.changed_on_disk {
                    return Step::task(self.reload(paths, Mode::Replace));
                }
                Step::none()
            }
            Event::RunnerPicked(choice) => {
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    if refused(editor.info.current.as_ref(), &choice) {
                        editor.refusal = Some(load::KIND_MISMATCH.to_owned());
                    } else {
                        editor.refusal = None;
                        editor.form.draft.runner = Some(choice.name);
                    }
                }
                Step::none()
            }
            Event::Switch(switch, on) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    *draft.switch_mut(switch) = Some(on);
                }
                Step::none()
            }
            Event::Reset(switch) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    *draft.switch_mut(switch) = None;
                }
                Step::none()
            }
            Event::Gamescope(on) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.gamescope = on;
                }
                Step::none()
            }
            Event::FieldText(field, value) => {
                if let Some(draft) = self.draft()
                    && draft.fields.set_text(field, value)
                {
                    self.edited();
                }
                Step::none()
            }
            Event::FieldChoice(field, choice) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.fields.set_choice(field, choice);
                }
                Step::none()
            }
            Event::GrabCursor(on) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.fields.grab_cursor = on;
                }
                Step::none()
            }
            Event::ArgsInput(value) => {
                if let Some(editor) = self.editor_mut() {
                    editor.args_input = value;
                }
                Step::none()
            }
            Event::ArgsAdd => {
                if let Some(editor) = self.editor_mut()
                    && !editor.args_input.is_empty()
                {
                    let arg = std::mem::take(&mut editor.args_input);
                    editor.form.draft.fields.args.push(arg);
                    self.edited();
                }
                Step::none()
            }
            Event::ArgsRemove(index) => {
                if let Some(draft) = self.draft()
                    && index < draft.fields.args.len()
                {
                    draft.fields.args.remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::BuildPicked(component, build) => {
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.build_refusal = editor
                        .form
                        .pick_build(component, build.0)
                        .err()
                        .map(str::to_owned);
                }
                Step::none()
            }
            Event::BuildRemoved(component) => {
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.build_refusal =
                        editor.form.remove_build(component).err().map(str::to_owned);
                }
                Step::none()
            }
            Event::ListInput(list, value) => {
                if let Some(editor) = self.editor_mut() {
                    *editor.list_input_mut(list) = value;
                }
                Step::none()
            }
            Event::ProxiedTitle(title) => {
                if let Some(proxied) = self.draft().and_then(|draft| draft.proxied.as_mut()) {
                    let title = (!title.is_empty()).then_some(title);
                    if proxied.title != title {
                        proxied.title = title;
                        self.edited();
                    }
                }
                Step::none()
            }
            Event::ProxiedDesktop(desktop) => {
                if let Some(proxied) = self.draft().and_then(|draft| draft.proxied.as_mut())
                    && proxied.desktop != desktop
                {
                    proxied.desktop = desktop;
                    self.edited();
                }
                Step::none()
            }
            Event::ListAdd(list) => {
                if let Some(editor) = self.editor_mut() {
                    let input = editor.list_input(list);
                    let items = editor.form.draft.list(list);
                    if input.is_empty() || list.problem(items, input).is_some() {
                        return Step::none();
                    }
                    let item = std::mem::take(editor.list_input_mut(list));
                    editor.form.draft.list_mut(list).push(item);
                    self.edited();
                }
                Step::none()
            }
            Event::ListRemove(list, index) => {
                if let Some(draft) = self.draft()
                    && index < draft.list(list).len()
                {
                    draft.list_mut(list).remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::EnvKey(index, key) => {
                if let Some(row) = self.draft().and_then(|draft| draft.env.get_mut(index)) {
                    row.key = key;
                    self.edited();
                } else if let View::Global(editor) = &mut self.view
                    && let Some(row) = editor.form.draft.env.get_mut(index)
                {
                    row.key = key;
                    self.edited();
                }
                Step::none()
            }
            Event::EnvValue(index, value) => {
                if let Some(row) = self.draft().and_then(|draft| draft.env.get_mut(index)) {
                    row.value = value;
                    self.edited();
                } else if let View::Global(editor) = &mut self.view
                    && let Some(row) = editor.form.draft.env.get_mut(index)
                {
                    row.value = value;
                    self.edited();
                }
                Step::none()
            }
            Event::EnvRemove(index) => {
                if let Some(draft) = self.draft()
                    && index < draft.env.len()
                {
                    draft.env.remove(index);
                    self.edited();
                }
                if let View::Global(editor) = &mut self.view
                    && index < editor.form.draft.env.len()
                {
                    editor.form.draft.env.remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::NewEnvKey(key) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_env.key = key;
                } else if let View::Global(editor) = &mut self.view {
                    editor.new_env.key = key;
                }
                Step::none()
            }
            Event::NewEnvValue(value) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_env.value = value;
                } else if let View::Global(editor) = &mut self.view {
                    editor.new_env.value = value;
                }
                Step::none()
            }
            Event::EnvAdd => {
                if let Some(editor) = self.editor_mut() {
                    let row = &editor.new_env;
                    if editor
                        .form
                        .draft
                        .new_env_problem(&row.key, &row.value)
                        .is_some()
                    {
                        return Step::none();
                    }
                    let row = std::mem::take(&mut editor.new_env);
                    editor.form.draft.env.push(row);
                    self.edited();
                }
                if let View::Global(editor) = &mut self.view {
                    let row = &editor.new_env;
                    if editor
                        .form
                        .draft
                        .new_env_problem(&row.key, &row.value)
                        .is_none()
                    {
                        let row = std::mem::take(&mut editor.new_env);
                        editor.form.draft.env.push(row);
                        self.edited();
                    }
                }
                Step::none()
            }
            Event::GlobalSwitch(key, on) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.set_switch(key, on);
                }
                Step::none()
            }
            Event::GlobalRunner(choice) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.draft.runner = choice.name;
                }
                Step::none()
            }
            Event::GlobalArch(arch) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.draft.arch = arch;
                }
                Step::none()
            }
            Event::GlobalField(field, value) => {
                if let View::Global(editor) = &mut self.view
                    && editor.form.draft.set_field(field, value)
                {
                    self.edited();
                }
                Step::none()
            }
            // A portable folder never uses other launchers' builds, so the
            // switch there would do nothing.
            Event::GlobalExternalRunners(_) if paths.portable_root().is_some() => Step::none(),
            Event::GlobalExternalRunners(on) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.set_external_runners(on);
                }
                Step::none()
            }
            Event::GlobalArtKind(kind, on) => {
                if let View::Global(editor) = &self.view {
                    let fetch = editor.form.draft.art_fetch.with(kind, on);
                    return self.update(paths, Event::GlobalArt(fetch));
                }
                Step::none()
            }
            Event::GlobalArt(fetch) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.draft.art_fetch = fetch;
                }
                Step::none()
            }
            Event::SteamGridDbKeySetOpen => {
                if self.global
                    && !self.locked()
                    && !self.loading()
                    && self.steamgriddb.busy.is_none()
                {
                    self.steamgriddb.input_open = true;
                    self.steamgriddb.input = steamgriddb::Secret::default();
                    self.steamgriddb_input_session = Some(self.bump_steamgriddb_input_session());
                    self.steamgriddb_input_session_touched = true;
                    self.steamgriddb.error = None;
                    self.steamgriddb.result = None;
                }
                Step::none()
            }
            Event::SteamGridDbKeySetCancel => {
                if self.global && self.steamgriddb.busy.is_none() {
                    self.steamgriddb.input_open = false;
                    self.steamgriddb.input = steamgriddb::Secret::default();
                    self.steamgriddb_input_session = None;
                    self.steamgriddb_input_session_touched = true;
                    self.steamgriddb.error = None;
                }
                Step::none()
            }
            Event::SteamGridDbKeyText(value) => {
                if self.global && self.steamgriddb.input_open && self.steamgriddb.busy.is_none() {
                    self.steamgriddb.input = value;
                    self.steamgriddb.error = None;
                }
                Step::none()
            }
            Event::SteamGridDbKeySet => {
                if !self.global
                    || self.locked()
                    || self.loading()
                    || self.steamgriddb.busy.is_some()
                    || self.steamgriddb.input.as_str().trim().is_empty()
                {
                    return Step::none();
                }
                self.steamgriddb.busy = Some(steamgriddb::Busy::Saving);
                self.steamgriddb.error = None;
                self.steamgriddb.result = None;
                let input_session = self.steamgriddb_input_session;
                let key = std::mem::take(&mut self.steamgriddb.input);
                let sequence = self.bump_steamgriddb_sequence();
                self.steamgriddb_busy_sequence = Some(sequence);
                Step::task(steamgriddb::save_task(
                    paths.clone(),
                    self.generation,
                    sequence,
                    input_session,
                    key,
                ))
            }
            Event::SteamGridDbKeyRemove => {
                if !self.global
                    || self.locked()
                    || self.loading()
                    || self.steamgriddb.busy.is_some()
                {
                    return Step::none();
                }
                self.steamgriddb.busy = Some(steamgriddb::Busy::Removing);
                self.steamgriddb.error = None;
                self.steamgriddb.result = None;
                let input_session = self.steamgriddb_input_session;
                let sequence = self.bump_steamgriddb_sequence();
                self.steamgriddb_busy_sequence = Some(sequence);
                Step::task(steamgriddb::remove_task(
                    paths.clone(),
                    self.generation,
                    sequence,
                    input_session,
                ))
            }
            Event::SteamGridDbKeyStatusLoaded(generation, sequence, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                if !self.global {
                    return Step::none();
                }
                let available = match result {
                    Ok(status) => {
                        let available = status.available();
                        self.steamgriddb.warning = status.warning(paths);
                        self.steamgriddb.status = Some(status);
                        self.steamgriddb.status_error = None;
                        available
                    }
                    Err(error) => {
                        self.steamgriddb.status_error = Some(error);
                        false
                    }
                };
                Step::task(Task::done(Message::SteamGridDbKeyChangedAt(
                    sequence, available,
                )))
            }
            Event::SteamGridDbKeySaved(generation, sequence, input_session, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                if !self.global {
                    return Step::none();
                }
                if self.steamgriddb_busy_sequence == Some(sequence) {
                    self.steamgriddb.busy = None;
                    self.steamgriddb_busy_sequence = None;
                }
                match result {
                    Ok(saved) => {
                        self.steamgriddb.warning = saved.warning;
                        self.steamgriddb.result = saved.result;
                        if input_session.is_some()
                            && input_session == self.steamgriddb_input_session
                        {
                            self.steamgriddb.error = None;
                            self.steamgriddb.input_open = false;
                            self.steamgriddb.input = steamgriddb::Secret::default();
                            self.steamgriddb_input_session = None;
                        }
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                    Err(error) => {
                        self.steamgriddb.error = Some(error);
                        if input_session.is_some()
                            && input_session == self.steamgriddb_input_session
                        {
                            self.steamgriddb.input_open = true;
                            self.steamgriddb.input = steamgriddb::Secret::default();
                            self.steamgriddb_input_session =
                                Some(self.bump_steamgriddb_input_session());
                        }
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                }
            }
            Event::SteamGridDbKeyRemoved(generation, sequence, input_session, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                if !self.global {
                    return Step::none();
                }
                if self.steamgriddb_busy_sequence == Some(sequence) {
                    self.steamgriddb.busy = None;
                    self.steamgriddb_busy_sequence = None;
                }
                match result {
                    Ok(removed) => {
                        self.steamgriddb.warning = None;
                        self.steamgriddb.result = Some(removed.result);
                        if input_session.is_some()
                            && input_session == self.steamgriddb_input_session
                        {
                            self.steamgriddb.error = None;
                            self.steamgriddb.input_open = false;
                            self.steamgriddb.input = steamgriddb::Secret::default();
                            self.steamgriddb_input_session = None;
                        }
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                    Err(error) => {
                        self.steamgriddb.error = Some(error);
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                }
            }
            Event::Prefix(value) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.prefix = value;
                }
                Step::none()
            }
            Event::GameId(value) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.gameid = value;
                }
                Step::none()
            }
            Event::Store(value) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.store = value;
                }
                Step::none()
            }
            Event::Target(target, edit) => {
                if self.target_edit(&target, edit) {
                    self.edited();
                }
                Step::none()
            }
            Event::Card(target, open) => {
                if let Some(editor) = self.editor_mut() {
                    if open {
                        editor.open_cards.insert(target);
                    } else {
                        editor.open_cards.remove(&target);
                    }
                }
                Step::none()
            }
            Event::KeepTarget => {
                if let Some(editor) = self.editor_mut() {
                    editor.removing = None;
                }
                Step::none()
            }
            Event::AddOpen(open) => {
                let generation = self.generation;
                self.scan_sequence = self.scan_sequence.wrapping_add(1);
                let sequence = self.scan_sequence;
                let Some(editor) = self.editor_mut() else {
                    return Step::none();
                };
                editor.add_open = open;
                // A native environment has no prefix to scan.
                if !open || editor.info.native {
                    return Step::none();
                }
                editor.scan = Some(Scan::Scanning);
                Step::task(load::scan_task(
                    paths.clone(),
                    self.name.clone(),
                    generation,
                    sequence,
                ))
            }
            Event::ShowHeldBack(show) => {
                if let Some(editor) = self.editor_mut() {
                    editor.show_held_back = show;
                }
                Step::none()
            }
            Event::Scanned(generation, sequence, scan) => {
                if generation == self.generation
                    && sequence == self.scan_sequence
                    && !self.locked()
                    && let Some(editor) = self.editor_mut()
                {
                    editor.scan = Some(scan);
                }
                Step::none()
            }
            Event::ProgramPicked(program) => {
                if let Some(editor) = self.editor_mut() {
                    editor.candidate_exe = Some(program.exe.clone());
                    editor.new_target = NewTarget {
                        name: editor.form.suggest_target(&program.name, &program.exe),
                        path: program.exe,
                        label: Some(program.name),
                    };
                    editor.add_error = None;
                }
                Step::none()
            }
            Event::NewTargetName(name) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_target.name = name;
                    editor.add_error = None;
                }
                Step::none()
            }
            Event::NewTargetPath(path) => {
                if let Some(editor) = self.editor_mut() {
                    editor.candidate_exe = wineo_core::target_path::candidate_exe(
                        paths,
                        editor.info.wine_prefix.as_deref(),
                        &path,
                    );
                    editor.new_target.path = path;
                    // The picked program's name no longer fits a typed path.
                    editor.new_target.label = None;
                    editor.add_error = None;
                }
                Step::none()
            }
            Event::TargetAdd => {
                let Some(editor) = self.editor() else {
                    return Step::none();
                };
                if !editor.can_add_target() || self.resolving.is_some() {
                    return Step::none();
                }
                let request = editor.new_target.clone();
                self.resolve_sequence = self.resolve_sequence.wrapping_add(1);
                self.resolving = Some(request.clone());
                Step::task(load::resolve_task(
                    paths.clone(),
                    self.name.clone(),
                    request.path,
                    self.generation,
                    self.resolve_sequence,
                ))
            }
            Event::Resolved(generation, sequence, result) => {
                if generation != self.generation || sequence != self.resolve_sequence {
                    return Step::none();
                }
                let Some(request) = self.resolving.take() else {
                    return Step::none();
                };
                // A save started since, or the form changed under the check,
                // so this no longer answers what's on the page.
                if self.locked() {
                    return Step::none();
                }
                let Some(editor) = self.editor_mut() else {
                    return Step::none();
                };
                if editor.new_target != request {
                    return Step::none();
                }
                match result.and_then(|exe| editor.add_resolved(&request, exe)) {
                    Ok(()) => self.edited(),
                    Err(error) => editor.add_error = Some(error),
                }
                Step::none()
            }
            Event::MenuOpened => {
                self.menu_open = true;
                Step::none()
            }
            Event::MenuClosed => {
                self.menu_open = false;
                Step::none()
            }
            Event::OpenConfig => Step::task(load::open_task(if self.global {
                paths.global_config()
            } else {
                paths.environment_config(&self.name)
            })),
            Event::Opened(result) => {
                self.error = result.err();
                Step::none()
            }
            Event::Loaded(generation, snapshot) => {
                if generation != self.generation {
                    return Step::none();
                }
                if let Some(pending) = self.pending.take() {
                    let after_save = pending.after_save;
                    self.land(pending, snapshot);
                    if self.global && matches!(self.view, View::Global(_)) {
                        return Step::task(self.refresh_steamgriddb(paths));
                    }
                    if self.open_add_after_load && self.editor().is_some() {
                        self.open_add_after_load = false;
                        return self.handle(paths, Event::AddOpen(true));
                    }
                    if after_save {
                        self.scan_sequence = self.scan_sequence.wrapping_add(1);
                        let sequence = self.scan_sequence;
                        if let Some(editor) = self.editor_mut() {
                            editor.scan = None;
                            if editor.add_open && !editor.info.native {
                                editor.scan = Some(Scan::Scanning);
                                return Step::task(load::scan_task(
                                    paths.clone(),
                                    self.name.clone(),
                                    self.generation,
                                    sequence,
                                ));
                            }
                        }
                    }
                }
                Step::none()
            }
            Event::Checked(generation, sequence, texts) => {
                if generation != self.generation || sequence != self.check_sequence {
                    return Step::none();
                }
                self.checking = false;
                let Some(disk) = &self.disk else {
                    return Step::none();
                };
                if self.loading() || self.saving {
                    return Step::none();
                }
                if *disk == texts {
                    // Changed back to what the page holds, so its edits are
                    // against the file again.
                    self.changed_on_disk = false;
                    return Step::none();
                }
                if self.global || disk.environment != texts.environment {
                    if self.is_dirty() {
                        self.changed_on_disk = true;
                        Step::none()
                    } else {
                        Step::task(self.reload(paths, Mode::Replace))
                    }
                } else {
                    // Only the global file changed under an environment page.
                    Step::task(self.reload(paths, Mode::Rebase))
                }
            }
            Event::CheckFailed(generation, sequence) => {
                if generation == self.generation && sequence == self.check_sequence {
                    self.checking = false;
                }
                Step::none()
            }
            Event::Saved(generation, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                self.saving = false;
                match result {
                    Ok(report) => {
                        self.notes = report.lines;
                        match report.failed {
                            // The file may or may not have been written before
                            // the failure, so read it back as the new base and
                            // keep the edits, which drops whatever did land.
                            Some(error) => {
                                self.error = Some(error);
                                Step::task(self.reload_after(paths, Mode::Rebase, true))
                            }
                            None => Step::task(self.reload_after(paths, Mode::AfterSave, true)),
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Step::none()
                    }
                }
            }
        }
    }
    /// Applies one target's edit. Returns whether the draft changed.
    fn target_edit(&mut self, target: &str, edit: TargetEdit) -> bool {
        let Some(editor) = self.editor_mut() else {
            return false;
        };
        let form = &mut editor.form;
        let Some(values) = form.draft.targets.get_mut(target) else {
            return false;
        };
        match edit {
            TargetEdit::Exe(exe) => values.exe = wineo_core::target_path::clean_path(&exe),
            TargetEdit::Cwd(cwd) => values.cwd = wineo_core::target_path::clean_path(&cwd),
            TargetEdit::Name(name) => values.name = name,
            TargetEdit::Desktop(on) => values.desktop = on,
            TargetEdit::Switch(switch, on) => *values.switch_mut(switch) = Some(on),
            TargetEdit::Reset(switch) => *values.switch_mut(switch) = None,
            TargetEdit::GamescopeReset => values.gamescope = form::TargetGamescope::Inherit,
            TargetEdit::ArgsAdd => {
                let Some(arg) = editor
                    .target_args_input
                    .remove(target)
                    .filter(|arg| !arg.is_empty())
                else {
                    return false;
                };
                values.args.push(arg);
            }
            TargetEdit::ArgsRemove(index) => {
                if index >= values.args.len() {
                    return false;
                }
                values.args.remove(index);
            }
            TargetEdit::Default(on) => form.set_default(target, on),
            TargetEdit::Gamescope(on) => return form.set_target_gamescope(target, on),
            TargetEdit::GamescopeHere => return form.override_target_gamescope(target),
            TargetEdit::FieldText(field, value) => {
                return form.edit_target_fields(target, |fields| fields.set_text(field, value));
            }
            TargetEdit::FieldChoice(field, choice) => {
                return form.edit_target_fields(target, |fields| {
                    fields.set_choice(field, choice);
                    true
                });
            }
            TargetEdit::GrabCursor(on) => {
                return form.edit_target_fields(target, |fields| {
                    fields.grab_cursor = on;
                    true
                });
            }
            TargetEdit::GamescopeArgsAdd => {
                let arg = editor.target_gamescope_args_input(target).to_owned();
                if arg.is_empty()
                    || !editor.form.edit_target_fields(target, |fields| {
                        fields.args.push(arg);
                        true
                    })
                {
                    return false;
                }
                editor.target_gamescope_args_input.remove(target);
            }
            TargetEdit::GamescopeArgsRemove(index) => {
                return form.edit_target_fields(target, |fields| {
                    if index >= fields.args.len() {
                        return false;
                    }
                    fields.args.remove(index);
                    true
                });
            }
            TargetEdit::ArgsInput(value) => {
                editor.target_args_input.insert(target.to_owned(), value);
                return false;
            }
            TargetEdit::GamescopeArgsInput(value) => {
                editor
                    .target_gamescope_args_input
                    .insert(target.to_owned(), value);
                return false;
            }
            TargetEdit::AskRemove => {
                editor.removing = Some(target.to_owned());
                return false;
            }
            TargetEdit::Remove => {
                if editor.removing.as_deref() != Some(target) {
                    return false;
                }
                editor.removing = None;
                editor.target_args_input.remove(target);
                editor.target_gamescope_args_input.remove(target);
                editor.open_cards.remove(target);
                return editor.form.remove_target(target);
            }
        }
        true
    }
    fn land(&mut self, pending: Pending, snapshot: Snapshot) {
        self.menu_open = false;
        let draft = self.editor().map(|editor| &editor.form.draft);
        let global_draft = match &self.view {
            View::Global(editor) => Some(&editor.form.draft),
            _ => None,
        };
        let edited_since = pending.mode == Mode::Replace
            && (draft != pending.draft.as_ref() || global_draft != pending.global_draft.as_ref());
        match (pending.mode, snapshot.content) {
            (Mode::Rebase, Content::Global(info)) if matches!(self.view, View::Global(_)) => {
                let editor = match &mut self.view {
                    View::Global(editor) => editor,
                    _ => unreachable!("checked above"),
                };
                editor.form.rebase(info.values.clone());
                editor.runners = info.runners;
                self.disk = Some(snapshot.texts);
            }
            (Mode::Rebase, Content::Form(info)) if self.editor().is_some() => {
                let keep_base = self.changed_on_disk;
                let editor = self.editor_mut().expect("checked above");
                let values = info.values.clone();
                editor.info = *info;
                if keep_base {
                    // The environment file changed under the edits too, so
                    // Reload stays the way forward.
                    if let Some(disk) = &mut self.disk {
                        disk.global = snapshot.texts.global;
                    }
                } else {
                    editor.form.rebase(values);
                    self.disk = Some(snapshot.texts);
                }
            }
            // Edits made while this ran are the user's, so they stay, and the
            // page says the file moved on under them.
            _ if edited_since || (pending.mode == Mode::Rebase && self.is_dirty()) => {
                self.changed_on_disk = true;
            }
            (_, content) => {
                self.disk = Some(snapshot.texts);
                self.changed_on_disk = false;
                let previous = self.editor().map(|editor| &editor.form);
                self.view = match content {
                    Content::Form(info) => {
                        let form = Form::reloaded(info.values.clone(), previous);
                        let mut editor = Editor::new(*info, form);
                        if let Some(previous) = self.editor() {
                            editor.carry_view(previous);
                        }
                        // A quiet reload shouldn't eat what's being typed. Reload
                        // and Revert clear it first, and a save reload drops it.
                        if pending.mode == Mode::Replace
                            && let Some(previous) = self.editor()
                        {
                            editor.carry_inputs(previous);
                        }
                        View::Form(Box::new(editor))
                    }
                    Content::Global(info) => {
                        let mut editor = global::Editor::new(info.values, info.runners);
                        if pending.mode == Mode::Replace
                            && let View::Global(previous) = &self.view
                        {
                            editor.new_env.clone_from(&previous.new_env);
                        }
                        View::Global(Box::new(editor))
                    }
                    Content::Error(error) => View::Error(error),
                };
            }
        }
    }
}

impl Editor {
    /// Adds the checked exe as a new target, set up the way `setup` sets one
    /// up: a desktop entry titled after the program, unless portable.
    fn add_resolved(&mut self, request: &NewTarget, exe: String) -> Result<(), String> {
        let name = if request.name.is_empty() {
            self.form
                .suggest_target(request.label.as_deref().unwrap_or(""), &exe)
        } else {
            request.name.clone()
        };
        let desktop = !self.info.portable;
        let label = request.label.clone().unwrap_or_else(|| file_stem(&exe));
        let values = TargetValues {
            exe,
            desktop,
            // A name only titles a desktop entry, so none is kept without one.
            name: if desktop { label } else { String::new() },
            ..TargetValues::default()
        };
        self.form.add_target(&name, values)?;
        self.new_target = NewTarget::default();
        self.candidate_exe = None;
        self.add_error = None;
        self.add_open = false;
        self.open_cards.insert(name);
        Ok(())
    }
}

fn file_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.rsplit_once('.')
        .map_or(file, |(stem, _)| stem)
        .to_owned()
}

/// Wine and Proton prefixes are laid out differently, so `settings::apply`
/// refuses a switch between them. Refusing it here says so before Save.
fn refused(current: Option<&Current>, choice: &RunnerChoice) -> bool {
    match (current, choice.proton) {
        (Some(current), Some(proton)) => current.proton != proton,
        _ => false,
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
    pub(crate) fn settings_update(&mut self, event: Event) -> Task<Message> {
        self.update_settings_work_refusal();
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
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use wineo_core::config::GlobalConfig;
    use wineo_core::settings::{self, ConfigFile};

    use super::*;
    use form::Source;

    fn global_page(paths: &Paths) -> Page {
        let mut page = Page::new_global(1);
        let _ = page.reload(paths, Mode::Replace);
        let _ = page.update(paths, Event::Loaded(1, load::snapshot_global(paths)));
        page
    }

    fn global_editor(page: &Page) -> &global::Editor {
        match &page.view {
            View::Global(editor) => editor,
            _ => panic!("global form expected"),
        }
    }

    #[test]
    fn steamgriddb_key_work_does_not_block_unrelated_saves() {
        let (_root, paths) = setup();
        let mut page = global_page(&paths);
        let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
        assert!(page.is_dirty());

        for event in [
            Event::SteamGridDbKeySetOpen,
            Event::SteamGridDbKeyText(steamgriddb::Secret::new("private-value".to_owned())),
            Event::SteamGridDbKeySet,
        ] {
            assert!(!event.edits());
            let _ = page.update(&paths, event);
        }
        assert!(!page.navigation_blocked());
        assert!(page.can_save());
        assert!(page.steamgriddb.input.as_str().is_empty());
        let _ = page.update(&paths, Event::Save);
        assert!(page.saving);
        let input_session = page.steamgriddb_input_session;
        let sequence = page.steamgriddb_sequence;
        let _ = page.update(
            &paths,
            Event::SteamGridDbKeySaved(
                page.generation,
                sequence,
                input_session,
                Err("SteamGridDB rejected the key".to_owned()),
            ),
        );

        assert!(page.is_dirty());
        assert_eq!(page.changes(), 1);
        assert!(!page.leave_prompt);
        assert!(page.navigation_blocked());
        assert!(page.steamgriddb.input_open);
        assert!(page.steamgriddb.input.as_str().is_empty());
        assert_eq!(
            page.steamgriddb.error.as_deref(),
            Some("SteamGridDB rejected the key")
        );
    }

    #[test]
    fn steamgriddb_key_save_and_remove_results_stay_outside_the_save_bar() {
        let (_root, paths) = setup();
        let mut page = global_page(&paths);
        let _ = page.update(&paths, Event::SteamGridDbKeySetOpen);
        let _ = page.update(
            &paths,
            Event::SteamGridDbKeyText(steamgriddb::Secret::new("private-value".to_owned())),
        );
        let _ = page.update(&paths, Event::SteamGridDbKeySet);
        let input_session = page.steamgriddb_input_session;
        let sequence = page.steamgriddb_sequence;
        let _ = page.update(
            &paths,
            Event::SteamGridDbKeySaved(
                page.generation,
                sequence,
                input_session,
                Ok(steamgriddb::Saved {
                    status: steamgriddb::Status {
                        source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                        override_note: false,
                    },
                    warning: None,
                    result: Some("Saved the SteamGridDB key to the keyring".to_owned()),
                }),
            ),
        );
        let status_sequence = page.steamgriddb_sequence;
        let _ = page.update(
            &paths,
            Event::SteamGridDbKeyStatusLoaded(
                page.generation,
                status_sequence,
                Ok(steamgriddb::Status {
                    source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                    override_note: false,
                }),
            ),
        );
        assert!(!page.is_dirty());
        assert_eq!(page.changes(), 0);
        assert!(!page.leave_prompt);
        assert_eq!(
            page.steamgriddb.status.as_ref().unwrap().label(&paths),
            "Saved in the keyring"
        );

        let _ = page.update(&paths, Event::SteamGridDbKeyRemove);
        let input_session = page.steamgriddb_input_session;
        let sequence = page.steamgriddb_sequence;
        let _ = page.update(
            &paths,
            Event::SteamGridDbKeyRemoved(
                page.generation,
                sequence,
                input_session,
                Ok(steamgriddb::Removed {
                    status: steamgriddb::Status {
                        source: None,
                        override_note: false,
                    },
                    result: "Removed the SteamGridDB key".to_owned(),
                }),
            ),
        );
        assert!(!page.is_dirty());
        assert!(!page.leave_prompt);
        assert_eq!(
            page.steamgriddb.result.as_deref(),
            Some("Removed the SteamGridDB key")
        );
    }

    #[test]
    fn steamgriddb_work_does_not_block_reverting_unrelated_edits() {
        let (_root, paths) = setup();
        let mut page = global_page(&paths);
        let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
        let _ = page.update(&paths, Event::SteamGridDbKeySetOpen);
        let _ = page.update(
            &paths,
            Event::SteamGridDbKeyText(steamgriddb::Secret::new("private-value".to_owned())),
        );
        let _ = page.update(&paths, Event::SteamGridDbKeySet);

        assert!(page.is_dirty());
        assert_eq!(page.steamgriddb.busy, Some(steamgriddb::Busy::Saving));
        let _ = page.update(&paths, Event::Revert);

        assert!(!page.is_dirty());
        assert_eq!(page.steamgriddb.busy, Some(steamgriddb::Busy::Saving));
        assert!(!page.navigation_blocked());
    }

    #[test]
    fn global_art_switches_save_every_combination() {
        use wineo_core::art::Kind;
        for mask in 0..16 {
            let (_root, paths) = setup();
            let mut page = global_page(&paths);
            for (index, kind) in Kind::ALL.into_iter().enumerate() {
                let _ = page.update(&paths, Event::GlobalArtKind(kind, mask & (1 << index) != 0));
            }
            let expected: Vec<_> = Kind::ALL
                .into_iter()
                .enumerate()
                .filter_map(|(index, kind)| (mask & (1 << index) != 0).then_some(kind))
                .collect();
            let edits = global_editor(&page).form.edits();
            load::save_global(&paths, &edits).unwrap();
            assert_eq!(
                GlobalConfig::load(&paths).unwrap().art.fetch.kinds(),
                expected
            );
            assert_eq!(global_editor(&page).form.draft.art_fetch.kinds(), expected);
        }
    }

    #[test]
    fn missing_global_file_shows_defaults_and_is_created_on_save() {
        let (_root, paths) = setup();
        let mut page = global_page(&paths);
        assert!(!page.has_file());
        assert!(!page.is_dirty());
        assert_eq!(global_editor(&page).form.draft.arch, Arch::Win64);
        let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
        let edits = global_editor(&page).form.edits();
        let report = load::save_global(&paths, &edits).unwrap();
        assert_eq!(report.failed, None);
        assert_eq!(GlobalConfig::load(&paths).unwrap().gamemode, Some(true));
        let _ = page.update(&paths, Event::Save);
        let _ = page.update(&paths, Event::Saved(1, Ok(report)));
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert!(page.has_file());
        assert!(!page.is_dirty());
    }

    #[test]
    fn broken_or_unsupported_global_file_shows_error_and_reloads() {
        let (_root, paths) = setup();
        write_global(&paths, "stop_timeout = [\n");
        let mut page = global_page(&paths);
        assert!(matches!(page.view, View::Error(_)));
        assert!(page.has_file());
        write_global(&paths, "schema_version = 999\n");
        let sequence = start_check(&mut page);
        let _ = page.update(
            &paths,
            Event::Checked(1, sequence, Texts::read_global(&paths)),
        );
        assert!(page.loading());
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert!(matches!(page.view, View::Error(_)));
        write_global(&paths, "gamemode = true\n");
        let sequence = start_check(&mut page);
        let _ = page.update(
            &paths,
            Event::Checked(1, sequence, Texts::read_global(&paths)),
        );
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert_eq!(global_editor(&page).form.draft.gamemode, Some(true));
    }

    #[test]
    fn global_page_checks_only_the_global_file_and_reloads_after_an_edit() {
        let (_root, paths) = setup();
        write_global(&paths, "gamemode = false\n");
        let mut page = global_page(&paths);
        write(&paths, GAME);
        let sequence = start_check(&mut page);
        let _ = page.update(
            &paths,
            Event::Checked(1, sequence, Texts::read_global(&paths)),
        );
        assert!(!page.loading());
        let _ = page.update(
            &paths,
            Event::GlobalField(global::Field::LogLimitMb, "75".into()),
        );
        write_global(&paths, "gamemode = true\n");
        let sequence = start_check(&mut page);
        let _ = page.update(
            &paths,
            Event::Checked(1, sequence, Texts::read_global(&paths)),
        );
        assert!(page.changed_on_disk);
        assert!(!page.can_save() && page.can_reload());
        let _ = page.update(&paths, Event::Reload);
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert!(!page.is_dirty());
        assert_eq!(global_editor(&page).form.draft.gamemode, Some(true));
    }

    #[test]
    fn global_controls_stay_locked_through_the_save_reload() {
        let (_root, paths) = setup();
        write_global(&paths, "[env]\nA = \"1\"\n");
        let mut page = global_page(&paths);
        let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
        let edits = global_editor(&page).form.edits();
        let _ = page.update(&paths, Event::Save);
        assert!(page.locked());
        let events = [
            Event::GlobalSwitch(Switch::Wayland, true),
            Event::GlobalRunner(RunnerChoice {
                name: "other".into(),
                launcher: None,
                proton: None,
            }),
            Event::GlobalArch(Arch::Win32),
            Event::GlobalField(global::Field::PrefixRoot, "/tmp/prefixes".into()),
            Event::GlobalField(global::Field::RunnerRoot, "/tmp/runners".into()),
            Event::GlobalField(global::Field::DxvkRoot, "/tmp/dxvk".into()),
            Event::GlobalField(global::Field::Vkd3dRoot, "/tmp/vkd3d".into()),
            Event::GlobalField(global::Field::StopTimeout, "15".into()),
            Event::GlobalField(global::Field::LogLimitMb, "80".into()),
            Event::GlobalArt(ArtFetch::Icon),
            Event::EnvKey(0, "B".into()),
            Event::EnvValue(0, "2".into()),
            Event::EnvRemove(0),
            Event::NewEnvKey("C".into()),
            Event::NewEnvValue("3".into()),
            Event::EnvAdd,
        ];
        for event in events.iter().cloned() {
            let _ = page.update(&paths, event);
        }
        assert_eq!(global_editor(&page).form.edits(), edits);
        assert_eq!(global_editor(&page).new_env, EnvRow::default());
        let report = load::save_global(&paths, &edits).unwrap();
        let _ = page.update(&paths, Event::Saved(1, Ok(report)));
        assert!(page.locked());
        for event in events {
            let _ = page.update(&paths, event);
        }
        assert_eq!(global_editor(&page).form.edits(), edits);
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert!(!page.locked() && !page.is_dirty());
    }

    #[test]
    fn global_pending_env_text_survives_a_quiet_reload_and_revert_clears_it() {
        let (_root, paths) = setup();
        write_global(&paths, "gamemode = false\n");
        let mut page = global_page(&paths);
        let _ = page.update(&paths, Event::NewEnvKey("NEW".into()));
        let _ = page.update(&paths, Event::NewEnvValue("value".into()));
        write_global(&paths, "gamemode = true\n");
        let sequence = start_check(&mut page);
        let _ = page.update(
            &paths,
            Event::Checked(1, sequence, Texts::read_global(&paths)),
        );
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert_eq!(global_editor(&page).new_env.key, "NEW");
        assert_eq!(global_editor(&page).new_env.value, "value");
        let _ = page.update(&paths, Event::GlobalArch(Arch::Win32));
        let _ = page.update(&paths, Event::Revert);
        assert_eq!(global_editor(&page).new_env, EnvRow::default());
        assert!(!page.is_dirty());
    }

    #[test]
    fn external_runners_switch_reads_and_writes_the_global_key() {
        let (_root, paths) = setup();
        let mut page = global_page(&paths);
        assert_eq!(global_editor(&page).form.draft.external_runners, None);
        let _ = page.update(&paths, Event::GlobalExternalRunners(true));
        assert_eq!(page.changes(), 1);
        let report = load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
        assert!(report.failed.is_none());
        assert_eq!(
            GlobalConfig::load(&paths).unwrap().external_runners,
            Some(true)
        );

        let mut page = global_page(&paths);
        assert_eq!(global_editor(&page).form.draft.external_runners, Some(true));
        let _ = page.update(&paths, Event::GlobalExternalRunners(false));
        load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
        assert_eq!(
            GlobalConfig::load(&paths).unwrap().external_runners,
            Some(false)
        );
    }

    #[test]
    fn an_alias_resolving_into_steams_folder_is_labeled_steam() {
        let (root, paths) = setup();
        let steam = crate::runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
        let tools = steam.parent().unwrap();
        write_global(
            &paths,
            &format!("[defaults]\nrunner_root = '{}'\n", tools.display()),
        );
        let global = GlobalConfig::load(&paths).unwrap();
        let (choices, _) = load::runner_choices(&paths, &global, "GE-Proton-Latest");
        let alias = choices
            .iter()
            .find(|choice| choice.name == "GE-Proton-Latest")
            .unwrap();
        assert_eq!(
            alias.launcher,
            Some(wineo_core::external_runners::Source::Steam)
        );
        assert_eq!(alias.proton, Some(true));
        let missing = choices
            .iter()
            .find(|choice| choice.name == "Proton-CachyOS-Latest")
            .unwrap();
        assert_eq!(missing.launcher, None);
        assert_eq!(missing.proton, None);
    }

    #[test]
    fn external_runners_switch_does_nothing_in_a_portable_folder() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let portable = root.path().join("portable");
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&portable),
        )
        .unwrap();
        let mut page = global_page(&paths);
        let _ = page.update(&paths, Event::GlobalExternalRunners(true));
        assert!(!page.is_dirty());
        assert_eq!(global_editor(&page).form.draft.external_runners, None);
    }

    #[test]
    fn portable_prefix_root_failure_is_shown_in_the_global_save_bar() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let portable = root.path().join("portable");
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&portable),
        )
        .unwrap();
        let mut page = global_page(&paths);
        let _ = page.update(
            &paths,
            Event::GlobalField(global::Field::PrefixRoot, "/tmp/outside".into()),
        );
        assert!(page.can_save());
        let report = load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
        assert!(report.failed.is_some());
        let _ = page.update(&paths, Event::Save);
        let _ = page.update(&paths, Event::Saved(1, Ok(report)));
        assert!(page.error.is_some());
        assert!(page.is_dirty());
    }

    #[test]
    fn commented_global_round_trip_changes_one_control_per_section() {
        let (_root, paths) = setup();
        let before = "# global\nschema_version = 1\ngamemode = false # launch\nstop_timeout = 5 # seconds\n\n[defaults]\narch = \"win64\" # new\n\n[art]\nfetch = \"all\" # art\n\n[env]\nA = \"old\" # env\n";
        write_global(&paths, before);
        let mut page = global_page(&paths);
        for event in [
            Event::GlobalSwitch(Switch::Gamemode, true),
            Event::GlobalArch(Arch::Win32),
            Event::GlobalField(global::Field::StopTimeout, "6".into()),
            Event::GlobalArt(ArtFetch::Icon),
            Event::EnvValue(0, "new".into()),
        ] {
            let _ = page.update(&paths, event);
        }
        assert_eq!(page.changes(), 5);
        let report = load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
        assert_eq!(report.failed, None);
        let after = fs::read_to_string(paths.global_config()).unwrap();
        assert_eq!(
            after,
            before
                .replace("gamemode = false", "gamemode = true")
                .replace("stop_timeout = 5", "stop_timeout = 6")
                .replace("arch = \"win64\"", "arch = \"win32\"")
                .replace("fetch = \"all\"", "fetch = \"icon\"")
                .replace("A = \"old\"", "A = \"new\"")
        );
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        (root, paths)
    }

    const GAME: &str = "# my game\nschema_version = 1\nprefix = \"~/prefixes/game\"\nrunner = \"missing\" # pinned\narch = \"win64\"\ngamemode = false # off here\n\n[gamescope]\nwidth = 1920 # native\n";

    fn write(paths: &Paths, text: &str) {
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn write_global(paths: &Paths, text: &str) {
        let path = paths.global_config();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn land(page: &mut Page, paths: &Paths) {
        assert!(page.loading());
        let snapshot = load::snapshot(paths, "game");
        let _ = page.update(paths, Event::Loaded(1, snapshot));
    }

    fn page(paths: &Paths) -> Page {
        let mut page = Page::new("game".into(), 1);
        let _ = page.reload(paths, Mode::Replace);
        land(&mut page, paths);
        page
    }

    fn editor(page: &Page) -> &Editor {
        page.editor().expect("form")
    }

    /// Starts a check the way the tick does and hands back its sequence.
    fn start_check(page: &mut Page) -> u64 {
        page.check_sequence += 1;
        page.checking = true;
        page.check_sequence
    }

    fn check(page: &mut Page, paths: &Paths) {
        let sequence = start_check(page);
        let _ = page.update(
            paths,
            Event::Checked(1, sequence, Texts::read(paths, "game")),
        );
    }

    /// Writes what the page would save, the way the worker does.
    fn apply(page: &Page, paths: &Paths) -> wineo_core::settings::Applied {
        let edits = editor(page).form.edits();
        let pairs: Vec<(&str, &str)> = edits
            .pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let unset: Vec<&str> = edits.unset.iter().map(String::as_str).collect();
        settings::apply(paths, ConfigFile::Environment("game"), &pairs, &unset).unwrap()
    }

    fn saved(page: &mut Page, paths: &Paths, lines: &[&str], failed: Option<&str>) {
        page.saving = true;
        let report = Report {
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            failed: failed.map(str::to_owned),
        };
        let _ = page.update(paths, Event::Saved(1, Ok(report)));
    }

    #[test]
    fn loads_file_values_and_tracks_changes() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let draft = &editor(&page).form.draft;
        assert_eq!(draft.gamemode, Some(false));
        assert!(draft.gamescope);
        assert!(!page.is_dirty());
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let _ = page.update(&paths, Event::Reset(Switch::Gamemode));
        assert_eq!(page.changes(), 2);
        let edits = editor(&page).form.edits();
        assert_eq!(edits.pairs, [("mangohud".to_owned(), "true".to_owned())]);
        assert_eq!(edits.unset, ["gamemode"]);
        let _ = page.update(&paths, Event::Revert);
        assert!(!page.is_dirty());
    }

    #[test]
    fn leaving_with_edits_asks_first() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        assert!(page.update(&paths, Event::Back).close);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        assert!(!page.update(&paths, Event::Back).close);
        assert!(page.leave_prompt);
        assert!(!page.update(&paths, Event::KeepEditing).close);
        assert!(!page.leave_prompt);
        assert!(page.is_dirty());
        let _ = page.update(&paths, Event::Back);
        assert!(page.update(&paths, Event::Discard).close);
    }

    #[test]
    fn the_leave_prompt_goes_when_the_page_is_clean_or_saving() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let _ = page.update(&paths, Event::Back);
        let _ = page.update(&paths, Event::Revert);
        assert!(!page.leave_prompt);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let _ = page.update(&paths, Event::Back);
        let _ = page.update(&paths, Event::Save);
        assert!(page.saving && !page.leave_prompt);
        page.leave_prompt = true;
        assert!(!page.update(&paths, Event::Discard).close);
    }

    #[test]
    fn a_file_change_reloads_quietly_without_edits() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        land(&mut page, &paths);
        assert!(!page.changed_on_disk);
        assert_eq!(editor(&page).form.draft.gamemode, Some(true));
        // The same text again is no change.
        check(&mut page, &paths);
        assert!(!page.loading());
    }

    #[test]
    fn a_file_change_under_edits_asks_for_reload() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        assert!(page.changed_on_disk && !page.loading());
        assert_eq!(editor(&page).form.draft.mangohud, Some(true));
        // Ctrl+S never throws edits away.
        let _ = page.update(&paths, Event::Save);
        assert!(!page.loading() && !page.saving);
        let _ = page.update(&paths, Event::Reload);
        land(&mut page, &paths);
        assert!(!page.changed_on_disk);
        assert!(!page.is_dirty());
        assert_eq!(editor(&page).form.draft.gamemode, Some(true));
    }

    #[test]
    fn edits_made_during_a_quiet_reload_are_kept() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        land(&mut page, &paths);
        assert!(page.changed_on_disk);
        assert_eq!(editor(&page).form.draft.mangohud, Some(true));
    }

    #[test]
    fn a_stale_load_is_ignored() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.reload(&paths, Mode::Replace);
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        let snapshot = load::snapshot(&paths, "game");
        let _ = page.update(&paths, Event::Loaded(7, snapshot));
        assert!(page.loading());
        assert_eq!(editor(&page).form.draft.gamemode, Some(false));
    }

    #[test]
    fn results_from_another_page_are_ignored() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        page.saving = true;
        let _ = page.update(&paths, Event::Saved(7, Err("old".into())));
        assert!(page.saving);
        assert_eq!(page.error, None);
    }

    #[test]
    fn a_save_reloads_the_page_from_disk() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        apply(&page, &paths);
        saved(&mut page, &paths, &["done"], None);
        assert!(page.loading());
        land(&mut page, &paths);
        assert!(!page.is_dirty() && !page.changed_on_disk);
        assert_eq!(editor(&page).form.loaded.mangohud, Some(true));
        assert_eq!(page.notes, ["done"]);
        check(&mut page, &paths);
        assert!(!page.loading() && !page.changed_on_disk);
    }

    #[test]
    fn a_save_restarts_an_open_add_form_scan() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::AddOpen(true));
        let old_sequence = page.scan_sequence;
        let _ = page.update(&paths, Event::Scanned(1, old_sequence, Scan::NoPrefix));
        assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        write(
            &paths,
            &TARGETED.replace("~/prefixes/game", "~/prefixes/other"),
        );
        saved(&mut page, &paths, &[], None);
        land(&mut page, &paths);
        assert!(editor(&page).add_open);
        assert_eq!(editor(&page).scan, Some(Scan::Scanning));
        let new_sequence = page.scan_sequence;
        assert_ne!(new_sequence, old_sequence);
        let _ = page.update(
            &paths,
            Event::Scanned(1, old_sequence, Scan::Failed("old prefix".into())),
        );
        assert_eq!(editor(&page).scan, Some(Scan::Scanning));
        let _ = page.update(
            &paths,
            Event::Scanned(1, new_sequence, load::scan(&paths, "game")),
        );
        assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
        let _ = page.update(&paths, Event::AddOpen(false));
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        saved(&mut page, &paths, &[], None);
        land(&mut page, &paths);
        assert!(!editor(&page).add_open);
        assert_eq!(editor(&page).scan, None);
    }

    #[test]
    fn controls_are_locked_until_a_save_has_reloaded() {
        let (_root, paths) = setup();
        write(
            &paths,
            &format!(
                "{}\n[targets.game]\nexe = \"/bin/game\"\n[targets.game.gamescope]\nwidth = 5\n",
                GAME.replace(
                    "[gamescope]",
                    "winetricks = [\"corefonts\"]\n\n[env]\nA = \"1\"\n\n[gamescope]",
                )
            ),
        );
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let _ = page.update(&paths, Event::Save);
        assert!(page.saving && page.locked());
        let edits = [
            Event::Switch(Switch::Gamemode, true),
            Event::Reset(Switch::Mangohud),
            Event::Gamescope(false),
            Event::FieldText(Field::Width, "1".into()),
            Event::FieldChoice(Field::Filter, Choice(Some("fsr"))),
            Event::GrabCursor(true),
            Event::ArgsInput("-x".into()),
            Event::ArgsAdd,
            Event::RunnerPicked(RunnerChoice {
                name: "other".into(),
                launcher: None,
                proton: None,
            }),
            Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
            Event::BuildRemoved(Component::Dxvk),
            Event::ListInput(List::Tags, "rpg".into()),
            Event::ListAdd(List::Tags),
            Event::ListRemove(List::Winetricks, 0),
            Event::EnvKey(0, "B".into()),
            Event::EnvValue(0, "2".into()),
            Event::EnvRemove(0),
            Event::NewEnvKey("C".into()),
            Event::NewEnvValue("3".into()),
            Event::EnvAdd,
            Event::Prefix("~/elsewhere".into()),
            Event::GameId("umu-1".into()),
            Event::Store("gog".into()),
            Event::ProgramPicked(ProgramChoice {
                name: "Tool".into(),
                exe: "/bin/tool".into(),
            }),
            Event::NewTargetName("tool".into()),
            Event::NewTargetPath("/bin/tool".into()),
            Event::TargetAdd,
        ]
        .into_iter()
        .chain(
            [
                TargetEdit::Exe("/bin/other".into()),
                TargetEdit::Cwd("/tmp".into()),
                TargetEdit::Name("Game".into()),
                TargetEdit::Desktop(true),
                TargetEdit::Default(true),
                TargetEdit::Switch(Switch::Gamemode, true),
                TargetEdit::Reset(Switch::Gamemode),
                TargetEdit::FieldText(Field::Width, "6".into()),
                TargetEdit::FieldChoice(Field::Filter, Choice(Some("fsr"))),
                TargetEdit::GrabCursor(true),
                TargetEdit::GamescopeArgsInput("-f".into()),
                TargetEdit::GamescopeArgsAdd,
                TargetEdit::GamescopeArgsRemove(0),
                TargetEdit::ArgsInput("-x".into()),
                TargetEdit::ArgsAdd,
                TargetEdit::GamescopeHere,
                TargetEdit::Gamescope(false),
                TargetEdit::GamescopeReset,
                TargetEdit::AskRemove,
                TargetEdit::Remove,
            ]
            .map(|edit| Event::Target("game".into(), edit)),
        )
        .chain([Event::Revert])
        .collect::<Vec<_>>();
        for event in edits.clone() {
            let _ = page.update(&paths, event);
        }
        apply(&page, &paths);
        saved(&mut page, &paths, &[], None);
        assert!(page.loading() && page.locked());
        for event in edits {
            let _ = page.update(&paths, event);
        }
        land(&mut page, &paths);
        assert!(!page.locked());
        assert!(!page.is_dirty() && !page.changed_on_disk);
        let draft = &editor(&page).form.draft;
        assert_eq!((draft.mangohud, draft.gamemode), (Some(true), Some(false)));
        assert!(draft.gamescope && draft.fields.args.is_empty());
        assert_eq!(
            (&draft.dxvk, &draft.prefix),
            (&None, &"~/prefixes/game".to_owned())
        );
        assert!(draft.tags.is_empty() && draft.gameid.is_empty() && draft.store.is_empty());
        assert_eq!(draft.winetricks, ["corefonts"]);
        assert_eq!(
            draft.env,
            [EnvRow {
                key: "A".into(),
                value: "1".into()
            }]
        );
        let game = &draft.targets["game"];
        assert_eq!(game.exe, "/bin/game");
        assert_eq!(game.gamescope, form::TargetGamescope::On);
        assert_eq!(game.fields.width, "5");
        assert!(game.args.is_empty() && game.cwd.is_empty() && game.gamemode.is_none());
        assert_eq!(draft.targets.len(), 1);
        let editor = editor(&page);
        assert!(editor.args_input.is_empty() && editor.tags_input.is_empty());
        assert_eq!(editor.new_env, EnvRow::default());
        assert_eq!(editor.new_target, NewTarget::default());
        assert!(editor.target_args_input.is_empty());
        assert!(editor.target_gamescope_args_input.is_empty());
        assert_eq!(editor.removing, None);
    }

    #[test]
    fn changing_the_file_back_clears_the_reload_state() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        assert!(page.changed_on_disk && !page.can_save());
        write(&paths, GAME);
        check(&mut page, &paths);
        assert!(!page.changed_on_disk && page.can_save() && !page.can_reload());
        assert!(page.is_dirty());
    }

    #[test]
    fn a_check_that_started_before_a_load_is_dropped() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let sequence = start_check(&mut page);
        let stale = Texts::read(&paths, "game");
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        land(&mut page, &paths);
        assert_eq!(editor(&page).form.draft.gamemode, Some(true));
        // The old file text, read before the reload, would look like a change.
        let _ = page.update(&paths, Event::Checked(1, sequence, stale));
        assert!(!page.loading() && !page.changed_on_disk);
        // A save drops checks in flight too.
        let sequence = start_check(&mut page);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let _ = page.update(&paths, Event::Save);
        let _ = page.update(&paths, Event::CheckFailed(1, sequence));
        assert!(!page.checking);
        let _ = page.update(&paths, Event::Saved(1, Err("busy".into())));
        let _ = page.update(&paths, Event::Checked(1, sequence, Texts::default()));
        assert!(!page.loading() && !page.changed_on_disk);
    }

    #[test]
    fn a_reload_closes_the_menu_state() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::MenuOpened);
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        assert!(!page.menu_open);
        let _ = page.update(&paths, Event::MenuOpened);
        land(&mut page, &paths);
        assert!(!page.menu_open);
    }

    #[test]
    fn a_failed_save_keeps_edits_and_shows_the_error() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        page.saving = true;
        let _ = page.update(&paths, Event::Saved(1, Err("Environment is busy".into())));
        assert!(!page.saving && !page.loading());
        assert_eq!(page.error.as_deref(), Some("Environment is busy"));
        assert!(page.is_dirty());
        // Failed before the write: the edits are still edits.
        saved(&mut page, &paths, &["Installed DXVK 2.4"], Some("bad"));
        land(&mut page, &paths);
        assert_eq!(page.notes, ["Installed DXVK 2.4"]);
        assert_eq!(page.error.as_deref(), Some("bad"));
        assert!(page.is_dirty() && !page.changed_on_disk);
    }

    #[test]
    fn a_save_that_failed_after_writing_is_not_an_outside_change() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        apply(&page, &paths);
        saved(&mut page, &paths, &[], Some("desktop entries failed"));
        land(&mut page, &paths);
        assert!(!page.is_dirty() && !page.changed_on_disk);
        assert_eq!(page.error.as_deref(), Some("desktop entries failed"));
        check(&mut page, &paths);
        assert!(!page.loading() && !page.changed_on_disk);
    }

    #[test]
    fn crossing_runner_kinds_is_refused_before_save() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        if let Some(editor) = page.editor_mut() {
            editor.info.current = Some(Current {
                name: "wine-tkg".into(),
                proton: false,
            });
        }
        let proton = RunnerChoice {
            name: "GE-Proton9-20".into(),
            launcher: None,
            proton: Some(true),
        };
        let _ = page.update(&paths, Event::RunnerPicked(proton));
        let editor = editor(&page);
        assert_eq!(editor.form.draft.runner.as_deref(), Some("missing"));
        assert_eq!(
            editor.refusal.as_deref(),
            Some("Can't switch between Wine and Proton runners. Make a new environment instead.")
        );
        assert!(!page.is_dirty());
        let wine = RunnerChoice {
            name: "wine-ge".into(),
            launcher: None,
            proton: Some(false),
        };
        let _ = page.update(&paths, Event::RunnerPicked(wine));
        assert_eq!(page.editor().unwrap().refusal, None);
        assert_eq!(
            page.editor().unwrap().form.draft.runner.as_deref(),
            Some("wine-ge")
        );
        // An unresolvable runner is left for save to check.
        let unknown = RunnerChoice {
            name: "gone".into(),
            launcher: None,
            proton: None,
        };
        assert!(!refused(
            page.editor().unwrap().info.current.as_ref(),
            &unknown
        ));
    }

    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_kind_mismatch_at_save_gets_the_gui_wording() {
        let (_root, paths) = setup();
        fake_runners(&paths);
        write(&paths, &GAME.replace("\"missing\"", "\"wine-tkg\""));
        let edits = form::Edits {
            pairs: vec![("runner".into(), "GE-Proton9-20".into())],
            unset: Vec::new(),
        };
        assert_eq!(
            load::save(&paths, "game", &edits),
            Err(load::KIND_MISMATCH.to_owned())
        );
    }

    #[test]
    fn a_proxied_environment_gets_the_reduced_form() {
        let (_root, paths) = setup();
        write_global(&paths, "mangohud = [");
        write(
            &paths,
            "# Imported Steam game\nkind = 'proxied' # launcher-owned\nlauncher = 'steam'\napp_id = '1086940'\ntitle = 'Old title' # keep title note\ntags = ['rpg'] # keep tag note\ndesktop = false # keep switch note\n",
        );
        let mut page = page(&paths);
        assert!(page.global_warning().is_none());
        let original = fs::read_to_string(paths.environment_config("game")).unwrap();
        let no_op = form::Edits::default();
        load::save(&paths, "game", &no_op).unwrap();
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            original
        );

        let draft = &editor(&page).form.draft;
        assert_eq!(draft.tags, ["rpg"]);
        let proxied = draft.proxied.as_ref().expect("proxied settings");
        assert_eq!(proxied.title.as_deref(), Some("Old title"));
        assert!(!proxied.desktop);

        for event in [
            Event::ProxiedTitle("New title".to_owned()),
            Event::ListInput(List::Tags, "coop".to_owned()),
            Event::ListAdd(List::Tags),
            Event::ProxiedDesktop(true),
        ] {
            let _ = page.update(&paths, event);
        }
        assert_eq!(page.changes(), 3);
        assert!(page.can_save());
        let edits = editor(&page).form.edits();
        assert_eq!(
            edits.pairs,
            [
                ("tags".to_owned(), "[\"rpg\", \"coop\"]".to_owned()),
                ("title".to_owned(), "New title".to_owned()),
                ("desktop".to_owned(), "true".to_owned()),
            ]
        );
        assert!(edits.unset.is_empty());
        let report = load::save(&paths, "game", &edits).unwrap();
        assert_eq!(report.failed, None);

        let saved = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(saved.contains("# Imported Steam game"));
        assert!(saved.contains("kind = 'proxied' # launcher-owned"));
        assert!(saved.contains("# keep title note"));
        assert!(saved.contains("# keep tag note"));
        assert!(saved.contains("# keep switch note"));
        let config =
            wineo_core::config::EnvironmentConfig::parse(&paths.environment_config("game"), &saved)
                .unwrap();
        assert_eq!(config.tags, ["rpg", "coop"]);
        let proxied = config.proxied().unwrap();
        assert_eq!(proxied.title.as_deref(), Some("New title"));
        assert!(proxied.desktop);
    }

    #[test]
    fn clearing_a_proxied_title_unsets_the_config_key() {
        let (_root, paths) = setup();
        write(
            &paths,
            "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\ntitle = 'Old title'\n",
        );
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::ProxiedTitle(String::new()));
        assert_eq!(page.changes(), 1);
        assert!(page.can_save());
        assert_eq!(
            editor(&page).form.draft.proxied.as_ref().unwrap().title,
            None
        );
        let edits = editor(&page).form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(edits.unset, ["title"]);
        let report = load::save(&paths, "game", &edits).unwrap();
        assert_eq!(report.failed, None);

        let saved = fs::read_to_string(paths.environment_config("game")).unwrap();
        let config =
            wineo_core::config::EnvironmentConfig::parse(&paths.environment_config("game"), &saved)
                .unwrap();
        assert_eq!(config.proxied().unwrap().title, None);
        assert!(!saved.contains("title ="));
    }

    #[test]
    fn an_unreadable_config_shows_its_error() {
        let (_root, paths) = setup();
        write(&paths, "this is [");
        let page = page(&paths);
        assert!(matches!(page.view, View::Error(_)));
        assert!(page.has_file());
        assert!(!page.can_save());
    }

    #[test]
    fn a_broken_global_config_warns_and_reloads_once_fixed() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        write_global(&paths, "mangohud = [");
        let mut page = page(&paths);
        let warning = page.global_warning().expect("warning").to_owned();
        assert!(
            warning.contains(&paths.global_config().display().to_string()),
            "{warning}"
        );
        assert_eq!(editor(&page).info.inherited, form::Inherited::default());
        let _ = page.update(&paths, Event::Switch(Switch::Wayland, true));
        write_global(&paths, "mangohud = true\n");
        check(&mut page, &paths);
        land(&mut page, &paths);
        assert_eq!(page.global_warning(), None);
        assert_eq!(editor(&page).info.inherited.mangohud, Some(true));
        // A global change doesn't touch the environment's edits.
        assert_eq!(editor(&page).form.draft.wayland, Some(true));
        assert!(!page.changed_on_disk);
        check(&mut page, &paths);
        assert!(!page.loading());
    }

    #[test]
    fn saving_one_control_changes_only_its_line() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Gamemode, true));
        let _ = page.update(&paths, Event::FieldText(Field::Width, "2560".into()));
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert_eq!(
            text,
            GAME.replace("gamemode = false", "gamemode = true")
                .replace("width = 1920", "width = 2560")
        );
        let before: Vec<&str> = GAME.lines().collect();
        let after: Vec<&str> = text.lines().collect();
        let changed = before
            .iter()
            .zip(&after)
            .filter(|(old, new)| old != new)
            .count();
        assert_eq!((before.len(), changed), (after.len(), 2));
    }

    #[test]
    fn switching_gamescope_on_saves_an_empty_table() {
        let (_root, paths) = setup();
        let text = "schema_version = 1\nprefix = \"~/p\"\nrunner = \"missing\"\narch = \"win64\"\ngamescope = false\n";
        write(&paths, text);
        let mut edited = page(&paths);
        assert!(!editor(&edited).form.draft.gamescope);
        let _ = edited.update(&paths, Event::Gamescope(true));
        let applied = apply(&edited, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let written = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(written.ends_with("[gamescope]\n"), "{written}");
        let reloaded = page(&paths);
        assert!(editor(&reloaded).form.draft.gamescope);
        assert_eq!(
            editor(&reloaded).form.draft.fields,
            form::GamescopeFields::default()
        );
    }

    #[test]
    fn switching_gamescope_off_removes_it_and_on_again_restores_fields() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Gamescope(false));
        apply(&page, &paths);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(!text.contains("gamescope"), "{text}");
        saved(&mut page, &paths, &[], None);
        land(&mut page, &paths);
        assert!(!page.is_dirty());
        let _ = page.update(&paths, Event::Gamescope(true));
        assert_eq!(editor(&page).form.draft.fields.width, "1920");
        assert_eq!(page.changes(), 2);
    }

    #[test]
    fn args_are_added_and_removed_as_chips() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::ArgsInput(String::new()));
        let _ = page.update(&paths, Event::ArgsAdd);
        assert!(!page.is_dirty());
        for arg in ["--backend", "sdl", "two words"] {
            let _ = page.update(&paths, Event::ArgsInput(arg.into()));
            let _ = page.update(&paths, Event::ArgsAdd);
        }
        let _ = page.update(&paths, Event::ArgsRemove(1));
        let _ = page.update(&paths, Event::ArgsRemove(9));
        assert_eq!(
            editor(&page).form.draft.fields.args,
            ["--backend", "two words"]
        );
        assert!(editor(&page).args_input.is_empty());
        apply(&page, &paths);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(
            text.contains("args = [\"--backend\", \"two words\"]"),
            "{text}"
        );
    }

    const FULL: &str = "# my game\nschema_version = 1\nprefix = \"~/prefixes/game\" # where it lives\nrunner = \"missing\" # pinned\narch = \"win64\"\ndxvk = \"2.3\" # pinned build\ntags = [\"rpg\"] # shelf\nwinetricks = [\"corefonts\"] # fonts\n\n[env]\n# chatty otherwise\nWINEDEBUG = \"-all\" # quiet\nDXVK_HUD = \"fps\"\n";

    #[test]
    fn saving_one_control_per_section_changes_only_those_lines() {
        let (root, paths) = setup();
        write(&paths, FULL);
        let mut page = page(&paths);
        let events = [
            Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
            Event::ListInput(List::Winetricks, "vcrun2019".into()),
            Event::ListAdd(List::Winetricks),
            Event::ListRemove(List::Tags, 0),
            Event::ListInput(List::Tags, "co-op".into()),
            Event::ListAdd(List::Tags),
            // Rows are in name order, so WINEDEBUG is second.
            Event::EnvValue(1, r#"a "b" \c = d"#.into()),
            Event::Prefix(root.path().join("Games/game").display().to_string()),
        ];
        for event in events {
            let _ = page.update(&paths, event);
        }
        assert_eq!(page.changes(), 5);
        assert!(page.can_save());
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        let before: Vec<&str> = FULL.lines().collect();
        let after: Vec<&str> = text.lines().collect();
        assert_eq!(before.len(), after.len(), "{text}");
        let changed: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(old, new)| old != new)
            .map(|(_, new)| *new)
            .collect();
        assert_eq!(
            changed[..4],
            [
                "prefix = \"~/Games/game\" # where it lives",
                "dxvk = \"latest\" # pinned build",
                "tags = [\"co-op\"] # shelf",
                "winetricks = [\"corefonts\", \"vcrun2019\"] # fonts",
            ]
        );
        assert!(changed[4].starts_with("WINEDEBUG = ") && changed[4].ends_with(" # quiet"));
        assert_eq!(changed.len(), 5);
        let config = wineo_core::environment::Environment::load(&paths, "game").unwrap();
        assert_eq!(config.config.env["WINEDEBUG"], r#"a "b" \c = d"#);
    }

    fn env(paths: &Paths) -> std::collections::BTreeMap<String, String> {
        wineo_core::environment::Environment::load(paths, "game")
            .unwrap()
            .config
            .env
    }

    #[test]
    fn an_env_name_with_a_dot_and_a_quote_round_trips() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let key = r#"MY.VAR"X"#;
        let _ = page.update(&paths, Event::NewEnvKey(key.into()));
        let _ = page.update(&paths, Event::NewEnvValue("1".into()));
        let _ = page.update(&paths, Event::EnvAdd);
        assert_eq!(
            editor(&page).form.edits().pairs,
            [(format!("env.{key}"), "1".to_owned())]
        );
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        assert_eq!(env(&paths)[key], "1");
        saved(&mut page, &paths, &[], None);
        land(&mut page, &paths);
        assert!(!page.is_dirty());
        assert_eq!(editor(&page).form.draft.env[0].key, key);
    }

    #[test]
    fn swapping_two_env_names_sets_both_and_unsets_neither() {
        let (_root, paths) = setup();
        write(&paths, &format!("{GAME}\n[env]\nA = \"1\"\nB = \"2\"\n"));
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::EnvKey(0, "B".into()));
        // A duplicate for a moment, until the other row is renamed too.
        assert!(!page.can_save());
        let _ = page.update(&paths, Event::EnvKey(1, "A".into()));
        assert!(page.can_save());
        let edits = editor(&page).form.edits();
        assert!(edits.unset.is_empty());
        assert_eq!(
            edits.pairs,
            [
                ("env.A".to_owned(), "2".to_owned()),
                ("env.B".to_owned(), "1".to_owned())
            ]
        );
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let written = env(&paths);
        assert_eq!((written["A"].as_str(), written["B"].as_str()), ("2", "1"));
        assert_eq!(written.len(), 2);
    }

    #[test]
    fn a_native_page_takes_no_wine_only_edits() {
        let (_root, paths) = setup();
        write(&paths, "schema_version = 1\nkind = \"native\"\n");
        let mut page = page(&paths);
        for event in [
            Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
            Event::ListInput(List::Winetricks, "corefonts".into()),
            Event::ListAdd(List::Winetricks),
            Event::Prefix("~/p".into()),
            Event::GameId("umu-1".into()),
            Event::Store("gog".into()),
            Event::Switch(Switch::Wayland, true),
        ] {
            let _ = page.update(&paths, event);
        }
        assert!(!page.is_dirty());
        assert!(editor(&page).winetricks_input.is_empty());
        // Native programs may have their own WINEPREFIX.
        let _ = page.update(&paths, Event::NewEnvKey("WINEPREFIX".into()));
        let _ = page.update(&paths, Event::NewEnvValue("/x".into()));
        let _ = page.update(&paths, Event::EnvAdd);
        let _ = page.update(&paths, Event::ListInput(List::Tags, "linux".into()));
        let _ = page.update(&paths, Event::ListAdd(List::Tags));
        assert_eq!(page.changes(), 2);
        apply(&page, &paths);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(text.contains("WINEPREFIX = \"/x\""), "{text}");
    }

    /// A wine build and a Proton build in the runner root, as `wineo runners`
    /// would find them.
    fn fake_runners(paths: &Paths) {
        let root = GlobalConfig::default().runner_root(paths).unwrap();
        for program in ["wine", "wineserver"] {
            executable(&root.join("wine-tkg/bin").join(program));
            executable(&root.join("GE-Proton9-20/files/bin").join(program));
        }
        executable(&root.join("GE-Proton9-20/proton"));
        fs::write(root.join("GE-Proton9-20/version"), "1 GE-Proton9-20\n").unwrap();
    }

    #[test]
    fn proton_disables_components_and_shows_protonfixes() {
        let (_root, paths) = setup();
        fake_runners(&paths);
        write(&paths, &GAME.replace("\"missing\"", "\"GE-Proton9-20\""));
        let mut page = page(&paths);
        assert!(editor(&page).proton());
        let _ = page.update(
            &paths,
            Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
        );
        assert!(!page.is_dirty());
        let shown = editor(&page);
        assert!(!shown.build_enabled("game", Component::Vkd3d));
        assert_eq!(
            shown.build_refusal("game", Component::Dxvk).as_deref(),
            Some(
                "`dxvk` only means something to wine, and `game` runs on GE-Proton9-20, which bundles its own"
            )
        );
        let _ = page.update(&paths, Event::GameId("umu-1".into()));
        assert_eq!(
            editor(&page).form.edits().pairs,
            [("gameid".to_owned(), "umu-1".to_owned())]
        );
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(text.contains("gameid = \"umu-1\""), "{text}");
    }

    #[test]
    fn stale_builds_on_proton_can_only_be_removed() {
        let (_root, paths) = setup();
        fake_runners(&paths);
        write(
            &paths,
            &GAME
                .replace("\"missing\"", "\"GE-Proton9-20\"")
                .replace("arch = ", "dxvk = \"latest\"\nvkd3d = \"latest\"\narch = "),
        );
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        // Core would refuse the save, so it doesn't get that far.
        let reason = page.problem().expect("problem");
        assert!(
            reason.starts_with("`dxvk` only means something to wine"),
            "{reason}"
        );
        assert!(!page.can_save());
        let _ = page.update(&paths, Event::BuildPicked(Component::Vkd3d, Build(None)));
        assert_eq!(editor(&page).form.draft.vkd3d.as_deref(), Some("latest"));
        let _ = page.update(&paths, Event::BuildRemoved(Component::Dxvk));
        let shown = editor(&page);
        assert_eq!(
            shown.build_refusal.as_deref(),
            Some(form::REMOVE_VKD3D_FIRST)
        );
        assert_eq!(shown.form.draft.dxvk.as_deref(), Some("latest"));
        let _ = page.update(&paths, Event::BuildRemoved(Component::Vkd3d));
        let _ = page.update(&paths, Event::BuildRemoved(Component::Dxvk));
        assert_eq!(editor(&page).build_refusal, None);
        assert_eq!(page.problem(), None);
        let edits = editor(&page).form.edits();
        assert_eq!(edits.unset, ["dxvk", "vkd3d"]);
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(!text.contains("dxvk") && !text.contains("vkd3d"), "{text}");
        // Nothing left to remove.
        let _ = page.update(&paths, Event::BuildRemoved(Component::Dxvk));
        assert_eq!(page.changes(), 3);
    }

    #[test]
    fn proton_with_only_vkd3d_says_to_remove_it() {
        let (_root, paths) = setup();
        fake_runners(&paths);
        write(
            &paths,
            &GAME
                .replace("\"missing\"", "\"GE-Proton9-20\"")
                .replace("arch = ", "vkd3d = \"latest\"\narch = "),
        );
        let mut page = page(&paths);
        let reason = page.problem().expect("problem");
        assert!(
            reason.starts_with("`vkd3d` only means something to wine"),
            "{reason}"
        );
        let _ = page.update(&paths, Event::BuildRemoved(Component::Vkd3d));
        assert_eq!(page.problem(), None);
        assert!(page.can_save());
    }

    #[test]
    fn gating_follows_the_runner_picked_in_the_draft() {
        let (_root, paths) = setup();
        fake_runners(&paths);
        // An unresolvable runner can go either way, so nothing is gated yet.
        write(&paths, GAME);
        let mut page = page(&paths);
        assert!(!editor(&page).proton());
        assert!(!editor(&page).protonfixes_shown());
        let proton = RunnerChoice {
            name: "GE-Proton9-20".into(),
            launcher: None,
            proton: Some(true),
        };
        let _ = page.update(&paths, Event::RunnerPicked(proton));
        let shown = editor(&page);
        assert!(shown.proton() && shown.protonfixes_shown());
        assert!(!shown.build_enabled("game", Component::Dxvk));
        let _ = page.update(&paths, Event::GameId("umu-1".into()));
        assert_eq!(editor(&page).form.draft.gameid, "umu-1");
        assert!(page.can_save());
        let wine = RunnerChoice {
            name: "wine-tkg".into(),
            launcher: None,
            proton: Some(false),
        };
        let _ = page.update(&paths, Event::RunnerPicked(wine));
        let shown = editor(&page);
        assert!(!shown.proton() && shown.protonfixes_shown());
        assert!(shown.build_enabled("game", Component::Dxvk));
        assert_eq!(
            page.problem().as_deref(),
            Some(
                "Game id and store only apply to Proton runners. Pick a Proton runner or clear them."
            )
        );
        assert!(!page.can_save());
        let _ = page.update(&paths, Event::GameId(String::new()));
        assert_eq!(page.problem(), None);
        assert!(!editor(&page).protonfixes_shown());
        let _ = page.update(
            &paths,
            Event::RunnerPicked(RunnerChoice {
                name: "GE-Proton9-20".into(),
                launcher: None,
                proton: Some(true),
            }),
        );
        let _ = page.update(&paths, Event::Store("gog".into()));
        let _ = page.update(
            &paths,
            Event::RunnerPicked(RunnerChoice {
                name: "wine-tkg".into(),
                launcher: None,
                proton: Some(false),
            }),
        );
        assert!(editor(&page).protonfixes_shown());
        assert!(!page.can_save());
        let _ = page.update(&paths, Event::Store(String::new()));
        assert_eq!(page.problem(), None);
    }

    #[test]
    fn vkd3d_without_dxvk_blocks_saving_only_when_core_checks_it() {
        let (_root, paths) = setup();
        fake_runners(&paths);
        write(
            &paths,
            &GAME.replace("arch = ", "vkd3d = \"latest\"\narch = "),
        );
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        assert_eq!(page.problem(), None);
        assert!(page.can_save());
        let wine = RunnerChoice {
            name: "wine-tkg".into(),
            launcher: None,
            proton: Some(false),
        };
        let _ = page.update(&paths, Event::RunnerPicked(wine));
        assert_eq!(page.problem().as_deref(), Some(form::VKD3D_NEEDS_DXVK));
        assert!(!page.can_save());
    }

    #[test]
    fn dropping_dxvk_under_vkd3d_is_refused_with_the_reason() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let vkd3d = Event::BuildPicked(Component::Vkd3d, Build(Some("latest".into())));
        let _ = page.update(&paths, vkd3d.clone());
        assert!(!page.is_dirty());
        let _ = page.update(
            &paths,
            Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
        );
        let _ = page.update(&paths, vkd3d);
        let _ = page.update(&paths, Event::BuildPicked(Component::Dxvk, Build(None)));
        let editor = editor(&page);
        assert_eq!(
            editor.build_refusal.as_deref(),
            Some(form::VKD3D_NEEDS_DXVK)
        );
        assert_eq!(editor.form.draft.dxvk.as_deref(), Some("latest"));
        assert!(page.can_save());
    }

    #[test]
    fn text_typed_but_not_added_survives_a_quiet_reload() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let typing = [
            Event::ArgsInput("-e".into()),
            Event::ListInput(List::Winetricks, "corefonts".into()),
            Event::ListInput(List::Tags, "rpg".into()),
            Event::NewEnvKey("A".into()),
            Event::NewEnvValue("1".into()),
        ];
        for event in typing {
            let _ = page.update(&paths, event);
        }
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        land(&mut page, &paths);
        assert!(!page.changed_on_disk);
        let shown = editor(&page);
        assert_eq!(shown.form.draft.gamemode, Some(true));
        assert_eq!(
            (
                shown.args_input.as_str(),
                shown.winetricks_input.as_str(),
                shown.tags_input.as_str()
            ),
            ("-e", "corefonts", "rpg")
        );
        assert_eq!(
            (shown.new_env.key.as_str(), shown.new_env.value.as_str()),
            ("A", "1")
        );
        // Typing while the reload runs lands too.
        write(&paths, GAME);
        check(&mut page, &paths);
        let _ = page.update(&paths, Event::ListInput(List::Tags, "co-op".into()));
        land(&mut page, &paths);
        assert_eq!(editor(&page).tags_input, "co-op");
        // Reload after an outside change throws it away with the edits.
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
        check(&mut page, &paths);
        let _ = page.update(&paths, Event::Reload);
        land(&mut page, &paths);
        assert!(editor(&page).tags_input.is_empty());
    }

    #[test]
    fn a_relative_prefix_turns_save_off() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Prefix("prefixes/game2".into()));
        assert!(page.is_dirty() && !page.can_save());
        assert_eq!(
            page.problem().as_deref(),
            Some("The prefix must be absolute or start with ~/.")
        );
        let _ = page.update(&paths, Event::Prefix("~/prefixes/game2".into()));
        assert!(page.can_save());
    }

    #[test]
    fn revert_clears_text_typed_but_not_added() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        for event in [
            Event::ArgsInput("-e".into()),
            Event::ListInput(List::Winetricks, "corefonts".into()),
            Event::ListInput(List::Tags, "rpg".into()),
            Event::NewEnvKey("A".into()),
            Event::NewEnvValue("1".into()),
            Event::Switch(Switch::Mangohud, true),
            Event::Revert,
        ] {
            let _ = page.update(&paths, event);
        }
        let editor = editor(&page);
        assert!(editor.args_input.is_empty() && editor.winetricks_input.is_empty());
        assert!(editor.tags_input.is_empty());
        assert_eq!(editor.new_env, EnvRow::default());
    }

    #[test]
    fn chips_and_env_rows_refuse_bad_input() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        for (list, bad) in [(List::Winetricks, "-q"), (List::Tags, "a b")] {
            let _ = page.update(&paths, Event::ListInput(list, bad.into()));
            let _ = page.update(&paths, Event::ListAdd(list));
        }
        let _ = page.update(&paths, Event::NewEnvKey("WINEPREFIX".into()));
        let _ = page.update(&paths, Event::EnvAdd);
        assert!(!page.is_dirty());
        let _ = page.update(&paths, Event::NewEnvKey("A".into()));
        let _ = page.update(&paths, Event::EnvAdd);
        let _ = page.update(&paths, Event::NewEnvKey("A".into()));
        let _ = page.update(&paths, Event::EnvAdd);
        assert_eq!(editor(&page).form.draft.env.len(), 1);
        assert_eq!(editor(&page).new_env.key, "A");
        // Renaming a row onto another's name blocks Save until it's fixed.
        let _ = page.update(&paths, Event::NewEnvKey("B".into()));
        let _ = page.update(&paths, Event::EnvAdd);
        let _ = page.update(&paths, Event::EnvKey(1, "A".into()));
        assert!(page.is_dirty() && !page.can_save());
        let _ = page.update(&paths, Event::Save);
        assert!(!page.saving);
        let _ = page.update(&paths, Event::EnvKey(1, "B".into()));
        assert!(page.can_save());
    }

    fn open_app(paths: &Paths) -> App {
        let mut app = App::boot(paths.clone()).0;
        app.selected = Some("game".into());
        let _ = app.update(Message::OpenSettings);
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot(paths, "game"),
        )));
        app
    }

    fn open_global_app(paths: &Paths) -> App {
        let mut app = App::boot(paths.clone()).0;
        let _ = app.open_global_settings();
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(paths),
        )));
        app
    }

    #[test]
    fn key_save_result_lands_after_leaving_and_returning_to_global_settings() {
        use crate::sidebar::Section;

        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
            steamgriddb::Secret::new("private-value".to_owned()),
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
        let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
        let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

        let _ = app.update(Message::Section(Section::Trash));
        assert!(app.settings.is_none());
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
            generation,
            sequence,
            input_session,
            Ok(steamgriddb::Saved {
                status: steamgriddb::Status {
                    source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                    override_note: false,
                },
                warning: None,
                result: Some("Saved the SteamGridDB key to the keyring".to_owned()),
            }),
        )));
        assert!(app.steamgriddb_late_result.is_some());

        let _ = app.update(Message::Section(Section::Settings));

        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(&paths),
        )));
        let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
            generation,
            sequence,
            Ok(steamgriddb::Status {
                source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                override_note: false,
            }),
        )));

        let page = app.settings.as_ref().unwrap();
        assert!(page.is_global());
        assert_eq!(
            page.steamgriddb.result.as_deref(),
            Some("Saved the SteamGridDB key to the keyring")
        );
        assert_eq!(
            page.steamgriddb.status.as_ref().unwrap().source,
            Some(wineo_core::steamgriddb::KeySource::Keyring)
        );
        assert!(app.steamgriddb_late_result.is_none());
    }

    #[test]
    fn failed_key_save_result_returns_with_an_empty_input_and_error() {
        use crate::sidebar::Section;

        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
            steamgriddb::Secret::new("private-value".to_owned()),
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
        let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
        let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

        let _ = app.update(Message::Section(Section::Trash));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
            generation,
            sequence,
            input_session,
            Err("SteamGridDB rejected the key".to_owned()),
        )));
        let _ = app.update(Message::Section(Section::Settings));

        let settings = &app.settings.as_ref().unwrap().steamgriddb;
        assert!(settings.input_open);
        assert!(settings.input.as_str().is_empty());
        assert_eq!(
            settings.error.as_deref(),
            Some("SteamGridDB rejected the key")
        );
    }

    #[test]
    fn completed_save_drops_an_older_status_result() {
        use crate::sidebar::Section;

        let (_root, paths) = setup();
        let config = paths.environment_config("game");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();

        let mut app = open_global_app(&paths);
        let old_generation = app.settings.as_ref().unwrap().generation;
        let old_status_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
        let identity = crate::art_page::EnvironmentIdentity::read(&paths, "game").unwrap();
        app.art_pages.insert(
            "game".to_owned(),
            crate::art_page::Page::new("game".to_owned(), 1, identity),
        );
        let _ = app.update(Message::Section(Section::Trash));
        let _ = app.update(Message::Section(Section::Settings));
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(&paths),
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
            steamgriddb::Secret::new("private-value".to_owned()),
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
        let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
        let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
            generation,
            sequence,
            input_session,
            Ok(steamgriddb::Saved {
                status: steamgriddb::Status {
                    source: None,
                    override_note: false,
                },
                warning: None,
                result: Some("Saved the SteamGridDB key".to_owned()),
            }),
        )));

        let refreshed_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
            generation,
            refreshed_sequence,
            Ok(steamgriddb::Status {
                source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                override_note: false,
            }),
        )));
        let _ = app.update(Message::SteamGridDbKeyChangedAt(refreshed_sequence, true));

        let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
            old_generation,
            old_status_sequence,
            Ok(steamgriddb::Status {
                source: None,
                override_note: false,
            }),
        )));

        assert_eq!(
            app.settings
                .as_ref()
                .unwrap()
                .steamgriddb
                .status
                .as_ref()
                .unwrap()
                .source,
            Some(wineo_core::steamgriddb::KeySource::Keyring)
        );
        assert!(app.steamgriddb_available);
    }

    #[test]
    fn newest_pending_status_can_land_after_reopening_settings() {
        use crate::sidebar::Section;

        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let old_generation = app.settings.as_ref().unwrap().generation;
        let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

        let _ = app.update(Message::Section(Section::Trash));
        let _ = app.update(Message::Section(Section::Settings));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
            old_generation,
            sequence,
            Ok(steamgriddb::Status {
                source: None,
                override_note: false,
            }),
        )));

        let page = app.settings.as_ref().unwrap();
        assert_ne!(page.generation, old_generation);
        assert_eq!(page.steamgriddb.status.as_ref().unwrap().source, None);
    }

    #[test]
    fn old_key_operation_results_leave_a_new_input_session_untouched() {
        use crate::sidebar::Section;

        for remove in [false, true] {
            let (_root, paths) = setup();
            let mut app = open_global_app(&paths);
            let old_generation = app.settings.as_ref().unwrap().generation;
            let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
            let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
                steamgriddb::Secret::new("old-input".to_owned()),
            )));
            let old_input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
            let operation = if remove {
                Event::SteamGridDbKeyRemove
            } else {
                Event::SteamGridDbKeySet
            };
            let _ = app.update(Message::Settings(operation));
            let old_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

            let _ = app.update(Message::Section(Section::Trash));
            let _ = app.update(Message::Section(Section::Settings));
            let new_generation = app.settings.as_ref().unwrap().generation;
            let _ = app.update(Message::Settings(Event::Loaded(
                new_generation,
                load::snapshot_global(&paths),
            )));
            let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
            let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
                steamgriddb::Secret::new("new-input".to_owned()),
            )));
            let new_input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;

            let result = if remove {
                Event::SteamGridDbKeyRemoved(
                    old_generation,
                    old_sequence,
                    old_input_session,
                    Ok(steamgriddb::Removed {
                        status: steamgriddb::Status {
                            source: None,
                            override_note: false,
                        },
                        result: "Old remove completed".to_owned(),
                    }),
                )
            } else {
                Event::SteamGridDbKeySaved(
                    old_generation,
                    old_sequence,
                    old_input_session,
                    Ok(steamgriddb::Saved {
                        status: steamgriddb::Status {
                            source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                            override_note: false,
                        },
                        warning: None,
                        result: Some("Old save completed".to_owned()),
                    }),
                )
            };
            let _ = app.update(Message::Settings(result));

            let settings = &app.settings.as_ref().unwrap().steamgriddb;
            assert!(settings.input_open);
            assert_eq!(settings.input.as_str(), "new-input");
            assert_eq!(
                app.settings.as_ref().unwrap().steamgriddb_input_session,
                new_input_session
            );
            assert_eq!(
                settings.result.as_deref(),
                Some(if remove {
                    "Old remove completed"
                } else {
                    "Old save completed"
                })
            );
        }
    }

    #[test]
    fn key_work_allows_saving_unrelated_global_edits() {
        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::GlobalSwitch(
            Switch::Gamemode,
            true,
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
            steamgriddb::Secret::new("private-value".to_owned()),
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
        let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
        let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
        assert!(app.settings.as_ref().unwrap().can_save());

        let _ = app.update(Message::Settings(Event::Save));

        assert!(app.settings.as_ref().unwrap().saving);
        assert!(!app.settings.as_ref().unwrap().can_save());
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
            generation,
            sequence,
            input_session,
            Ok(steamgriddb::Saved {
                status: steamgriddb::Status {
                    source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                    override_note: false,
                },
                warning: None,
                result: None,
            }),
        )));
        let status_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
            generation,
            status_sequence,
            Ok(steamgriddb::Status {
                source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                override_note: false,
            }),
        )));
        assert!(app.settings.as_ref().unwrap().saving);
        assert!(app.settings.as_ref().unwrap().is_dirty());
        assert_eq!(
            app.settings
                .as_ref()
                .unwrap()
                .steamgriddb
                .status
                .as_ref()
                .unwrap()
                .source,
            Some(wineo_core::steamgriddb::KeySource::Keyring)
        );
    }

    #[test]
    fn save_key_reaches_both_settings_pages_without_the_library_search_widget() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        for global in [false, true] {
            let mut app = if global {
                let mut app = App::boot(paths.clone()).0;
                app.settings = Some(global_page(&paths));
                app
            } else {
                open_app(&paths)
            };
            let event = if global {
                Event::GlobalSwitch(Switch::Gamemode, true)
            } else {
                Event::Switch(Switch::Wayland, true)
            };
            let _ = app.update(Message::Settings(event));
            let _ = app.update(Message::Key(
                Key::Character("s".into()),
                Modifiers::CTRL,
                iced::event::Status::Captured,
            ));
            assert!(app.settings.as_ref().unwrap().saving);
        }
    }

    #[test]
    fn escape_leaves_environment_settings_but_not_global_settings() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        for global in [false, true] {
            let mut app = if global {
                let mut app = App::boot(paths.clone()).0;
                app.settings = Some(global_page(&paths));
                app
            } else {
                open_app(&paths)
            };
            let _ = app.update(Message::Key(
                Key::Named(Named::Escape),
                Modifiers::empty(),
                iced::event::Status::Captured,
            ));
            assert!(app.settings.is_some());
            let _ = app.update(Message::Key(
                Key::Named(Named::Escape),
                Modifiers::empty(),
                iced::event::Status::Ignored,
            ));
            assert_eq!(app.settings.is_some(), global);
        }
    }

    #[test]
    fn mouse_back_leaves_environment_settings_even_when_captured() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = open_app(&paths);
        crate::tests::press_back(&mut app, iced::event::Status::Captured);
        assert!(app.settings.is_none());
    }

    #[test]
    fn escape_asks_before_leaving_and_again_keeps_editing() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = open_app(&paths);
        let escape = Key::Named(Named::Escape);
        // A focused field takes the first Escape.
        let _ = app.settings_key(&escape, Modifiers::empty(), true);
        assert!(!app.settings.as_ref().unwrap().leave_prompt);
        let _ = app.update(Message::Settings(Event::Switch(Switch::Wayland, true)));
        let _ = app.settings_key(&escape, Modifiers::empty(), false);
        assert!(app.settings.as_ref().unwrap().leave_prompt);
        let _ = app.settings_key(&escape, Modifiers::empty(), false);
        let page = app.settings.as_ref().unwrap();
        assert!(!page.leave_prompt && page.is_dirty());
        let _ = app.settings_key(&Key::Character("s".into()), Modifiers::CTRL, true);
        assert!(app.settings.as_ref().unwrap().saving);
    }

    #[test]
    fn escape_with_a_menu_open_stays_on_the_page() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = open_app(&paths);
        let escape = Key::Named(Named::Escape);
        let _ = app.update(Message::Settings(Event::MenuOpened));
        let _ = app.settings_key(&escape, Modifiers::empty(), false);
        assert!(app.settings.is_some());
        let _ = app.update(Message::Settings(Event::MenuClosed));
        let _ = app.settings_key(&escape, Modifiers::empty(), false);
        assert!(app.settings.is_none());
    }

    #[test]
    fn leaving_a_clean_page_returns_to_the_library() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = open_app(&paths);
        let _ = app.update(Message::Settings(Event::Back));
        assert!(app.settings.is_none());
        assert_eq!(app.selected.as_deref(), Some("game"));
    }

    fn sidebar_app(paths: &Paths, global: bool) -> App {
        if global {
            let mut app = App::boot(paths.clone()).0;
            app.settings = Some(global_page(paths));
            app
        } else {
            open_app(paths)
        }
    }

    fn sidebar_request(section: crate::sidebar::Section, keyboard: bool) -> Message {
        if keyboard {
            let digit = match section {
                crate::sidebar::Section::Library => "1",
                crate::sidebar::Section::Trash => "2",
                crate::sidebar::Section::Checkup => "3",
                crate::sidebar::Section::Downloads => "4",
                crate::sidebar::Section::Snapshots => "5",
                crate::sidebar::Section::Settings => ",",
            };
            let (mut messages, status) =
                crate::shortcuts::tests::input_messages("settings field", digit, Modifiers::CTRL);
            assert_eq!(status, iced::event::Status::Captured);
            assert!(matches!(messages.as_slice(), [Message::Section(_)]));
            messages.pop().unwrap()
        } else {
            Message::Section(section)
        }
    }

    #[test]
    fn sidebar_leaves_clean_settings_and_confirms_dirty_settings() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        for global in [false, true] {
            for keyboard in [false, true] {
                for section in [
                    Section::Library,
                    Section::Trash,
                    Section::Checkup,
                    Section::Downloads,
                ] {
                    let mut app = sidebar_app(&paths, global);
                    let _ = app.update(sidebar_request(section, keyboard));
                    assert!(app.settings.is_none());
                    assert_eq!(app.section, section);

                    let mut app = sidebar_app(&paths, global);
                    let edit = if global {
                        Event::GlobalSwitch(Switch::Gamemode, true)
                    } else {
                        Event::Switch(Switch::Wayland, true)
                    };
                    let _ = app.update(Message::Settings(edit));
                    let _ = app.update(sidebar_request(section, keyboard));
                    let page = app.settings.as_ref().unwrap();
                    assert!(page.is_dirty() && page.leave_prompt);
                    assert_eq!(app.section, Section::Library);
                    assert_eq!(app.pending_section, Some(section));
                    let _ = app.update(Message::Settings(Event::Discard));
                    assert!(app.settings.is_none());
                    assert_eq!(app.section, section);
                    assert_eq!(app.pending_section, None);
                }
            }
        }
    }

    #[test]
    fn checkup_heading_opens_details_after_discard_and_forgets_kept_edits() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        std::fs::write(paths.environment_config("other"), "kind = 'native'\n").unwrap();
        for global in [false, true] {
            for discard in [false, true] {
                let mut app = sidebar_app(&paths, global);
                app.cards = crate::library::load_library(&paths).unwrap();
                app.selected = Some("other".into());
                let edit = if global {
                    Event::GlobalSwitch(Switch::Gamemode, true)
                } else {
                    Event::Switch(Switch::Wayland, true)
                };
                let _ = app.update(Message::Settings(edit));
                let _ = app.update(Message::Checkup(crate::checkup::Event::Environment(
                    "game".into(),
                )));
                assert!(app.settings.as_ref().unwrap().leave_prompt);
                assert_eq!(app.selected.as_deref(), Some("other"));
                assert_eq!(app.pending_checkup_environment.as_deref(), Some("game"));
                if discard {
                    let _ = app.update(Message::Settings(Event::Discard));
                    assert!(app.settings.is_none() && app.panel_open);
                    assert_eq!(app.selected.as_deref(), Some("game"));
                    assert_eq!(app.section, crate::sidebar::Section::Library);
                } else {
                    let _ = app.update(Message::Settings(Event::KeepEditing));
                    assert!(app.settings.as_ref().unwrap().is_dirty());
                    assert_eq!(app.selected.as_deref(), Some("other"));
                    assert!(app.pending_checkup_environment.is_none());
                    let _ = app.update(Message::Settings(Event::Back));
                    let _ = app.update(Message::Settings(Event::Discard));
                    assert_eq!(app.selected.as_deref(), Some("other"));
                }
                assert!(app.pending_section.is_none());
                assert!(app.pending_checkup_environment.is_none());
            }
        }
    }

    #[test]
    fn sidebar_keys_respect_fields_and_the_unsaved_prompt() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        for global in [false, true] {
            let mut app = sidebar_app(&paths, global);
            let edit = if global {
                Event::GlobalSwitch(Switch::Gamemode, true)
            } else {
                Event::Switch(Switch::Wayland, true)
            };
            let _ = app.update(Message::Settings(edit));
            for digit in ["1", "2", "3"] {
                let _ = app.update(Message::Key(
                    Key::Character(digit.into()),
                    Modifiers::CTRL,
                    iced::event::Status::Captured,
                ));
                assert!(!app.settings.as_ref().unwrap().leave_prompt);
                assert_eq!(app.pending_section, None);
            }
            let _ = app.update(sidebar_request(Section::Trash, true));
            assert!(app.settings.as_ref().unwrap().leave_prompt);
            assert_eq!(app.pending_section, Some(Section::Trash));
            for section in [Section::Library, Section::Checkup] {
                let _ = app.update(sidebar_request(section, true));
                let _ = app.update(Message::Section(section));
                let _ = app.update(Message::KeyWithFocus(
                    Key::Character("3".into()),
                    Modifiers::CTRL,
                    iced::event::Status::Ignored,
                    false,
                ));
                assert_eq!(app.section, Section::Library);
                assert_eq!(app.pending_section, Some(Section::Trash));
            }
            let _ = app.update(Message::Settings(Event::KeepEditing));
            assert!(app.settings.as_ref().unwrap().is_dirty());
            assert!(!app.settings.as_ref().unwrap().leave_prompt);
            assert_eq!(app.pending_section, None);
            let _ = app.update(Message::Settings(Event::Back));
            let _ = app.update(Message::Settings(Event::Discard));
            assert_eq!(app.section, Section::Library);
        }
    }

    #[test]
    fn sidebar_cannot_leave_settings_during_a_save_or_open_menu() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        for global in [false, true] {
            let mut app = sidebar_app(&paths, global);
            let _ = app.update(Message::Settings(Event::MenuOpened));
            let _ = app.update(sidebar_request(Section::Trash, true));
            let _ = app.update(Message::Section(Section::Trash));
            assert!(app.settings.is_some());
            assert_eq!(app.section, Section::Library);
            assert_eq!(app.pending_section, None);
            let _ = app.update(Message::Settings(Event::MenuClosed));
            app.settings.as_mut().unwrap().saving = true;
            let _ = app.update(sidebar_request(Section::Checkup, true));
            let _ = app.update(Message::Section(Section::Checkup));
            assert!(app.settings.is_some());
            assert_eq!(app.section, Section::Library);
            assert_eq!(app.pending_section, None);
        }
    }

    #[test]
    fn settings_row_is_selected_only_for_global_settings() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        let app = App::boot(paths.clone()).0;
        assert!(app.section_selected(Section::Library));
        assert!(!app.section_selected(Section::Settings));

        let app = sidebar_app(&paths, true);
        assert!(app.section_selected(Section::Settings));
        assert!(!app.section_selected(Section::Library));
        assert!(!app.section_selected(Section::Trash));

        let app = sidebar_app(&paths, false);
        assert!(app.section_selected(Section::Library));
        assert!(!app.section_selected(Section::Settings));
    }

    #[test]
    fn control_comma_and_the_row_open_global_settings_from_any_section() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        for section in [
            Section::Library,
            Section::Trash,
            Section::Checkup,
            Section::Downloads,
        ] {
            for keyboard in [false, true] {
                let mut app = App::boot(paths.clone()).0;
                let _ = app.update(Message::Section(section));
                let _ = app.update(sidebar_request(Section::Settings, keyboard));
                assert!(app.global_settings_open());
                assert_eq!(app.section, Section::Library);
                assert!(app.section_selected(Section::Settings));
            }
        }
        let mut app = App::boot(paths.clone()).0;
        crate::shortcuts::tests::press(&mut app, ",", Modifiers::CTRL);
        assert!(app.global_settings_open());
    }

    #[test]
    fn opening_settings_again_keeps_global_edits_and_its_page() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = sidebar_app(&paths, true);
        let _ = app.update(Message::Settings(Event::GlobalSwitch(
            Switch::Gamemode,
            true,
        )));
        let generation = app.settings.as_ref().unwrap().generation;
        for keyboard in [false, true] {
            let _ = app.update(sidebar_request(Section::Settings, keyboard));
            let page = app.settings.as_ref().unwrap();
            assert!(page.is_dirty() && !page.leave_prompt);
            assert_eq!(page.generation, generation);
            assert_eq!(app.pending_section, None);
        }
    }

    #[test]
    fn environment_settings_with_edits_prompt_before_opening_global_settings() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        for keyboard in [false, true] {
            let mut app = sidebar_app(&paths, false);
            let _ = app.update(Message::Settings(Event::Switch(Switch::Wayland, true)));
            let _ = app.update(sidebar_request(Section::Settings, keyboard));
            let page = app.settings.as_ref().unwrap();
            assert!(page.is_dirty() && page.leave_prompt && !page.is_global());
            assert_eq!(app.pending_section, Some(Section::Settings));
            assert!(app.section_selected(Section::Library));

            let _ = app.update(Message::Settings(Event::KeepEditing));
            assert!(!app.settings.as_ref().unwrap().is_global());
            assert_eq!(app.pending_section, None);

            let _ = app.update(sidebar_request(Section::Settings, keyboard));
            let _ = app.update(Message::Settings(Event::Discard));
            assert!(app.global_settings_open());
            assert_eq!(app.pending_section, None);
            assert!(app.section_selected(Section::Settings));
        }
        let mut app = sidebar_app(&paths, false);
        let _ = app.update(Message::Section(Section::Settings));
        assert!(app.global_settings_open());
    }

    #[test]
    fn settings_row_waits_for_a_save_or_open_menu() {
        use crate::sidebar::Section;
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = sidebar_app(&paths, false);
        let _ = app.update(Message::Settings(Event::MenuOpened));
        let _ = app.update(sidebar_request(Section::Settings, true));
        let _ = app.update(Message::Section(Section::Settings));
        assert!(!app.global_settings_open());
        assert_eq!(app.pending_section, None);
    }

    fn target(page: &mut Page, paths: &Paths, name: &str, edit: TargetEdit) {
        let _ = page.update(paths, Event::Target(name.into(), edit));
    }

    /// Sends Add and lands the check the way the worker would.
    fn add(page: &mut Page, paths: &Paths) {
        let path = editor(page).new_target.path.clone();
        let _ = page.update(paths, Event::TargetAdd);
        assert!(page.resolving());
        let result = load::resolve_exe(paths, "game", &path);
        let sequence = page.resolve_sequence;
        let _ = page.update(paths, Event::Resolved(1, sequence, result));
    }

    const TARGETED: &str = r#"# my game
schema_version = 1
prefix = "~/prefixes/game"
runner = "missing" # pinned
arch = "win64"

# the main one
[targets.game]
exe = 'C:\Games\game.exe' # launcher
args = ["-a"] # flags
default = true # starts first

# a helper
[targets.tool]
exe = "/usr/bin/tool" # tool
cwd = "/tmp" # scratch
name = "Tool" # entry
desktop = true # shown

# going away
[targets.old]
exe = "/usr/bin/old"
"#;

    #[test]
    fn existing_target_paths_are_cleaned_when_edited() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        let mut page = page(&paths);
        target(
            &mut page,
            &paths,
            "tool",
            TargetEdit::Exe(" '/opt/My App/app.exe' ".into()),
        );
        target(
            &mut page,
            &paths,
            "tool",
            TargetEdit::Cwd(r"/opt/My\ App".into()),
        );
        let values = &editor(&page).form.draft.targets["tool"];
        assert_eq!(values.exe, "/opt/My App/app.exe");
        assert_eq!(values.cwd, "/opt/My App");
        let edits = editor(&page).form.edits();
        assert_eq!(
            edits.pairs,
            [
                ("targets.tool.exe".into(), "/opt/My App/app.exe".into()),
                ("targets.tool.cwd".into(), "/opt/My App".into()),
            ]
        );
        let _ = page.update(&paths, Event::Revert);
        assert_eq!(
            editor(&page).form.draft.targets["tool"].exe,
            "/usr/bin/tool"
        );
        assert!(!page.is_dirty());
    }

    #[test]
    fn saving_target_edits_changes_only_those_lines() {
        let (root, paths) = setup();
        write(&paths, TARGETED);
        let extra = root.path().join("Apps/extra.exe");
        fs::create_dir_all(extra.parent().unwrap()).unwrap();
        fs::write(&extra, "").unwrap();
        let mut page = page(&paths);
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::ArgsInput("-b".into()),
        );
        target(&mut page, &paths, "game", TargetEdit::ArgsAdd);
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::Cwd(r"C:\Games".into()),
        );
        target(&mut page, &paths, "game", TargetEdit::Name("Game".into()));
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::Switch(Switch::Gamemode, false),
        );
        target(&mut page, &paths, "game", TargetEdit::Gamescope(false));
        target(
            &mut page,
            &paths,
            "tool",
            TargetEdit::Exe("/usr/bin/tool2".into()),
        );
        target(&mut page, &paths, "tool", TargetEdit::Desktop(false));
        target(&mut page, &paths, "tool", TargetEdit::Default(true));
        target(&mut page, &paths, "old", TargetEdit::AskRemove);
        target(&mut page, &paths, "old", TargetEdit::Remove);
        let _ = page.update(&paths, Event::AddOpen(true));
        let _ = page.update(
            &paths,
            Event::NewTargetPath(format!("\"{}\"", extra.display())),
        );
        add(&mut page, &paths);
        let shown = editor(&page);
        assert_eq!(shown.add_error, None);
        assert!(!shown.add_open && shown.open_cards.contains("extra"));
        assert_eq!(page.changes(), 10);
        assert!(page.can_save(), "{:?}", page.problem());
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert_eq!(
            text,
            r#"# my game
schema_version = 1
prefix = "~/prefixes/game"
runner = "missing" # pinned
arch = "win64"

# the main one
[targets.game]
exe = 'C:\Games\game.exe' # launcher
args = ["-a", "-b"] # flags
cwd = 'C:\Games'
name = "Game"
gamemode = false
gamescope = false

# a helper
[targets.tool]
exe = "/usr/bin/tool2" # tool
cwd = "/tmp" # scratch
name = "Tool" # entry
default = true

[targets.extra]
exe = "~/Apps/extra.exe"
name = "extra"
desktop = true
"#
        );
        saved(&mut page, &paths, &[], None);
        land(&mut page, &paths);
        assert!(!page.is_dirty(), "{:?}", editor(&page).form.edits());
        assert!(editor(&page).open_cards.contains("extra"));
    }

    #[test]
    fn removing_a_target_asks_first_and_shows_its_art_and_entry() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        let art = wineo_core::art::target_dir(&paths.art_dir("game"), "tool");
        fs::create_dir_all(&art).unwrap();
        fs::write(art.join("icon.png"), "").unwrap();
        let mut page = page(&paths);
        assert_eq!(
            editor(&page).info.target_art["tool"],
            "~/.local/share/wineo/art/game/tool"
        );
        assert!(!editor(&page).info.target_art.contains_key("game"));
        // Remove without asking does nothing.
        target(&mut page, &paths, "tool", TargetEdit::Remove);
        assert!(!page.is_dirty());
        target(&mut page, &paths, "tool", TargetEdit::AskRemove);
        let _ = page.update(&paths, Event::KeepTarget);
        target(&mut page, &paths, "tool", TargetEdit::Remove);
        assert!(!page.is_dirty());
        target(&mut page, &paths, "tool", TargetEdit::AskRemove);
        assert_eq!(editor(&page).removing.as_deref(), Some("tool"));
        target(&mut page, &paths, "tool", TargetEdit::Remove);
        assert_eq!(editor(&page).form.edits().unset, ["targets.tool"]);
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        // Core cleans up the art of a target a save removes.
        assert!(!art.exists());
    }

    #[test]
    fn a_target_follows_the_environment_draft_live() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        write_global(&paths, "mangohud = true\n");
        let mut page = page(&paths);
        let label = |page: &Page, switch| {
            let shown = editor(page);
            shown
                .form
                .target_switch("game", switch, &shown.info.inherited)
        };
        assert_eq!(label(&page, Switch::Mangohud), (true, Source::Global));
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, false));
        assert_eq!(label(&page, Switch::Mangohud), (false, Source::Environment));
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::Switch(Switch::Mangohud, true),
        );
        assert_eq!(label(&page, Switch::Mangohud), (true, Source::Here));
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::Reset(Switch::Mangohud),
        );
        assert_eq!(label(&page, Switch::Mangohud), (false, Source::Environment));
        // Editing gamescope while inheriting copies the environment's draft.
        let _ = page.update(&paths, Event::Gamescope(true));
        let _ = page.update(&paths, Event::FieldText(Field::Width, "1280".into()));
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::FieldText(Field::Height, "720".into()),
        );
        let fields = &editor(&page).form.draft.targets["game"].fields;
        assert_eq!(
            (fields.width.as_str(), fields.height.as_str()),
            ("1280", "720")
        );
        assert_eq!(
            editor(&page).form.target_gamescope("game"),
            (true, Source::Here)
        );
        target(&mut page, &paths, "game", TargetEdit::GamescopeReset);
        target(&mut page, &paths, "tool", TargetEdit::GamescopeHere);
        let tool = &editor(&page).form.draft.targets["tool"];
        assert_eq!(tool.gamescope, form::TargetGamescope::On);
        assert_eq!(tool.fields.width, "1280");
    }

    #[test]
    fn typed_target_text_survives_a_quiet_reload_and_revert_clears_it() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::AddOpen(true));
        let _ = page.update(&paths, Event::NewTargetName("new".into()));
        let _ = page.update(&paths, Event::NewTargetPath("/bin/new".into()));
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::ArgsInput("-x".into()),
        );
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::GamescopeArgsInput("-f".into()),
        );
        let _ = page.update(&paths, Event::Card("game".into(), true));
        write(&paths, &TARGETED.replace("[\"-a\"]", "[\"-z\"]"));
        check(&mut page, &paths);
        land(&mut page, &paths);
        let shown = editor(&page);
        assert_eq!(shown.form.draft.targets["game"].args, ["-z"]);
        assert_eq!(
            (
                shown.new_target.name.as_str(),
                shown.new_target.path.as_str()
            ),
            ("new", "/bin/new")
        );
        assert_eq!(shown.target_args_input("game"), "-x");
        assert_eq!(shown.target_gamescope_args_input("game"), "-f");
        assert!(shown.add_open && shown.open_cards.contains("game"));
        target(&mut page, &paths, "game", TargetEdit::ArgsAdd);
        assert_eq!(editor(&page).form.draft.targets["game"].args, ["-z", "-x"]);
        let _ = page.update(&paths, Event::Revert);
        let shown = editor(&page);
        assert_eq!(shown.new_target, NewTarget::default());
        assert_eq!(shown.target_gamescope_args_input("game"), "");
        assert!(!page.is_dirty());
    }

    #[test]
    fn adding_a_target_checks_the_path_and_the_name() {
        let (root, paths) = setup();
        write(&paths, TARGETED);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::NewTargetName("tool".into()));
        let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
        assert!(!editor(&page).can_add_target());
        let _ = page.update(&paths, Event::TargetAdd);
        assert!(!page.resolving());
        let _ = page.update(&paths, Event::NewTargetName("sh".into()));
        for (typed, error) in [
            ("bin/sh", "The path must be absolute or start with ~/."),
            ("/nowhere/x.exe", "`/nowhere/x.exe` does not exist."),
            (r"C:\x.exe", r"`C:\x.exe` does not exist."),
        ] {
            let _ = page.update(&paths, Event::NewTargetPath(typed.into()));
            add(&mut page, &paths);
            assert_eq!(editor(&page).add_error.as_deref(), Some(error), "{typed}");
        }
        assert!(!page.is_dirty());
        // A check that lands after the form changed is dropped, and so is
        // one that isn't the latest.
        let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
        let _ = page.update(&paths, Event::TargetAdd);
        let _ = page.update(&paths, Event::NewTargetName("shell".into()));
        let sequence = page.resolve_sequence;
        let _ = page.update(&paths, Event::Resolved(1, sequence, Ok("/bin/sh".into())));
        assert!(!page.is_dirty() && !page.resolving());
        let _ = page.update(&paths, Event::TargetAdd);
        let _ = page.update(&paths, Event::Resolved(1, sequence, Ok("/bin/sh".into())));
        assert!(!page.is_dirty() && page.resolving());
        // One that lands while a save is locking the page is dropped too.
        let sequence = page.resolve_sequence;
        page.saving = true;
        let _ = page.update(&paths, Event::Resolved(1, sequence, Ok("/bin/sh".into())));
        page.saving = false;
        assert!(!page.is_dirty() && !page.resolving());
        // An empty name takes one suggested from the exe.
        let exe = root.path().join("Apps/My Tool.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "").unwrap();
        let _ = page.update(&paths, Event::NewTargetName(String::new()));
        let _ = page.update(
            &paths,
            Event::NewTargetPath(exe.display().to_string().replace(' ', "\\ ")),
        );
        add(&mut page, &paths);
        let added = &editor(&page).form.draft.targets["my-tool"];
        assert_eq!(added.exe, "~/Apps/My Tool.exe");
        assert!(added.desktop);
        assert_eq!(added.name, "My Tool");
    }

    #[test]
    fn add_target_notes_an_exe_already_used_in_the_draft() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        let prefix = fake_prefix(&paths);
        let game = prefix.join("drive_c/Games/game.exe");
        fs::write(&game, "").unwrap();
        let mut page = page(&paths);
        let _ = page.update(
            &paths,
            Event::ProgramPicked(ProgramChoice {
                name: "Game Again".into(),
                exe: r"c:\games\GAME.exe".into(),
            }),
        );
        assert_eq!(editor(&page).duplicate_target(), Some("game"));
        assert!(editor(&page).can_add_target());
        let _ = page.update(&paths, Event::NewTargetPath(game.display().to_string()));
        assert_eq!(editor(&page).duplicate_target(), Some("game"));
        target(&mut page, &paths, "tool", TargetEdit::Exe("/bin/sh".into()));
        let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
        assert_eq!(editor(&page).duplicate_target(), Some("tool"));
        assert!(editor(&page).can_add_target());
    }

    fn fake_prefix(paths: &Paths) -> std::path::PathBuf {
        let prefix = paths.expand("~/prefixes/game");
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        fs::write(prefix.join("system.reg"), "").unwrap();
        std::os::unix::fs::symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
        for exe in [
            "drive_c/Games/Hades/Hades.exe",
            "drive_c/Games/Hades/unins000.exe",
        ] {
            let path = prefix.join(exe);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        prefix
    }

    #[test]
    fn installed_programs_are_offered_once_the_prefix_exists() {
        let (_root, paths) = setup();
        write(&paths, TARGETED);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::AddOpen(true));
        let sequence = page.scan_sequence;
        assert_eq!(editor(&page).scan, Some(Scan::Scanning));
        let _ = page.update(
            &paths,
            Event::Scanned(1, sequence, load::scan(&paths, "game")),
        );
        assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
        let prefix = fake_prefix(&paths);
        let shortcut =
            prefix.join("drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs/Broken.lnk");
        fs::create_dir_all(shortcut.parent().unwrap()).unwrap();
        fs::write(shortcut, "not a shortcut").unwrap();
        let _ = page.update(
            &paths,
            Event::Scanned(1, sequence, load::scan(&paths, "game")),
        );
        let current = editor(&page).scan.clone();
        let _ = page.update(
            &paths,
            Event::Scanned(7, sequence, Scan::Failed("stale".into())),
        );
        assert_eq!(editor(&page).scan, current);
        let Some(Scan::Programs {
            programs,
            held_back,
            unreadable,
        }) = editor(&page).scan.clone()
        else {
            panic!("{:?}", editor(&page).scan);
        };
        assert_eq!(
            programs,
            [ProgramChoice {
                name: "Hades".into(),
                exe: r"C:\Games\Hades\Hades.exe".into(),
            }]
        );
        assert_eq!(held_back.len(), 1);
        assert_eq!(unreadable, ["Skipped the shortcut Broken: not a shortcut"]);
        let _ = page.update(&paths, Event::ProgramPicked(programs[0].clone()));
        assert_eq!(
            editor(&page).new_target,
            NewTarget {
                name: "hades".into(),
                path: r"C:\Games\Hades\Hades.exe".into(),
                label: Some("Hades".into()),
            }
        );
        add(&mut page, &paths);
        assert_eq!(
            editor(&page).form.draft.targets["hades"].exe,
            r"C:\Games\Hades\Hades.exe"
        );
        // A Unix path inside drive_c is stored as its C: path.
        let _ = page.update(&paths, Event::NewTargetName("uninstall".into()));
        let unins = prefix.join("drive_c/Games/Hades/unins000.exe");
        let _ = page.update(&paths, Event::NewTargetPath(unins.display().to_string()));
        add(&mut page, &paths);
        let added = &editor(&page).form.draft.targets["uninstall"];
        assert_eq!(added.exe, r"C:\Games\Hades\unins000.exe");
        assert_eq!(added.name, "unins000");
    }

    #[test]
    fn a_native_page_adds_targets_by_path_only() {
        let (root, paths) = setup();
        let bin = root.path().join("bin");
        executable(&bin.join("dusk"));
        write(
            &paths,
            &format!(
                "schema_version = 1\nkind = \"native\"\n[env]\nPATH = \"{}\"\n[targets.game]\nexe = \"dusk\"\n",
                bin.display()
            ),
        );
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::AddOpen(true));
        assert_eq!(editor(&page).scan, None);
        let _ = page.update(
            &paths,
            Event::ProgramPicked(ProgramChoice {
                name: "X".into(),
                exe: r"C:\x.exe".into(),
            }),
        );
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::Switch(Switch::Wayland, true),
        );
        target(
            &mut page,
            &paths,
            "game",
            TargetEdit::Reset(Switch::Wayland),
        );
        assert!(!page.is_dirty());
        assert_eq!(editor(&page).new_target, NewTarget::default());
        let _ = page.update(&paths, Event::NewTargetName("win".into()));
        let _ = page.update(&paths, Event::NewTargetPath(r"C:\x.exe".into()));
        add(&mut page, &paths);
        assert!(
            editor(&page)
                .add_error
                .as_deref()
                .is_some_and(|error| error.contains("native")),
            "{:?}",
            editor(&page).add_error
        );
        // A bare name is looked up on the environment's PATH.
        let _ = page.update(&paths, Event::NewTargetName("dusk".into()));
        let _ = page.update(&paths, Event::NewTargetPath("dusk".into()));
        add(&mut page, &paths);
        assert_eq!(editor(&page).form.draft.targets["dusk"].exe, "dusk");
        target(&mut page, &paths, "game", TargetEdit::Default(true));
        let applied = apply(&page, &paths);
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(text.contains("[targets.dusk]\nexe = \"dusk\""), "{text}");
    }

    #[test]
    fn a_portable_page_refuses_desktop_entries() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let portable = root.path().join("portable");
        fs::create_dir_all(&portable).unwrap();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&portable),
        )
        .unwrap();
        write(
            &paths,
            "schema_version = 1\nprefix = \"prefixes/game\"\nrunner = \"missing\"\narch = \"win64\"\n[targets.game]\nexe = \"/bin/sh\"\ndesktop = true\n[targets.tool]\nexe = \"/bin/true\"\ndefault = true\n",
        );
        let mut page = page(&paths);
        let shown = editor(&page);
        assert_eq!(
            shown.desktop_refusal().as_deref(),
            Some("desktop entries aren't supported for portable environments")
        );
        target(&mut page, &paths, "tool", TargetEdit::Desktop(true));
        assert!(!page.is_dirty());
        // A hand-written one can still be turned off.
        target(&mut page, &paths, "game", TargetEdit::Desktop(false));
        assert_eq!(editor(&page).form.edits().unset, ["targets.game.desktop"]);
        // New targets get no entry, and no name to title one.
        let _ = page.update(&paths, Event::NewTargetName("sh".into()));
        let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
        add(&mut page, &paths);
        let added = &editor(&page).form.draft.targets["sh"];
        assert!(!added.desktop && added.name.is_empty());
        let relative = portable.join("apps/other.exe");
        fs::create_dir_all(relative.parent().unwrap()).unwrap();
        fs::write(relative, "").unwrap();
        let _ = page.update(&paths, Event::NewTargetName("other".into()));
        let _ = page.update(&paths, Event::NewTargetPath("apps/other.exe".into()));
        add(&mut page, &paths);
        assert_eq!(editor(&page).add_error, None);
        assert_eq!(
            editor(&page).form.draft.targets["other"].exe,
            "apps/other.exe"
        );
    }
}
