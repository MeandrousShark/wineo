//! The hint bar's hints and how they fit: which hints each pad gets, and
//! the bar laid out for real in the window, beside a panel or not.

use iced::advanced::widget::{Id, Operation};
use iced::{Rectangle, Size};
use wineo_core::library::{PadAction, PadButton};

use super::*;
use crate::grid::NOTES_MARGIN;
use crate::grid_motion::DOCK_WIDTH;
use crate::layout_probe::lay_out;
use crate::pad::Style;
use crate::pad::glyph::{HINT_HEIGHT, Hint};
use crate::pad::hint_fit::BAR_ID;

fn hint(buttons: &[PadButton], word: &'static str) -> Hint {
    Hint {
        buttons: buttons.to_vec(),
        word,
    }
}

/// Every hint the default map gives with no panel, in order.
fn all_hints() -> Vec<Hint> {
    vec![
        hint(&[PadButton::A], "Open"),
        hint(&[PadButton::B], "Back"),
        hint(&[PadButton::Y], "Favorite"),
        hint(&[PadButton::X], "Mark"),
        hint(&[PadButton::Ls], "Random"),
        hint(&[PadButton::Lb, PadButton::Rb], "Page"),
        hint(&[PadButton::View], "Layout"),
        hint(&[PadButton::Start], "Full screen"),
    ]
}

fn words_of(app: &App) -> Vec<&'static str> {
    words(&app.pad_hints())
}

#[test]
fn the_default_map_hints_eight_actions_on_each_pad_and_a_gamecube_lacks_random() {
    let (_home, mut app) = ready();
    for style in [Style::Xbox, Style::PlayStation, Style::Nintendo] {
        app.pad_hint = Some(style);
        assert_eq!(app.pad_hints(), all_hints(), "{style:?}");
    }
    // Random's LS is a stick click, which a GameCube pad lacks.
    app.pad_hint = Some(Style::GameCube);
    let mut gamecube = all_hints();
    gamecube.retain(|hint| hint.word != "Random");
    assert_eq!(app.pad_hints(), gamecube);
}

#[test]
fn an_open_panel_says_play_and_leaves_out_layout_and_full_screen() {
    let (_home, mut app) = ready();
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
    assert_eq!(
        words_of(&app),
        ["Play", "Back", "Favorite", "Mark", "Random", "Page"]
    );
    app.pad_hint = Some(Style::GameCube);
    assert_eq!(words_of(&app), ["Play", "Back", "Favorite", "Mark", "Page"]);
}

#[test]
fn a_panel_a_coverflow_card_hides_gives_layout_and_full_screen_back() {
    let (_home, mut app) = coverflow_ready();
    tap(&mut app, Raw::DPadRight);
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
    assert!(!words_of(&app).contains(&"Layout"));
    // On b's collapsed card, a header: no Favorite or Mark either.
    tap(&mut app, Raw::DPadRight);
    assert!(app.panel_hidden_by_card());
    assert_eq!(
        words_of(&app),
        ["Open", "Back", "Random", "Page", "Layout", "Full screen"]
    );
}

#[test]
fn a_header_leaves_out_favorite_and_mark() {
    let (_home, mut app) = coverflow_ready();
    tap(&mut app, Raw::DPadRight);
    tap(&mut app, Raw::DPadRight);
    assert!(app.header_cursor().is_some(), "on b's card");
    assert!(!app.panel_open);
    assert_eq!(
        words_of(&app),
        ["Open", "Back", "Random", "Page", "Layout", "Full screen"]
    );
}

#[test]
fn page_shows_the_half_that_is_bound() {
    let (_home, mut app) = ready();
    app.pad_hint = Some(Style::Xbox);
    // Confirm on RB leaves Page's right half with no button.
    app.input.buttons.insert(PadAction::Confirm, PadButton::Rb);
    let hints = app.pad_hints();
    assert_eq!(hints[0], hint(&[PadButton::Rb], "Open"));
    assert!(hints.contains(&hint(&[PadButton::Lb], "Page")), "{hints:?}");
    // And the left half, the other way.
    app.input.buttons.clear();
    app.input.buttons.insert(PadAction::Confirm, PadButton::Lb);
    assert!(app.pad_hints().contains(&hint(&[PadButton::Rb], "Page")));
    // Neither: no Page hint.
    app.input.buttons.insert(PadAction::Back, PadButton::Rb);
    assert!(!words_of(&app).contains(&"Page"));
}

