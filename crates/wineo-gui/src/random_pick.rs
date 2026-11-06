//! The toolbar's dice button and the R key: pick one shown game at random,
//! select it, open its panel, and scroll it into view. It never launches.

use std::collections::HashSet;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::button;
use iced::widget::{column, container, row, text};
use iced::{Center, Element, Fill, Point, Rectangle, Task};
use wineo_core::library::RandomPool;

use crate::grid::{ACTION_MENU_PADDING, menu_position_of};
use crate::library::Card;
use crate::library_menu::{anchor_under, check_box, icon, menu_row};
use crate::sort_control::Field;
use crate::{App, Message, focus_button, ui, zoom_control};

pub(crate) const BUTTON_ID: &str = "library-random";
pub(crate) const LABEL: &str = "Pick one at random (right-click for options)";
const LAUNCHABLE_LABEL: &str = "Only games that can launch";
const LAUNCHABLE_ID: &str = "library-random-launchable";

/// A die showing five, with the pips filled so the tint reads at 16 px.
const DICE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="2" y="2" width="12" height="12" rx="2.5" fill="none" stroke="#000" stroke-width="1.6"/><circle cx="5.5" cy="5.5" r="1.1" fill="#000"/><circle cx="10.5" cy="5.5" r="1.1" fill="#000"/><circle cx="8" cy="8" r="1.1" fill="#000"/><circle cx="5.5" cy="10.5" r="1.1" fill="#000"/><circle cx="10.5" cy="10.5" r="1.1" fill="#000"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Pick,
    /// The pick is selected and the panel is opening, so the grid knows
    /// where its card will be.
    Landed,
    /// A right-click, Shift+F10, or the Menu key on the dice, which opens
    /// the menu under it once its bounds are found.
    OpenMenu,
    /// The dice's bounds, found for the open request with this number.
    OpenUnder(u64, Option<Rectangle>),
    /// A click outside the menu.
    CloseMenu,
    /// The "Only games that can launch" row.
    ToggleLaunchable,
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The reply always arrives so it can clear its request;
        // `OpenUnder` checks the window itself.
        !matches!(self, Self::Landed | Self::OpenUnder(..))
    }
}

/// A shown card, as the pick sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry<'a> {
    pub(crate) name: &'a str,
    pub(crate) launchable: bool,
}

/// The game to show: one chance per game, however many cards show it. With
/// `Launchable` the draw is from the ones that can launch, or from all of
/// them when none can; with `Any` it is from all of them. It skips
/// `current` unless that leaves nothing.
pub(crate) fn choose<'a>(
    shown: &[Entry<'a>],
    current: Option<&str>,
    pool: RandomPool,
    rng: &mut fastrand::Rng,
) -> Option<&'a str> {
    let mut seen = HashSet::new();
    let games: Vec<Entry<'a>> = shown
        .iter()
        .copied()
        .filter(|entry| seen.insert(entry.name))
        .collect();
    let launchable: Vec<Entry<'a>> = games
        .iter()
        .copied()
        .filter(|entry| entry.launchable)
        .collect();
    let candidates = if pool == RandomPool::Launchable && !launchable.is_empty() {
        launchable
    } else {
        games
    };
    let others: Vec<Entry<'a>> = candidates
        .iter()
        .copied()
        .filter(|entry| Some(entry.name) != current)
        .collect();
    let candidates = if others.is_empty() {
        candidates
    } else {
        others
    };
    if candidates.is_empty() {
        return None;
    }
    Some(candidates[rng.usize(..candidates.len())].name)
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Pick => app.pick_random(),
        Event::Landed => app.random_landed(),
        Event::OpenMenu => {
            if app.random_menu {
                return Task::none();
            }
            crate::library_menu::request_bounds(
                &mut app.random_opening,
                BUTTON_ID,
                |request, bounds| Message::Random(Event::OpenUnder(request, bounds)),
            )
        }
        Event::OpenUnder(request, bounds) => {
            // Whatever took the window since the press keeps it.
            if !app.random_opening.answer(request) || !app.library_menu_may_open() {
                return Task::none();
            }
            match bounds {
                Some(bounds) => open_menu(app, anchor_under(bounds)),
                None => Task::none(),
            }
        }
        Event::CloseMenu => {
            app.random_opening.cancel();
            app.random_menu = false;
            Task::none()
        }
        Event::ToggleLaunchable => {
            app.random_pool = match app.random_pool {
                RandomPool::Launchable => RandomPool::Any,
                RandomPool::Any => RandomPool::Launchable,
            };
            app.save_field(Field::Random(app.random_pool))
        }
    }
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.close_library_menus();
    app.random_menu = true;
    app.random_menu_position = position;
    // Focus moves into the menu, so a second Enter or Space toggles the row
    // instead of opening it again.
    iced::widget::operation::focus(iced::widget::Id::new(LAUNCHABLE_ID))
}

