//! Full screen: F11, the owner, the coverflow switch, compositor exits, the
//! kept windowed size, and the tray's reopen.

use iced::Size;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::window::Mode;
use iced_runtime::window::Action as WindowAction;
use wineo_core::runtime::State;

use super::*;
use crate::tests::card;

/// What a task asked of the window, with `GetMode` answered as the
/// compositor would.
#[derive(Default)]
struct Calls {
    set_modes: Vec<Mode>,
    read_modes: usize,
    opened: Vec<iced::window::Settings>,
    messages: Vec<Message>,
}

fn run(app: &App, task: Task<Message>, compositor: Mode) -> Calls {
    use iced::futures::StreamExt;
    let mut calls = Calls::default();
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return calls;
    };
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_millis(200), stream.next()).await
        {
            match action {
                iced_runtime::Action::Window(WindowAction::SetMode(_, mode)) => {
                    calls.set_modes.push(mode);
                }
                iced_runtime::Action::Window(WindowAction::GetMode(_, channel)) => {
                    calls.read_modes += 1;
                    let _ = channel.send(compositor);
                }
                iced_runtime::Action::Window(WindowAction::Open(_, settings, _)) => {
                    calls.opened.push(settings);
                }
                iced_runtime::Action::Output(message) => calls.messages.push(message),
                iced_runtime::Action::Widget(operation) => laid.run(operation),
                _ => {}
            }
        }
    });
    calls
}

/// `run`, with the messages it sent given back to the app until none are
/// left, as the runtime would, and every call added up.
fn settle(app: &mut App, task: Task<Message>, compositor: Mode) -> Calls {
    let mut total = Calls::default();
    let mut pending = vec![task];
    for _ in 0..8 {
        let Some(task) = pending.pop() else { break };
        let calls = run(app, task, compositor);
        total.set_modes.extend(calls.set_modes);
        total.read_modes += calls.read_modes;
        total.opened.extend(calls.opened);
        for message in calls.messages {
            pending.push(app.update(message));
        }
    }
    total
}

fn paths_in(root: &tempfile::TempDir) -> wineo_core::paths::Paths {
    let home = root.path().to_owned();
    wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap()
}

fn booted(library: &str) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths_in(&root);
    if !library.is_empty() {
        std::fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
        std::fs::write(paths.library_file(), library).unwrap();
    }
    let mut app = App::boot(paths).0;
    let cards = (0..6)
        .map(|index| card(&format!("game-{index}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

/// An app in `layout` with the switch as given, windowed.
fn app_in(layout: ViewLayout, switch: bool) -> (tempfile::TempDir, App) {
    let (root, mut app) = booted("");
    app.layout = layout;
    app.coverflow_fullscreen = switch;
    (root, app)
}

fn key(app: &mut App, key: Key, modifiers: Modifiers) -> Calls {
    let task = app.update(Message::Key(key, modifiers, iced::event::Status::Ignored));
    settle(app, task, Mode::Fullscreen)
}

fn f11(app: &mut App) -> Calls {
    key(app, Key::Named(Named::F11), Modifiers::empty())
}

fn ctrl_l(app: &mut App) -> Calls {
    key(app, Key::Character("l".into()), Modifiers::CTRL)
}

fn set_layout(app: &mut App, layout: ViewLayout) -> Calls {
    let task = app.set_layout(layout);
    settle(app, task, Mode::Fullscreen)
}

#[test]
fn f11_enters_as_the_user_and_leaves_again_in_every_layout() {
    for layout in [ViewLayout::List, ViewLayout::Grid, ViewLayout::Coverflow] {
        let (_root, mut app) = app_in(layout, false);
        assert_eq!(f11(&mut app).set_modes, [Mode::Fullscreen], "{layout:?}");
        assert_eq!(app.fullscreen, Owner::User);
        assert_eq!(f11(&mut app).set_modes, [Mode::Windowed], "{layout:?}");
        assert_eq!(app.fullscreen, Owner::Nobody);
    }
}

#[test]
fn f11_with_ctrl_alt_or_logo_is_not_full_screen() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    for modifiers in [Modifiers::CTRL, Modifiers::ALT, Modifiers::LOGO] {
        let calls = key(&mut app, Key::Named(Named::F11), modifiers);
        assert!(calls.set_modes.is_empty());
    }
    assert_eq!(app.fullscreen, Owner::Nobody);
}

#[test]
fn f11_works_while_typing_in_search_or_with_the_key_captured() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let _ = app.update(Message::SearchFocusChanged(true));
    // A captured key is one a widget took; F11 isn't any widget's.
    let task = app.update(Message::Key(
        Key::Named(Named::F11),
        Modifiers::empty(),
        iced::event::Status::Captured,
    ));
    assert_eq!(
        run(&app, task, Mode::Fullscreen).set_modes,
        [Mode::Fullscreen]
    );
}

#[test]
fn f11_does_nothing_in_the_close_prompt() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    app.close_prompt = true;
    assert!(f11(&mut app).set_modes.is_empty());
    assert_eq!(app.fullscreen, Owner::Nobody);
}

