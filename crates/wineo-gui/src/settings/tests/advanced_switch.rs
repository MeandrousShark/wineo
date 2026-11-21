use iced::advanced::widget::operation::focusable;
use iced::keyboard::{Modifiers, key::Named};
use iced::widget::Id;

use super::*;
use crate::layout_probe::{lay_out, relay_out};
use crate::sort_control::{self, Field as ViewField};
use crate::theme::Palette;
use view::{ADVANCED_NOTE, ADVANCED_SWITCH, ADVANCED_SWITCH_ID, PREFIX_ID};

const SIZE: iced::Size = iced::Size::new(900.0, 3000.0);

/// What only the Advanced tab's sections show.
const SECTIONS: [&str; 5] = [
    "New environments",
    "Prefix root",
    "Runs",
    "Stop timeout (seconds)",
    "Environment variables",
];

const BROKEN_LIBRARY: &str = "[view\nbroken";

fn page_of(app: &App) -> &Page {
    app.settings.as_ref().unwrap()
}

fn texts(app: &App, paths: &Paths) -> Vec<String> {
    page_texts(page_of(app), paths)
}

fn shows(app: &App, paths: &Paths, label: &str) -> bool {
    texts(app, paths).iter().any(|text| text == label)
}

fn send(app: &mut App, event: Event) -> Task<Message> {
    app.update(Message::Settings(event))
}

fn open_advanced(paths: &Paths) -> App {
    let mut app = open_global_app(paths);
    let _ = send(&mut app, Event::Tab(Tab::Advanced));
    app
}

fn flip(app: &mut App, on: bool) {
    let _ = send(app, Event::AdvancedSettings(on));
}

/// Runs the switch's write in flight the way its task would, and lands it.
fn finish_write(app: &mut App) {
    let on = app.view_writes.advanced_settings.expect("a switch write");
    let written = ViewField::AdvancedSettings(on);
    let result = sort_control::save(&app.paths, &app.shown, written);
    let _ = app.update(Message::Sort(sort_control::Event::Saved(written, result)));
}

fn remembered(paths: &Paths) -> bool {
    wineo_core::library::load(paths).view.advanced_settings
}

fn write_library(paths: &Paths, text: &str) {
    let path = paths.library_file();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Ids after the tab bar, in order.
fn page_ids(app: &App, paths: &Paths) -> Vec<Option<String>> {
    let mut laid = lay_out(page_of(app).view(paths, Palette::DARK), SIZE);
    crate::focus_button::focusables(&mut laid)[Tab::ALL.len()..].to_vec()
}

fn id(name: &str) -> Option<String> {
    Some(format!("{:?}", Id::from(name.to_owned())))
}

/// What a press and release on the text `label` sends.
fn click_label(app: &App, paths: &Paths, label: &str) -> Vec<Message> {
    use iced::advanced::widget::Operation;
    use std::sync::{Arc, Mutex};

    struct Find(String, Arc<Mutex<Option<iced::Rectangle>>>);
    impl Operation for Find {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _id: Option<&Id>, bounds: iced::Rectangle, text: &str) {
            if text == self.0 {
                *self.1.lock().unwrap() = Some(bounds);
            }
        }
    }
    let found = Arc::new(Mutex::new(None));
    let mut laid = lay_out(page_of(app).view(paths, Palette::DARK), SIZE);
    laid.run(Box::new(Find(label.to_owned(), found.clone())));
    let bounds = found.lock().unwrap().expect(label);
    let cursor = iced::mouse::Cursor::Available(bounds.center());
    let left = iced::mouse::Button::Left;
    let mut messages = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
        cursor,
    );
    messages.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
        cursor,
    ));
    messages
}

#[test]
fn the_switch_starts_off_with_only_itself_on_advanced() {
    let (_root, paths) = setup();
    let app = open_advanced(&paths);
    assert!(!app.advanced_settings);
    assert!(!page_of(&app).advanced_settings);
    let shown = texts(&app, &paths);
    assert!(shown.iter().any(|text| text == ADVANCED_SWITCH));
    assert!(shown.iter().any(|text| text == ADVANCED_NOTE));
    for label in SECTIONS {
        assert!(!shown.iter().any(|text| text == label), "{label}");
    }
    assert_eq!(page_ids(&app, &paths), [id(ADVANCED_SWITCH_ID)]);
}

