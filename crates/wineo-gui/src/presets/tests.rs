use std::fs;

use iced::Rectangle;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::library::{GroupBy, SortKey, ViewLayout};
use wineo_core::runtime::State;

use super::*;
use crate::filter_control::tests::{library, open_filters};
use crate::filter_control::{Event as FilterEvent, FILTER_BUTTON_ID, button_label};
use crate::grid::ACTION_MENU_SPACING;
use crate::sort_control::tests::{answers, press};

/// Couch co-op narrows to GOG and Steam games tagged game but not rpg, with
/// every other part of the view set away from its default; Tools is
/// written the short way, with defaults for the rest.
const FILE: &str = r#"schema_version = 1

# Kept through every edit.
[presets."Couch co-op"]
sort = "last-played"
descending = true
favorites_first = true
stores = ["steam", "gog"]
tags = ["game"]
not_tags = ["rpg"]
show_hidden = true
group = "tag"
layout = "list"

[presets.Tools]
tags = ["tool"]
"#;

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn write_file(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

fn file(app: &App) -> String {
    fs::read_to_string(app.paths.library_file()).unwrap()
}

/// An app booted over `text` as its `library.toml`, with the filter tests'
/// library loaded.
fn booted(text: &str) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    write_file(&paths, text);
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(library())));
    (root, app)
}

fn send(app: &mut App, event: Event) -> Task<Message> {
    app.update(preset_message(event))
}

fn filter(app: &mut App, event: FilterEvent) -> Task<Message> {
    app.update(Message::Filter(event))
}

fn shown(app: &App) -> Vec<&str> {
    app.filtered()
        .iter()
        .map(|card| card.name.as_str())
        .collect()
}

fn names(app: &App) -> Vec<&str> {
    app.presets
        .list()
        .iter()
        .map(|(name, _)| name.as_str())
        .collect()
}

/// Runs the write in flight, as its task would, and hands the result back.
fn finish(app: &mut App) -> Task<Message> {
    let write = app.presets.writing.clone().expect("a write in flight");
    let (result, presets) = super::write(&app.paths, &app.shown, &write);
    send(
        app,
        Event::Saved(Box::new(Done {
            write,
            result,
            presets,
        })),
    )
}

/// Runs the direction write in flight and hands its result back.
fn finish_direction(app: &mut App) -> Task<Message> {
    let field = app
        .view_writes
        .descending
        .map(Field::Descending)
        .expect("a direction write");
    let result = crate::sort_control::save(&app.paths, &app.shown, field);
    app.update(Message::Sort(crate::sort_control::Event::Saved(
        field, result,
    )))
}

fn flip(app: &mut App) {
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
}

fn key(app: &mut App, named: Named) -> Task<Message> {
    app.update(Message::Key(
        Key::Named(named),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ))
}

fn key_event(key: Key, modifiers: Modifiers, text: Option<&str>) -> iced::Event {
    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: text.map(Into::into),
        repeat: false,
    })
}

fn char_key(character: &str, modifiers: Modifiers) -> iced::Event {
    key_event(Key::Character(character.into()), modifiers, None)
}

/// The preset events among `messages`, as text.
fn preset_events(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Filter(FilterEvent::Preset(event)) => Some(format!("{event:?}")),
            _ => None,
        })
        .collect()
}

fn one(event: &Event) -> Vec<String> {
    vec![format!("{event:?}")]
}

/// Runs `task` against `laid`, answering its widget operations.
fn drive(laid: &mut crate::layout_probe::Laid<'_, Message>, task: Task<Message>) {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return;
    };
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

/// What `event` publishes while the control `id` has focus.
fn on_focused(app: &App, id: &str, event: &iced::Event) -> Vec<Message> {
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    drive(
        &mut laid,
        iced::widget::operation::focus(iced::widget::Id::from(id.to_owned())),
    );
    assert_eq!(laid.focused(), [iced::widget::Id::from(id.to_owned())]);
    laid.event(event)
}

/// Every focusable control's id and bounds, laid out in the app's window.
fn controls(app: &App) -> Vec<(iced::widget::Id, Rectangle)> {
    type Found = std::sync::Arc<std::sync::Mutex<Vec<(iced::widget::Id, Rectangle)>>>;
    struct Collect(Found);
    impl iced::advanced::widget::Operation for Collect {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if let Some(id) = id {
                self.0.lock().unwrap().push((id.clone(), bounds));
            }
        }
    }
    let found = Found::default();
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    laid.run(Box::new(Collect(found.clone())));
    found.lock().unwrap().clone()
}

fn bounds(controls: &[(iced::widget::Id, Rectangle)], id: &str) -> Rectangle {
    let wanted = iced::widget::Id::from(id.to_owned());
    controls
        .iter()
        .find(|(found, _)| *found == wanted)
        .unwrap_or_else(|| panic!("{id} is laid out"))
        .1
}

#[test]
fn presets_load_at_launch_a_to_z() {
    let (_root, app) = booted(FILE);
    assert_eq!(names(&app), ["Couch co-op", "Tools"]);
    assert!(app.presets.active.is_none());
    assert_eq!(button_label(&app), "Filters");
}

/// Decisions 14 and 16: one click sets the filters, Show hidden, the sort
/// and its direction, Favorites first, grouping, and layout, all in memory.
#[test]
fn applying_a_preset_sets_the_whole_view_and_writes_nothing() {
    let (_root, mut app) = booted(FILE);
    let before = file(&app);
    let zoom = app.zoom;
    app.marked
        .extend(["balatro".to_owned(), "papers-please".to_owned()]);
    let _ = app.update(Message::SearchChanged("p".into()));
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));

    assert_eq!(app.filters.stores, ["GOG", "Steam"].into_iter().collect());
    assert_eq!(app.filters.tags, ["game".to_owned()].into());
    assert_eq!(app.filters.not_tags, ["rpg".to_owned()].into());
    assert!(app.filters.show_hidden);
    assert_eq!(app.query.sort, SortKey::LastPlayed);
    assert!(app.query.descending);
    assert!(app.query.favorites_first);
    assert_eq!(app.group, GroupBy::Tag);
    assert_eq!(app.layout, ViewLayout::List);
    // Search and zoom stay, and marks keep only what's still shown.
    assert_eq!(app.search, "p");
    assert_eq!(app.zoom, zoom);
    assert_eq!(shown(&app), ["papers-please"]);
    assert_eq!(app.marked, ["papers-please".to_owned()].into());
    // The popup closes, and the button names the preset.
    assert!(!app.filter_menu);
    assert_eq!(button_label(&app), "Filters (Couch co-op)");
    assert!(!app.view_writes.pending());
    assert_eq!(file(&app), before);
}

#[test]
fn applying_a_preset_never_reads_the_order_before_rebuilding_it() {
    // A debug run of the app stops on a miss, so the layout switch has to
    // find its place before the filters, sort, and group change, and lay
    // out the order rebuilt after.
    let (_root, mut app) = booted(FILE);
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    open_filters(&mut app);
    let misses = app.order.misses.get();
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));
    assert_eq!(app.layout, ViewLayout::List);
    assert_eq!(app.group, GroupBy::Tag);
    assert_eq!(app.order.misses.get(), misses);
}