#[test]
fn layout_and_full_screen_show_only_when_bound() {
    let (_home, mut app) = ready();
    app.pad_hint = Some(Style::Xbox);
    app.input.buttons.insert(PadAction::Back, PadButton::View);
    assert_eq!(
        words_of(&app),
        [
            "Open",
            "Back",
            "Favorite",
            "Mark",
            "Random",
            "Page",
            "Full screen"
        ]
    );
    app.input.buttons.clear();
    app.input.buttons.insert(PadAction::Back, PadButton::Start);
    assert_eq!(
        words_of(&app),
        [
            "Open", "Back", "Favorite", "Mark", "Random", "Page", "Layout"
        ]
    );
}

#[test]
fn a_gamecube_pad_leaves_out_what_sits_on_a_stick_click() {
    let (_home, mut app) = adapter_ready();
    app.pad_hint = Some(Style::GameCube);
    // One half of Page on LS: the other half shows.
    app.input.buttons.insert(PadAction::PageLeft, PadButton::Ls);
    assert!(app.pad_hints().contains(&hint(&[PadButton::Rb], "Page")));
    app.input.buttons.clear();
    app.input
        .buttons
        .insert(PadAction::PageRight, PadButton::Rs);
    assert!(app.pad_hints().contains(&hint(&[PadButton::Lb], "Page")));
    // Both: no Page hint at all.
    app.input.buttons.insert(PadAction::PageLeft, PadButton::Ls);
    assert!(!words_of(&app).contains(&"Page"));
    // Layout or Full screen on one goes the same way.
    app.input.buttons.clear();
    app.input.buttons.insert(PadAction::Layout, PadButton::Ls);
    assert!(!words_of(&app).contains(&"Layout"));
    assert!(words_of(&app).contains(&"Full screen"));
    app.input.buttons.clear();
    app.input
        .buttons
        .insert(PadAction::Fullscreen, PadButton::Rs);
    assert!(!words_of(&app).contains(&"Full screen"));
    assert!(words_of(&app).contains(&"Layout"));
}

#[test]
fn first_and_last_never_get_a_hint() {
    let (_home, mut app) = ready();
    let maps: [&[(PadAction, PadButton)]; 3] = [
        &[],
        &[
            (PadAction::First, PadButton::A),
            (PadAction::Last, PadButton::B),
        ],
        &[
            (PadAction::First, PadButton::Lb),
            (PadAction::Last, PadButton::Start),
        ],
    ];
    for map in maps {
        app.input.buttons = map.iter().copied().collect();
        let bindings = app.input.bindings();
        let theirs = [PadAction::First, PadAction::Last].map(|action| bindings.button(action));
        for style in [
            Style::Xbox,
            Style::PlayStation,
            Style::Nintendo,
            Style::GameCube,
        ] {
            app.pad_hint = Some(style);
            for hint in app.pad_hints() {
                for button in hint.buttons {
                    assert!(!theirs.contains(&Some(button)), "{map:?} {style:?}");
                }
            }
        }
    }
}

// The bar laid out in the window.

/// What a layout of the whole window shows: the bar's bounds, and each
/// text with its bounds.
#[derive(Default)]
struct Seen {
    bar: Option<Rectangle>,
    texts: Vec<(String, Rectangle)>,
}

fn seen(app: &App) -> Seen {
    use std::sync::{Arc, Mutex};
    struct Look(Arc<Mutex<Seen>>);
    impl Operation for Look {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
            if id == Some(&Id::new(BAR_ID)) {
                self.0.lock().unwrap().bar = Some(bounds);
            }
        }
        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
            self.0.lock().unwrap().texts.push((text.to_owned(), bounds));
        }
    }
    let found = Arc::new(Mutex::new(Seen::default()));
    lay_out(app.view(), app.window_size).run(Box::new(Look(found.clone())));
    std::mem::take(&mut *found.lock().unwrap())
}