#[test]
fn turning_it_on_shows_the_sections_and_writes_at_once() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    assert!(app.advanced_settings);
    assert!(page_of(&app).advanced_settings);
    // Written now, outside the draft, and closing waits for it.
    assert_eq!(app.view_writes.advanced_settings, Some(true));
    assert!(app.library_writes_pending());
    assert!(!page_of(&app).is_dirty());
    assert_eq!(page_of(&app).changes(), 0);
    let shown = texts(&app, &paths);
    for label in SECTIONS {
        assert!(shown.iter().any(|text| text == label), "{label}");
    }
    assert!(!shown.iter().any(|text| text == ADVANCED_NOTE));
    finish_write(&mut app);
    assert!(!app.library_writes_pending());
    assert!(remembered(&paths));
    assert!(app.advanced_settings_error.is_none());

    // The next start reads it, for either page.
    let app = open_advanced(&paths);
    assert!(app.advanced_settings && page_of(&app).advanced_settings);
    write(&paths, GAME);
    let app = open_app(&paths);
    assert!(page_of(&app).advanced_settings);

    // Off is written too, and absent reads as off.
    let mut app = open_advanced(&paths);
    flip(&mut app, false);
    finish_write(&mut app);
    assert!(!remembered(&paths));
    assert!(!shows(&app, &paths, "Runs"));
}

#[test]
fn an_unsaved_advanced_edit_keeps_it_on_until_save_or_revert() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    finish_write(&mut app);
    // An edit on another tab doesn't hold it.
    let _ = send(&mut app, Event::GlobalSwitch(Switch::Gamemode, true));
    assert!(page_of(&app).advanced_switch_enabled());
    let _ = send(&mut app, Event::Revert);

    let edits = [
        Event::GlobalField(global::Field::PrefixRoot, "~/prefixes".into()),
        Event::GlobalArch(Arch::Win32),
        Event::GlobalField(global::Field::StopTimeout, "15".into()),
        Event::GlobalExternalRunners(true),
    ];
    for edit in edits {
        let _ = send(&mut app, edit.clone());
        assert_eq!(page_of(&app).advanced_hold(), Some(HOLD_EDITS), "{edit:?}");
        assert!(!page_of(&app).advanced_switch_enabled());
        assert!(shows(&app, &paths, HOLD_EDITS));
        flip(&mut app, false);
        assert!(app.advanced_settings && page_of(&app).advanced_settings);
        assert!(app.view_writes.advanced_settings.is_none());
        let _ = send(&mut app, Event::Revert);
        assert!(page_of(&app).advanced_switch_enabled(), "{edit:?}");
        assert!(!shows(&app, &paths, HOLD_EDITS));
    }

    let _ = send(
        &mut app,
        Event::GlobalField(global::Field::PrefixRoot, "~/prefixes".into()),
    );
    let edits = global_editor(page_of(&app)).form.edits();
    let _ = send(&mut app, Event::Save);
    let generation = page_of(&app).generation;
    let report = load::save_global(&paths, &edits).unwrap();
    let _ = send(&mut app, Event::Saved(generation, Ok(report)));
    // Locked until the save's reload lands.
    assert!(!page_of(&app).advanced_switch_enabled());
    let _ = send(
        &mut app,
        Event::Loaded(generation, load::snapshot_global(&paths)),
    );
    assert!(!page_of(&app).is_dirty());
    assert!(page_of(&app).advanced_switch_enabled());
    flip(&mut app, false);
    assert!(!app.advanced_settings);
}

#[test]
fn a_duplicate_variable_with_no_edit_keeps_it_on() {
    let (_root, paths) = setup();
    write_global(&paths, "[env]\nA = \"1\"\n");
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    for event in [
        Event::NewEnvKey("B".into()),
        Event::NewEnvValue("1".into()),
        Event::EnvAdd,
        Event::EnvKey(1, "A".into()),
    ] {
        let _ = send(&mut app, event);
    }
    // Both rows collapse to the loaded A=1, so nothing counts as edited.
    assert_eq!(page_of(&app).changes(), 0);
    assert_eq!(page_of(&app).advanced_hold(), Some(HOLD_EDITS));
    assert!(!page_of(&app).advanced_switch_enabled());
    let _ = send(&mut app, Event::EnvRemove(1));
    assert!(page_of(&app).advanced_switch_enabled());
}