#[test]
fn f11_reaches_past_pages_and_open_menus() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let _ = app.open_global_settings();
    assert!(app.settings.is_some());
    assert_eq!(f11(&mut app).set_modes, [Mode::Fullscreen]);
    assert_eq!(f11(&mut app).set_modes, [Mode::Windowed]);

    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    app.sort_menu = true;
    assert!(app.sort_menu_shown());
    assert_eq!(f11(&mut app).set_modes, [Mode::Fullscreen]);
}

#[test]
fn entering_coverflow_with_the_switch_on_enters_owned_by_the_switch() {
    // The layout switch, then Ctrl+L, which cycles list, grid, coverflow.
    let (_root, mut app) = app_in(ViewLayout::Grid, true);
    assert_eq!(
        set_layout(&mut app, ViewLayout::Coverflow).set_modes,
        [Mode::Fullscreen]
    );
    assert_eq!(app.fullscreen, Owner::Switch);

    let (_root, mut app) = app_in(ViewLayout::Grid, true);
    let calls = ctrl_l(&mut app);
    assert_eq!(calls.set_modes, [Mode::Fullscreen]);
    assert_eq!(app.layout, ViewLayout::Coverflow);
    assert_eq!(app.fullscreen, Owner::Switch);
}

#[test]
fn entering_coverflow_with_the_switch_off_stays_windowed() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    assert!(
        set_layout(&mut app, ViewLayout::Coverflow)
            .set_modes
            .is_empty()
    );
    assert_eq!(app.fullscreen, Owner::Nobody);
}

#[test]
fn a_preset_into_coverflow_enters_full_screen() {
    let (_root, mut app) = booted(
        "schema_version = 1\n[view]\ncoverflow_fullscreen = true\n\n[presets.Couch]\nlayout = \"coverflow\"\n",
    );
    assert!(app.coverflow_fullscreen);
    assert_eq!(app.layout, ViewLayout::Grid);
    crate::filter_control::tests::open_filters(&mut app);
    let task = app.update(crate::presets::preset_message(
        crate::presets::Event::Apply("Couch".into()),
    ));
    assert_eq!(
        settle(&mut app, task, Mode::Fullscreen).set_modes,
        [Mode::Fullscreen]
    );
    assert_eq!(app.layout, ViewLayout::Coverflow);
    assert_eq!(app.fullscreen, Owner::Switch);
}

#[test]
fn leaving_coverflow_leaves_only_a_full_screen_the_switch_owns() {
    let (_root, mut app) = app_in(ViewLayout::Grid, true);
    let _ = set_layout(&mut app, ViewLayout::Coverflow);
    assert_eq!(
        set_layout(&mut app, ViewLayout::List).set_modes,
        [Mode::Windowed]
    );
    assert_eq!(app.fullscreen, Owner::Nobody);

    // The user's own full screen stays through the same trip.
    let (_root, mut app) = app_in(ViewLayout::Grid, true);
    let _ = f11(&mut app);
    assert!(
        set_layout(&mut app, ViewLayout::Coverflow)
            .set_modes
            .is_empty()
    );
    assert_eq!(app.fullscreen, Owner::User);
    assert!(set_layout(&mut app, ViewLayout::Grid).set_modes.is_empty());
    assert_eq!(app.fullscreen, Owner::User);
}