const WORDS: [&str; 9] = [
    "Open",
    "Play",
    "Back",
    "Favorite",
    "Mark",
    "Random",
    "Page",
    "Layout",
    "Full screen",
];

impl Seen {
    /// The texts drawn in the bar, in order.
    fn in_bar(&self) -> Vec<(&str, Rectangle)> {
        let Some(bar) = self.bar else {
            return Vec::new();
        };
        self.texts
            .iter()
            .filter(|(_, bounds)| crate::layout_probe::within(*bounds, bar))
            .map(|(text, bounds)| (text.as_str(), *bounds))
            .collect()
    }

    /// The hint words the bar shows, in order.
    fn words(&self) -> Vec<&str> {
        self.in_bar()
            .into_iter()
            .map(|(text, _)| text)
            .filter(|text| WORDS.contains(text))
            .collect()
    }

    fn bar(&self) -> Rectangle {
        self.bar.expect("the bar shows")
    }

    /// Whether any hint word shows anywhere in the window.
    fn any_word(&self) -> bool {
        self.texts
            .iter()
            .any(|(text, _)| WORDS.contains(&text.as_str()))
    }
}

fn right(bounds: Rectangle) -> f32 {
    bounds.x + bounds.width
}

fn center(bounds: Rectangle) -> f32 {
    bounds.x + bounds.width / 2.0
}

fn resize(app: &mut App, width: f32) {
    let _ = app.window_resized(Size::new(width, 800.0));
}

/// `ready`, with the bar showing for `style` and the panel's close done.
fn showing(style: Style) -> (tempfile::TempDir, App) {
    let (home, mut app) = ready();
    tap(&mut app, Raw::DPadDown);
    app.pad_hint = Some(style);
    app.panel_motion.snap(false);
    assert!(!app.panel_reserved());
    (home, app)
}

/// The bar's padding, as the fit counts it.
const PADDING: f32 = 12.0;

#[test]
fn a_narrower_window_drops_whole_hints_from_the_end() {
    let (_home, mut app) = showing(Style::GameCube);
    let full = seen(&app);
    let all = [
        "Open",
        "Back",
        "Favorite",
        "Mark",
        "Page",
        "Layout",
        "Full screen",
    ];
    assert_eq!(full.words(), all);
    // Each hint ends with its word, so the room the first `count` need is
    // up to that word's right edge and the padding after it.
    let bar = full.bar();
    let ends: Vec<f32> = full
        .in_bar()
        .into_iter()
        .filter(|(text, _)| WORDS.contains(text))
        .map(|(_, bounds)| right(bounds) - bar.x + PADDING)
        .collect();
    assert!(
        (right(bar) - bar.x - ends[6]).abs() < 0.5,
        "the bar ends there"
    );
    for count in [7, 3, 1] {
        let needed = ends[count - 1];
        resize(&mut app, needed + 2.0 * NOTES_MARGIN + 0.5);
        let roomy = seen(&app);
        assert_eq!(roomy.words(), all[..count], "{count} fit");
        let bar = roomy.bar();
        assert!(bar.x >= NOTES_MARGIN - 0.5, "{count}: {bar:?}");
        assert!(right(bar) <= app.window_size.width - NOTES_MARGIN + 0.5);
        // A pixel less and the last of them leaves, whole.
        resize(&mut app, needed + 2.0 * NOTES_MARGIN - 1.0);
        let tight = seen(&app);
        assert_eq!(tight.words(), all[..count - 1], "{count} less one");
        if count == 1 {
            assert_eq!(tight.bar, None, "nothing fits, nothing draws");
            assert!(!tight.any_word());
        }
    }
}

