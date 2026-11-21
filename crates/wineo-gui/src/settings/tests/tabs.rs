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

const TABS: usize = Tab::ALL.len();

/// Shows `tab`, going through `show_advanced` for Advanced.
fn open_tab(app: &mut App, paths: &Paths, tab: Tab) {
    if tab == Tab::Advanced {
        let _ = show_advanced(app.settings.as_mut().unwrap(), paths);
    } else {
        select(app, tab);
    }
    assert_eq!(page_of(app).tab, tab);
}

#[test]
fn the_global_page_opens_on_general_with_all_five_tabs_in_order() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    assert_eq!(page_of(&app).tab, Tab::General);
    assert_eq!(
        Tab::ALL,
        [
            Tab::General,
            Tab::Appearance,
            Tab::Controller,
            Tab::Stores,
            Tab::Advanced
        ]
    );
    let ids = focusable_ids(&app, &paths);
    assert_eq!(ids[..TABS], tab_ids());
    for tab in Tab::ALL {
        assert_eq!(Tab::ALL[tab.index()], tab);
    }
}

/// One control, or a heading where a section's controls share their text
/// with another's, from every section, by the tab it belongs on.
const SECTIONS: [(Tab, &[&str]); 5] = [
    (
        Tab::General,
        &[
            "Desktop",
            "Show a tray icon",
            "Coverflow",
            "Open coverflow in full screen",
            "Launch defaults",
            "Gamemode",
            "Art to fetch",
            "Hero",
            "RetroArch",
            "Quit RetroArch when its game closes",
        ],
    ),
    (
        Tab::Appearance,
        &["Follow the system", "Improve readability", "Interface size"],
    ),
    (
        Tab::Controller,
        &[
            view::CONTROLLER_SWITCH,
            view::NO_CONTROLLER,
            "Open or play",
            view::RESET_BUTTONS,
        ],
    ),
    (
        Tab::Stores,
        &[
            "Steam",
            "Import new Steam games automatically",
            "Get a key at https://steamcommunity.com/dev/apikey.",
            "GOG",
            "Games folder",
            "GOG language",
            "SteamGridDB",
        ],
    ),
    (
        Tab::Advanced,
        &[
            "New environments",
            "Runner",
            "Arch",
            "Prefix root",
            "Builds from Steam, Heroic, and Lutris",
            "Runs",
            "Stop timeout (seconds)",
            "Environment variables",
        ],
    ),
];

#[test]
fn each_section_shows_on_its_tab_and_no_other() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let mut shown = Vec::new();
    for tab in Tab::ALL {
        open_tab(&mut app, &paths, tab);
        shown.push((tab, page_texts(page_of(&app), &paths)));
    }
    for (home, labels) in SECTIONS {
        for label in labels {
            for (tab, texts) in &shown {
                assert_eq!(
                    texts.iter().any(|text| text == label),
                    *tab == home,
                    "{label} on {tab:?}"
                );
            }
        }
    }
    // Steam's switch and key row are one section now.
    let (_, stores) = &shown[Tab::Stores.index()];
    assert!(!stores.iter().any(|text| text == "Steam Web API"));
    let at = |wanted: &str| stores.iter().position(|text| text == wanted).unwrap();
    assert!(at("Steam") < at("Import new Steam games automatically"));
    assert!(
        at("Import new Steam games automatically")
            < at("Get a key at https://steamcommunity.com/dev/apikey.")
    );
    assert!(at("Get a key at https://steamcommunity.com/dev/apikey.") < at("GOG"));
    assert!(at("GOG") < at("Games folder"));
    assert!(at("Games folder") < at("GOG language"));
    assert!(at("GOG language") < at("SteamGridDB"));
    let (_, general) = &shown[Tab::General.index()];
    let at = |wanted: &str| general.iter().position(|text| text == wanted).unwrap();
    assert!(at("Desktop") < at("Coverflow"));
    assert!(at("Coverflow") < at("Launch defaults"));
    assert!(at("Launch defaults") < at("Art to fetch"));
    assert!(at("Art to fetch") < at("RetroArch"));
    let (_, advanced) = &shown[Tab::Advanced.index()];
    let at = |wanted: &str| advanced.iter().position(|text| text == wanted).unwrap();
    assert!(at("New environments") < at("Runs"));
    assert!(at("Runs") < at("Environment variables"));
}

