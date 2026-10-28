use iced::advanced::widget::operation::focusable;
use iced::event::Status;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::Id;

use super::*;
use crate::layout_probe::lay_out;
use crate::theme::Palette;

const SIZE: iced::Size = iced::Size::new(900.0, 700.0);

fn tab_ids() -> Vec<Option<String>> {
    Tab::ALL
        .map(|tab| Some(format!("{:?}", Id::new(tab.id()))))
        .to_vec()
}

fn page_of(app: &App) -> &Page {
    app.settings.as_ref().unwrap()
}

/// The focusable ids of the page as it shows now.
fn focusable_ids(app: &App, paths: &Paths) -> Vec<Option<String>> {
    let mut laid = lay_out(page_of(app).view(paths, Palette::DARK), SIZE);
    crate::focus_button::focusables(&mut laid)
}

fn select(app: &mut App, tab: Tab) {
    let _ = app.update(Message::Settings(Event::Tab(tab)));
}

#[test]
fn the_global_page_opens_on_general_with_both_tabs() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    assert_eq!(page_of(&app).tab, Tab::General);
    let ids = focusable_ids(&app, &paths);
    assert_eq!(ids[..2], tab_ids());
    assert!(ids.len() > 2, "General's controls follow the tabs");
}

#[test]
fn appearance_hides_general_and_general_brings_it_back() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let general = focusable_ids(&app, &paths);
    select(&mut app, Tab::Appearance);
    assert_eq!(page_of(&app).tab, Tab::Appearance);
    let appearance = focusable_ids(&app, &paths);
    assert_eq!(appearance[..2], tab_ids());
    assert!(
        general[2..].iter().all(|id| !appearance.contains(id)),
        "{appearance:?}"
    );
    assert_eq!(
        appearance[2..].first(),
        Some(&Some(format!(
            "{:?}",
            Id::new(super::super::appearance::FOLLOW_ID)
        )))
    );
    assert_eq!(
        appearance.last(),
        Some(&Some(format!(
            "{:?}",
            Id::new(super::super::appearance::READABILITY_ID)
        )))
    );
    select(&mut app, Tab::General);
    assert_eq!(focusable_ids(&app, &paths), general);
}

#[test]
fn tab_reaches_the_tabs_right_after_the_title() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    // Tab's task doesn't read the page, so a second app can send the keys
    // while the first one's view is laid out.
    let mut keys = open_global_app(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    for tab in Tab::ALL {
        let task = keys.settings_key(&Key::Named(Named::Tab), Modifiers::empty(), false);
        crate::focus_button::run_task(&mut laid, task);
        assert_eq!(laid.focused(), [Id::new(tab.id())]);
    }
    let task = keys.settings_key(&Key::Named(Named::Tab), Modifiers::empty(), false);
    crate::focus_button::run_task(&mut laid, task);
    // The next stop is one of General's fields, which have no id to name.
    assert!(
        laid.focused()
            .iter()
            .all(|id| *id != Id::new(Tab::General.id()) && *id != Id::new(Tab::Appearance.id())),
        "the third Tab leaves the tabs"
    );
}

fn arrow(laid: &mut crate::layout_probe::Laid<'_, Message>, key: Named) -> (Vec<String>, Status) {
    let (messages, status) = crate::focus_button::press(laid, key, Modifiers::empty());
    (
        messages
            .iter()
            .map(|message| format!("{message:?}"))
            .collect(),
        status,
    )
}

#[test]
fn left_and_right_select_and_focus_the_neighbor_and_stop_at_the_ends() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    let select = |tab: Tab| format!("{:?}", Message::Settings(Event::Tab(tab)));
    laid.run(Box::new(focusable::focus::<()>(Id::new(Tab::General.id()))));

    let (sent, status) = arrow(&mut laid, Named::ArrowLeft);
    assert!(sent.is_empty(), "General is the first tab");
    assert_eq!(status, Status::Captured);
    assert_eq!(laid.focused(), [Id::new(Tab::General.id())]);

    let (sent, status) = arrow(&mut laid, Named::ArrowRight);
    assert_eq!(sent, [select(Tab::Appearance)]);
    assert_eq!(status, Status::Captured);
    assert_eq!(laid.focused(), [Id::new(Tab::Appearance.id())]);

    let (sent, status) = arrow(&mut laid, Named::ArrowRight);
    assert!(sent.is_empty(), "Appearance is the last tab");
    assert_eq!(status, Status::Captured);

    let (sent, _) = arrow(&mut laid, Named::ArrowLeft);
    assert_eq!(sent, [select(Tab::General)]);
    assert_eq!(laid.focused(), [Id::new(Tab::General.id())]);
}

