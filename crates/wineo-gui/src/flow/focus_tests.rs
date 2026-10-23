//! Keyboard focus on the Add choices, through the app's own keys.

use iced::Task;
use iced::advanced::widget::Tree;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::Id;

use super::{CHOICE_IDS, Page};
use crate::focus_button::{focusables, landed, press, press_key, run_task, walk_rows};
use crate::layout_probe::{Laid, relay_out};
use crate::tests::app;
use crate::{App, Message};

fn add() -> (App, Task<Message>) {
    let mut app = app();
    let task = app.update(Message::Add);
    assert!(matches!(app.flow, Some(Page::Choose(_))));
    (app, task)
}

/// The window again after an update, keeping the widget state in `tree`.
fn relaid(app: &App, tree: Tree) -> Laid<'_, Message> {
    relay_out(app.view(), tree, iced::Size::new(1200.0, 800.0))
}

/// Add opened from the keyboard, with Down pressed `downs` times, and the
/// widget state that leaves.
fn add_at(downs: usize) -> (App, Tree) {
    let (app, task) = add();
    let mut laid = landed(&app, task);
    for _ in 0..downs {
        press(&mut laid, Named::ArrowDown, Modifiers::empty());
    }
    let tree = laid.into_tree();
    (app, tree)
}

/// Presses `key` on the window, then hands the app what the widgets
/// published and the key with whether they kept it, as the runtime does.
/// Returns the published messages, the widget state after, and the key's
/// own task. The pages' work isn't run.
fn key(
    app: &mut App,
    tree: Tree,
    key: Key,
    modifiers: Modifiers,
) -> (Vec<String>, Tree, Task<Message>) {
    let mut laid = relaid(app, tree);
    let (messages, status) = press_key(&mut laid, key.clone(), modifiers);
    let tree = laid.into_tree();
    let shown = messages
        .iter()
        .map(|message| format!("{message:?}"))
        .collect();
    for message in messages {
        let _ = app.update(message);
    }
    let task = app.update(Message::Key(key, modifiers, status));
    (shown, tree, task)
}

fn focused(app: &App, tree: Tree) -> (Vec<Id>, Tree) {
    let mut laid = relaid(app, tree);
    (laid.focused(), laid.into_tree())
}

fn opened(app: &App) -> &'static str {
    match app.flow {
        Some(Page::Choose(_)) => "choose",
        Some(Page::New(_)) => "new",
        Some(Page::Prefix(_)) => "prefix",
        Some(Page::Heroic(_)) => "heroic",
        Some(Page::Steam(_)) => "steam",
        Some(Page::Gog(_)) => "gog",
        _ => "other",
    }
}

#[test]
fn add_opens_on_its_first_choice_and_the_arrows_walk_the_choices() {
    let (app, task) = add();
    let mut laid = landed(&app, task);
    assert_eq!(laid.focused(), [Id::new(CHOICE_IDS[0])]);
    walk_rows(&mut laid, &CHOICE_IDS.map(str::to_owned));
}

#[test]
fn tab_walks_the_choices_after_the_controls_it_walked_before() {
    let (mut app, tree) = add_at(0);
    let mut laid = relaid(&app, tree);
    let all = focusables(&mut laid);
    let mut tree = laid.into_tree();
    // The rows join the end; the controls before them are as they were.
    let rows: Vec<_> = CHOICE_IDS
        .iter()
        .map(|id| Some(format!("{:?}", Id::new(id))))
        .collect();
    let (before, after) = all.split_at(all.len() - rows.len());
    assert_eq!(after, rows);
    assert!(before.iter().all(|id| id.is_some() && !rows.contains(id)));

    let tab = |app: &mut App, tree: Tree, shift: bool| -> (Vec<Id>, Tree) {
        let modifiers = if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::empty()
        };
        let (_, tree, task) = key(app, tree, Key::Named(Named::Tab), modifiers);
        let mut laid = relaid(app, tree);
        run_task(&mut laid, task);
        (laid.focused(), laid.into_tree())
    };
    let mut now;
    for id in &CHOICE_IDS[1..] {
        (now, tree) = tab(&mut app, tree, false);
        assert_eq!(now, [Id::new(id)]);
    }
    // Past the last control Tab lets go, and the next Tab starts over at
    // the first, as iced's own walk always has.
    (now, tree) = tab(&mut app, tree, false);
    assert!(now.is_empty());
    (now, tree) = tab(&mut app, tree, false);
    assert_eq!(
        now.iter()
            .map(|id| Some(format!("{id:?}")))
            .collect::<Vec<_>>(),
        [before[0].clone()]
    );
    (now, tree) = tab(&mut app, tree, true);
    assert!(now.is_empty());
    (now, tree) = tab(&mut app, tree, true);
    assert_eq!(now, [Id::new(CHOICE_IDS[4])]);
    (now, _) = tab(&mut app, tree, true);
    assert_eq!(now, [Id::new(CHOICE_IDS[3])]);
}