#[test]
fn each_tab_keeps_its_controls_to_itself_and_brings_them_back() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let mut shown = Vec::new();
    for tab in Tab::ALL {
        open_tab(&mut app, &paths, tab);
        let ids = focusable_ids(&app, &paths);
        assert_eq!(ids[..TABS], tab_ids(), "{tab:?}");
        shown.push(ids[TABS..].to_vec());
    }
    for (index, ids) in shown.iter().enumerate() {
        for (other, others) in shown.iter().enumerate() {
            if index != other {
                assert!(
                    ids.iter()
                        .flatten()
                        .all(|id| !others.contains(&Some(id.clone()))),
                    "{:?} and {:?}",
                    Tab::ALL[index],
                    Tab::ALL[other]
                );
            }
        }
    }
    let remap = |action| Some(format!("{:?}", Id::from(view::remap_id(action))));
    let controller = &shown[Tab::Controller.index()];
    for action in wineo_core::library::PadAction::ALL {
        assert!(controller.contains(&remap(action)), "{action:?}");
    }
    select(&mut app, Tab::Appearance);
    let appearance = focusable_ids(&app, &paths);
    assert_eq!(
        appearance[TABS..].first(),
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
    for tab in Tab::ALL {
        open_tab(&mut app, &paths, tab);
        assert_eq!(focusable_ids(&app, &paths)[TABS..], shown[tab.index()]);
    }
}

/// Where focus is, as an index into the page's focusables in tree order.
fn focused_at(laid: &mut crate::layout_probe::Laid<'_, Message>) -> Option<usize> {
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::Focusable;
    use std::sync::{Arc, Mutex};
    struct At(usize, Arc<Mutex<Option<usize>>>);
    impl Operation for At {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn focusable(
            &mut self,
            _id: Option<&Id>,
            _bounds: iced::Rectangle,
            state: &mut dyn Focusable,
        ) {
            if state.is_focused() {
                *self.1.lock().unwrap() = Some(self.0);
            }
            self.0 += 1;
        }
    }
    let found = Arc::new(Mutex::new(None));
    laid.run(Box::new(At(0, found.clone())));
    *found.lock().unwrap()
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
    let mut tab = |laid: &mut crate::layout_probe::Laid<'_, Message>| {
        let task = keys.settings_key(&Key::Named(Named::Tab), Modifiers::empty(), false);
        crate::focus_button::run_task(laid, task);
    };
    tab(&mut laid);
    // Past the last tab comes General's first control when it has one, and
    // otherwise nothing, before Tab comes around to the first tab again.
    let after = crate::focus_button::focusables(&mut laid).len() > TABS;
    if after {
        assert_eq!(focused_at(&mut laid), Some(TABS));
    } else {
        assert_eq!(focused_at(&mut laid), None);
        tab(&mut laid);
        assert_eq!(laid.focused(), [Id::new(Tab::General.id())]);
    }
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

/// The page at the window's minimum width less the sidebar, where the bar
/// gets 228 px after the header's padding.
const NARROW: iced::Size = iced::Size::new(260.0, 700.0);

#[test]
fn left_and_right_walk_every_tab_in_order_and_stop_at_the_ends() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    // In one line, and wrapped onto three.
    for size in [SIZE, NARROW] {
        let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), size);
        let select = |tab: Tab| format!("{:?}", Message::Settings(Event::Tab(tab)));
        laid.run(Box::new(focusable::focus::<()>(Id::new(Tab::General.id()))));

        let (sent, status) = arrow(&mut laid, Named::ArrowLeft);
        assert!(sent.is_empty(), "General is the first tab");
        assert_eq!(status, Status::Captured);
        assert_eq!(laid.focused(), [Id::new(Tab::General.id())]);

        for tab in &Tab::ALL[1..] {
            let (sent, status) = arrow(&mut laid, Named::ArrowRight);
            assert_eq!(sent, [select(*tab)], "{size:?}");
            assert_eq!(status, Status::Captured);
            assert_eq!(laid.focused(), [Id::new(tab.id())]);
        }
        let (sent, status) = arrow(&mut laid, Named::ArrowRight);
        assert!(sent.is_empty(), "Advanced is the last tab");
        assert_eq!(status, Status::Captured);
        assert_eq!(laid.focused(), [Id::new(Tab::Advanced.id())]);

        for tab in Tab::ALL[..TABS - 1].iter().rev() {
            let (sent, status) = arrow(&mut laid, Named::ArrowLeft);
            assert_eq!(sent, [select(*tab)], "{size:?}");
            assert_eq!(status, Status::Captured);
            assert_eq!(laid.focused(), [Id::new(tab.id())]);
        }
    }
}