#[test]
fn arrows_are_left_alone_off_the_tabs_and_with_a_modifier() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    for key in [Named::ArrowLeft, Named::ArrowRight] {
        assert_eq!(arrow(&mut laid, key), (Vec::new(), Status::Ignored));
    }
    laid.run(Box::new(focusable::focus::<()>(Id::new(Tab::General.id()))));
    let (sent, status) = crate::focus_button::press(&mut laid, Named::ArrowRight, Modifiers::CTRL);
    assert!(sent.is_empty());
    assert_eq!(status, Status::Ignored);
}

#[test]
fn a_prompt_over_the_page_takes_the_arrows_and_tab_switches() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    app.settings.as_mut().unwrap().leave_prompt = true;
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    laid.run(Box::new(focusable::focus::<()>(Id::new(Tab::General.id()))));
    assert_eq!(arrow(&mut laid, Named::ArrowRight).1, Status::Ignored);
    drop(laid);
    select(&mut app, Tab::Appearance);
    assert_eq!(page_of(&app).tab, Tab::General);
}

#[test]
fn edits_survive_a_tab_switch_and_stay_counted() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    assert_eq!(page_of(&app).changes(), 1);
    select(&mut app, Tab::Appearance);
    assert_eq!(page_of(&app).changes(), 1);
    assert!(page_of(&app).is_dirty());
    assert!(page_of(&app).can_save(), "Save works from Appearance");
    select(&mut app, Tab::General);
    assert_eq!(page_of(&app).changes(), 1);
    assert_eq!(
        global_editor(page_of(&app))
            .form
            .draft
            .switch(Switch::Gamemode),
        Some(true)
    );
    // Switching never counts as an edit.
    let clean = open_global_app(&paths);
    let mut clean = clean;
    select(&mut clean, Tab::Appearance);
    assert_eq!(page_of(&clean).changes(), 0);
    assert!(!page_of(&clean).is_dirty());
}

#[test]
fn the_environment_page_has_no_tabs() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = open_app(&paths);
    let ids = focusable_ids(&app, &paths);
    assert!(tab_ids().iter().all(|tab| !ids.contains(tab)));
    select(&mut app, Tab::Appearance);
    assert_eq!(page_of(&app).tab, Tab::General);
    assert_eq!(focusable_ids(&app, &paths), ids);
}

#[test]
fn reopening_settings_starts_on_general() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    select(&mut app, Tab::Appearance);
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_none());
    reopen_global(&mut app, &paths);
    assert_eq!(page_of(&app).tab, Tab::General);
}

#[test]
fn a_reload_keeps_the_tab() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    select(&mut app, Tab::Appearance);
    let generation = page_of(&app).generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    assert_eq!(page_of(&app).tab, Tab::Appearance);
}

#[test]
fn each_underline_spans_its_tab_along_the_bottom_edge() {
    let laid = lay_out(
        crate::settings::tabs::bar(Tab::General, true, Palette::DARK),
        SIZE,
    );
    assert_eq!(laid.node.children().len(), 2);
    for tab in laid.node.children() {
        let [label, cover] = tab.children() else {
            panic!("a label and the underline's layer");
        };
        assert!(label.bounds().width < 200.0, "{:?}", label.bounds());
        assert_eq!(cover.bounds(), label.bounds());
        let underline = cover.children()[0].bounds();
        assert_eq!(underline.width, label.bounds().width);
        assert_eq!(underline.height, 2.0);
        assert_eq!(
            underline.y + underline.height,
            label.bounds().y + label.bounds().height
        );
    }
}

#[test]
fn only_the_selected_tab_is_underlined_in_primary() {
    let palette = Palette::DARK;
    for selected in Tab::ALL {
        let mut bar = crate::settings::tabs::bar(selected, true, palette);
        let pixels = crate::layout_probe::render(&mut bar, SIZE);
        let laid = lay_out(crate::settings::tabs::bar(selected, true, palette), SIZE);
        let primary = [palette.primary.r, palette.primary.g, palette.primary.b]
            .map(|c| (c * 255.0).round() as u8);
        for (tab, node) in Tab::ALL.into_iter().zip(laid.node.children()) {
            let bounds = node.bounds();
            let x = (bounds.x + bounds.width / 2.0) as usize;
            let y = (bounds.y + bounds.height - 1.0) as usize;
            let at = (y * SIZE.width as usize + x) * 4;
            assert_eq!(
                pixels[at..at + 3] == primary && pixels[at + 3] == 255,
                tab == selected,
                "{tab:?} with {selected:?} selected"
            );
        }
    }
}

