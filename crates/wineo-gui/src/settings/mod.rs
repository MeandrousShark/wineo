//! The environment settings page, which replaces the library while open.

mod form;
mod load;
mod view;

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::components::Component;
use wineo_core::paths::Paths;

use std::collections::{BTreeMap, BTreeSet};

use crate::{App, Message};
use form::{Build, Choice, EnvRow, Field, Form, List, Switch, TargetValues, Values};
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
    EnvKey(usize, String),
    EnvValue(usize, String),
    EnvRemove(usize),
    NewEnvKey(String),
    NewEnvValue(String),
    EnvAdd,
    Prefix(String),
    GameId(String),
    Store(String),
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
    Scanned(u64, Scan),
    ProgramPicked(ProgramChoice),
    NewTargetName(String),
    NewTargetPath(String),
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
                | Event::EnvKey(..)
                | Event::EnvValue(..)
                | Event::EnvRemove(_)
                | Event::NewEnvKey(_)
                | Event::NewEnvValue(_)
                | Event::EnvAdd
                | Event::Prefix(_)
                | Event::GameId(_)
                | Event::Store(_)
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
    /// Follows a save, so the controls stay locked until it lands.
    after_save: bool,
}

pub(crate) struct Page {
    pub(crate) name: String,
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
    pub(crate) changed_on_disk: bool,
    pub(crate) leave_prompt: bool,
    /// A pick list's menu is open, so Escape belongs to it.
    menu_open: bool,
    /// What the last save did.
    pub(crate) notes: Vec<String>,
    pub(crate) error: Option<String>,
    /// Bumped by every typed exe check, so only the latest one lands.
    resolve_sequence: u64,
    /// The add target form as it was sent to be checked.
    resolving: Option<NewTarget>,
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
            generation,
            view: View::Loading,
            disk: None,
            pending: None,
            checking: false,
            check_sequence: 0,
            saving: false,
            changed_on_disk: false,
            leave_prompt: false,
            menu_open: false,
            notes: Vec::new(),
            error: None,
            resolve_sequence: 0,
            resolving: None,
        }
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
    pub(crate) fn is_dirty(&self) -> bool {
        self.editor().is_some_and(|editor| editor.form.is_dirty())
    }
    pub(crate) fn changes(&self) -> usize {
        self.editor().map_or(0, |editor| editor.form.changes())
    }
    pub(crate) fn has_file(&self) -> bool {
        self.disk
            .as_ref()
            .is_some_and(|texts| texts.environment.is_some())
    }
    /// Why Save is off even though there are changes.
    pub(crate) fn problem(&self) -> Option<String> {
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
            after_save,
        });
        load::load_task(paths.clone(), self.name.clone(), self.generation)
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
        if let Some(editor) = self.editor()
            && !editor.accepts(&self.name, &event)
        {
            return Step::none();
        }
        match event {
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
                let Some(editor) = self.editor() else {
                    return Step::none();
                };
                let edits = editor.form.edits();
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
                }
                Step::none()
            }
            Event::EnvValue(index, value) => {
                if let Some(row) = self.draft().and_then(|draft| draft.env.get_mut(index)) {
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
                Step::none()
            }
            Event::NewEnvKey(key) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_env.key = key;
                }
                Step::none()
            }
            Event::NewEnvValue(value) => {
                if let Some(editor) = self.editor_mut() {
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
                Step::none()
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
                ))
            }
            Event::ShowHeldBack(show) => {
                if let Some(editor) = self.editor_mut() {
                    editor.show_held_back = show;
                }
                Step::none()
            }
            Event::Scanned(generation, scan) => {
                if generation == self.generation
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
            Event::OpenConfig => Step::task(load::open_task(paths.environment_config(&self.name))),
            Event::Opened(result) => {
                self.error = result.err();
                Step::none()
            }
            Event::Loaded(generation, snapshot) => {
                if generation != self.generation {
                    return Step::none();
                }
                if let Some(pending) = self.pending.take() {
                    self.land(pending, snapshot);
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
                if disk.environment != texts.environment {
                    if self.is_dirty() {
                        self.changed_on_disk = true;
                        Step::none()
                    } else {
                        Step::task(self.reload(paths, Mode::Replace))
                    }
                } else {
                    // Only the global file changed, which nothing on the page
                    // edits, so the inherited values can move under the edits.
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
            TargetEdit::Exe(exe) => values.exe = exe,
            TargetEdit::Cwd(cwd) => values.cwd = cwd,
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
        let edited_since = pending.mode == Mode::Replace && draft != pending.draft.as_ref();
        match (pending.mode, snapshot.content) {
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
    pub(crate) fn open_settings(&mut self) -> Task<Message> {
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        self.settings_generation = self.settings_generation.wrapping_add(1);
        let mut page = Page::new(name, self.settings_generation);
        let task = page.reload(&self.paths, Mode::Replace);
        self.settings = Some(page);
        task
    }
    pub(crate) fn settings_update(&mut self, event: Event) -> Task<Message> {
        let Some(page) = &mut self.settings else {
            return Task::none();
        };
        let step = page.update(&self.paths, event);
        if step.close {
            self.settings = None;
        }
        step.task
    }
    pub(crate) fn settings_tick(&mut self) -> Task<Message> {
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
        let (prompt, menu_open) = (page.leave_prompt, page.menu_open);
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
                } else {
                    self.settings_update(Event::Back)
                }
            }
            _ => Task::none(),
        }
    }
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
                proton: Some(true),
            }),
        );
        let _ = page.update(&paths, Event::Store("gog".into()));
        let _ = page.update(
            &paths,
            Event::RunnerPicked(RunnerChoice {
                name: "wine-tkg".into(),
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
        assert_eq!(editor(&page).scan, Some(Scan::Scanning));
        let _ = page.update(&paths, Event::Scanned(1, load::scan(&paths, "game")));
        assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
        let prefix = fake_prefix(&paths);
        let _ = page.update(&paths, Event::Scanned(7, Scan::NoPrefix));
        let _ = page.update(&paths, Event::Scanned(1, load::scan(&paths, "game")));
        let Some(Scan::Programs {
            programs,
            held_back,
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
    }
}