#[test]
fn a_typed_new_variable_keeps_it_on_with_its_own_note() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    for typed in [Event::NewEnvKey("X".into()), Event::NewEnvValue("1".into())] {
        let _ = send(&mut app, typed);
        assert_eq!(page_of(&app).advanced_hold(), Some(HOLD_NEW_ENV));
        assert!(!page_of(&app).advanced_switch_enabled());
        assert!(shows(&app, &paths, HOLD_NEW_ENV));
        assert!(!page_of(&app).is_dirty());
    }
    // Clearing it lets go.
    let _ = send(&mut app, Event::NewEnvKey(String::new()));
    let _ = send(&mut app, Event::NewEnvValue(String::new()));
    assert!(page_of(&app).advanced_switch_enabled());
    // Adding it moves the hold to the edit it made, which Revert clears.
    let _ = send(&mut app, Event::NewEnvKey("X".into()));
    let _ = send(&mut app, Event::EnvAdd);
    assert_eq!(page_of(&app).advanced_hold(), Some(HOLD_EDITS));
    let _ = send(&mut app, Event::Revert);
    assert!(page_of(&app).advanced_switch_enabled());
}

#[test]
fn it_waits_while_saving_and_behind_the_leave_prompt() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    let _ = send(&mut app, Event::GlobalSwitch(Switch::Gamemode, true));
    let _ = send(&mut app, Event::Save);
    assert!(page_of(&app).locked());
    assert!(!page_of(&app).advanced_switch_enabled());
    flip(&mut app, true);
    assert!(!app.advanced_settings);

    let mut app = open_advanced(&paths);
    let _ = send(&mut app, Event::GlobalSwitch(Switch::Gamemode, true));
    app.settings.as_mut().unwrap().leave_prompt = true;
    assert!(!page_of(&app).advanced_switch_enabled());
    flip(&mut app, true);
    assert!(!app.advanced_settings);
}

#[test]
fn revert_and_reload_leave_the_switch_alone() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    finish_write(&mut app);
    let _ = send(&mut app, Event::GlobalSwitch(Switch::Gamemode, true));
    let _ = send(&mut app, Event::Revert);
    assert!(app.advanced_settings && page_of(&app).advanced_settings);

    let _ = send(&mut app, Event::GlobalSwitch(Switch::Gamemode, true));
    write_global(&paths, "mangohud = true\n");
    let generation = page_of(&app).generation;
    let sequence = start_check(app.settings.as_mut().unwrap());
    let _ = send(
        &mut app,
        Event::Checked(generation, sequence, Texts::read_global(&paths)),
    );
    assert!(page_of(&app).changed_on_disk);
    let _ = send(&mut app, Event::Reload);
    let _ = send(
        &mut app,
        Event::Loaded(generation, load::snapshot_global(&paths)),
    );
    assert!(!page_of(&app).changed_on_disk);
    assert!(app.advanced_settings && page_of(&app).advanced_settings);
    assert!(app.view_writes.advanced_settings.is_none());
    assert!(remembered(&paths));
}

#[test]
fn flipping_it_under_appearance_edits_is_no_change_on_disk() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    let _ = send(
        &mut app,
        Event::Appearance(appearance::Edit::FollowSystem(false)),
    );
    assert!(page_of(&app).is_dirty());
    flip(&mut app, true);
    finish_write(&mut app);
    assert!(remembered(&paths));
    let generation = page_of(&app).generation;
    let sequence = start_check(app.settings.as_mut().unwrap());
    let _ = send(
        &mut app,
        Event::Checked(generation, sequence, Texts::read_global(&paths)),
    );
    assert!(!page_of(&app).changed_on_disk);
    assert!(!page_of(&app).loading());
    assert!(page_of(&app).is_dirty());
}