/// Each tab's button and its label, by the bounds the page lays them out at.
fn tab_bounds(
    laid: &mut crate::layout_probe::Laid<'_, Message>,
) -> Vec<(Tab, iced::Rectangle, iced::Rectangle)> {
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::Focusable;
    use std::sync::{Arc, Mutex};
    type Found = Arc<Mutex<(Vec<(Id, iced::Rectangle)>, Vec<(String, iced::Rectangle)>)>>;
    struct All(Found);
    impl Operation for All {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn focusable(&mut self, id: Option<&Id>, bounds: iced::Rectangle, _: &mut dyn Focusable) {
            if let Some(id) = id {
                self.0.lock().unwrap().0.push((id.clone(), bounds));
            }
        }
        fn text(&mut self, _id: Option<&Id>, bounds: iced::Rectangle, text: &str) {
            self.0.lock().unwrap().1.push((text.to_owned(), bounds));
        }
    }
    let found: Found = Arc::default();
    laid.run(Box::new(All(found.clone())));
    let (buttons, texts) = &*found.lock().unwrap();
    Tab::ALL
        .into_iter()
        .map(|tab| {
            let button = buttons
                .iter()
                .find(|(id, _)| *id == Id::new(tab.id()))
                .unwrap_or_else(|| panic!("no {tab:?} tab"))
                .1;
            let label = texts
                .iter()
                .find(|(text, bounds)| text == tab.label() && button.contains(bounds.center()))
                .unwrap_or_else(|| panic!("no {tab:?} label"))
                .1;
            (tab, button, label)
        })
        .collect()
}

#[test]
fn the_bar_wraps_at_the_narrowest_page_with_no_label_cut() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    let mut wide = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    let wide = tab_bounds(&mut wide);
    // One line when there's room.
    assert!(wide.iter().all(|(_, button, _)| button.y == wide[0].1.y));
    let mut narrow = lay_out(page_of(&app).view(&paths, Palette::DARK), NARROW);
    let narrow_tabs = tab_bounds(&mut narrow);
    let mut lines: Vec<Vec<Tab>> = Vec::new();
    let mut last_y = None;
    for (tab, button, label) in &narrow_tabs {
        // Inside the header's 16 px padding on each side.
        assert!(button.x >= 16.0, "{tab:?} {button:?}");
        assert!(
            button.x + button.width <= NARROW.width - 16.0,
            "{tab:?} {button:?}"
        );
        // The label keeps its full width and one line, inside its button.
        let (_, _, whole) = wide.iter().find(|(wide, ..)| wide == tab).unwrap();
        assert_eq!(label.width, whole.width, "{tab:?}");
        assert_eq!(label.height, whole.height, "{tab:?}");
        assert!(crate::layout_probe::within(*label, *button), "{tab:?}");
        if last_y == Some(button.y) {
            lines.last_mut().unwrap().push(*tab);
        } else {
            lines.push(vec![*tab]);
        }
        last_y = Some(button.y);
    }
    assert_eq!(
        lines,
        [
            vec![Tab::General, Tab::Appearance],
            vec![Tab::Controller, Tab::Stores],
            vec![Tab::Advanced],
        ]
    );
    // The divider sits under the last line, which sits on it.
    let bottom = narrow_tabs
        .iter()
        .map(|(_, button, _)| button.y + button.height)
        .fold(0.0, f32::max);
    let divider = crate::layout_probe::bounds_at(&narrow.node, &[0, 0, 1]);
    assert_eq!(divider.height, 1.0);
    assert_eq!(divider.y, bottom);
    // Nothing on the page spills out sideways.
    let mut page = page_of(&app).view(&paths, Palette::DARK);
    let probe = crate::layout_probe::probe(&mut page, NARROW);
    let frame = iced::Rectangle::new(iced::Point::ORIGIN, NARROW);
    let header = crate::layout_probe::bounds_at(&probe.node, &[0, 0, 0]);
    assert!(
        probe
            .outside(frame)
            .iter()
            .all(|(_, bounds)| !header.intersects(bounds)),
        "{:?}",
        probe.outside(frame)
    );
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

