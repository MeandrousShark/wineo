//! Pages that open global settings at a key row: the import flow's Steam
//! key action and the art page's Settings. The page lands on the row's tab
//! (Stores for the Steam key, General for the SteamGridDB key) before it
//! looks for the row.

use std::sync::{Arc, Mutex};

use iced::Rectangle;
use iced::advanced::widget::{Id, Operation};

use super::*;
use crate::grid_motion::CardBounds;
use crate::layout_probe::{Laid, lay_out};
use crate::theme::Palette;

const SHORT: iced::Size = iced::Size::new(900.0, 300.0);

/// What `task` sends when run on `laid`, leaving out anything that waits.
fn outputs(laid: &mut Laid<'_, Message>, task: Task<Message>) -> Vec<Message> {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = crate::test_tasks::paused_runtime();
    let mut messages = Vec::new();
    crate::test_tasks::each_ready(&runtime, &mut stream, |action| match action {
        iced_runtime::Action::Output(message) => messages.push(message),
        iced_runtime::Action::Widget(operation) => laid.run(operation),
        _ => {}
    });
    messages
}

/// The located answers `task` sends for the page as it shows now.
fn located(page: &Page, paths: &Paths, task: Task<Message>) -> Vec<(Reveal, CardBounds)> {
    let mut laid = lay_out(page.view(paths, Palette::DARK), SHORT);
    outputs(&mut laid, task)
        .into_iter()
        .filter_map(|message| match message {
            Message::Settings(Event::SteamKeyLocated(_, bounds)) => {
                Some((Reveal::SteamKey, bounds))
            }
            Message::Settings(Event::SteamGridDbKeyLocated(_, bounds)) => {
                Some((Reveal::SteamGridDbKey, bounds))
            }
            _ => None,
        })
        .collect()
}

fn answer(generation: u64, reveal: Reveal, bounds: CardBounds) -> Event {
    match reveal {
        Reveal::SteamKey => Event::SteamKeyLocated(generation, bounds),
        Reveal::SteamGridDbKey => Event::SteamGridDbKeyLocated(generation, bounds),
    }
}

/// The page's scroll offset once `task` has run on it.
fn offset_after(page: &Page, paths: &Paths, task: Task<Message>) -> f32 {
    let mut laid = lay_out(page.view(paths, Palette::DARK), SHORT);
    let _ = outputs(&mut laid, task);
    laid.offsets()[0]
}

/// Where the row puts the page: its top 24 px under the viewport's.
fn row_offset(bounds: CardBounds) -> f32 {
    (bounds.card.y - bounds.viewport.y - 24.0).max(0.0)
}

fn text_bounds(page: &Page, paths: &Paths, wanted: &str) -> Rectangle {
    struct Find(String, Arc<Mutex<Option<Rectangle>>>);
    impl Operation for Find {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
            if text == self.0 {
                *self.1.lock().unwrap() = Some(bounds);
            }
        }
    }
    let found = Arc::new(Mutex::new(None));
    let mut laid = lay_out(page.view(paths, Palette::DARK), SHORT);
    laid.run(Box::new(Find(wanted.to_owned(), found.clone())));
    let found = *found.lock().unwrap();
    found.unwrap_or_else(|| panic!("no {wanted}"))
}

#[test]
fn each_reveal_opens_its_tab_and_scrolls_to_its_row() {
    for reveal in [Reveal::SteamKey, Reveal::SteamGridDbKey] {
        let (_root, paths) = setup();
        let mut app = App::boot(paths.clone()).0;
        assert!(app.settings.is_none(), "the page isn't open yet");
        let _ = match reveal {
            Reveal::SteamKey => app.open_steam_key_settings(),
            Reveal::SteamGridDbKey => app.open_settings_at(reveal),
        };
        let page = app.settings.as_ref().unwrap();
        assert!(page.is_global());
        assert_eq!(page.reveal, Some(reveal));
        let generation = page.generation;
        let task = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(&paths),
        )));
        let page = app.settings.as_ref().unwrap();
        let tab = match reveal {
            Reveal::SteamKey => Tab::Stores,
            Reveal::SteamGridDbKey => Tab::General,
        };
        assert_eq!(page.tab, tab, "{reveal:?}");
        assert_eq!(page.reveal, None);
        let found = located(page, &paths, task);
        assert_eq!(found.len(), 1, "{reveal:?}");
        let (kind, bounds) = found[0];
        assert_eq!(kind, reveal);
        let expected = row_offset(bounds);
        assert!(expected > 0.0, "{reveal:?} starts below the top");
        let task = app.update(Message::Settings(answer(generation, reveal, bounds)));
        let page = app.settings.as_ref().unwrap();
        let offset = offset_after(page, &paths, task);
        // The row's top comes near the viewport's, or as near as the end of
        // the page lets it, and the whole row shows.
        assert!(offset > 0.0, "{reveal:?}");
        assert!(offset <= expected + 0.5, "{reveal:?}: {offset} {expected}");
        let (card, viewport) = (bounds.card, bounds.viewport);
        assert!(card.y - offset >= viewport.y, "{reveal:?}");
        assert!(
            card.y + card.height - offset <= viewport.y + viewport.height,
            "{reveal:?}: {card:?} {viewport:?} {offset}"
        );
        if reveal == Reveal::SteamKey {
            // The scrollable rounds its offset to the pixel.
            assert!((offset - expected).abs() < 0.5, "{offset} {expected}");
        }
    }
}