#[test]
fn a_preset_that_switches_the_layout_holds_the_card_as_it_was_drawn() {
    // The held card is where the selection was drawn before the preset
    // changed the order, not wherever the new order puts something.
    let (_root, mut app) =
        booted("schema_version = 1\n\n[presets.Backwards]\ndescending = true\nlayout = \"list\"\n");
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    let cards = (0..60)
        .map(|index| crate::tests::card(&format!("game-{index:02}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.snap_grid();
    let places = app.grid_places();
    let index = places.first("game-20").unwrap();
    app.scroll_offset = (places.slots[index].y - 300.0).max(0.0);
    app.selected = Some("game-20".into());
    open_filters(&mut app);
    let (held, _) = app.find_anchor();
    let (held, offset) = held.expect("the selection is in view");
    assert_eq!(held.name(), "game-20");
    let _ = send(&mut app, Event::Apply("Backwards".into()));
    assert_eq!(app.layout, ViewLayout::List);
    let places = app.grid_places();
    let index = places.index(&held).unwrap();
    let now = places.slots[index].y - app.scroll_offset;
    assert!((now - offset).abs() < 0.5, "{now} {offset}");
}

#[test]
fn a_preset_whose_held_card_is_gone_lands_the_selection_from_where_it_was() {
    // Ungrouping moves the held card from rpg to All, so its slot is gone,
    // and the selection lands from the old place, as a regroup does.
    let (_root, mut app) = booted("schema_version = 1\n\n[presets.Flat]\nlayout = \"list\"\n");
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    app.group = GroupBy::Tag;
    let cards = (0..60)
        .map(|index| {
            let mut card = crate::tests::card(&format!("game-{index:02}"), State::Inactive, 60);
            card.tags = vec!["rpg".into()];
            card
        })
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.snap_grid();
    let places = app.grid_places();
    let index = places.first("game-50").unwrap();
    let before = (places.slots[index].y - 300.0).max(0.0);
    app.scroll_offset = before;
    app.selected = Some("game-50".into());
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Flat".into()));
    assert_eq!(app.layout, ViewLayout::List);
    assert_eq!(app.group, GroupBy::None);
    let places = app.grid_places();
    let card = places.slots[places.first("game-50").unwrap()];
    let visible = app.viewport_height - app.bar_room();
    let top = app.scroll_offset;
    assert!(card.y >= top && card.y + card.height <= top + visible);
    // From the top it would have landed with the card on the bottom edge.
    let from_top = card.y + card.height - visible;
    assert!(
        (top - before).abs() < (from_top - before).abs(),
        "{top} {from_top} {before}"
    );
}

#[test]
fn a_preset_can_group_by_recently_played_and_saves_that_group() {
    let (_root, mut app) = booted("schema_version = 1\n\n[presets.Recents]\ngroup = \"recent\"\n");
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Recents".into()));
    assert_eq!(app.group, GroupBy::Recent);
    // Nothing in the fixture library is played, so All is the one section.
    let keys: Vec<_> = app
        .grid_places()
        .headers
        .iter()
        .map(|place| place.header.key.clone())
        .collect();
    assert_eq!(keys, [crate::slot::SectionKey::All]);
    assert_eq!(button_label(&app), "Filters (Recents)");

    // Saving the view as a preset keeps the group.
    let saved = preset_of(app.query, &app.filters, app.group, app.layout);
    assert_eq!(saved.group, GroupBy::Recent);
    wineo_core::library::set_preset(&app.paths, "Mine", &saved).unwrap();
    let text = file(&app);
    assert!(text.contains("group = \"recent\""), "{text}");
    assert_eq!(
        wineo_core::library::load(&app.paths)
            .preset("Mine")
            .unwrap()
            .group,
        GroupBy::Recent
    );
}

#[test]
fn applying_a_preset_gives_the_filters_button_focus() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let task = send(&mut app, Event::Apply("Tools".into()));
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    drive(&mut laid, task);
    assert_eq!(laid.focused(), [iced::widget::Id::new(FILTER_BUTTON_ID)]);
    assert_eq!(shown(&app), ["notepad"]);
}

/// Decision 16: a control changed after applying still saves its own field,
/// and only that one.
#[test]
fn a_control_changed_after_applying_saves_only_its_field() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));
    flip(&mut app);
    assert_eq!(app.view_writes.descending, Some(false));
    let _ = finish_direction(&mut app);
    let view = wineo_core::library::load(&app.paths).view;
    assert!(!view.descending);
    // The preset's sort, grouping, and layout weren't saved with it.
    assert_eq!(view.sort, SortKey::Name);
    assert_eq!(view.group, GroupBy::None);
    assert_eq!(view.layout, ViewLayout::Grid);
}

/// Decision 16: the next launch opens the saved view, not the preset.
#[test]
fn the_next_launch_opens_the_saved_view() {
    let (root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));
    let next = App::boot(paths(&root)).0;
    assert_eq!(next.filters.active(), 0);
    assert!(!next.filters.show_hidden);
    assert_eq!(next.layout, ViewLayout::Grid);
    assert_eq!(next.group, GroupBy::None);
    assert!(next.presets.active.is_none());
    assert_eq!(names(&next), ["Couch co-op", "Tools"]);
}

#[test]
fn a_view_write_a_preset_overtook_does_not_follow_up_with_the_preset() {
    let (_root, mut app) = booted(FILE);
    // A direction write is in flight when the preset lands.
    flip(&mut app);
    assert_eq!(app.view_writes.descending, Some(true));
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    assert!(!app.query.descending);
    let _ = finish_direction(&mut app);
    assert!(!app.view_writes.pending());
    assert!(wineo_core::library::load(&app.paths).view.descending);

    // A control changed while the write is in flight still follows up.
    flip(&mut app);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));
    flip(&mut app);
    let _ = finish_direction(&mut app);
    assert_eq!(app.view_writes.descending, Some(false));
}

/// A tag no game has and ids this build doesn't know cost only themselves
/// (decision 14).
#[test]
fn a_preset_with_an_unknown_id_or_a_missing_tag_applies_the_rest() {
    let (_root, mut app) = booted(
        r#"schema_version = 1

[presets.Odd]
sort = "playtime"
stores = ["itch", "gog"]
platforms = ["amiga"]
not_platforms = ["beos", "native"]
tags = ["co-op"]
layout = "list"
"#,
    );
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Odd".into()));
    assert_eq!(app.filters.stores, ["GOG"].into());
    assert!(app.filters.platforms.is_empty());
    assert_eq!(app.filters.not_platforms, ["Native"].into());
    assert_eq!(app.filters.tags, ["co-op".to_owned()].into());
    assert_eq!(app.query.sort, SortKey::Playtime);
    assert_eq!(app.layout, ViewLayout::List);
    // The missing tag stays offered so its filter can be turned off.
    assert!(app.offered().tags.contains(&"co-op".to_owned()));
    assert!(shown(&app).is_empty());
    assert_eq!(button_label(&app), "Filters (Odd)");
    assert!(app.preset_to_update().is_none());
}

