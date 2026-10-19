//! Favoriting and hiding a card: the context menu rows, the panel star, the
//! F key, the poster marks, and saving each change to `library.toml`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use iced::widget::{button, column, container, tooltip};
use iced::{Color, Element, Fill, Task, border};
use wineo_core::library;
use wineo_core::paths::Paths;

use crate::library::Card;
use crate::library_menu::icon;
use crate::ui;
use crate::{App, Message, focus_button, rescan};

pub(crate) const PANEL_FAVORITE_ID: &str = "panel-favorite";
const NOTICE_TIME: Duration = Duration::from_secs(8);

const STAR: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 1.6l1.95 4.1 4.45.55-3.3 3.05.85 4.45L8 11.55l-3.95 2.2.85-4.45-3.3-3.05 4.45-.55z" fill="#000" stroke="#000" stroke-width="1" stroke-linejoin="round"/></svg>"##;
const STAR_OUTLINE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 1.6l1.95 4.1 4.45.55-3.3 3.05.85 4.45L8 11.55l-3.95 2.2.85-4.45-3.3-3.05 4.45-.55z" fill="none" stroke="#000" stroke-width="1.3" stroke-linejoin="round"/></svg>"##;
const EYE_OFF: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8s-2.4 4.5-6.5 4.5S1.5 8 1.5 8z" fill="none" stroke="#000" stroke-width="1.4" stroke-linejoin="round"/><circle cx="8" cy="8" r="2" fill="none" stroke="#000" stroke-width="1.4"/><path d="M2.5 13.5l11-11" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round"/></svg>"##;

/// The poster marks' size and their backdrop, which keeps them readable on
/// light and dark art alike.
const MARK_SIZE: f32 = 24.0;
const MARK_ICON: f32 = 14.0;
const MARK_BACKDROP: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Flag {
    Favorite,
    Hidden,
}

impl Flag {
    pub(crate) fn of(self, card: &Card) -> bool {
        match self {
            Self::Favorite => card.favorite,
            Self::Hidden => card.hidden,
        }
    }

    fn set(self, card: &mut Card, value: bool) {
        match self {
            Self::Favorite => card.favorite = value,
            Self::Hidden => card.hidden = value,
        }
    }

    fn in_entry(self, entry: &library::Entry) -> bool {
        match self {
            Self::Favorite => entry.favorite,
            Self::Hidden => entry.hidden,
        }
    }

