use std::collections::BTreeSet;
use std::fs;

use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::paths::Paths;
use wineo_core::runtime::State;

use super::*;
use crate::bulk_tags::Saved;
use crate::library::Card;
use crate::sort_control::tests::{answers, run_counting};
use crate::tests::card;

const CONFIG: &str =
    "schema_version = 1\nprefix = \"~/prefixes/game\"\nrunner = \"wine\"\narch = \"win64\"\n";

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn tagged(name: &str, tags: &[&str]) -> Card {
    let mut card = card(name, State::Inactive, 0);
    card.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    card
}

/// alpha and bravo have rpg, charlie doesn't, and delta, never marked here,
/// has solo.
fn library() -> Vec<Card> {
    vec![
        tagged("alpha", &["rpg"]),
        tagged("bravo", &["rpg"]),
        tagged("charlie", &[]),
        tagged("delta", &["solo"]),
    ]
}

fn config_text(tags: &[String]) -> String {
    let tags: Vec<String> = tags.iter().map(|tag| format!("{tag:?}")).collect();
    format!("{CONFIG}tags = [{}]\n", tags.join(", "))
}

fn write_config(paths: &Paths, card: &Card) {
    let path = paths.environment_config(&card.name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, config_text(&card.tags)).unwrap();
}

fn read_config(app: &App, name: &str) -> String {
    fs::read_to_string(app.paths.environment_config(name)).unwrap()
}

/// The tags `name`'s config has now.
fn on_disk(app: &App, name: &str) -> Vec<String> {
    let path = app.paths.environment_config(name);
    wineo_core::config::EnvironmentConfig::parse(&path, &fs::read_to_string(&path).unwrap())
        .unwrap()
        .tags
}

/// An app on its own HOME with `library()` loaded and its configs written,
/// and alpha, bravo, and charlie marked.
fn marked() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    for card in library() {
        write_config(&paths, &card);
    }
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(library())));
    for name in ["alpha", "bravo", "charlie"] {
        app.toggle_mark(name);
    }
    (root, app)
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn tags(app: &App, name: &str) -> Vec<String> {
    app.cards
        .iter()
        .find(|card| card.name == name)
        .unwrap()
        .tags
        .clone()
}

fn has(app: &App, name: &str, tag: &str) -> bool {
    tags(app, name).iter().any(|existing| existing == tag)
}

fn notice(app: &App) -> Option<&str> {
    app.feature_notice.as_ref().map(|(text, _)| text.as_str())
}

fn tags_event(app: &mut App, event: Event) -> Task<Message> {
    app.update(Message::Tags(event))
}

/// Opens the popup the way a click on Tags does, finding the button in the
/// laid out window.
fn open(app: &mut App) -> Task<Message> {
    let task = tags_event(app, Event::ToggleMenu);
    let mut focus = Vec::new();
    for message in answers(app, task) {
        focus.push(app.update(message));
    }
    assert!(app.tag_menu_shown());
    Task::batch(focus)
}

fn toggle(app: &mut App, tag: &str) -> Task<Message> {
    tags_event(app, Event::Toggle(tag.into()))
}

fn is_saved(message: &Message) -> bool {
    matches!(message, Message::Tags(Event::Saved(_)))
}

/// Runs `task`, which must finish `writes` batches, and returns their
/// replies.
fn saved(task: Task<Message>, writes: usize) -> Vec<Message> {
    run_counting(task, false, writes, is_saved)
        .messages
        .into_iter()
        .filter(is_saved)
        .collect()
}

/// Runs the batch `task` starts and hands its reply back, and so on for any
/// batch that reply starts.
fn finish(app: &mut App, task: Task<Message>) {
    let mut replies = saved(task, 1);
    while let Some(reply) = replies.pop() {
        let next = app.update(reply);
        replies.extend(
            run_counting(next, false, 0, is_saved)
                .messages
                .into_iter()
                .filter(is_saved),
        );
    }
}

fn key(app: &mut App, named: Named) -> Task<Message> {
    app.update(Message::Key(
        Key::Named(named),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ))
}