#[test]
fn a_value_a_preset_both_includes_and_excludes_is_left_out_and_still_matches() {
    let (_root, mut app) = booted(
        r#"schema_version = 1

[presets.Both]
tags = ["rpg", "game"]
not_tags = ["rpg"]
stores = ["steam"]
not_stores = ["steam"]
"#,
    );
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Both".into()));
    assert_eq!(app.filters.tags, ["game".to_owned()].into());
    assert_eq!(app.filters.not_tags, ["rpg".to_owned()].into());
    assert!(app.filters.stores.is_empty());
    assert_eq!(app.filters.not_stores, ["Steam"].into());
    assert_eq!(button_label(&app), "Filters (Both)");
    assert!(app.preset_to_update().is_none());
}

/// Decision 15: Update shows only while the view differs from the active
/// preset, and the button names the preset only while it matches.
#[test]
fn update_shows_only_while_the_view_differs_from_the_active_preset() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    assert!(!app.preset_ids().contains(&UPDATE_ID.to_owned()));
    let _ = send(&mut app, Event::Apply("Tools".into()));
    open_filters(&mut app);
    assert!(!app.preset_ids().contains(&UPDATE_ID.to_owned()));

    for change in [
        FilterEvent::ToggleStore("Steam"),
        FilterEvent::ExcludeTag("game".into()),
        FilterEvent::ToggleShowHidden,
    ] {
        let _ = filter(&mut app, change.clone());
        assert_eq!(app.preset_to_update(), Some("Tools"));
        assert!(app.preset_ids().contains(&UPDATE_ID.to_owned()));
        assert_ne!(button_label(&app), "Filters (Tools)");
        // Undoing it matches again.
        let _ = filter(&mut app, change);
        assert!(app.preset_to_update().is_none());
        assert_eq!(button_label(&app), "Filters (Tools)");
    }
    // The sort and layout count too.
    flip(&mut app);
    assert_eq!(app.preset_to_update(), Some("Tools"));
    flip(&mut app);
    assert!(app.preset_to_update().is_none());
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    let _ = app.update(Message::Layout(crate::layout_switch::Event::Toggle));
    assert_eq!(app.preset_to_update(), Some("Tools"));
}

/// Decision 15: Save as preset names the view, which becomes the active
/// preset.
#[test]
fn saving_the_view_under_a_new_name() {
    let (_root, mut app) = booted(FILE);
    let _ = filter(&mut app, FilterEvent::ToggleTag("Indie".into()));
    flip(&mut app);
    let _ = finish_direction(&mut app);
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    assert_eq!(app.presets.edit, Some(Edit::Save(String::new())));
    assert!(app.preset_ids().contains(&NAME_ID.to_owned()));
    assert!(!app.preset_ids().contains(&SAVE_ID.to_owned()));
    let _ = send(&mut app, Event::Draft("  Night owl ".into()));
    let _ = send(&mut app, Event::Submit);
    // The write runs off the UI thread.
    assert!(!file(&app).contains("Night owl"));
    let view = app.view_preset();
    assert_eq!(
        app.presets.writing,
        Some(Write::Create("Night owl".into(), Box::new(view.clone())))
    );
    let _ = finish(&mut app);
    assert!(app.presets.writing.is_none());
    assert_eq!(
        wineo_core::library::load(&app.paths).preset("Night owl"),
        Some(&view)
    );
    assert_eq!(names(&app), ["Couch co-op", "Night owl", "Tools"]);
    assert_eq!(app.presets.active.as_deref(), Some("Night owl"));
    assert_eq!(button_label(&app), "Filters (Night owl)");
    assert!(app.presets.edit.is_none());
    assert!(file(&app).contains("# Kept through every edit."));
}

#[test]
fn a_bad_name_stays_in_the_field_and_enter_does_nothing() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let before = file(&app);
    for draft in ["   ".to_owned(), "x".repeat(61)] {
        let _ = send(&mut app, Event::Draft(draft.clone()));
        let _ = send(&mut app, Event::Submit);
        assert!(app.presets.writing.is_none());
        assert_eq!(app.presets.edit, Some(Edit::Save(draft.clone())));
        assert!(library::normalize_preset_name(&draft).is_err());
        let _ = app.view();
    }
    assert_eq!(file(&app), before);
}

/// Decision 19: saving as a taken name offers Update "X" instead, and a
/// name differing only in case counts as taken.
#[test]
fn a_taken_name_offers_update_instead() {
    let (_root, mut app) = booted(FILE);
    let _ = filter(&mut app, FilterEvent::ToggleTag("Indie".into()));
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("couch CO-OP".into()));
    let _ = send(&mut app, Event::Submit);
    assert!(app.presets.writing.is_none());
    assert_eq!(app.save_taken("couch CO-OP"), Some("Couch co-op"));
    assert!(app.preset_ids().contains(&OFFER_ID.to_owned()));
    let _ = app.view();

    let messages = on_focused(&app, OFFER_ID, &press(Named::Enter));
    assert_eq!(
        preset_events(&messages),
        one(&Event::Update("Couch co-op".into()))
    );
    let _ = send(&mut app, Event::Update("Couch co-op".into()));
    let view = app.view_preset();
    let _ = finish(&mut app);
    assert_eq!(
        wineo_core::library::load(&app.paths).preset("Couch co-op"),
        Some(&view)
    );
    assert_eq!(names(&app), ["Couch co-op", "Tools"]);
    assert_eq!(app.presets.active.as_deref(), Some("Couch co-op"));
    assert!(app.presets.edit.is_none());
    assert!(file(&app).contains("# Kept through every edit."));
}

/// An entry under `[presets]` that isn't a preset holds its name, but Update
/// would be refused too, so none is offered.
#[test]
fn a_name_a_non_preset_entry_holds_is_taken_without_an_update_offer() {
    let text = format!("{FILE}\n[presets]\n\"\u{3c2}b\" = 1\nbroken = 1\n");
    let (_root, mut app) = booted(&text);
    assert_eq!(names(&app), ["Couch co-op", "Tools"]);
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    for (draft, held) in [
        ("broken", "broken"),
        ("BROKEN", "broken"),
        ("\u{3a3}B", "\u{3c2}b"),
    ] {
        let _ = send(&mut app, Event::Draft(draft.into()));
        assert_eq!(app.save_taken(draft), Some(held), "{draft}");
        assert!(app.update_offer(draft).is_none(), "{draft}");
        assert!(!app.preset_ids().contains(&OFFER_ID.to_owned()), "{draft}");
        let _ = send(&mut app, Event::Submit);
        assert!(app.presets.writing.is_none(), "{draft}");
        let _ = app.view();
    }
    assert_eq!(app.rename_taken("Tools", "Broken"), Some("broken"));
    // A real preset still offers Update.
    let _ = send(&mut app, Event::Draft("tools".into()));
    assert_eq!(app.update_offer("tools"), Some("Tools"));
    assert!(app.preset_ids().contains(&OFFER_ID.to_owned()));
    assert_eq!(file(&app), text);
}

#[test]
fn update_saves_the_view_over_the_active_preset() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = filter(&mut app, FilterEvent::ToggleShowHidden);
    open_filters(&mut app);
    assert_eq!(app.preset_to_update(), Some("Tools"));
    let _ = send(&mut app, Event::Update("Tools".into()));
    let _ = finish(&mut app);
    let saved = wineo_core::library::load(&app.paths);
    assert!(saved.preset("Tools").unwrap().show_hidden);
    assert!(app.preset_to_update().is_none());
    assert_eq!(button_label(&app), "Filters (Tools)");
}

