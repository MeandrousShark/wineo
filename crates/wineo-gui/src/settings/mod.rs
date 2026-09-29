//! The environment settings page, which replaces the library while open.

mod form;
mod load;
mod view;

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::paths::Paths;

use crate::{App, Message};
use form::{Choice, Field, Form, GamescopeState, Switch};
use load::{Content, Current, Loaded, Report, RunnerChoice, Snapshot};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Discard,
    KeepEditing,
    /// Save, or Reload once the file changed under unsaved edits.
    Save,
    Revert,
    RunnerPicked(RunnerChoice),
    Switch(Switch, bool),
    Reset(Switch),
    Gamescope(bool),
    ResetGamescope,
    FieldText(Field, String),
    FieldChoice(Field, Choice),
    GrabCursor(bool),
    OpenConfig,
    Opened(Result<(), String>),
    Loaded(u64, Snapshot),
    Checked(u64, Option<String>),
    CheckFailed(u64),
    Saved(u64, Result<Report, String>),
}

pub(crate) struct Editor {
    pub(crate) info: Loaded,
    pub(crate) form: Form,
    /// Why the last runner picked was refused.
    pub(crate) refusal: Option<String>,
}

pub(crate) enum View {
    Loading,
    Form(Box<Editor>),
    Error(String),
}