#[test]
fn the_number_keys_still_choose_with_a_row_focused() {
    for (digit, page) in [
        ("1", "new"),
        ("2", "prefix"),
        ("3", "heroic"),
        ("4", "steam"),
        ("5", "gog"),
    ] {
        // Focus on another choice than the one the key picks.
        let (mut app, tree) = add_at(2);
        let (now, tree) = focused(&app, tree);
        assert_eq!(now, [Id::new("add-heroic")]);
        let (published, _, _) = key(
            &mut app,
            tree,
            Key::Character(digit.into()),
            Modifiers::empty(),
        );
        assert!(published.is_empty(), "{published:?}");
        assert_eq!(opened(&app), page, "{digit}");
    }
}

#[test]
fn ctrl_enter_submits_once_and_leaves_the_focused_row_alone() {
    let (mut app, tree) = add_at(2);
    let (now, tree) = focused(&app, tree);
    assert_eq!(now, [Id::new("add-heroic")]);
    let (published, _, _) = key(&mut app, tree, Key::Named(Named::Enter), Modifiers::CTRL);
    assert!(published.is_empty(), "the row chose too: {published:?}");
    // Add's submit is New, not the focused Heroic.
    assert_eq!(opened(&app), "new");
}

#[test]
fn enter_chooses_the_focused_row() {
    let (mut app, tree) = add_at(2);
    let (published, _, _) = key(&mut app, tree, Key::Named(Named::Enter), Modifiers::empty());
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(opened(&app), "heroic");
}

#[test]
fn going_back_to_add_lands_on_its_first_choice_again() {
    let (mut app, _) = add();
    let _ = app.update(Message::Flow(app.flow_generation, super::Event::New));
    assert_eq!(opened(&app), "new");
    let task = app.update(Message::Flow(app.flow_generation, super::Event::Back));
    assert_eq!(opened(&app), "choose");
    let mut laid = landed(&app, task);
    assert_eq!(laid.focused(), [Id::new(CHOICE_IDS[0])]);
}

#[test]
fn the_arrows_scroll_adds_list_in_a_short_dialog() {
    let (mut app, _) = add();
    let page = Page::choose(app.paths.clone());
    let ids = CHOICE_IDS.map(str::to_owned).to_vec();
    let mut laid = crate::layout_probe::lay_out(
        page.view(crate::theme::Palette::DARK, 280.0, false),
        iced::Size::new(280.0, 240.0),
    );
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus::<()>(Id::new(CHOICE_IDS[0])),
    ));
    let mut deepest = 0.0_f32;
    let mut walk = |laid: &mut Laid<'_, super::Event>, key: Named| {
        let (_, status) = press(laid, key, Modifiers::empty());
        let task = app.update(Message::Key(Key::Named(key), Modifiers::empty(), status));
        run_task(laid, task);
        let offset = laid.offsets()[0];
        // Shown already: a second reveal moves nothing.
        run_task(laid, crate::focus_button::reveal::<()>(ids.clone()));
        assert_eq!(laid.offsets()[0], offset);
        offset
    };
    for _ in 1..CHOICE_IDS.len() {
        deepest = deepest.max(walk(&mut laid, Named::ArrowDown));
    }
    assert!(deepest > 0.0, "the dialog is short enough to scroll");
    for _ in 1..CHOICE_IDS.len() {
        walk(&mut laid, Named::ArrowUp);
    }
    // The viewport is shorter than the first row and the list's 4 px
    // padding, so the row's own top is what shows.
    assert!(laid.offsets()[0] <= 4.0, "{:?}", laid.offsets());
}