/// The widget operations `task` asks for, run in the laid out window, and
/// what has focus after.
fn focused_after(app: &App, task: Task<Message>) -> Vec<iced::widget::Id> {
    use iced::futures::StreamExt;
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    if let Some(mut stream) = iced_runtime::task::into_stream(task) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            while let Ok(Some(action)) =
                tokio::time::timeout(std::time::Duration::from_millis(500), stream.next()).await
            {
                if let iced_runtime::Action::Widget(operation) = action {
                    laid.run(operation);
                }
            }
        });
    }
    laid.focused()
}

// The bar.

#[test]
fn the_bar_shows_while_games_are_marked_and_counts_them() {
    let (_root, mut app) = marked();
    assert!(app.action_bar_shown());
    assert_eq!(count_label(app.marked.len()), "3 selected");
    let _ = app.view();
    app.clear_marks();
    assert!(!app.action_bar_shown());
    // Nothing to press without marks.
    assert!(crate::library_states::tests::press_control(&app, TAGS_BUTTON_ID).is_empty());
    app.toggle_mark("delta");
    assert_eq!(count_label(app.marked.len()), "1 selected");
    assert!(app.action_bar_shown());
    // Off the Library it hides with the marks kept.
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert!(!app.action_bar_shown());
    assert_eq!(app.marked, names(&["delta"]));
}

#[test]
fn the_bar_controls_take_keyboard_focus_and_the_x_clears_the_marks() {
    let (_root, mut app) = marked();
    let messages = crate::library_states::tests::press_control(&app, TAGS_BUTTON_ID);
    assert!(matches!(
        messages.as_slice(),
        [Message::Tags(Event::ToggleMenu)]
    ));
    let messages = crate::library_states::tests::press_control(&app, CLEAR_ID);
    assert!(matches!(messages.as_slice(), [Message::Tags(Event::Clear)]));
    for message in messages {
        let _ = app.update(message);
    }
    assert!(app.marked.is_empty());
    assert!(!app.action_bar_shown());
}

#[test]
fn the_bar_sits_at_the_bottom_under_the_notes() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let alone = app.tag_popup.anchor;
    // At the bottom of the window, inside the notes' 16 px padding.
    assert!(alone.y + alone.height <= app.window_size.height - 16.0);
    assert!(alone.y > app.window_size.height - 80.0);
    // A notice stacks above it rather than moving it.
    let _ = key(&mut app, Named::Escape);
    app.flag_notice("Something happened".to_owned());
    let _ = open(&mut app);
    assert_eq!(app.tag_popup.anchor, alone);
}

// The popup.

#[test]
fn the_popup_shows_each_tag_across_the_marked_games() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    assert_eq!(app.tag_rows(), ["rpg", "solo"]);
    assert_eq!(app.tag_share("rpg"), Share::Some);
    assert_eq!(app.tag_share("solo"), Share::None);
    let _ = toggle(&mut app, "rpg");
    assert_eq!(app.tag_share("rpg"), Share::All);
    let _ = app.view();
}

#[test]
fn the_popup_opens_above_the_tags_button_with_focus_on_its_first_row() {
    let (_root, mut app) = marked();
    let focus = open(&mut app);
    assert!(app.tag_popup.anchor.y > 0.0);
    assert_eq!(
        focused_after(&app, focus),
        [iced::widget::Id::from(row_id("rpg"))]
    );
    // The arrows move through the rows and on to the field.
    let down = key(&mut app, Named::ArrowDown);
    assert_eq!(
        focused_after(
            &app,
            crate::library_menu::focus(row_id("rpg"), MENU_SCROLL_ID).chain(down)
        ),
        [iced::widget::Id::from(row_id("solo"))]
    );
}