#[test]
fn f11_in_coverflow_leaves_the_switchs_full_screen_and_it_stays_left() {
    let (_root, mut app) = app_in(ViewLayout::Grid, true);
    let _ = set_layout(&mut app, ViewLayout::Coverflow);
    assert_eq!(f11(&mut app).set_modes, [Mode::Windowed]);
    assert_eq!(app.fullscreen, Owner::Nobody);
    // Nothing owns it now, so leaving coverflow has nothing to leave.
    assert!(set_layout(&mut app, ViewLayout::Grid).set_modes.is_empty());
}

#[test]
fn f11_again_after_leaving_makes_the_user_the_owner() {
    let (_root, mut app) = app_in(ViewLayout::Coverflow, true);
    app.fullscreen = Owner::Switch;
    let _ = f11(&mut app);
    let _ = f11(&mut app);
    assert_eq!(app.fullscreen, Owner::User);
    assert!(set_layout(&mut app, ViewLayout::Grid).set_modes.is_empty());
}

#[test]
fn a_start_in_coverflow_with_the_switch_opens_full_screen_and_grid_does_not() {
    let boot_settings = |library: &str| {
        let root = tempfile::tempdir().unwrap();
        let paths = paths_in(&root);
        std::fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
        std::fs::write(paths.library_file(), library).unwrap();
        let (app, boot) = App::boot(paths);
        let opened = run(&app, boot, Mode::Windowed).opened;
        (app.fullscreen, opened[0].fullscreen)
    };
    let (owner, opens) = boot_settings(
        "schema_version = 1\n[view]\nlayout = \"coverflow\"\ncoverflow_fullscreen = true\n",
    );
    assert_eq!((owner, opens), (Owner::Switch, true));
    let (owner, opens) = boot_settings("schema_version = 1\n[view]\ncoverflow_fullscreen = true\n");
    assert_eq!((owner, opens), (Owner::Nobody, false));
    let (owner, opens) = boot_settings("schema_version = 1\n[view]\nlayout = \"coverflow\"\n");
    assert_eq!((owner, opens), (Owner::Nobody, false));
}

#[test]
fn a_compositor_exit_clears_the_owner_and_its_resize_is_not_the_windowed_size() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let id = app.main_window.unwrap();
    let _ = app.update(app.window_event(iced::window::Event::Resized(Size::new(900.0, 600.0))));
    let _ = f11(&mut app);
    assert_eq!(app.windowed_size, Size::new(900.0, 600.0));

    // Full screen's own resize: the mode still reads full screen.
    let full = Size::new(1920.0, 1080.0);
    let task = app.update(Message::Window(id, iced::window::Event::Resized(full)));
    let calls = run(&app, task, Mode::Fullscreen);
    assert_eq!(calls.read_modes, 1);
    for message in calls.messages {
        let _ = app.update(message);
    }
    assert_eq!(app.fullscreen, Owner::User);
    assert_eq!(app.window_size, full);
    assert_eq!(app.windowed_size, Size::new(900.0, 600.0));

    // The compositor leaves full screen: the resize back is windowed, and
    // only the answer says so.
    let back = Size::new(1000.0, 700.0);
    let task = app.update(Message::Window(id, iced::window::Event::Resized(back)));
    assert_eq!(app.windowed_size, Size::new(900.0, 600.0), "not yet stored");
    for message in run(&app, task, Mode::Windowed).messages {
        let _ = app.update(message);
    }
    assert_eq!(app.fullscreen, Owner::Nobody);
    assert_eq!(app.windowed_size, back);
}