#[test]
fn the_steam_key_id_marks_only_the_key_row() {
    let (_root, paths) = setup();
    let mut page = Page::new_global(1);
    page.reveal = Some(Reveal::SteamKey);
    let _ = page.reload(&paths, Mode::Replace);
    let step = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    let (_, bounds) = located(&page, &paths, step.task)[0];
    // The Steam section's switch is above the row, outside it.
    let switch = text_bounds(&page, &paths, "Import new Steam games automatically");
    assert!(
        switch.y + switch.height <= bounds.card.y,
        "{switch:?} {bounds:?}"
    );
    let steam = text_bounds(&page, &paths, "Steam");
    assert!(steam.y < switch.y);
}

#[test]
fn a_reload_with_a_reveal_moves_another_tab_to_the_rows_tab() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    for (reveal, tab) in [
        (Reveal::SteamGridDbKey, Tab::General),
        (Reveal::SteamKey, Tab::Stores),
    ] {
        let _ = page.update(&paths, Event::Tab(Tab::Appearance));
        page.reveal = Some(reveal);
        let _ = page.reload(&paths, Mode::Replace);
        let step = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        assert_eq!(page.tab, tab, "{reveal:?}");
        let found = located(&page, &paths, step.task);
        assert_eq!(found.len(), 1, "{reveal:?}");
        assert_eq!(found[0].0, reveal);
    }
}

#[test]
fn a_located_row_scrolls_only_while_its_tab_shows() {
    let (_root, paths) = setup();
    for (reveal, home, other) in [
        (Reveal::SteamKey, Tab::Stores, Tab::General),
        (Reveal::SteamGridDbKey, Tab::General, Tab::Stores),
    ] {
        let mut page = Page::new_global(1);
        page.reveal = Some(reveal);
        let _ = page.reload(&paths, Mode::Replace);
        let step = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
        let (_, bounds) = located(&page, &paths, step.task)[0];
        // Switched away before the answer came back.
        let _ = page.update(&paths, Event::Tab(other));
        let step = page.update(&paths, answer(1, reveal, bounds));
        assert_eq!(step.task.units(), 0, "{reveal:?}");
        let _ = page.update(&paths, Event::Tab(home));
        let step = page.update(&paths, answer(1, reveal, bounds));
        assert!(step.task.units() > 0, "{reveal:?}");
        // An answer for a page closed earlier does nothing.
        let step = page.update(&paths, answer(0, reveal, bounds));
        assert_eq!(step.task.units(), 0, "{reveal:?}");
    }
}

#[test]
fn the_steamgriddb_key_row_is_last_in_art_to_fetch_and_not_on_stores() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let texts = page_texts(&page, &paths);
    let at = |wanted: &str| texts.iter().position(|text| text == wanted).unwrap();
    assert!(at("Logo") < at("SteamGridDB key"));
    assert!(at("SteamGridDB key") < at("RetroArch"));
    // Nothing between the row's own pieces and the next section's heading
    // is another switch.
    assert_eq!(texts[at("SteamGridDB key") + 1], "Checking...");
    let _ = page.update(&paths, Event::Tab(Tab::Stores));
    let texts = page_texts(&page, &paths);
    assert!(!texts.iter().any(|text| text == "SteamGridDB key"));
    assert!(!texts.iter().any(|text| text == "SteamGridDB"));
    // The Steam key row keeps its plain label.
    assert!(texts.iter().any(|text| text == "Key"));
}

#[test]
fn the_steamgriddb_key_id_marks_only_the_key_row() {
    let (_root, paths) = setup();
    let mut page = Page::new_global(1);
    page.reveal = Some(Reveal::SteamGridDbKey);
    let _ = page.reload(&paths, Mode::Replace);
    let step = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    let (_, bounds) = located(&page, &paths, step.task)[0];
    // The Logo switch is above the row, outside it.
    let logo = text_bounds(&page, &paths, "Logo");
    assert!(logo.y + logo.height <= bounds.card.y, "{logo:?} {bounds:?}");
    let label = text_bounds(&page, &paths, "SteamGridDB key");
    assert!(label.y >= bounds.card.y, "{label:?} {bounds:?}");
}

#[test]
fn without_a_reveal_the_page_opens_on_general() {
    let (_root, paths) = setup();
    let page = global_page(&paths);
    assert_eq!(page.tab, Tab::General);
    assert_eq!(page.reveal, None);
}