#[test]
fn a_refused_write_keeps_the_switch_and_says_why_on_both_pages() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    write_library(&paths, BROKEN_LIBRARY);
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    finish_write(&mut app);
    // This session keeps it on.
    assert!(app.advanced_settings && page_of(&app).advanced_settings);
    let error = app.advanced_settings_error.clone().expect("refused");
    assert!(error.checkup);
    assert_eq!(page_of(&app).advanced_error.as_ref(), Some(&error));
    // Its own field, which an edit doesn't clear.
    assert!(page_of(&app).error.is_none());
    let _ = send(&mut app, Event::GlobalSwitch(Switch::Gamemode, true));
    assert!(shows(&app, &paths, &error.text));
    assert!(shows(&app, &paths, "Open Checkup"));
    assert_eq!(
        page_ids(&app, &paths)[..2],
        [id(ADVANCED_SWITCH_ID), id("settings-advanced-checkup")]
    );
    let _ = send(&mut app, Event::Revert);

    // The environment page shows it under its Advanced heading.
    app.selected = Some("game".into());
    let _ = app.update(Message::OpenSettings);
    let generation = page_of(&app).generation;
    let _ = send(
        &mut app,
        Event::Loaded(generation, load::snapshot(&paths, "game")),
    );
    let shown = texts(&app, &paths);
    let heading = shown.iter().position(|text| text == "Advanced").unwrap();
    assert_eq!(shown[heading + 1], error.text);
    assert_eq!(shown[heading + 2], "Open Checkup");

    // A newer wineo's file has nothing for Checkup to repair.
    write_library(&paths, "schema_version = 99\n");
    let mut app = open_advanced(&paths);
    flip(&mut app, true);
    finish_write(&mut app);
    let error = app.advanced_settings_error.clone().expect("refused");
    assert!(!error.checkup);
    assert!(shows(&app, &paths, &error.text));
    assert!(!shows(&app, &paths, "Open Checkup"));

    // The next write that lands clears it.
    write_library(&paths, "");
    flip(&mut app, false);
    finish_write(&mut app);
    assert!(app.advanced_settings_error.is_none());
    assert!(page_of(&app).advanced_error.is_none());
    assert!(!shows(&app, &paths, &error.text));
}

#[test]
fn the_environment_pages_button_refused_says_so_under_advanced() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    write_library(&paths, BROKEN_LIBRARY);
    let mut app = open_app(&paths);
    let _ = send(&mut app, Event::ShowAdvanced);
    finish_write(&mut app);
    assert!(app.advanced_settings && page_of(&app).advanced_settings);
    let error = page_of(&app).advanced_error.clone().expect("refused");
    let shown = texts(&app, &paths);
    let heading = shown.iter().position(|text| text == "Advanced").unwrap();
    assert_eq!(shown[heading + 1], error.text);
    assert_eq!(shown[heading + 2], "Open Checkup");
}

#[test]
fn the_keyboard_flips_it_and_focus_stays_on_it() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    laid.run(Box::new(focusable::focus::<()>(Id::new(
        ADVANCED_SWITCH_ID,
    ))));
    let mut tree = laid.into_tree();
    for on in [true, false, true] {
        let mut laid = relay_out(page_of(&app).view(&paths, Palette::DARK), tree, SIZE);
        assert_eq!(laid.focused(), [Id::new(ADVANCED_SWITCH_ID)], "before {on}");
        let (messages, _) = crate::focus_button::press(&mut laid, Named::Space, Modifiers::empty());
        tree = laid.into_tree();
        assert!(
            matches!(
                messages.as_slice(),
                [Message::Settings(Event::AdvancedSettings(sent))] if *sent == on
            ),
            "{messages:?}"
        );
        for message in messages {
            let _ = app.update(message);
        }
        finish_write(&mut app);
        assert_eq!(app.advanced_settings, on);
        assert_eq!(shows(&app, &paths, "Runs"), on);
    }
    let mut laid = relay_out(page_of(&app).view(&paths, Palette::DARK), tree, SIZE);
    assert_eq!(laid.focused(), [Id::new(ADVANCED_SWITCH_ID)]);
}