/// The bar alone in its widest and narrowest room.
const BARS: [iced::Size; 2] = [SIZE, iced::Size::new(228.0, 700.0)];

#[test]
fn each_underline_spans_its_tab_along_the_bottom_edge() {
    for size in BARS {
        let laid = lay_out(
            crate::settings::tabs::bar(Tab::General, true, Palette::DARK),
            size,
        );
        assert_eq!(laid.node.children().len(), TABS);
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
}

#[test]
fn only_the_selected_tab_is_underlined_in_primary() {
    let palette = Palette::DARK;
    for size in BARS {
        for selected in Tab::ALL {
            let mut bar = crate::settings::tabs::bar(selected, true, palette);
            let pixels = crate::layout_probe::render(&mut bar, size);
            let laid = lay_out(crate::settings::tabs::bar(selected, true, palette), size);
            let primary = [palette.primary.r, palette.primary.g, palette.primary.b]
                .map(|c| (c * 255.0).round() as u8);
            for (tab, node) in Tab::ALL.into_iter().zip(laid.node.children()) {
                let bounds = node.bounds();
                let x = (bounds.x + bounds.width / 2.0) as usize;
                let y = (bounds.y + bounds.height - 1.0) as usize;
                let at = (y * size.width as usize + x) * 4;
                assert_eq!(
                    pixels[at..at + 3] == primary && pixels[at + 3] == 255,
                    tab == selected,
                    "{tab:?} with {selected:?} selected at {size:?}"
                );
            }
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

fn switch(app: &mut App, paths: &Paths, tab: Tab) -> Task<Message> {
    if tab == Tab::Advanced {
        show_advanced(app.settings.as_mut().unwrap(), paths)
    } else {
        app.update(Message::Settings(Event::Tab(tab)))
    }
}

#[test]
fn each_tab_keeps_its_own_scroll_position() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let scroll = |y| {
        iced::widget::operation::scroll_to(
            Id::new(super::super::view::SCROLL_ID),
            iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
        )
    };
    let wanted = |tab: Tab| 10.0 * (tab.index() + 1) as f32;
    // Each tab starts at the top, rather than where the last one was, and
    // is really scrolled.
    let mut tree = None;
    for tab in Tab::ALL {
        let task = switch(&mut app, &paths, tab);
        let (offset, next) = show(&app, &paths, tree, task);
        assert_eq!(offset, 0.0, "{tab:?}");
        let (offset, next) = show(&app, &paths, Some(next), scroll(wanted(tab)));
        assert_eq!(offset, wanted(tab), "{tab:?}");
        let _ = app.update(Message::Settings(Event::Scrolled(tab, offset)));
        tree = Some(next);
    }
    assert_eq!(page_of(&app).scroll, Tab::ALL.map(wanted));
    // Each gets its own back, in any order.
    for tab in [
        Tab::General,
        Tab::Stores,
        Tab::Appearance,
        Tab::Advanced,
        Tab::Controller,
    ] {
        let task = switch(&mut app, &paths, tab);
        let (offset, next) = show(&app, &paths, tree, task);
        assert_eq!(offset, wanted(tab), "{tab:?}");
        tree = Some(next);
    }
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
    assert_eq!(page.scroll, [120.0, 7.0, 0.0, 0.0, 0.0]);

    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::General, 120.0)));
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    reopen_global(&mut app, &paths);
    assert_eq!(page_of(&app).scroll, [0.0; 5]);
}

#[test]
fn a_scroll_queued_before_a_switch_lands_on_the_tab_it_was_built_for() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    // The batch the QA case describes: the switch is handled first.
    let _ = switch(&mut app, &paths, Tab::Appearance);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::General, 120.0)));
    assert_eq!(page_of(&app).scroll, [120.0, 0.0, 0.0, 0.0, 0.0]);
    let _ = app.update(Message::Settings(Event::Scrolled(Tab::Appearance, 5.0)));
    assert_eq!(page_of(&app).scroll, [120.0, 5.0, 0.0, 0.0, 0.0]);
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