#[test]
fn page_leaves_whole() {
    let (_home, mut app) = showing(Style::Xbox);
    let full = seen(&app);
    let bar = full.bar();
    let lb = full
        .in_bar()
        .into_iter()
        .find(|(text, _)| *text == "LB")
        .expect("Page's LB")
        .1;
    // Page's two buttons sit 4 px apart: LB's text, its pill's 6 px
    // padding, the gap, then RB's padding and text.
    let rb = full
        .in_bar()
        .into_iter()
        .find(|(text, _)| *text == "RB")
        .expect("Page's RB")
        .1;
    assert!(
        (rb.x - right(lb) - (6.0 + 4.0 + 6.0)).abs() <= 0.5,
        "{lb:?} {rb:?}"
    );
    // Room for LB and more, but not all of Page.
    resize(
        &mut app,
        right(lb) - bar.x + 20.0 + PADDING + 2.0 * NOTES_MARGIN,
    );
    let cut = seen(&app);
    assert_eq!(cut.words(), ["Open", "Back", "Favorite", "Mark", "Random"]);
    let texts: Vec<&str> = cut.in_bar().into_iter().map(|(text, _)| text).collect();
    assert!(
        !texts.contains(&"LB") && !texts.contains(&"RB"),
        "{texts:?}"
    );
    assert!(!cut.texts.iter().any(|(text, _)| text == "LB"));
}

#[test]
fn a_larger_interface_size_fits_fewer_hints() {
    let (_home, mut app) = showing(Style::Xbox);
    assert_eq!(seen(&app).words().len(), 8);
    // The same window at 200% has half the logical pixels.
    app.interface_size = 200;
    let _ = app.window_resized(crate::app_window::launch_size(200));
    assert_eq!(app.window_size.width, 600.0);
    let big = seen(&app);
    let words = big.words();
    assert!(!words.is_empty() && words.len() < 8, "{words:?}");
    assert_eq!(words, WORDS_XBOX[..words.len()]);
    assert!(right(big.bar()) <= 600.0 - NOTES_MARGIN);
}

const WORDS_XBOX: [&str; 8] = [
    "Open",
    "Back",
    "Favorite",
    "Mark",
    "Random",
    "Page",
    "Layout",
    "Full screen",
];

#[test]
fn with_no_panel_the_bar_centers_in_the_window() {
    let (_home, app) = showing(Style::Xbox);
    let bar = seen(&app).bar();
    assert!(
        (center(bar) - app.window_size.width / 2.0).abs() <= 0.5,
        "{bar:?}"
    );
    assert!((bar.y + bar.height - (800.0 - NOTES_MARGIN)).abs() <= 0.5);
    assert_eq!(bar.height, HINT_HEIGHT);
}

/// `showing`, with alpha's panel open and docked.
fn docked(style: Style) -> (tempfile::TempDir, App) {
    let (home, mut app) = showing(style);
    tap(&mut app, Raw::South);
    app.pad_hint = Some(style);
    assert!(app.panel_open);
    app.panel_motion.snap(true);
    (home, app)
}

/// The room between the sidebar and the docked panel, and its center.
fn room(app: &App) -> (f32, f32) {
    let left = crate::sidebar::WIDTH;
    let right = app.window_size.width - DOCK_WIDTH;
    (right - left - 2.0 * NOTES_MARGIN, (left + right) / 2.0)
}

#[test]
fn beside_a_docked_panel_the_bar_centers_in_the_room_left_of_it() {
    let (_home, app) = docked(Style::Xbox);
    let shown = seen(&app);
    let bar = shown.bar();
    let panel_left = app.window_size.width - DOCK_WIDTH;
    // The panel really is there: its text starts right of the bar.
    assert!(
        shown
            .texts
            .iter()
            .any(|(text, bounds)| text == "alpha" && bounds.x >= panel_left),
        "the panel's title"
    );
    assert!(right(bar) < panel_left, "{bar:?}");
    assert!(bar.x > crate::sidebar::WIDTH);
    let (width, middle) = room(&app);
    assert_eq!(width, 598.0);
    assert!((center(bar) - middle).abs() <= 0.5, "{bar:?}");
    assert_eq!(
        shown.words(),
        ["Play", "Back", "Favorite", "Mark", "Random", "Page"]
    );
}