#[test]
fn a_dash_or_empty_row_adds_to_every_marked_game_and_a_checked_one_removes() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let before_alpha = read_config(&app, "alpha");
    // rpg has a dash: only charlie lacks it, so only charlie is written.
    let task = toggle(&mut app, "rpg");
    assert!(has(&app, "charlie", "rpg"), "the card changes at once");
    assert_eq!(on_disk(&app, "charlie"), Vec::<String>::new());
    assert!(app.tag_writes.get("charlie", "rpg").is_some());
    assert!(app.tag_writes.get("alpha", "rpg").is_none());
    finish(&mut app, task);
    assert_eq!(on_disk(&app, "charlie"), ["rpg"]);
    assert_eq!(read_config(&app, "alpha"), before_alpha, "already tagged");
    assert!(!app.tag_writes.pending());
    assert!(app.feature_notice.is_none());

    // Checked: removed from all three.
    let task = toggle(&mut app, "rpg");
    for name in ["alpha", "bravo", "charlie"] {
        assert!(!has(&app, name, "rpg"));
    }
    finish(&mut app, task);
    for name in ["alpha", "bravo", "charlie"] {
        assert_eq!(on_disk(&app, name), Vec::<String>::new(), "{name}");
    }

    // Empty: added to all three, and delta, unmarked, is never touched.
    let task = toggle(&mut app, "solo");
    finish(&mut app, task);
    for name in ["alpha", "bravo", "charlie"] {
        assert_eq!(on_disk(&app, name), ["solo"], "{name}");
    }
    assert_eq!(on_disk(&app, "delta"), ["solo"]);
    assert_eq!(app.tag_share("solo"), Share::All);
}

#[test]
fn a_tag_no_game_has_any_more_stays_listed_while_the_popup_is_open() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let _ = toggle(&mut app, "rpg");
    let task = toggle(&mut app, "rpg");
    finish(&mut app, task);
    assert_eq!(app.tag_rows(), ["rpg", "solo"]);
    assert_eq!(app.tag_share("rpg"), Share::None);
    let _ = key(&mut app, Named::Escape);
    let _ = open(&mut app);
    assert_eq!(app.tag_rows(), ["solo"]);
}

#[test]
fn rapid_clicks_end_with_the_last_state_written() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let first = toggle(&mut app, "rpg");
    // Now all have it, so this removes it: charlie's waits behind its add.
    let second = toggle(&mut app, "rpg");
    assert_eq!(
        app.tag_writes
            .get("charlie", "rpg")
            .map(|pending| pending.next),
        Some(Some(false))
    );
    finish(&mut app, first);
    finish(&mut app, second);
    assert!(!app.tag_writes.pending());
    for name in ["alpha", "bravo", "charlie"] {
        assert!(!has(&app, name, "rpg"));
        assert_eq!(on_disk(&app, name), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn enter_in_the_field_adds_a_new_tag_to_every_marked_game() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let _ = tags_event(&mut app, Event::Draft("co-op".into()));
    let task = tags_event(&mut app, Event::Submit);
    assert_eq!(app.tag_draft(), "");
    assert_eq!(app.tag_rows(), ["co-op", "rpg", "solo"]);
    assert_eq!(app.tag_share("co-op"), Share::All);
    finish(&mut app, task);
    assert_eq!(on_disk(&app, "alpha"), ["rpg", "co-op"]);
    assert_eq!(on_disk(&app, "charlie"), ["co-op"]);
    assert!(app.tag_menu_shown(), "the popup stays for more");
    // The order the cards show matches the file, so a quiet rescan has
    // nothing to report.
    assert_eq!(tags(&app, "alpha"), ["rpg", "co-op"]);
}

#[test]
fn a_bad_tag_name_is_refused_before_anything_is_written() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let before: Vec<String> = ["alpha", "bravo", "charlie"]
        .iter()
        .map(|name| read_config(&app, name))
        .collect();
    let _ = tags_event(&mut app, Event::Draft("co op".into()));
    assert_eq!(
        problem(app.tag_draft()).as_deref(),
        Some("Invalid tag \"co op\": spaces not allowed.")
    );
    let _ = app.view();
    let task = tags_event(&mut app, Event::Submit);
    assert!(saved(task, 0).is_empty());
    assert!(!app.tag_writes.pending());
    assert_eq!(app.tag_draft(), "co op", "kept to fix");
    assert!(!has(&app, "alpha", "co op"));
    let after: Vec<String> = ["alpha", "bravo", "charlie"]
        .iter()
        .map(|name| read_config(&app, name))
        .collect();
    assert_eq!(before, after);
    // An empty field adds nothing either.
    let _ = tags_event(&mut app, Event::Draft(String::new()));
    assert!(saved(tags_event(&mut app, Event::Submit), 0).is_empty());
    assert_eq!(problem(""), None);
}