const SHORT: iced::Size = iced::Size::new(900.0, 300.0);

type Tree = iced::advanced::widget::Tree;

/// Lays out the page over the widget state of the last layout, as the
/// runtime does after an update, runs the widget operations of `task` on
/// it, and returns the page's scroll offset and the state to carry on.
fn show(app: &App, paths: &Paths, tree: Option<Tree>, task: Task<Message>) -> (f32, Tree) {
    let view = page_of(app).view(paths, Palette::DARK);
    let mut laid = match tree {
        Some(tree) => crate::layout_probe::relay_out(view, tree, SHORT),
        None => lay_out(view, SHORT),
    };
    crate::focus_button::run_task(&mut laid, task);
    (laid.offsets()[0], laid.into_tree())
}

fn switch(app: &mut App, tab: Tab) -> Task<Message> {
    app.update(Message::Settings(Event::Tab(tab)))
}

#[test]
fn each_tab_keeps_its_own_scroll_position() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    // General is really scrolled, and Appearance, which starts at the top,
    // doesn't take its offset.
    let scroll = |y| {
        iced::widget::operation::scroll_to(
            Id::new(super::super::view::SCROLL_ID),
            iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
        )
    };
    let (offset, tree) = show(&app, &paths, None, scroll(120.0));
    assert_eq!(offset, 120.0);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::General, offset)));
    let task = switch(&mut app, Tab::Appearance);
    let (offset, tree) = show(&app, &paths, Some(tree), task);
    assert_eq!(offset, 0.0);
    let task = switch(&mut app, Tab::General);
    let (offset, tree) = show(&app, &paths, Some(tree), task);
    assert_eq!(offset, 120.0);
    // Appearance remembers its own too, apart from General's.
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::General, 40.0)));
    let task = switch(&mut app, Tab::Appearance);
    let (_, tree) = show(&app, &paths, Some(tree), task);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::Appearance, 7.0)));
    let task = switch(&mut app, Tab::General);
    let (offset, _) = show(&app, &paths, Some(tree), task);
    assert_eq!(offset, 40.0);
    assert_eq!(page_of(&app).scroll, [40.0, 7.0]);
}

#[test]
fn a_reload_keeps_both_scroll_positions_and_a_new_page_starts_at_the_top() {
    let (_root, paths) = setup();
    write_global(&paths, "gamemode = false\n");
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::Scrolled(Tab::General, 120.0));
    let _ = page.update(&paths, Event::Tab(Tab::Appearance));
    let _ = page.update(&paths, Event::Scrolled(Tab::Appearance, 7.0));
    // A hand edit with no edits on the page reloads it on its own.
    write_global(&paths, "gamemode = true\n");
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    assert!(page.loading(), "the file change started a reload");
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.loading());
    assert_eq!(global_editor(&page).form.draft.gamemode, Some(true));
    assert_eq!(page.tab, Tab::Appearance);
    assert_eq!(page.scroll, [120.0, 7.0]);

    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::General, 120.0)));
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    reopen_global(&mut app, &paths);
    assert_eq!(page_of(&app).scroll, [0.0, 0.0]);
}

#[test]
fn a_scroll_queued_before_a_switch_lands_on_the_tab_it_was_built_for() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    // The batch the QA case describes: the switch is handled first.
    let _ = switch(&mut app, Tab::Appearance);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::General, 120.0)));
    assert_eq!(page_of(&app).scroll, [120.0, 0.0]);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::Appearance, 5.0)));
    assert_eq!(page_of(&app).scroll, [120.0, 5.0]);
}

#[test]
fn scrolling_the_page_reports_its_offset_for_the_shown_tab() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SHORT);
    let messages = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
            delta: iced::mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
        }),
        iced::mouse::Cursor::Available(iced::Point::new(450.0, 150.0)),
    );
    let reported = messages.iter().any(
        |message| matches!(message, Message::Settings(Event::Scrolled(Tab::General, offset)) if *offset > 0.0),
    );
    assert!(reported, "{messages:?}");
}