pub(crate) struct Page {
    pub(crate) name: String,
    /// Tells this page's worker results from those of a page closed earlier.
    generation: u64,
    pub(crate) view: View,
    /// The file text the shown state was read from, `None` before the first
    /// load, and `Some(None)` when there was no file.
    disk: Option<Option<String>>,
    loading: bool,
    /// The load in flight is a background reload, which must not replace
    /// edits made while it ran.
    quiet: bool,
    checking: bool,
    pub(crate) saving: bool,
    pub(crate) changed_on_disk: bool,
    pub(crate) leave_prompt: bool,
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

impl Page {
    fn new(name: String, generation: u64) -> Self {
        Self {
            name,
            generation,
            view: View::Loading,
            disk: None,
            loading: false,
            quiet: false,
            checking: false,
            saving: false,
            changed_on_disk: false,
            leave_prompt: false,
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
    pub(crate) fn is_dirty(&self) -> bool {
        self.editor().is_some_and(|editor| editor.form.is_dirty())
    }
    pub(crate) fn changes(&self) -> usize {
        self.editor().map_or(0, |editor| editor.form.changes())
    }
    pub(crate) fn has_file(&self) -> bool {
        matches!(self.disk, Some(Some(_)))
    }
    pub(crate) fn can_save(&self) -> bool {
        !self.saving && !self.loading && (self.changed_on_disk || self.is_dirty())
    }
    fn reload(&mut self, paths: &Paths, quiet: bool) -> Task<Message> {
        // A reload asked for outright takes over one already running quietly.
        self.quiet = quiet && (self.quiet || !self.loading);
        if self.loading {
            return Task::none();
        }
        self.loading = true;
        load::load_task(paths.clone(), self.name.clone(), self.generation)
    }
    /// A stale save report would describe edits that are no longer there.
    fn edited(&mut self) {
        self.notes.clear();
        self.error = None;
    }
    fn draft(&mut self) -> Option<&mut form::Values> {
        self.editor_mut().map(|editor| &mut editor.form.draft)
    }
    fn tick(&mut self, paths: &Paths) -> Task<Message> {
        if self.loading || self.saving || self.checking || self.disk.is_none() {
            return Task::none();
        }
        self.checking = true;
        load::check_task(paths.environment_config(&self.name), self.generation)
    }
    fn update(&mut self, paths: &Paths, event: Event) -> Step {
        match event {
            Event::Back => {
                if self.saving {
                    Step::none()
                } else if self.is_dirty() {
                    self.leave_prompt = true;
                    Step::none()
                } else {
                    Step::close()
                }
            }
            Event::Discard => Step::close(),
            Event::KeepEditing => {
                self.leave_prompt = false;
                Step::none()
            }
            Event::Save => {
                if !self.can_save() {
                    return Step::none();
                }
                if self.changed_on_disk {
                    self.edited();
                    return Step::task(self.reload(paths, false));
                }
                let Some(editor) = self.editor() else {
                    return Step::none();
                };
                let edits = editor.form.edits();
                if edits.is_empty() {
                    return Step::none();
                }
                self.saving = true;
                self.edited();
                Step::task(load::save_task(
                    paths.clone(),
                    self.name.clone(),
                    edits,
                    self.generation,
                ))
            }
            Event::Revert => {
                if self.saving {
                    return Step::none();
                }
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.form.revert();
                    editor.refusal = None;
                }
                if self.changed_on_disk {
                    return Step::task(self.reload(paths, false));
                }
                Step::none()
            }
            Event::RunnerPicked(choice) => {
                let name = self.name.clone();
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    match refusal(&name, editor.info.current.as_ref(), &choice) {
                        Some(reason) => editor.refusal = Some(reason),
                        None => {
                            editor.refusal = None;
                            editor.form.draft.runner = Some(choice.name);
                        }
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
                    draft.gamescope = if on {
                        GamescopeState::On
                    } else {
                        GamescopeState::Off
                    };
                }
                Step::none()
            }
            Event::ResetGamescope => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.gamescope = GamescopeState::Unset;
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
            Event::OpenConfig => Step::task(load::open_task(paths.environment_config(&self.name))),
            Event::Opened(result) => {
                self.error = result.err();
                Step::none()
            }
            Event::Loaded(generation, snapshot) => {
                if generation != self.generation {
                    return Step::none();
                }
                self.loading = false;
                let quiet = std::mem::take(&mut self.quiet);
                if quiet && self.is_dirty() {
                    self.changed_on_disk = true;
                    return Step::none();
                }
                self.apply(snapshot);
                Step::none()
            }
            Event::Checked(generation, text) => {
                if generation != self.generation {
                    return Step::none();
                }
                self.checking = false;
                if self.loading || self.saving || self.disk.as_ref() == Some(&text) {
                    return Step::none();
                }
                if self.is_dirty() {
                    self.changed_on_disk = true;
                    Step::none()
                } else {
                    Step::task(self.reload(paths, true))
                }
            }
            Event::CheckFailed(generation) => {
                if generation == self.generation {
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
                            // Components may have changed, but the file
                            // didn't, so the edits stay for another try.
                            Some(error) => {
                                self.error = Some(error);
                                Step::none()
                            }
                            None => Step::task(self.reload(paths, false)),
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
    fn apply(&mut self, snapshot: Snapshot) {
        self.disk = Some(snapshot.text);
        self.changed_on_disk = false;
        self.view = match snapshot.content {
            Content::Form(info) => View::Form(Box::new(Editor {
                form: Form::new(info.values.clone()),
                info: *info,
                refusal: None,
            })),
            Content::Error(error) => View::Error(error),
        };
    }
}

/// Wine and Proton prefixes are laid out differently, so `settings::apply`
/// refuses a switch between them. Refusing it here says so before Save.
fn refusal(name: &str, current: Option<&Current>, choice: &RunnerChoice) -> Option<String> {
    let current = current?;
    let proton = choice.proton?;
    (current.proton != proton).then(|| {
        load::kind_mismatch(
            name,
            &current.name,
            &choice.name,
            if proton { "proton" } else { "wine" },
        )
    })
}

impl App {
    pub(crate) fn open_settings(&mut self) -> Task<Message> {
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        self.settings_generation = self.settings_generation.wrapping_add(1);
        let mut page = Page::new(name, self.settings_generation);
        let task = page.reload(&self.paths, false);
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
        match key.as_ref() {
            Key::Character(character)
                if modifiers.control() && character.eq_ignore_ascii_case("s") =>
            {
                self.settings_update(Event::Save)
            }
            // Only text inputs take focus in iced, so Tab moves between those.
            Key::Named(Named::Tab) if modifiers.shift() => {
                iced::widget::operation::focus_previous()
            }
            Key::Named(Named::Tab) => iced::widget::operation::focus_next(),
            // A focused field uses Escape to let go of focus first.
            Key::Named(Named::Escape) if !captured => {
                if self.settings.as_ref().is_some_and(|page| page.leave_prompt) {
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

    fn page(paths: &Paths) -> Page {
        let mut page = Page::new("game".into(), 1);
        page.loading = true;
        let snapshot = load::snapshot(paths, "game");
        let _ = page.update(paths, Event::Loaded(1, snapshot));
        page
    }

    fn editor(page: &Page) -> &Editor {
        page.editor().expect("form")
    }

    #[test]
    fn loads_file_values_and_tracks_changes() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let draft = &editor(&page).form.draft;
        assert_eq!(draft.gamemode, Some(false));
        assert_eq!(draft.gamescope, GamescopeState::On);
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
    fn a_file_change_reloads_quietly_without_edits() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let changed = GAME.replace("gamemode = false", "gamemode = true");
        write(&paths, &changed);
        let _ = page.update(&paths, Event::Checked(1, Some(changed.clone())));
        assert!(page.loading && page.quiet);
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot(&paths, "game")));
        assert!(!page.changed_on_disk);
        assert_eq!(editor(&page).form.draft.gamemode, Some(true));
        // The same text again is no change.
        let _ = page.update(&paths, Event::Checked(1, Some(changed)));
        assert!(!page.loading);
    }

    #[test]
    fn a_file_change_under_edits_turns_save_into_reload() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let changed = GAME.replace("gamemode = false", "gamemode = true");
        write(&paths, &changed);
        let _ = page.update(&paths, Event::Checked(1, Some(changed)));
        assert!(page.changed_on_disk);
        assert!(!page.loading);
        assert_eq!(editor(&page).form.draft.mangohud, Some(true));
        let _ = page.update(&paths, Event::Save);
        assert!(page.loading && !page.saving && !page.quiet);
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot(&paths, "game")));
        assert!(!page.changed_on_disk);
        assert!(!page.is_dirty());
        assert_eq!(editor(&page).form.draft.gamemode, Some(true));
    }

    #[test]
    fn edits_made_during_a_quiet_reload_are_kept() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let changed = GAME.replace("gamemode = false", "gamemode = true");
        write(&paths, &changed);
        let _ = page.update(&paths, Event::Checked(1, Some(changed)));
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        let _ = page.update(&paths, Event::Loaded(1, load::snapshot(&paths, "game")));
        assert!(page.changed_on_disk);
        assert_eq!(editor(&page).form.draft.mangohud, Some(true));
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
    fn a_failed_save_keeps_edits_and_shows_the_error() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
        page.saving = true;
        let _ = page.update(&paths, Event::Saved(1, Err("Environment is busy".into())));
        assert!(!page.saving);
        assert_eq!(page.error.as_deref(), Some("Environment is busy"));
        assert!(page.is_dirty());
        page.saving = true;
        let report = Report {
            lines: vec!["Installed DXVK 2.4".into()],
            failed: Some("bad".into()),
        };
        let _ = page.update(&paths, Event::Saved(1, Ok(report)));
        assert_eq!(page.notes, ["Installed DXVK 2.4"]);
        assert_eq!(page.error.as_deref(), Some("bad"));
        assert!(page.is_dirty() && !page.loading);
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
            Some(
                "`game` runs on wine-tkg and `GE-Proton9-20` is a proton runner. Their prefix layouts are not compatible, so make a new environment with `wineo new --runner GE-Proton9-20` instead"
            )
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
        assert_eq!(
            refusal(
                "game",
                page.editor().unwrap().info.current.as_ref(),
                &unknown
            ),
            None
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
    fn saving_one_control_changes_only_its_line() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut page = page(&paths);
        let _ = page.update(&paths, Event::Switch(Switch::Gamemode, true));
        let _ = page.update(&paths, Event::FieldText(Field::Width, "2560".into()));
        let edits = editor(&page).form.edits();
        let pairs: Vec<(&str, &str)> = edits
            .pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let unset: Vec<&str> = edits.unset.iter().map(String::as_str).collect();
        let applied =
            settings::apply(&paths, ConfigFile::Environment("game"), &pairs, &unset).unwrap();
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
        let text = "schema_version = 1\nprefix = \"~/p\"\nrunner = \"missing\"\narch = \"win64\"\n";
        write(&paths, text);
        let mut edited = page(&paths);
        let _ = edited.update(&paths, Event::Gamescope(true));
        let edits = editor(&edited).form.edits();
        let pairs: Vec<(&str, &str)> = edits
            .pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let unset: Vec<&str> = edits.unset.iter().map(String::as_str).collect();
        let applied =
            settings::apply(&paths, ConfigFile::Environment("game"), &pairs, &unset).unwrap();
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        assert!(applied.not_set.is_empty());
        let reloaded = page(&paths);
        assert_eq!(editor(&reloaded).form.draft.gamescope, GamescopeState::On);
        assert_eq!(
            editor(&reloaded).form.draft.fields,
            form::GamescopeFields::default()
        );
    }

    #[test]
    fn switching_gamescope_off_replaces_the_table() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut edited = page(&paths);
        let _ = edited.update(&paths, Event::Gamescope(false));
        let edits = editor(&edited).form.edits();
        assert_eq!(edits.pairs, [("gamescope".to_owned(), "false".to_owned())]);
        let applied = settings::apply(
            &paths,
            ConfigFile::Environment("game"),
            &[("gamescope", "false")],
            &[],
        )
        .unwrap();
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        let reloaded = page(&paths);
        assert_eq!(editor(&reloaded).form.draft.gamescope, GamescopeState::Off);
        let text = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(text.starts_with("# my game\n"), "{text}");
    }

    #[test]
    fn escape_asks_before_leaving_and_again_keeps_editing() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = App::boot(paths.clone()).0;
        app.selected = Some("game".into());
        let _ = app.update(Message::OpenSettings);
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot(&paths, "game"),
        )));
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
    fn leaving_a_clean_page_returns_to_the_library() {
        let (_root, paths) = setup();
        write(&paths, GAME);
        let mut app = App::boot(paths.clone()).0;
        app.selected = Some("game".into());
        let _ = app.update(Message::OpenSettings);
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot(&paths, "game"),
        )));
        let _ = app.update(Message::Settings(Event::Back));
        assert!(app.settings.is_none());
        assert_eq!(app.selected.as_deref(), Some("game"));
    }
}