/// Decision 19: a rename to a taken name is refused with core's message,
/// whether the window knew of that name or core found it.
#[test]
fn a_rename_to_a_taken_name_is_refused_with_the_core_message() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = send(&mut app, Event::Draft("couch co-op".into()));
    assert_eq!(
        app.rename_taken("Tools", "couch co-op"),
        Some("Couch co-op")
    );
    let _ = send(&mut app, Event::Submit);
    assert!(app.presets.writing.is_none());
    let _ = app.view();

    // Another window saved Late since this one last read the file.
    library::set_preset(&app.paths, "Late", &Preset::default()).unwrap();
    let _ = send(&mut app, Event::Draft("Late".into()));
    let _ = send(&mut app, Event::Submit);
    assert_eq!(
        app.presets.writing,
        Some(Write::Rename("Tools".into(), "Late".into()))
    );
    let _ = finish(&mut app);
    assert_eq!(
        app.presets
            .failure
            .as_ref()
            .map(|failure| failure.text.as_str()),
        Some("Couldn't rename \"Tools\": The name Late is taken.")
    );
    // The typed name stays for another try, and nothing moved.
    assert_eq!(
        app.presets.edit,
        Some(Edit::Rename {
            old: "Tools".into(),
            draft: "Late".into()
        })
    );
    assert_eq!(names(&app), ["Couch co-op", "Late", "Tools"]);
    let _ = app.view();
}

/// Decision 19: the same name writes nothing; a change of case renames.
#[test]
fn a_rename_to_the_same_name_writes_nothing_and_a_case_change_renames() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    open_filters(&mut app);
    let before = file(&app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = send(&mut app, Event::Draft(" Tools ".into()));
    let _ = send(&mut app, Event::Submit);
    assert!(app.presets.writing.is_none());
    assert!(app.presets.edit.is_none());
    assert_eq!(file(&app), before);

    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = send(&mut app, Event::Draft("tools".into()));
    assert!(app.rename_taken("Tools", "tools").is_none());
    let _ = send(&mut app, Event::Submit);
    let _ = finish(&mut app);
    assert_eq!(names(&app), ["Couch co-op", "tools"]);
    // The active preset's label follows the rename.
    assert_eq!(app.presets.active.as_deref(), Some("tools"));
    assert_eq!(button_label(&app), "Filters (tools)");
    assert!(app.presets.edit.is_none());
}

/// Deleting asks first, as deleting a custom theme does.
#[test]
fn deleting_asks_first_and_keep_it_keeps_the_preset() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    open_filters(&mut app);
    let before = file(&app);
    let _ = send(&mut app, Event::AskDelete("Tools".into()));
    let ids = app.preset_ids();
    let row = ids.iter().position(|id| *id == row_id("Tools")).unwrap();
    assert_eq!(ids[row + 1..row + 3], [KEEP_ID, CONFIRM_ID]);
    let _ = app.view();
    let _ = send(&mut app, Event::Keep);
    assert!(app.presets.edit.is_none());
    assert_eq!(file(&app), before);

    let _ = send(&mut app, Event::AskDelete("Tools".into()));
    let _ = send(&mut app, Event::Delete("Tools".into()));
    let _ = finish(&mut app);
    assert_eq!(names(&app), ["Couch co-op"]);
    assert!(app.presets.active.is_none());
    assert_eq!(button_label(&app), "Filters (1)");
    // The view stays as it was.
    assert_eq!(app.filters.tags, ["tool".to_owned()].into());
    assert!(file(&app).contains("# Kept through every edit."));
}

/// Decision 19: a failed write keeps the view, the label, and the typed
/// name, and says why, offering Checkup when it can repair the file.
#[test]
fn a_failed_save_keeps_the_typed_name_and_says_why() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = filter(&mut app, FilterEvent::ToggleShowHidden);
    let filters = app.filters.clone();
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("Night owl".into()));
    let _ = send(&mut app, Event::Submit);
    // The file broke after the window last read it.
    write_file(&app.paths, "schema_version = [");
    let _ = finish(&mut app);
    let failure = app.presets.failure.clone().expect("the save failed");
    assert!(
        failure.text.starts_with("Couldn't save \"Night owl\": "),
        "{failure:?}"
    );
    assert!(failure.checkup);
    assert_eq!(app.presets.edit, Some(Edit::Save("Night owl".into())));
    assert!(app.preset_ids().contains(&CHECKUP_ID.to_owned()));
    assert_eq!(app.filters, filters);
    assert_eq!(app.presets.active.as_deref(), Some("Tools"));
    let _ = app.view();

    // Closed and opened again, the name is still there.
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    open_filters(&mut app);
    assert_eq!(app.presets.edit, Some(Edit::Save("Night owl".into())));
    // Typing clears the old reason.
    let _ = send(&mut app, Event::Draft("Night owls".into()));
    assert!(app.presets.failure.is_none());

    let _ = send(&mut app, Event::Submit);
    let _ = finish(&mut app);
    assert!(app.presets.failure.is_some());
    let _ = send(&mut app, Event::OpenCheckup);
    assert!(!app.filter_menu);
    assert_eq!(app.section, crate::sidebar::Section::Checkup);
}

#[test]
fn a_failure_with_the_popup_closed_shows_a_notice() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("Night owl".into()));
    let _ = send(&mut app, Event::Submit);
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    write_file(&app.paths, "schema_version = [");
    let _ = finish(&mut app);
    let (notice, _) = app.feature_notice.clone().expect("a notice");
    assert!(
        notice.starts_with("Couldn't save \"Night owl\": "),
        "{notice}"
    );
    open_filters(&mut app);
    assert_eq!(app.presets.edit, Some(Edit::Save("Night owl".into())));
}

/// An unreadable `library.toml` refuses before anything is written, as the
/// favorite toggle does.
#[test]
fn an_unreadable_library_refuses_every_write() {
    let (_root, mut app) = booted(FILE);
    write_file(&app.paths, "schema_version = [");
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    let before = file(&app);
    open_filters(&mut app);
    for (start, lead) in [
        (Event::StartSave, "Couldn't save \"Fresh\": "),
        (Event::Rename("Tools".into()), "Couldn't rename \"Tools\": "),
    ] {
        let _ = send(&mut app, start);
        let _ = send(&mut app, Event::Draft("Fresh".into()));
        let _ = send(&mut app, Event::Submit);
        assert!(app.presets.writing.is_none());
        let failure = app.presets.failure.clone().expect("refused");
        assert!(failure.text.starts_with(lead), "{failure:?}");
        assert!(failure.checkup);
        let _ = app.view();
    }
    let _ = send(&mut app, Event::Update("Tools".into()));
    assert!(app.presets.writing.is_none());
    let _ = send(&mut app, Event::Delete("Tools".into()));
    assert!(app.presets.writing.is_none());
    assert_eq!(file(&app), before);
}

