//! The toolbar's dice button and the R key: pick one shown game at random,
//! select it, open its panel, and scroll it into view. It never launches.

use std::collections::HashSet;

use iced::widget::{button, container};
use iced::{Element, Fill, Task};

use crate::library::Card;
use crate::library_menu::icon;
use crate::{App, Message, focus_button, ui, zoom_control};

pub(crate) const BUTTON_ID: &str = "library-random";
pub(crate) const LABEL: &str = "Pick one at random";

/// A die showing five, with the pips filled so the tint reads at 16 px.
const DICE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="2" y="2" width="12" height="12" rx="2.5" fill="none" stroke="#000" stroke-width="1.6"/><circle cx="5.5" cy="5.5" r="1.1" fill="#000"/><circle cx="10.5" cy="5.5" r="1.1" fill="#000"/><circle cx="8" cy="8" r="1.1" fill="#000"/><circle cx="5.5" cy="10.5" r="1.1" fill="#000"/><circle cx="10.5" cy="10.5" r="1.1" fill="#000"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Pick,
    /// The pick is selected and the panel is opening, so the grid knows
    /// where its card will be.
    Landed,
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        matches!(self, Self::Pick)
    }
}

/// A shown card, as the pick sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry<'a> {
    pub(crate) name: &'a str,
    pub(crate) launchable: bool,
}

/// The game to show: one chance per game, however many cards show it, from
/// the ones that can launch, or from all of them when none can. It skips
/// `current` unless that leaves nothing.
pub(crate) fn choose<'a>(
    shown: &[Entry<'a>],
    current: Option<&str>,
    rng: &mut fastrand::Rng,
) -> Option<&'a str> {
    let mut seen = HashSet::new();
    let games: Vec<Entry<'a>> = shown
        .iter()
        .copied()
        .filter(|entry| seen.insert(entry.name))
        .collect();
    let mut pool: Vec<Entry<'a>> = games
        .iter()
        .copied()
        .filter(|entry| entry.launchable)
        .collect();
    if pool.is_empty() {
        pool = games;
    }
    let others: Vec<Entry<'a>> = pool
        .iter()
        .copied()
        .filter(|entry| Some(entry.name) != current)
        .collect();
    let pool = if others.is_empty() { pool } else { others };
    if pool.is_empty() {
        return None;
    }
    Some(pool[rng.usize(..pool.len())].name)
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Pick => app.pick_random(),
        Event::Landed => app.random_landed(),
    }
}

/// The dice button, for the toolbar after Filters. Nothing shown, nothing
/// to pick.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    let enabled = !app.filtered().is_empty();
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
    focus_button::control(
        zoom_control::with_tooltip(dice.into(), LABEL, palette),
        enabled,
        BUTTON_ID,
        message,
    )
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

    /// The game a pick would show, from the cards shown now.
    pub(crate) fn random_choice(&self, rng: &mut fastrand::Rng) -> Option<String> {
        let entries: Vec<Entry<'_>> = self
            .filtered()
            .into_iter()
            .map(|card| Entry {
                name: &card.name,
                launchable: self.random_launchable(card),
            })
            .collect();
        choose(&entries, self.selected.as_deref(), rng).map(str::to_owned)
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