// Pending writes and rescans.

#[test]
fn a_read_from_before_the_write_landed_keeps_the_change_on_screen() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let task = toggle(&mut app, "rpg");
    // A rescan read the file before the write reached it.
    let _ = app.update(Message::Loaded(Ok(library())));
    assert!(has(&app, "charlie", "rpg"));
    finish(&mut app, task);
    assert!(has(&app, "charlie", "rpg"));
    assert_eq!(on_disk(&app, "charlie"), ["rpg"]);
}

#[test]
fn a_rescan_that_read_before_the_write_finished_is_dropped() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let task = toggle(&mut app, "rpg");
    let stale = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(library())),
        ..Default::default()
    };
    finish(&mut app, task);
    assert!(app.tag_writes.get("charlie", "rpg").is_none());
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert!(has(&app, "charlie", "rpg"));
}

// Failures.

#[test]
fn a_partial_failure_names_what_failed_and_rolls_back_only_those() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let broken = "schema_version = 1\ntags = [\n";
    fs::write(app.paths.environment_config("bravo"), broken).unwrap();
    let _ = tags_event(&mut app, Event::Draft("co-op".into()));
    let task = tags_event(&mut app, Event::Submit);
    assert!(has(&app, "bravo", "co-op"));
    finish(&mut app, task);
    assert!(has(&app, "alpha", "co-op"));
    assert!(has(&app, "charlie", "co-op"));
    assert!(!has(&app, "bravo", "co-op"), "rolled back");
    assert_eq!(on_disk(&app, "alpha"), ["rpg", "co-op"]);
    assert_eq!(on_disk(&app, "charlie"), ["co-op"]);
    assert_eq!(read_config(&app, "bravo"), broken);
    let text = notice(&app).unwrap();
    assert!(text.starts_with("Couldn't tag 1 of 3 (bravo): "), "{text}");
    assert!(text.contains("bravo.toml"), "{text}");
    assert_eq!(app.marked.len(), 3, "marks stay");

    // Removing says so in its own words.
    let mut cards = library();
    cards[2].tags = vec!["rpg".into()];
    write_config(&app.paths, &cards[2]);
    let _ = app.update(Message::Loaded(Ok(cards)));
    let task = toggle(&mut app, "rpg");
    finish(&mut app, task);
    let text = notice(&app).unwrap();
    assert!(
        text.starts_with("Couldn't untag 1 of 3 (bravo): "),
        "{text}"
    );
    assert!(has(&app, "bravo", "rpg"));
    assert!(!has(&app, "alpha", "rpg"));
    assert!(!has(&app, "charlie", "rpg"));
}

#[test]
fn a_dead_worker_reloads_the_library_rather_than_guess() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let _ = toggle(&mut app, "rpg");
    let _ = app.update(Message::Tags(Event::Saved(Saved {
        tag: "rpg".into(),
        value: true,
        games: vec![("charlie".into(), None)],
        results: Err("worker panicked".into()),
    })));
    assert_eq!(notice(&app), Some("Couldn't tag charlie: worker panicked"));
    assert!(app.refreshing, "a reload reads what the file has");
    assert!(!app.tag_writes.pending());
}

// Removals and undo.

#[test]
fn a_game_removed_before_its_write_ran_is_left_gone() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let task = toggle(&mut app, "rpg");
    // `rm` moved charlie to the trash before the write reached it.
    let config = app.paths.environment_config("charlie");
    fs::remove_file(&config).unwrap();
    finish(&mut app, task);
    assert!(!config.exists(), "nothing recreated");
    assert!(app.feature_notice.is_none());
    assert!(app.refreshing, "a reload shows it gone");
    let without: Vec<Card> = library()
        .into_iter()
        .filter(|card| card.name != "charlie")
        .collect();
    let _ = app.update(Message::LibraryRead(
        app.library_generation,
        Ok(without),
        None,
    ));
    assert_eq!(app.marked, names(&["alpha", "bravo"]));
}