#[test]
fn a_preset_saved_elsewhere_arrives_with_a_rescan_and_one_removed_stops_being_active() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    library::set_preset(&app.paths, "Elsewhere", &Preset::default()).unwrap();
    library::remove_preset(&app.paths, "Tools").unwrap();
    let scan = crate::rescan::read(&app.paths, &app.shown, false, None);
    let scan = app.shown.current(scan);
    let scan = app
        .shown
        .changes(scan, jiff::Timestamp::now())
        .expect("the presets changed");
    assert!(scan.presets.is_some());
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    assert_eq!(names(&app), ["Couch co-op", "Elsewhere"]);
    assert!(app.presets.active.is_none());
    // The next one has nothing new about them.
    let scan = crate::rescan::read(&app.paths, &app.shown, false, None);
    let scan = app.shown.current(scan);
    assert!(
        app.shown
            .changes(scan, jiff::Timestamp::now())
            .is_none_or(|scan| scan.presets.is_none())
    );
}

/// Decision 15: the presets sit above the filters, one Up away from where
/// the popup opens, and Tab runs through them in order.
#[test]
fn the_presets_come_first_in_the_popup_keys() {
    let (_root, mut app) = booted(FILE);
    let open = filter(&mut app, FilterEvent::ToggleMenu);
    let mut opened = Vec::new();
    for message in answers(&app, open) {
        opened.push(app.update(message));
    }
    assert!(app.filter_menu_shown());
    let ids = app.popup_ids();
    assert_eq!(
        ids[..4],
        [
            row_id("Couch co-op"),
            row_id("Tools"),
            SAVE_ID.to_owned(),
            "library-filter-store-gog".to_owned(),
        ]
    );
    let up = key(&mut app, Named::ArrowUp);
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    drive(&mut laid, Task::batch(opened));
    assert_eq!(
        laid.focused(),
        [iced::widget::Id::new("library-filter-store-gog")]
    );
    drive(&mut laid, up);
    assert_eq!(laid.focused(), [iced::widget::Id::new(SAVE_ID)]);
}

/// Decision 15: Enter or Space applies a focused preset, R renames it,
/// Delete asks to delete it, and Shift+F10 or a right-click shows both.
#[test]
fn a_focused_preset_row_takes_enter_space_r_delete_and_the_menu_keys() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let row = row_id("Tools");
    let cases: [(iced::Event, Event); 7] = [
        (press(Named::Enter), Event::Apply("Tools".into())),
        (press(Named::Space), Event::Apply("Tools".into())),
        (
            char_key("r", Modifiers::empty()),
            Event::Rename("Tools".into()),
        ),
        (
            char_key("R", Modifiers::SHIFT),
            Event::Rename("Tools".into()),
        ),
        (press(Named::Delete), Event::AskDelete("Tools".into())),
        (
            key_event(Key::Named(Named::F10), Modifiers::SHIFT, None),
            Event::Actions("Tools".into()),
        ),
        (
            key_event(Key::Named(Named::ContextMenu), Modifiers::empty(), None),
            Event::Actions("Tools".into()),
        ),
    ];
    for (event, expected) in cases {
        let messages = on_focused(&app, &row, &event);
        assert_eq!(preset_events(&messages), one(&expected), "{event:?}");
    }
    // A command key leaves R alone, and X has nothing to exclude here.
    for event in [
        char_key("r", Modifiers::CTRL),
        char_key("x", Modifiers::empty()),
    ] {
        let messages = on_focused(&app, &row, &event);
        assert!(preset_events(&messages).is_empty(), "{event:?}");
    }

    let target = bounds(&controls(&app), &row);
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    let messages = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
            iced::mouse::Button::Right,
        )),
        iced::mouse::Cursor::Available(target.center()),
    );
    assert_eq!(
        preset_events(&messages),
        one(&Event::Actions("Tools".into()))
    );
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    assert!(app.filter_menu);
    assert_eq!(app.presets.edit, Some(Edit::Actions("Tools".into())));
}

#[test]
fn the_action_rows_and_the_delete_question_answer_from_the_keyboard() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Actions("Tools".into()));
    let ids = app.preset_ids();
    let row = ids.iter().position(|id| *id == row_id("Tools")).unwrap();
    assert_eq!(ids[row + 1..row + 3], [RENAME_ID, DELETE_ID]);
    let messages = on_focused(&app, RENAME_ID, &press(Named::Enter));
    assert_eq!(
        preset_events(&messages),
        one(&Event::Rename("Tools".into()))
    );
    let messages = on_focused(&app, DELETE_ID, &press(Named::Space));
    assert_eq!(
        preset_events(&messages),
        one(&Event::AskDelete("Tools".into()))
    );
    let _ = send(&mut app, Event::AskDelete("Tools".into()));
    let messages = on_focused(&app, KEEP_ID, &press(Named::Enter));
    assert_eq!(preset_events(&messages), one(&Event::Keep));
    let messages = on_focused(&app, CONFIRM_ID, &press(Named::Enter));
    assert_eq!(
        preset_events(&messages),
        one(&Event::Delete("Tools".into()))
    );
}

#[test]
fn enter_in_the_name_field_submits_and_r_types_into_it() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    assert_eq!(app.preset_ids()[1], NAME_ID);
    let messages = on_focused(&app, NAME_ID, &press(Named::Enter));
    assert_eq!(preset_events(&messages), one(&Event::Submit));
    let typed = key_event(Key::Character("r".into()), Modifiers::empty(), Some("r"));
    let messages = on_focused(&app, NAME_ID, &typed);
    assert_eq!(
        preset_events(&messages),
        one(&Event::Draft("Toolsr".into()))
    );
}

/// Escape closes an edit first, giving focus back where it started, and
/// only then the popup.
#[test]
fn escape_closes_an_edit_before_the_popup() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    for start in [
        Event::Rename("Tools".into()),
        Event::Actions("Tools".into()),
        Event::AskDelete("Tools".into()),
        Event::StartSave,
    ] {
        let back = if matches!(start, Event::StartSave) {
            SAVE_ID.to_owned()
        } else {
            row_id("Tools")
        };
        let _ = send(&mut app, start);
        let task = key(&mut app, Named::Escape);
        assert!(app.presets.edit.is_none());
        assert!(app.filter_menu);
        let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
        drive(&mut laid, task);
        assert_eq!(laid.focused(), [iced::widget::Id::from(back)]);
    }
    let _ = key(&mut app, Named::Escape);
    assert!(!app.filter_menu);
}

#[test]
fn opening_the_popup_starts_the_presets_afresh() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    open_filters(&mut app);
    assert!(app.presets.edit.is_none());
    // Nothing in it acts once the popup is gone.
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = send(&mut app, Event::StartSave);
    assert!(app.presets.active.is_none());
    assert!(app.presets.edit.is_none());
}