#[test]
fn every_saved_key_belongs_to_exactly_one_tab() {
    let (_root, paths) = setup();
    write_global(&paths, "[env]\nOLD = \"1\"\n");
    let mut page = global_page(&paths);
    let editor = match &mut page.view {
        View::Global(editor) => editor,
        _ => unreachable!(),
    };
    let form = &mut editor.form;
    // Named in full, so a new field fails to build until it's edited here.
    let global::Values {
        gamemode,
        mangohud,
        wayland,
        runner,
        arch,
        prefix_root,
        runner_root,
        dxvk_root,
        vkd3d_root,
        stop_timeout,
        log_limit_mb,
        games_root,
        gog_language,
        art_fetch,
        env,
        external_runners,
        retroarch_quit,
        auto_import,
        desktop,
        coverflow_fullscreen,
        input,
    } = &mut form.draft;
    *gamemode = Some(true);
    *mangohud = Some(true);
    *wayland = Some(true);
    *runner = "other".into();
    *arch = Arch::Win32;
    *prefix_root = "/p".into();
    *runner_root = "/r".into();
    *dxvk_root = "/d".into();
    *vkd3d_root = "/v".into();
    *stop_timeout = "15".into();
    *log_limit_mb = "80".into();
    *games_root = "/g".into();
    *gog_language = "de".into();
    *art_fetch = ArtFetch::Icon;
    env.push(EnvRow {
        key: "NEW".into(),
        value: "2".into(),
    });
    env.remove(0);
    *external_runners = Some(true);
    *retroarch_quit = Some(false);
    *auto_import = true;
    desktop.tray = !desktop.tray;
    *coverflow_fullscreen = true;
    input.controller = !input.controller;

    let edits = form.edits();
    let library = form.library_edits();
    let keys: Vec<&str> = edits
        .pairs
        .iter()
        .map(|(key, _)| key.as_str())
        .chain(edits.unset.iter().map(String::as_str))
        .chain(global::library_keys(&library))
        .collect();
    let mut advanced = Vec::new();
    for key in &keys {
        let tab = global::tab_of(key).unwrap_or_else(|| panic!("{key} has no tab"));
        if tab == Tab::Advanced {
            advanced.push(*key);
        }
    }
    advanced.sort_unstable();
    assert_eq!(
        advanced,
        [
            "defaults.arch",
            "defaults.dxvk_root",
            "defaults.prefix_root",
            "defaults.runner",
            "defaults.runner_root",
            "defaults.vkd3d_root",
            "env.NEW",
            "env.OLD",
            "external_runners",
            "log_limit_mb",
            "stop_timeout",
        ]
    );
    assert_eq!(form.advanced_changes(), advanced.len());
    // Every field above made a key: 21 fields, one of them two env keys.
    assert_eq!(keys.len(), 22, "{keys:?}");
}

fn wine_app(paths: &Paths) -> App {
    write(paths, GAME);
    open_app(paths)
}

#[test]
fn the_environment_page_hides_advanced_until_its_button_shows_it() {
    let (_root, paths) = setup();
    let mut app = wine_app(&paths);
    let shown = texts(&app, &paths);
    assert!(!shown.iter().any(|text| text == "Advanced"));
    assert!(!shown.iter().any(|text| text == "Prefix"));
    // Environment variables stay.
    assert!(shown.iter().any(|text| text == "Environment variables"));
    // At the bottom of the page, above the save bar.
    let at = |label: &str| shown.iter().position(|text| text == label).unwrap();
    assert_eq!(at(ADVANCED_SWITCH), at("No changes") - 1);
    assert!(at("Targets") < at(ADVANCED_SWITCH));

    let messages = click_label(&app, &paths, ADVANCED_SWITCH);
    assert!(
        matches!(
            messages.as_slice(),
            [Message::Settings(Event::ShowAdvanced)]
        ),
        "{messages:?}"
    );
    let mut task = Task::none();
    for message in messages {
        task = app.update(message);
    }
    assert!(app.advanced_settings && page_of(&app).advanced_settings);
    assert_eq!(app.view_writes.advanced_settings, Some(true));
    // The task focuses Prefix and writes the switch.
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    let mut results = Vec::new();
    crate::test_tasks::each_action(task, |action| match action {
        iced_runtime::Action::Widget(operation) => laid.run(operation),
        iced_runtime::Action::Output(message) => results.push(message),
        _ => {}
    });
    assert_eq!(laid.focused(), [Id::new(PREFIX_ID)]);
    drop(laid);
    let shown = texts(&app, &paths);
    assert!(shown.iter().any(|text| text == "Advanced"));
    assert!(shown.iter().any(|text| text == "Prefix"));
    assert!(!shown.iter().any(|text| text == ADVANCED_SWITCH));
    for message in results {
        let _ = app.update(message);
    }
    assert!(app.view_writes.advanced_settings.is_none());
    assert!(remembered(&paths));
}