#[test]
fn a_reply_for_a_game_that_left_is_ignored_and_undo_brings_it_back_unmarked() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let _ = tags_event(&mut app, Event::Draft("co-op".into()));
    let task = tags_event(&mut app, Event::Submit);
    // The window saw charlie leave before the write's reply came back.
    let without: Vec<Card> = library()
        .into_iter()
        .filter(|card| card.name != "charlie")
        .collect();
    let _ = app.update(Message::Loaded(Ok(without)));
    assert!(app.tag_writes.get("charlie", "co-op").is_none());
    assert_eq!(app.marked, names(&["alpha", "bravo"]));
    let replies = saved(task, 1);
    for reply in replies {
        let _ = app.update(reply);
    }
    assert!(!app.tag_writes.pending());
    assert!(app.feature_notice.is_none());

    // Undo puts the environment back as it was, with no mark and none of
    // the change shown over it.
    let mut restored = library();
    for card in &mut restored[..2] {
        card.tags.push("co-op".into());
    }
    let _ = app.update(Message::Loaded(Ok(restored)));
    assert_eq!(app.marked, names(&["alpha", "bravo"]));
    assert!(!has(&app, "charlie", "co-op"));
    assert!(has(&app, "alpha", "co-op"));
}

#[test]
fn a_late_reply_for_a_recreated_game_changes_nothing() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let _ = toggle(&mut app, "rpg");
    let late = Message::Tags(Event::Saved(Saved {
        tag: "rpg".into(),
        value: true,
        games: vec![("charlie".into(), Some(jiff::Timestamp::UNIX_EPOCH))],
        results: Ok(vec![Err("failed".into())]),
    }));
    let _ = app.update(late);
    assert!(app.tag_writes.get("charlie", "rpg").is_some());
    assert!(has(&app, "charlie", "rpg"));
    assert!(app.feature_notice.is_none());
}

// Keys.

#[test]
fn esc_closes_the_popup_first_then_clears_the_marks() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let task = key(&mut app, Named::Escape);
    assert!(!app.tag_menu);
    assert_eq!(app.marked.len(), 3);
    assert_eq!(
        focused_after(&app, task),
        [iced::widget::Id::new(TAGS_BUTTON_ID)]
    );
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(Named::Escape),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    assert!(app.marked.is_empty());
}

#[test]
fn the_popup_takes_every_key_while_open() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    // Ctrl+A would mark delta too, and R would pick at random.
    let _ = app.update(Message::KeyWithFocus(
        Key::Character("a".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
        false,
    ));
    let _ = app.update(Message::Key(
        Key::Character("r".into()),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert_eq!(app.marked.len(), 3);
    assert!(app.selected.is_none());
    assert!(app.tag_menu_shown());
}

#[test]
fn marks_stay_through_a_tag_change_and_the_popup_closes_with_them() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let task = toggle(&mut app, "solo");
    finish(&mut app, task);
    assert_eq!(app.marked, names(&["alpha", "bravo", "charlie"]));
    // Clearing the marks takes the bar and its popup away.
    app.clear_marks();
    let _ = app.update(Message::Modifiers(Modifiers::empty()));
    assert!(!app.tag_menu);
}

#[test]
fn enter_on_a_focused_row_toggles_its_tag() {
    let (_root, mut app) = marked();
    let _ = open(&mut app);
    let row = Box::leak(row_id("solo").into_boxed_str());
    let messages = crate::library_states::tests::press_control(&app, row);
    assert!(matches!(
        messages.as_slice(),
        [Message::Tags(Event::Toggle(tag))] if tag == "solo"
    ));
    for message in messages {
        let _ = app.update(message);
    }
    assert_eq!(app.tag_share("solo"), Share::All);
}