#[test]
fn the_presets_fit_the_popup_at_the_narrowest_window() {
    let long = "A very long preset name that goes on and on for sixty chars!";
    assert_eq!(long.chars().count(), 60);
    let (_root, mut app) = booted(&format!(
        "{FILE}\n[presets.\"{long}\"]\ntags = [\"game\"]\n"
    ));
    app.window_size = iced::Size::new(300.0, 440.0);
    open_filters(&mut app);
    let menu = crate::grid::menu_position(app.filter_menu_position, app.window_size, 0.0);
    let (left, right) = (menu.x, menu.x + ACTION_MENU_WIDTH);
    assert!(left >= 0.0 && right <= 300.0, "{left} {right}");
    let check = |app: &App| {
        let controls = controls(app);
        for id in app.preset_ids() {
            let found = bounds(&controls, &id);
            assert!(
                found.x >= left + ACTION_MENU_PADDING - 0.5
                    && found.x + found.width <= right - ACTION_MENU_PADDING + 0.5,
                "{id}: {found:?} in {left}..{right}"
            );
        }
    };
    check(&app);
    let _ = send(&mut app, Event::Actions(long.into()));
    check(&app);
    let _ = send(&mut app, Event::AskDelete(long.into()));
    check(&app);
    let _ = send(&mut app, Event::Rename(long.into()));
    check(&app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft(long.to_lowercase()));
    check(&app);

    // The button's label keeps a long name short.
    let _ = send(&mut app, Event::Apply(long.into()));
    let label = button_label(&app);
    assert!(
        label.starts_with("Filters (A very") && label.ends_with(')'),
        "{label}"
    );
    let width = crate::tile_title::line_width(&label, 14.0, Font::DEFAULT);
    let frame = crate::tile_title::line_width("Filters ()", 14.0, Font::DEFAULT);
    assert!(width <= frame + BUTTON_NAME + 1.0, "{label}: {width}");
}

#[test]
fn the_popup_draws_every_presets_state() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = filter(&mut app, FilterEvent::ToggleShowHidden);
    open_filters(&mut app);
    for event in [
        Event::Actions("Tools".into()),
        Event::AskDelete("Tools".into()),
        Event::Rename("Tools".into()),
        Event::StartSave,
    ] {
        let _ = send(&mut app, event);
        let _ = crate::layout_probe::layout(&mut app.view(), app.window_size);
    }
}

#[test]
fn a_typed_name_survives_closing_the_popup() {
    let (_root, mut app) = booted(FILE);
    let typed = [
        (Event::StartSave, Edit::Save("Evening".into())),
        (
            Event::Rename("Tools".into()),
            Edit::Rename {
                old: "Tools".into(),
                draft: "Evening".into(),
            },
        ),
    ];
    for (start, edit) in typed {
        open_filters(&mut app);
        let _ = send(&mut app, start);
        let _ = send(&mut app, Event::Draft("Evening".into()));
        let _ = filter(&mut app, FilterEvent::CloseMenu);
        open_filters(&mut app);
        assert_eq!(app.presets.edit, Some(edit));
        assert!(app.preset_ids().contains(&NAME_ID.to_owned()));
        let _ = key(&mut app, Named::Escape);
        let _ = filter(&mut app, FilterEvent::CloseMenu);
    }
}

#[test]
fn a_failed_save_keeps_its_name_after_it_is_edited_and_the_popup_closes() {
    let (_root, mut app) = booted(FILE);
    write_file(&app.paths, "schema_version = [");
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("Evening".into()));
    let _ = send(&mut app, Event::Submit);
    assert!(app.presets.failure.is_some());
    let _ = send(&mut app, Event::Draft("Evenings".into()));
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    open_filters(&mut app);
    assert_eq!(app.presets.edit, Some(Edit::Save("Evenings".into())));
}

#[test]
fn a_typed_name_keeps_only_its_own_failure_when_the_popup_opens_again() {
    let (_root, mut app) = booted(FILE);
    write_file(&app.paths, "schema_version = [");
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("Evening".into()));
    let _ = send(&mut app, Event::Submit);
    let failed = app.presets.failure.clone();
    assert!(failed.is_some());
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    open_filters(&mut app);
    assert_eq!(app.presets.failure, failed);

    // An Update of another preset fails with the name still typed.
    let _ = send(&mut app, Event::Update("Tools".into()));
    assert_ne!(app.presets.failure, failed);
    assert!(app.presets.failure.is_some());
    let _ = filter(&mut app, FilterEvent::CloseMenu);
    open_filters(&mut app);
    assert!(app.presets.failure.is_none());
    assert_eq!(app.presets.edit, Some(Edit::Save("Evening".into())));
}

#[test]
fn a_failed_delete_or_update_is_forgotten_when_the_popup_opens_again() {
    let (_root, mut app) = booted(FILE);
    write_file(&app.paths, "schema_version = [");
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    for start in [
        Event::AskDelete("Tools".into()),
        Event::Update("Tools".into()),
    ] {
        open_filters(&mut app);
        let _ = send(&mut app, start);
        if let Some(Edit::Delete(name)) = app.presets.edit.clone() {
            let _ = send(&mut app, Event::Delete(name));
        }
        assert!(app.presets.failure.is_some());
        let _ = filter(&mut app, FilterEvent::CloseMenu);
        open_filters(&mut app);
        assert!(app.presets.failure.is_none());
        assert!(app.presets.edit.is_none());
        let _ = filter(&mut app, FilterEvent::CloseMenu);
    }
}

/// Runs the write in flight on disk, then a rescan that reads it before the
/// write's reply arrives, then the reply.
fn land_with_a_rescan_first(app: &mut App) {
    let write = app.presets.writing.clone().expect("a write in flight");
    let (result, presets) = super::write(&app.paths, &app.shown, &write);
    rescan(app);
    let _ = send(
        app,
        Event::Saved(Box::new(Done {
            write,
            result,
            presets,
        })),
    );
}

fn rescan(app: &mut App) {
    let scan = crate::rescan::read(&app.paths, &app.shown, false, None);
    let scan = app.shown.current(scan);
    if let Some(scan) = app.shown.changes(scan, jiff::Timestamp::now()) {
        let _ = app.update(Message::Rescanned(Box::new(scan)));
    }
}

/// Another window saved the same name after this one last read the file:
/// Save never overwrites it, and shows the name as taken, offering Update.
#[test]
fn a_name_saved_elsewhere_since_the_last_read_is_never_overwritten() {
    let (_root, mut app) = booted(FILE);
    let elsewhere = Preset {
        show_hidden: true,
        ..Preset::default()
    };
    for popup_open in [true, false] {
        let name = if popup_open { "Late" } else { "Later" };
        open_filters(&mut app);
        let _ = send(&mut app, Event::StartSave);
        let _ = send(&mut app, Event::Draft(name.into()));
        library::set_preset(&app.paths, name, &elsewhere).unwrap();
        assert!(app.save_taken(name).is_none());
        let _ = send(&mut app, Event::Submit);
        if !popup_open {
            let _ = filter(&mut app, FilterEvent::CloseMenu);
        }
        let _ = finish(&mut app);
        assert_eq!(
            wineo_core::library::load(&app.paths).preset(name),
            Some(&elsewhere)
        );
        assert!(names(&app).contains(&name));
        assert!(app.presets.active.is_none());
        if popup_open {
            // The same state as a name the list already had.
            assert_eq!(app.save_taken(name), Some(name));
            assert!(app.preset_ids().contains(&OFFER_ID.to_owned()));
            assert!(app.presets.failure.is_none());
        } else {
            let (notice, _) = app.feature_notice.clone().expect("a notice");
            assert_eq!(
                notice,
                format!("Couldn't save \"{name}\": The name {name} is taken.")
            );
            open_filters(&mut app);
            assert!(app.preset_ids().contains(&OFFER_ID.to_owned()));
        }
        assert_eq!(app.presets.edit, Some(Edit::Save(name.into())));
        let _ = app.view();
        let _ = filter(&mut app, FilterEvent::CloseMenu);
        app.presets.edit = None;
        app.presets.failure = None;
    }
}