#[test]
fn a_native_page_hides_advanced_with_no_button() {
    let (_root, paths) = setup();
    write(&paths, NATIVE_GAME);
    let mut app = open_app(&paths);
    let shown = texts(&app, &paths);
    assert!(
        !shown
            .iter()
            .any(|text| text == "Advanced" || text == "Kind")
    );
    assert!(!shown.iter().any(|text| text == ADVANCED_SWITCH));
    let _ = send(&mut app, Event::ShowAdvanced);
    assert!(!app.advanced_settings);
    assert!(app.view_writes.advanced_settings.is_none());
}

#[test]
fn a_proxied_page_shows_no_button() {
    let (_root, paths) = setup();
    write(
        &paths,
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\n",
    );
    let mut app = open_app(&paths);
    assert!(!shows(&app, &paths, ADVANCED_SWITCH));
    let _ = send(&mut app, Event::ShowAdvanced);
    assert!(!app.advanced_settings);
    // And with the switch on, nothing changes there.
    app.advanced_settings = true;
    let _ = app.update(Message::OpenSettings);
    let generation = page_of(&app).generation;
    let _ = send(
        &mut app,
        Event::Loaded(generation, load::snapshot(&paths, "game")),
    );
    assert!(!shows(&app, &paths, "Advanced"));
    assert!(!shows(&app, &paths, ADVANCED_SWITCH));
}

#[test]
fn the_global_page_ignores_the_button_and_the_environment_page_the_switch() {
    let (_root, paths) = setup();
    let mut app = open_advanced(&paths);
    let _ = send(&mut app, Event::ShowAdvanced);
    assert!(!app.advanced_settings);
    let mut app = wine_app(&paths);
    flip(&mut app, true);
    assert!(!app.advanced_settings);
}

#[test]
fn tab_reaches_the_environment_pages_button_and_enter_shows_prefix() {
    let (_root, paths) = setup();
    let mut app = wine_app(&paths);
    let button = Id::new(view::SHOW_ADVANCED_ID);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    let ids = crate::focus_button::focusables(&mut laid);
    // The page's last Tab stop.
    assert_eq!(ids.last(), Some(&id(view::SHOW_ADVANCED_ID)), "{ids:?}");
    // Tab walks there from the top, as the page's Tab key does.
    let mut reached = false;
    for _ in 0..ids.len() {
        crate::focus_button::run_task(&mut laid, iced::widget::operation::focus_next::<Message>());
        if laid.focused() == [button.clone()] {
            reached = true;
            break;
        }
    }
    assert!(reached);
    let (messages, _) = crate::focus_button::press(&mut laid, Named::Enter, Modifiers::empty());
    let tree = laid.into_tree();
    assert!(
        matches!(
            messages.as_slice(),
            [Message::Settings(Event::ShowAdvanced)]
        ),
        "{messages:?}"
    );
    let mut task = Task::none();
    for message in messages {
        task = app.update(message);
    }
    let mut laid = relay_out(page_of(&app).view(&paths, Palette::DARK), tree, SIZE);
    let mut results = Vec::new();
    crate::test_tasks::each_action(task, |action| match action {
        iced_runtime::Action::Widget(operation) => laid.run(operation),
        iced_runtime::Action::Output(message) => results.push(message),
        _ => {}
    });
    assert_eq!(laid.focused(), [Id::new(PREFIX_ID)]);
    drop(laid);
    assert!(shows(&app, &paths, "Prefix"));
    for message in results {
        let _ = app.update(message);
    }
    assert!(remembered(&paths));
}