#[test]
fn a_windowed_resize_is_stored_without_asking_the_window() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let id = app.main_window.unwrap();
    let size = Size::new(1100.0, 750.0);
    let task = app.update(Message::Window(id, iced::window::Event::Resized(size)));
    assert_eq!(run(&app, task, Mode::Windowed).read_modes, 0);
    assert_eq!(app.windowed_size, size);
    assert_eq!(app.window_size, size);
}

#[test]
fn leaving_full_screen_by_f11_keeps_the_size_from_before() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let id = app.main_window.unwrap();
    let windowed = Size::new(1100.0, 750.0);
    let _ = app.update(Message::Window(id, iced::window::Event::Resized(windowed)));
    let _ = f11(&mut app);
    let _ = app.update(Message::Window(
        id,
        iced::window::Event::Resized(Size::new(2560.0, 1440.0)),
    ));
    let _ = f11(&mut app);
    assert_eq!(app.windowed_size, windowed);
    // The compositor then restores the window and says so.
    let _ = app.update(Message::Window(id, iced::window::Event::Resized(windowed)));
    assert_eq!(app.windowed_size, windowed);
}

#[test]
fn a_window_opening_full_screen_is_not_read_as_windowed() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let id = app.main_window.unwrap();
    let _ = f11(&mut app);
    let size = Size::new(1920.0, 1080.0);
    let task = app.update(Message::Window(
        id,
        iced::window::Event::Opened {
            position: None,
            size,
        },
    ));
    assert_eq!(run(&app, task, Mode::Windowed).read_modes, 0);
    assert_eq!(app.fullscreen, Owner::User);
    assert_ne!(app.windowed_size, size);
}

#[test]
fn a_stale_mode_answer_for_another_window_is_ignored() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let _ = f11(&mut app);
    let _ = app.update(Message::WindowMode(
        iced::window::Id::unique(),
        Mode::Windowed,
        Size::new(10.0, 10.0),
    ));
    assert_eq!(app.fullscreen, Owner::User);
}

#[test]
fn the_tray_reopens_full_screen_with_the_same_owner_and_the_windowed_size() {
    for owner_by in [Owner::User, Owner::Switch] {
        let (_root, mut app) = app_in(ViewLayout::Grid, true);
        let id = app.main_window.unwrap();
        let windowed = Size::new(1100.0, 750.0);
        let _ = app.update(Message::Window(id, iced::window::Event::Resized(windowed)));
        match owner_by {
            Owner::User => {
                let _ = f11(&mut app);
            }
            _ => {
                let _ = set_layout(&mut app, ViewLayout::Coverflow);
            }
        }
        assert_eq!(app.fullscreen, owner_by);
        let _ = app.update(Message::Window(
            id,
            iced::window::Event::Resized(Size::new(1920.0, 1080.0)),
        ));

        let _ = app.hide_window();
        assert_eq!(app.main_window, None);
        assert_eq!(app.fullscreen, owner_by, "hiding remembers the owner");
        let reopened = {
            let task = app.open_window();
            run(&app, task, Mode::Windowed)
        };
        assert_eq!(reopened.opened.len(), 1);
        assert!(reopened.opened[0].fullscreen);
        assert_eq!(reopened.opened[0].size, windowed);
        assert_eq!(app.fullscreen, owner_by);
    }
}

#[test]
fn the_tray_reopens_a_windowed_window_windowed() {
    let (_root, mut app) = app_in(ViewLayout::Grid, true);
    let _ = app.hide_window();
    let reopened = {
        let task = app.open_window();
        run(&app, task, Mode::Windowed)
    };
    assert!(!reopened.opened[0].fullscreen);
}

#[test]
fn f11_while_hidden_to_the_tray_only_changes_the_owner() {
    let (_root, mut app) = app_in(ViewLayout::Grid, false);
    let _ = app.hide_window();
    let calls = {
        let task = app.toggle_fullscreen();
        run(&app, task, Mode::Windowed)
    };
    assert!(calls.set_modes.is_empty());
    assert_eq!(app.fullscreen, Owner::User);
}