/// Keys while the menu is open: Escape closes it and gives the dice its
/// focus back, Tab and the arrows stay on its one row, and the row takes
/// Enter and Space itself. Nothing else reaches the grid behind it.
pub(crate) fn key(app: &mut App, key: &Key, _modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => {
            app.random_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(BUTTON_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            iced::widget::operation::focus(iced::widget::Id::new(LAUNCHABLE_ID))
        }
        _ => Task::none(),
    }
}

/// The dice button, for the toolbar after Filters. Nothing shown, nothing
/// to pick.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    // Only what's in view can come up (plan 0062, question 22).
    let enabled = !app.grid_places().ids.is_empty();
    let ink = if enabled {
        palette.foreground
    } else {
        palette.muted
    };
    let message = Message::Random(Event::Pick);
    let dice = button(container(icon(DICE, ink, 16.0)).center(Fill))
        .width(ui::BUTTON_HEIGHT)
        .height(ui::BUTTON_HEIGHT)
        .padding(0)
        .on_press_maybe(enabled.then(|| message.clone()))
        .style(move |_, status| ui::secondary_style(palette, status));
    focus_button::with_menu(
        zoom_control::with_tooltip(dice.into(), LABEL, palette),
        enabled,
        BUTTON_ID,
        message,
        Message::Random(Event::OpenMenu),
    )
}

/// Wider than the action menu so its one label stays on a line.
const MENU_WIDTH: f32 = 272.0;

fn menu_height() -> f32 {
    2.0 * ACTION_MENU_PADDING + ui::BUTTON_HEIGHT
}

impl App {
    /// The menu shows only over the Library itself.
    pub(crate) fn random_menu_shown(&self) -> bool {
        self.random_menu
            && self.section == crate::sidebar::Section::Library
            && !self.panel_hidden()
            && self.hook_stop_confirm.is_none()
    }

    /// The open dice menu over the page, placed like the Sort menu.
    pub(crate) fn random_menu_view(&self) -> [Element<'_, Message>; 2] {
        let palette = self.palette;
        let height = menu_height();
        let position = menu_position_of(
            self.random_menu_position,
            self.window_size,
            MENU_WIDTH,
            height,
        );
        let rows = column![menu_row(
            row![
                check_box(self.random_pool == RandomPool::Launchable, palette),
                text(LAUNCHABLE_LABEL).size(14),
            ]
            .spacing(8)
            .align_y(Center)
            .into(),
            palette,
            false,
            LAUNCHABLE_ID,
            Message::Random(Event::ToggleLaunchable),
        )];
        let menu = container(rows)
            .padding(ACTION_MENU_PADDING)
            .width(MENU_WIDTH)
            .height(height)
            .style(move |_| crate::library_menu::surface_style(palette));
        crate::library_menu::layers(menu.into(), position, Message::Random(Event::CloseMenu))
    }
}

impl App {
    /// Whether Play would be on for `card` now, by the panel's own rule.
    /// Other cards use their default target, since `target` is the selected
    /// card's. A running operation elsewhere is left out: it would rule out
    /// every card at once and the skip would mean nothing.
    fn random_launchable(&self, card: &Card) -> bool {
        crate::panel::play_enabled(
            card,
            self.tool_blocks_launch(&card.name),
            card.default_target.as_deref(),
        )
    }

    /// The game a pick would show, from the games in view now, once each
    /// however many copies show (plan 0062, question 22).
    pub(crate) fn random_choice(&self, rng: &mut fastrand::Rng) -> Option<String> {
        let visible = self.visible_names();
        let entries: Vec<Entry<'_>> = self
            .filtered()
            .into_iter()
            .filter(|card| visible.contains(&card.name))
            .map(|card| Entry {
                name: &card.name,
                launchable: self.random_launchable(card),
            })
            .collect();
        choose(&entries, self.selected.as_deref(), self.random_pool, rng).map(str::to_owned)
    }

    fn pick_random(&mut self) -> Task<Message> {
        let Some(name) = self.random_choice(&mut fastrand::Rng::new()) else {
            return Task::none();
        };
        // Selecting the open panel's own card would close it, and a pick
        // only ever opens.
        let select = if self.panel_open && self.selected.as_deref() == Some(&name) {
            Task::none()
        } else {
            self.select_card(name)
        };
        // The dock's motion starts after this update, so the scroll waits
        // for a message that comes once it has.
        select.chain(Task::done(Message::Random(Event::Landed)))
    }

    fn random_landed(&mut self) -> Task<Message> {
        if self.grid_motion.active() {
            // The dock keeps its anchor while it moves, so the card is
            // revealed when the motion ends.
            self.scroll_selection()
        } else {
            Task::batch([self.scroll_to_landing(), self.load_visible_art()])
        }
    }
}

#[cfg(test)]
mod tests;