/// A save that lands after another preset was applied leaves that one active.
#[test]
fn a_save_landing_after_a_later_apply_leaves_the_later_preset_active() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = filter(&mut app, FilterEvent::ToggleShowHidden);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Update("Tools".into()));
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));
    let _ = finish(&mut app);
    assert_eq!(app.presets.active.as_deref(), Some("Couch co-op"));
    assert_eq!(button_label(&app), "Filters (Couch co-op)");
    assert!(
        wineo_core::library::load(&app.paths)
            .preset("Tools")
            .unwrap()
            .show_hidden
    );

    // The same for a new one.
    open_filters(&mut app);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("Night owl".into()));
    let _ = send(&mut app, Event::Submit);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = finish(&mut app);
    assert_eq!(app.presets.active.as_deref(), Some("Tools"));
    assert!(names(&app).contains(&"Night owl"));
}

#[test]
fn a_rename_of_the_active_preset_keeps_it_active_through_a_rescan_before_its_reply() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    open_filters(&mut app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = send(&mut app, Event::Draft("Gadgets".into()));
    let _ = send(&mut app, Event::Submit);
    land_with_a_rescan_first(&mut app);
    assert_eq!(names(&app), ["Couch co-op", "Gadgets"]);
    assert_eq!(app.presets.active.as_deref(), Some("Gadgets"));
    assert_eq!(button_label(&app), "Filters (Gadgets)");
}

#[test]
fn a_preset_removed_elsewhere_stops_being_active_while_another_is_renamed() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Couch co-op".into()));
    open_filters(&mut app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = send(&mut app, Event::Draft("Gadgets".into()));
    let _ = send(&mut app, Event::Submit);
    library::remove_preset(&app.paths, "Couch co-op").unwrap();
    land_with_a_rescan_first(&mut app);
    assert_eq!(names(&app), ["Gadgets"]);
    assert!(app.presets.active.is_none());
}

#[test]
fn a_delete_or_update_of_the_active_preset_survives_a_rescan_before_its_reply() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    let _ = filter(&mut app, FilterEvent::ToggleShowHidden);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Update("Tools".into()));
    land_with_a_rescan_first(&mut app);
    assert_eq!(app.presets.active.as_deref(), Some("Tools"));
    assert_eq!(button_label(&app), "Filters (Tools)");

    let _ = send(&mut app, Event::AskDelete("Tools".into()));
    let _ = send(&mut app, Event::Delete("Tools".into()));
    land_with_a_rescan_first(&mut app);
    assert_eq!(names(&app), ["Couch co-op"]);
    assert!(app.presets.active.is_none());
}

/// Every `[view]` write in flight, run and answered in turn, follow-ups too.
fn finish_view(app: &mut App) {
    loop {
        let writes = &app.view_writes;
        let field = writes
            .sort
            .map(|(sort, descending)| Field::Sort(sort, descending))
            .or(writes.descending.map(Field::Descending))
            .or(writes.favorites_first.map(Field::FavoritesFirst))
            .or(writes.layout.map(Field::Layout))
            .or(writes.group.map(Field::Group));
        let Some(field) = field else {
            return;
        };
        let result = crate::sort_control::save(&app.paths, &app.shown, field);
        let _ = app.update(Message::Sort(crate::sort_control::Event::Saved(
            field, result,
        )));
    }
}

fn sort_event(app: &mut App, event: crate::sort_control::Event) {
    let _ = app.update(Message::Sort(event));
}

/// Decision 16, for each control the preset sets: after applying, a change
/// saves only its own field.
#[test]
fn after_applying_each_control_saves_only_its_own_field() {
    use crate::sort_control::Event as Sort;
    type Check = fn(&wineo_core::library::View) -> bool;
    type Change = fn(&mut App);
    let cases: [(&str, Change, Check); 4] = [
        (
            "sort",
            |app| sort_event(app, Sort::Pick(SortKey::Playtime)),
            |view| view.sort == SortKey::Playtime && view.descending,
        ),
        (
            "favorites first",
            |app| sort_event(app, Sort::ToggleFavoritesFirst),
            |view| !view.favorites_first && view.sort == SortKey::Name,
        ),
        (
            "group",
            |app| sort_event(app, Sort::Group(GroupBy::State)),
            |view| view.group == GroupBy::State && view.sort == SortKey::Name,
        ),
        (
            "layout",
            |app| {
                let _ = app.update(Message::Layout(crate::layout_switch::Event::Toggle));
            },
            |view| view.layout == ViewLayout::Grid && view.sort == SortKey::Name,
        ),
    ];
    for (what, change, check) in cases {
        // Saved away from every default, so a stray write of the preset's
        // value would show.
        let (_root, mut app) = booted(&format!(
            "schema_version = 1\n\n[view]\nfavorites_first = true\nlayout = \"list\"\n\n{}",
            FILE.trim_start_matches("schema_version = 1\n")
        ));
        let before = wineo_core::library::load(&app.paths).view;
        open_filters(&mut app);
        let _ = send(&mut app, Event::Apply("Couch co-op".into()));
        change(&mut app);
        finish_view(&mut app);
        let view = wineo_core::library::load(&app.paths).view;
        assert!(check(&view), "{what}: {view:?}");
        // Nothing else moved from what was saved before.
        let mut expected = before.clone();
        match what {
            "sort" => {
                expected.sort = view.sort;
                expected.descending = view.descending;
            }
            "favorites first" => expected.favorites_first = view.favorites_first,
            "group" => expected.group = view.group,
            _ => expected.layout = view.layout,
        }
        assert_eq!(view, expected, "{what}");
    }
}

/// A write running when a preset lands doesn't follow up with the preset's
/// value, but one a control changed again meanwhile does, for each field.
#[test]
fn an_overtaken_write_follows_up_only_after_another_change_for_every_field() {
    use crate::sort_control::Event as Sort;
    fn toggle_layout(app: &mut App) {
        let _ = app.update(Message::Layout(crate::layout_switch::Event::Toggle));
    }
    type Step = fn(&mut App);
    type Read = fn(&wineo_core::library::View) -> String;
    // Tools is the default view. Each second change ends on a value other
    // than both the first change and Tools', so only a follow-up writes it.
    let cases: [(&str, Step, Step, Read, &str, &str); 4] = [
        (
            "sort",
            |app| sort_event(app, Sort::Pick(SortKey::Playtime)),
            |app| sort_event(app, Sort::Pick(SortKey::Added)),
            |view| format!("{:?}", view.sort),
            "Playtime",
            "Added",
        ),
        (
            "favorites first",
            |app| sort_event(app, Sort::ToggleFavoritesFirst),
            |app| {
                sort_event(app, Sort::ToggleFavoritesFirst);
                sort_event(app, Sort::ToggleFavoritesFirst);
            },
            |view| view.favorites_first.to_string(),
            "true",
            "false",
        ),
        (
            "group",
            |app| sort_event(app, Sort::Group(GroupBy::State)),
            |app| sort_event(app, Sort::Group(GroupBy::Source)),
            |view| format!("{:?}", view.group),
            "State",
            "Source",
        ),
        (
            "layout",
            toggle_layout,
            |app| {
                toggle_layout(app);
                toggle_layout(app);
            },
            |view| format!("{:?}", view.layout),
            "List",
            "Grid",
        ),
    ];
    for (what, first, again, read, overtaken, followed) in cases {
        for change_again in [false, true] {
            let (_root, mut app) = booted(FILE);
            first(&mut app);
            assert!(app.view_writes.pending(), "{what}");
            open_filters(&mut app);
            let _ = send(&mut app, Event::Apply("Tools".into()));
            if change_again {
                again(&mut app);
            }
            finish_view(&mut app);
            let view = wineo_core::library::load(&app.paths).view;
            let expected = if change_again { followed } else { overtaken };
            assert_eq!(read(&view), expected, "{what}, again: {change_again}");
        }
    }
}