    /// What toggling a card whose flag is `on` does, for the context menu
    /// row and the panel star's tooltip.
    pub(crate) fn action_label(self, on: bool) -> &'static str {
        match (self, on) {
            (Self::Favorite, false) => "Add to favorites",
            (Self::Favorite, true) => "Remove from favorites",
            (Self::Hidden, false) => "Hide",
            (Self::Hidden, true) => "Unhide",
        }
    }

    /// The notice's lead-in when setting the flag to `value` fails.
    fn failure(self, value: bool, title: &str) -> String {
        match (self, value) {
            (Self::Favorite, true) => format!("Couldn't add {title} to favorites"),
            (Self::Favorite, false) => format!("Couldn't remove {title} from favorites"),
            (Self::Hidden, true) => format!("Couldn't hide {title}"),
            (Self::Hidden, false) => format!("Couldn't unhide {title}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// The context menu row or the panel star for this card.
    Toggle(String, Flag),
    /// A write finished.
    Saved(Saved),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        matches!(self, Self::Toggle(..))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Saved {
    pub(crate) name: String,
    pub(crate) flag: Flag,
    /// The value written.
    pub(crate) value: bool,
    pub(crate) result: Result<(), String>,
    /// The flag as the file has it once the write is over, or None when the
    /// worker died before it could look.
    pub(crate) on_disk: Option<bool>,
}

/// The writes in flight, one per card and flag. A toggle made meanwhile is
/// written when the one in flight returns, so the last state wins.
#[derive(Debug, Default)]
pub(crate) struct Writes(BTreeMap<(String, Flag), Pending>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pending {
    pub(crate) writing: bool,
    /// What to write once `writing` lands, when the window wants otherwise.
    pub(crate) next: Option<bool>,
}

impl Pending {
    fn wanted(self) -> bool {
        self.next.unwrap_or(self.writing)
    }
}

impl Writes {
    /// A write is in flight, which closing the window waits for.
    pub(crate) fn pending(&self) -> bool {
        !self.0.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, name: &str, flag: Flag) -> Option<Pending> {
        self.0.get(&(name.to_owned(), flag)).copied()
    }

    /// Puts the window's pending choices over cards just read, so a read
    /// made before a write landed doesn't undo it on screen meanwhile.
    pub(crate) fn overlay(&self, cards: &mut [Card]) {
        for ((name, flag), pending) in &self.0 {
            if let Some(card) = cards.iter_mut().find(|card| &card.name == name) {
                flag.set(card, pending.wanted());
            }
        }
    }
}

/// Writes one flag, then rereads the rescan's cache whatever happened and
/// returns the flag as the file has it now.
pub(crate) fn save(
    paths: &Paths,
    watch: &rescan::Watch,
    name: &str,
    flag: Flag,
    value: bool,
) -> (Result<(), String>, bool) {
    let result = match flag {
        Flag::Favorite => library::set_favorite(paths, name, value),
        Flag::Hidden => library::set_hidden(paths, name, value),
    };
    let entry = watch.refresh_library_entry(paths, name);
    (
        result
            .map(|_| ())
            .map_err(|error| crate::sort_control::error_text(paths, &error)),
        flag.in_entry(&entry),
    )
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Toggle(name, flag) => {
            app.action_menu = None;
            app.toggle_flag(&name, flag)
        }
        Event::Saved(saved) => app.flag_saved(saved),
    }
}

impl App {
    /// Flips `flag` on the card `name`: the card changes at once and the
    /// write runs off the UI thread.
    pub(crate) fn toggle_flag(&mut self, name: &str, flag: Flag) -> Task<Message> {
        let Some(card) = self.cards.iter().find(|card| card.name == name) else {
            return Task::none();
        };
        let value = !flag.of(card);
        // The banner already says the file can't be used, so nothing is
        // written and nothing changes.
        if let Some(problem) = &self.library_problem {
            let refused = wineo_core::error::Error::LibraryUnreadable {
                newer_version: problem.newer_version,
                source: problem.message().into(),
            };
            let notice = format!("{}: {refused}", flag.failure(value, &card.display_title));
            self.flag_notice(notice);
            return Task::none();
        }
        // Hiding the selected card hands its place to the one after it.
        let place = (flag == Flag::Hidden
            && value
            && !self.filters.show_hidden
            && self.selected.as_deref() == Some(name))
        .then(|| self.filtered().iter().position(|card| card.name == name))
        .flatten();
        let write = self.write_flag(name, flag, value);
        let shown = self.set_flag(name, flag, value);
        let selection = place.map_or_else(Task::none, |place| self.select_in_place(place));
        Task::batch([shown, selection, write])
    }

    /// Starts writing `value`, or queues it behind the write in flight.
    fn write_flag(&mut self, name: &str, flag: Flag, value: bool) -> Task<Message> {
        if let Some(pending) = self.flag_writes.0.get_mut(&(name.to_owned(), flag)) {
            pending.next = (value != pending.writing).then_some(value);
            return Task::none();
        }
        self.flag_writes.0.insert(
            (name.to_owned(), flag),
            Pending {
                writing: value,
                next: None,
            },
        );
        let paths = self.paths.clone();
        let watch = self.shown.clone();
        let name = name.to_owned();
        let reply = name.clone();
        crate::task::blocking_value(
            move || save(&paths, &watch, &name, flag, value),
            move |result| {
                let (result, on_disk) = match result {
                    Ok((result, on_disk)) => (result, Some(on_disk)),
                    Err(error) => (Err(error), None),
                };
                Message::Mark(Event::Saved(Saved {
                    name: reply,
                    flag,
                    value,
                    result,
                    on_disk,
                }))
            },
        )
    }

    fn flag_saved(&mut self, saved: Saved) -> Task<Message> {
        let next = self
            .flag_writes
            .0
            .remove(&(saved.name.clone(), saved.flag))
            .and_then(|pending| pending.next);
        if self.exiting {
            return Task::none();
        }
        let reads = self.outdate_reads();
        let shown = match saved.result {
            Ok(()) => match next {
                Some(next) => self.write_flag(&saved.name, saved.flag, next),
                // Normally what was written; another writer may have
                // changed it since.
                None => saved.on_disk.map_or_else(Task::none, |on_disk| {
                    self.set_flag(&saved.name, saved.flag, on_disk)
                }),
            },
            Err(error) => {
                // A toggle queued behind a failed write is dropped too: the
                // card goes back to what the file has.
                let title = self
                    .cards
                    .iter()
                    .find(|card| card.name == saved.name)
                    .map_or(saved.name.as_str(), |card| card.display_title.as_str());
                let notice = format!("{}: {error}", saved.flag.failure(saved.value, title));
                self.flag_notice(notice);
                match saved.on_disk {
                    Some(on_disk) => self.set_flag(&saved.name, saved.flag, on_disk),
                    None => self.reload_library_once(),
                }
            }
        };
        Task::batch([reads, shown])
    }

    /// Drops every library read that started before now, since it may have
    /// read the file before a write that just finished. The card already
    /// shows that write's result, so an older read must not replace it: the
    /// rescan's comes back stale, and an explicit reload starts over.
    fn outdate_reads(&mut self) -> Task<Message> {
        self.shown.reloading_library();
        if self.refreshing {
            self.reload_library()
        } else {
            Task::none()
        }
    }

    fn flag_notice(&mut self, notice: String) {
        self.feature_notice = Some((notice, Instant::now() + NOTICE_TIME));
    }

    /// Shows `value` on the card `name`. The rescan compares against it from
    /// now on, and the order is rebuilt; cards that moved snap, and the
    /// selection stays in view.
    fn set_flag(&mut self, name: &str, flag: Flag, value: bool) -> Task<Message> {
        let before: Vec<String> = self
            .filtered()
            .iter()
            .map(|card| card.name.clone())
            .collect();
        let Some(card) = self.cards.iter_mut().find(|card| card.name == name) else {
            return Task::none();
        };
        if flag.of(card) == value {
            return Task::none();
        }
        flag.set(card, value);
        self.shown.show_library(Ok(self.cards.clone()));
        self.reorder();
        let moved = {
            let after = self.filtered();
            after.len() != before.len()
                || after
                    .iter()
                    .zip(&before)
                    .any(|(card, name)| &card.name != name)
        };
        if !moved {
            return Task::none();
        }
        self.snap_grid();
        if self.selected_shown() {
            Task::batch([self.scroll_selection(), self.load_visible_art()])
        } else {
            self.load_visible_art()
        }
    }

    /// Selects the card now at `place` in the shown order, or the last one,
    /// or nothing when nothing is left.
    fn select_in_place(&mut self, place: usize) -> Task<Message> {
        let shown = self.filtered();
        let Some(next) = shown
            .get(place)
            .or(shown.last())
            .map(|card| card.name.clone())
        else {
            self.selected = None;
            self.close_panel();
            return Task::none();
        };
        self.selected = Some(next);
        self.selection_changed();
        Task::batch([
            self.scroll_selection(),
            self.load_visible_art(),
            self.load_panel_art(),
        ])
    }
}

/// The marks in a poster's top right corner: a star on a favorite and a
/// crossed eye on a hidden card. They stack, so a long GOG badge in the
/// other corner never runs into them.
pub(crate) fn poster_marks(card: &Card, alpha: f32) -> Option<Element<'static, Message>> {
    if !card.favorite && !card.hidden {
        return None;
    }
    let mut marks = column![].spacing(4);
    if card.favorite {
        marks = marks.push(mark(STAR, alpha));
    }
    if card.hidden {
        marks = marks.push(mark(EYE_OFF, alpha));
    }
    Some(
        container(marks)
            .width(Fill)
            .height(Fill)
            .align_right(Fill)
            .align_top(Fill)
            .padding(8)
            .into(),
    )
}

fn mark(bytes: &'static [u8], alpha: f32) -> Element<'static, Message> {
    container(icon(bytes, Color::WHITE.scale_alpha(alpha), MARK_ICON))
        .width(MARK_SIZE)
        .height(MARK_SIZE)
        .center(MARK_SIZE)
        .style(move |_| {
            container::Style::default()
                .background(Color::BLACK.scale_alpha(MARK_BACKDROP * alpha))
                .border(border::rounded(MARK_SIZE / 2.0))
        })
        .into()
}

impl App {
    /// The panel header's star, filled in the accent for a favorite.
    pub(crate) fn favorite_star<'a>(&self, card: &Card) -> Element<'a, Message> {
        let palette = self.palette;
        let star = if card.favorite {
            icon(STAR, palette.primary, 16.0)
        } else {
            icon(STAR_OUTLINE, palette.foreground, 16.0)
        };
        let toggle = Message::Mark(Event::Toggle(card.name.clone(), Flag::Favorite));
        let star = button(container(star).center(Fill))
            .width(ui::BUTTON_HEIGHT)
            .height(ui::BUTTON_HEIGHT)
            .padding(0)
            .on_press(toggle.clone())
            .style(move |_, status| ui::secondary_style(palette, status));
        let star = tooltip(
            star,
            container(iced::widget::text(Flag::Favorite.action_label(card.favorite)).size(13))
                .padding([4, 8]),
            tooltip::Position::Bottom,
        )
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(8).width(1).color(palette.border))
        })
        .gap(6);
        focus_button::control(star.into(), true, PANEL_FAVORITE_ID, toggle)
    }
}

#[cfg(test)]
mod tests;
