//! The environment settings page, which replaces the library while open.

mod form;
mod load;
mod view;

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::paths::Paths;

use crate::{App, Message};
use form::{Choice, Field, Form, Switch, Values};
use load::{Content, Current, Loaded, Report, RunnerChoice, Snapshot, Texts};

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
        )
    }
}

pub(crate) struct Editor {
    pub(crate) info: Loaded,
    pub(crate) form: Form,
    /// Why the last runner picked was refused.
    pub(crate) refusal: Option<String>,
    /// An extra gamescope arg typed but not added yet.
    pub(crate) args_input: String,
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
        }
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
    pub(crate) fn can_save(&self) -> bool {
        !self.saving && !self.loading() && !self.changed_on_disk && self.is_dirty()
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
                Step::task(self.reload(paths, Mode::Replace))
            }
            Event::Revert => {
                if self.locked() {
                    return Step::none();
                }
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.form.revert();
                    editor.refusal = None;
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
                    Content::Form(info) => View::Form(Box::new(Editor {
                        form: Form::reloaded(info.values.clone(), previous),
                        info: *info,
                        refusal: None,
                        args_input: String::new(),
                    })),
                    Content::Error(error) => View::Error(error),
                };
            }
        }
    }
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
        write(&paths, GAME);
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
            Event::Revert,
        ];
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
        assert!(editor(&page).args_input.is_empty());
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
        let root = GlobalConfig::default().runner_root(&paths).unwrap();
        for program in ["wine", "wineserver"] {
            executable(&root.join("wine-tkg/bin").join(program));
            executable(&root.join("GE-Proton9-20/files/bin").join(program));
        }
        executable(&root.join("GE-Proton9-20/proton"));
        fs::write(root.join("GE-Proton9-20/version"), "1 GE-Proton9-20\n").unwrap();
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
}