/// The focused row's bounds, the popup's scroll offset, and its viewport,
/// laid out in the app's window after `tasks`.
fn focused_in_popup(app: &App, tasks: Vec<Task<Message>>) -> (Rectangle, f32, Rectangle) {
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    for task in tasks {
        drive(&mut laid, task);
    }
    let offset = laid
        .offsets()
        .into_iter()
        .rev()
        .find(|offset| *offset > 0.0)
        .unwrap_or(0.0);
    type Slot = std::sync::Arc<std::sync::Mutex<Option<Rectangle>>>;
    struct Focused(Slot);
    impl iced::advanced::widget::Operation for Focused {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn focusable(
            &mut self,
            _id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if state.is_focused() {
                *self.0.lock().unwrap() = Some(bounds);
            }
        }
    }
    let slot = Slot::default();
    laid.run(Box::new(Focused(slot.clone())));
    let row = slot.lock().unwrap().expect("a control has focus");
    let viewport = crate::layout_probe::probe(&mut app.view(), app.window_size)
        .scrollables
        .into_iter()
        .map(|(viewport, _)| viewport)
        .find(|viewport| viewport.width < ACTION_MENU_WIDTH)
        .expect("the popup scrolls");
    (row, offset, viewport)
}

fn assert_visible(row: Rectangle, offset: f32, viewport: Rectangle, what: &str) {
    let top = row.y - offset;
    assert!(
        top >= viewport.y - 0.5 && top + row.height <= viewport.y + viewport.height + 0.5,
        "{what}: {row:?} offset {offset} in {viewport:?}"
    );
}

#[test]
fn a_popup_with_many_presets_scrolls_to_the_focused_row_and_shows_a_failure() {
    let mut text = FILE.to_owned();
    for index in 0..22 {
        text.push_str(&format!(
            "\n[presets.\"Preset {index:02} with a long name that needs cutting short\"]\ntags = [\"game\"]\n"
        ));
    }
    let (_root, mut app) = booted(&text);
    app.window_size = iced::Size::new(900.0, 420.0);
    assert_eq!(app.presets.list().len(), 24);
    open_filters(&mut app);
    // Up from the first filter row, where the popup opens, walks back
    // through every preset. Keys don't change the app, so each check
    // replays them on a fresh layout.
    let first = "library-filter-store-gog";
    let ups = app.preset_ids().len();
    for count in [1, 6, 12, 18, ups] {
        let mut tasks = vec![crate::library_menu::focus(
            first,
            crate::filter_control::MENU_SCROLL_ID,
        )];
        tasks.extend((0..count).map(|_| key(&mut app, Named::ArrowUp)));
        let (row, offset, viewport) = focused_in_popup(&app, tasks);
        assert_visible(row, offset, viewport, &format!("after {count} Ups"));
    }

    // A refused save shows its reason and Open Checkup at the end of the
    // presets, scrolled into view with the focus.
    write_file(&app.paths, "schema_version = [");
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    let _ = send(&mut app, Event::StartSave);
    let _ = send(&mut app, Event::Draft("Fresh".into()));
    let _ = send(&mut app, Event::Submit);
    assert!(
        app.presets
            .failure
            .as_ref()
            .is_some_and(|failure| failure.checkup)
    );
    let focus = crate::library_menu::focus(CHECKUP_ID, crate::filter_control::MENU_SCROLL_ID);
    let (button, offset, viewport) = focused_in_popup(&app, vec![focus]);
    assert_eq!(
        bounds(&controls(&app), CHECKUP_ID).height,
        button.height,
        "Open Checkup has focus"
    );
    assert_visible(button, offset, viewport, "Open Checkup");
    // The reason sits right above it, in view too.
    let line = Rectangle {
        y: button.y - ACTION_MENU_SPACING - 16.0,
        height: 16.0,
        ..button
    };
    assert_visible(line, offset, viewport, "the reason");
}

/// The taken-name refusal stays on screen when the reread list can't show
/// the name as taken: the file became unreadable, or the taker went again.
#[test]
fn a_taken_name_refusal_stays_when_the_list_cannot_show_it() {
    for unreadable in [true, false] {
        let (_root, mut app) = booted(FILE);
        open_filters(&mut app);
        let _ = send(&mut app, Event::StartSave);
        let _ = send(&mut app, Event::Draft("Late".into()));
        library::set_preset(&app.paths, "Late", &Preset::default()).unwrap();
        let _ = send(&mut app, Event::Submit);
        let write = app.presets.writing.clone().expect("a write in flight");
        let (result, _) = super::write(&app.paths, &app.shown, &write);
        assert!(result.as_ref().is_err_and(|failure| failure.taken));
        let presets = if unreadable {
            None
        } else {
            // Gone again before the list was read.
            library::remove_preset(&app.paths, "Late").unwrap();
            app.shown.refresh_library_presets(&app.paths)
        };
        let _ = send(
            &mut app,
            Event::Saved(Box::new(Done {
                write,
                result,
                presets,
            })),
        );
        let failure = app.presets.failure.clone().expect("the refusal shows");
        assert_eq!(
            failure.text,
            "Couldn't save \"Late\": The name Late is taken."
        );
        assert!(app.save_taken("Late").is_none(), "unreadable: {unreadable}");
        assert_eq!(app.presets.edit, Some(Edit::Save("Late".into())));
        let _ = app.view();
    }
}

/// The rename guard keeps only the preset this window is renaming: one
/// another window deleted first still stops being active.
#[test]
fn renaming_the_active_preset_after_another_window_deleted_it_clears_it() {
    let (_root, mut app) = booted(FILE);
    open_filters(&mut app);
    let _ = send(&mut app, Event::Apply("Tools".into()));
    open_filters(&mut app);
    let _ = send(&mut app, Event::Rename("Tools".into()));
    let _ = send(&mut app, Event::Draft("Gadgets".into()));
    let _ = send(&mut app, Event::Submit);
    library::remove_preset(&app.paths, "Tools").unwrap();
    land_with_a_rescan_first(&mut app);
    assert!(app.presets.active.is_none());
    let on_disk: Vec<String> = wineo_core::library::load(&app.paths)
        .presets
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names(&app), on_disk);
    assert_eq!(names(&app), ["Couch co-op"]);
    assert!(app.presets.failure.is_some());
}