#[test]
fn a_panel_a_card_hides_keeps_the_narrow_room() {
    let (_home, mut app) = coverflow_ready();
    tap(&mut app, Raw::DPadRight);
    tap(&mut app, Raw::South);
    app.panel_motion.snap(true);
    tap(&mut app, Raw::DPadRight);
    assert!(app.panel_hidden_by_card());
    let _ = app.window_resized(Size::new(1200.0, 800.0));
    let asked = words_of(&app);
    assert_eq!(
        asked,
        ["Open", "Back", "Random", "Page", "Layout", "Full screen"]
    );
    let shown = seen(&app);
    let bar = shown.bar();
    let (_, middle) = room(&app);
    assert!((center(bar) - middle).abs() <= 0.5, "{bar:?}");
    assert!(right(bar) < app.window_size.width - DOCK_WIDTH);
    assert_eq!(shown.words(), asked);
    // At 150% the room holds only the first hints, and the bar stays in it.
    app.interface_size = 150;
    let _ = app.window_resized(crate::app_window::launch_size(150));
    let shown = seen(&app);
    let bar = shown.bar();
    let words = shown.words();
    assert!(!words.is_empty() && words.len() < asked.len(), "{words:?}");
    assert_eq!(words, asked[..words.len()]);
    let (width, middle) = room(&app);
    assert!(bar.width <= width);
    assert!((center(bar) - middle).abs() <= 0.5, "{bar:?}");
}

#[test]
fn a_room_of_zero_or_less_draws_nothing() {
    let (_home, mut app) = docked(Style::Xbox);
    for width in [
        crate::sidebar::WIDTH + DOCK_WIDTH + 2.0 * NOTES_MARGIN,
        400.0,
    ] {
        resize(&mut app, width);
        assert!(room(&app).0 <= 0.0);
        assert!(app.pad_hint_shown());
        let shown = seen(&app);
        assert_eq!(shown.bar, None, "{width}");
        assert!(!shown.any_word(), "{width}");
    }
}

#[test]
fn the_other_toasts_stay_centered_in_the_window_above_the_bar() {
    let (_home, mut app) = docked(Style::Xbox);
    let notice = "A notice over the library";
    app.feature_notice = Some((notice.to_owned(), std::time::Instant::now()));
    let shown = seen(&app);
    let (_, bounds) = shown
        .texts
        .iter()
        .find(|(text, _)| text == notice)
        .expect("the notice");
    assert!(
        (center(*bounds) - app.window_size.width / 2.0).abs() <= 0.5,
        "{bounds:?}"
    );
    // Its 12 px padding, then the gap and the bar's slot under it.
    let bottom = bounds.y + bounds.height + 12.0;
    let slot = 800.0 - NOTES_MARGIN - HINT_HEIGHT - 8.0;
    assert!((bottom - slot).abs() <= 0.5, "{bottom} {slot}");
    assert!(bottom < shown.bar().y);
}

#[test]
fn a_bar_with_no_room_for_its_first_hint_draws_no_pixel() {
    let size = Size::new(240.0, 60.0);
    let drawn = |room: f32| {
        let mut bar: iced::Element<'_, Message> = crate::pad::glyph::bar(
            all_hints(),
            Style::Xbox,
            crate::theme::Palette::DARK,
            room,
        );
        crate::layout_probe::render(&mut bar, size)
            .chunks(4)
            .filter(|pixel| pixel[3] != 0)
            .count()
    };
    // Room for Open alone draws it; a pixel less draws nothing at all, no
    // empty pill and no shadow.
    let (_home, app) = showing(Style::Xbox);
    let seen = seen(&app);
    let open = seen
        .in_bar()
        .into_iter()
        .find(|(text, _)| *text == "Open")
        .expect("Open")
        .1;
    let needed = right(open) - seen.bar().x + PADDING;
    assert!(drawn(needed + 0.5) > 0);
    assert_eq!(drawn(needed - 1.0), 0);
    assert_eq!(drawn(0.0), 0);
    assert_eq!(drawn(-100.0), 0);
}
