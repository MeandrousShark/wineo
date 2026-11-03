//! The toast that says Steam auto-import brought games in. A pass reports
//! app ids, but only a card knows its environment name and title, so the ids
//! wait for the reload that follows the pass and are resolved against its
//! cards.

use std::time::{Duration, Instant};

use iced::widget::{button, container, text};
use iced::{Element, border};

use crate::library::Card;
use crate::ui::secondary_style;
use crate::{App, Message, focus_button, sidebar};

const TIME: Duration = Duration::from_secs(8);
const ID: &str = "import-toast";
const MAX_WIDTH: f32 = 480.0;

#[derive(Debug, Default)]
pub(crate) struct ImportToast {
    /// App ids from the last pass whose cards the next reload shows. A failed
    /// reload leaves them for the next good one.
    awaiting: Vec<String>,
    shown: Option<Shown>,
}

#[derive(Debug)]
struct Shown {
    text: String,
    /// The environment a click selects. None for several games.
    target: Option<String>,
    /// Set once the Library is on screen, so the toast isn't missed behind
    /// another section.
    until: Option<Instant>,
}

impl ImportToast {
    /// A pass imported `ids`. A newer pass replaces the ones before it.
    pub(crate) fn await_cards(&mut self, ids: &[String]) {
        self.awaiting = ids.to_vec();
    }

    /// Turns the awaited ids into a toast, from the cards the reload found.
    /// A card that's gone since is left out.
    pub(crate) fn resolve(&mut self, cards: &[Card]) {
        // Two environments can share an app id; the first card stands for
        // it, since telling them apart would need the pass to say which it
        // made.
        let mut found: Vec<(&str, &str)> = Vec::new();
        for id in std::mem::take(&mut self.awaiting) {
            let card = cards
                .iter()
                .find(|card| card.steam.as_ref().is_some_and(|steam| steam.app_id == id));
            if let Some(card) = card
                && !found.iter().any(|(name, _)| *name == card.name)
            {
                found.push((&card.name, &card.display_title));
            }
        }
        self.shown = match found.as_slice() {
            [] => return,
            [(name, title)] => Some(Shown {
                text: format!("Imported {title}"),
                target: Some((*name).to_owned()),
                until: None,
            }),
            many => Some(Shown {
                text: format!("Imported {} Steam games", many.len()),
                target: None,
                until: None,
            }),
        };
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.shown.as_ref().and_then(|shown| shown.until)
    }

    /// Starts the clock the first time the Library is on screen, and drops
    /// the toast once the clock runs out.
    pub(crate) fn settle(&mut self, library_shown: bool, now: Instant) {
        let Some(shown) = &mut self.shown else {
            return;
        };
        match shown.until {
            None if library_shown => shown.until = Some(now + TIME),
            Some(until) if now >= until => self.shown = None,
            _ => {}
        }
    }

    /// Drops the toast if its time is up, without starting a waiting one.
    pub(crate) fn expire(&mut self, now: Instant) {
        if self.deadline().is_some_and(|until| now >= until) {
            self.shown = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> Option<&str> {
        self.shown.as_ref().map(|shown| shown.text.as_str())
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|shown| shown.until.is_none())
    }

    #[cfg(test)]
    pub(crate) fn expires_at(&self) -> Option<Instant> {
        self.deadline()
    }

    #[cfg(test)]
    pub(crate) fn set_deadline(&mut self, until: Instant) {
        if let Some(shown) = &mut self.shown {
            shown.until = Some(until);
        }
    }
}

impl App {
    /// The Library, with nothing replacing it, is what the user is looking at.
    fn library_shown(&self) -> bool {
        self.section == sidebar::Section::Library
            && self.settings.is_none()
            && self.art_page.is_none()
            && self.tool_page.is_none()
            && self.portable_page.is_none()
    }

    pub(crate) fn settle_import_toast(&mut self, now: Instant) {
        let shown = self.library_shown();
        self.import_toast.settle(shown, now);
    }

    /// Selects the one game it names, as clicking its card would.
    pub(crate) fn import_toast_clicked(&mut self) -> iced::Task<Message> {
        self.import_toast.expire(Instant::now());
        let Some(shown) = self
            .import_toast
            .shown
            .take_if(|shown| shown.target.is_some() && shown.until.is_some())
        else {
            return iced::Task::none();
        };
        match shown.target {
            Some(name) if self.cards.iter().any(|card| card.name == name) => self.select_card(name),
            _ => iced::Task::none(),
        }
    }

    pub(crate) fn import_toast_view(&self) -> Option<Element<'_, Message>> {
        let toast = self.import_toast.shown.as_ref()?;
        toast.until?;
        let palette = self.palette;
        let label = text(&toast.text)
            .wrapping(crate::removal::TOAST_WRAPPING)
            .color(palette.foreground);
        let body: Element<'_, Message> = if toast.target.is_some() {
            let enabled = !self.input_covered();
            focus_button::keyboard_control(
                button(label)
                    .padding(12)
                    .style(move |_, status| secondary_style(palette, status))
                    .on_press_maybe(enabled.then_some(Message::ImportToast))
                    .into(),
                enabled,
                ID,
                Message::ImportToast,
            )
        } else {
            container(label)
                .padding(12)
                .style(move |_| {
                    container::Style::default()
                        .background(palette.surface)
                        .border(border::rounded(8).width(1).color(palette.border))
                })
                .into()
        };
        Some(container(body).max_width(MAX_WIDTH).into())
    }
}
