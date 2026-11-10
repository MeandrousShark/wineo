//! Message handlers for the tray icon: its menu, its clicks, the light poll,
//! and hiding and showing the window.

use std::collections::BTreeSet;

use iced::widget::scrollable;
use iced::{Task, keyboard, window};
use jiff::Timestamp;
use wineo_core::library::CloseWindow;

use crate::library::Card;
use crate::tray::{self, Event, Menu, Row};
use crate::{App, Message, OperationKind, SCROLL_ID};

impl App {
    /// The tray icon is on and a host shows it, so the window can go
    /// without leaving the app unreachable.
    pub(crate) fn tray_shown(&self) -> bool {
        self.tray.service && self.desktop.tray && self.tray.available
    }

    /// The window's close button hides instead of quitting.
    pub(crate) fn keeps_running(&self) -> bool {
        self.desktop.close == CloseWindow::Tray && self.tray_shown() && !self.closing
    }

    /// Whether `name` runs, by the light poll while it stands in for the
    /// rescan and by its card otherwise.
    fn tray_running(&self, card: &Card) -> bool {
        match &self.tray.polled {
            Some(running) => running.contains(&card.name),
            None => card.active(),
        }
    }

    fn last_played(&self, card: &Card) -> Option<Timestamp> {
        let polled = self.tray.played.get(&card.name).copied();
        polled.max(card.last_played_at)
    }

    /// The menu for what the app knows now: the three most recently played
    /// things that aren't hidden or running, newest first, and what runs in
    /// the library's sort order.
    pub(crate) fn tray_menu(&self) -> Menu {
        let starting = self
            .operation
            .as_ref()
            .filter(|operation| operation.kind == OperationKind::Start)
            .map(|operation| operation.name.as_str());
        let trashed = self.toast.as_ref().and_then(crate::removal::Toast::trashed);
        let mut played: Vec<(Timestamp, &Card)> = self
            .cards
            .iter()
            .filter(|card| {
                let name = Some(card.name.as_str());
                !card.hidden && !self.tray_running(card) && starting != name && trashed != name
            })
            .filter_map(|card| Some((self.last_played(card)?, card)))
            .collect();
        played.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        let play = played
            .into_iter()
            .take(tray::PLAY_ROWS)
            .map(|(_, card)| Row::play(&card.name, &card.display_title))
            .collect();
        let running: Vec<Card> = self
            .cards
            .iter()
            .filter(|card| self.tray_running(card))
            .cloned()
            .collect();
        let shown = crate::order::Filters {
            show_hidden: true,
            ..crate::order::Filters::default()
        };
        let sorted = crate::order::Order::sorted(crate::order::Of {
            cards: &running,
            search: "",
            query: self.query,
            filters: &shown,
            group: wineo_core::library::GroupBy::None,
            collapsed: &BTreeSet::new(),
        });
        let stop = sorted
            .indices()
            .iter()
            .take(tray::STOP_ROWS)
            .map(|&index| Row::stop(&running[index].name, &running[index].display_title))
            .collect();
        Menu {
            play,
            stop,
            more: running.len() > tray::STOP_ROWS,
        }
    }

    /// Hands the tray the menu for now. A menu that didn't change isn't
    /// sent, since every send redraws it on the host.
    pub(crate) fn sync_tray_menu(&mut self) {
        let menu = self.tray_menu();
        self.tray.menu.send_if_modified(|shown| {
            if *shown == menu {
                false
            } else {
                *shown = menu;
                true
            }
        });
    }

    /// Follows General's saved `[desktop]` while the global Settings page
    /// has read it, like the appearance.
    pub(crate) fn sync_desktop(&mut self) {
        if let Some(desktop) = self.settings.as_ref().and_then(|page| page.desktop()) {
            self.desktop = desktop;
        }
        // The service stops with the setting, and a new one says again.
        if !self.desktop.tray {
            self.tray.available = false;
        }
    }

    pub(crate) fn tray_event(&mut self, event: Event) -> Task<Message> {
        if self.exiting {
            return Task::none();
        }
        match event {
            Event::Available(available) => {
                self.tray.available = available;
                Task::none()
            }
            Event::Open => self.show_window(),
            Event::Add => {
                let show = self.show_window();
                // Over a flow or page the window just shows; Add itself
                // refuses while anything else is busy.
                if self.page_open() {
                    show
                } else {
                    Task::batch([show, self.update(Message::Add)])
                }
            }
            Event::Play(name) => self.start_named(name),
            Event::Stop(name) => self.stop_named(name),
            Event::Quit => {
                let task = self.quit_requested();
                // Foreground work asks first, and the question needs the
                // window.
                if self.close_prompt && !self.close_quietly {
                    Task::batch([self.show_window(), task])
                } else {
                    task
                }
            }
        }
    }

    /// A page, flow, or dialog is over the library, which a tray click must
    /// never throw away.
    pub(crate) fn page_open(&self) -> bool {
        self.panel_hidden() || self.input_covered()
    }

    /// A tray Play or Stop that can't go straight through: the window shows
    /// on that card with its panel open, where the reason or choice is, or
    /// as it is with `notice` when a page or flow is open.
    pub(crate) fn tray_blocked(&mut self, name: String, notice: &str) -> Task<Message> {
        let show = self.show_window();
        if self.page_open() {
            self.flag_notice(notice.to_owned());
            return show;
        }
        let open = if self.selected.as_deref() == Some(&name) && self.panel_open {
            Task::none()
        } else {
            self.update(Message::OpenCard(name))
        };
        Task::batch([show, open])
    }

    /// Shows the window where it was, or raises it if it's open. A raise
    /// can't pass the tray's activation token, since iced takes none, so a
    /// compositor may refuse to focus it.
    pub(crate) fn show_window(&mut self) -> Task<Message> {
        let shown = match self.main_window {
            Some(id) => window::gain_focus(id),
            None => {
                let (id, open) = crate::open_main_window(self.window_size);
                self.main_window = Some(id);
                // The new window's scrollable starts at the top.
                let scroll = iced::widget::operation::scroll_to(
                    iced::widget::Id::new(SCROLL_ID),
                    scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: self.scroll_offset,
                    },
                );
                // A compositor that refuses the raise sends no Focused.
                let reload = if self.focused {
                    Task::none()
                } else {
                    self.focused = true;
                    self.update(Message::Refresh)
                };
                Task::batch([open.chain(scroll), reload])
            }
        };
        Task::batch([shown, self.release_held_error()])
    }

    /// Closes the window and keeps running in the tray. The window's state
    /// stays, so it comes back where it was.
    pub(crate) fn hide_window(&mut self) -> Task<Message> {
        let drag = self.end_zoom_drag();
        let Some(id) = self.main_window.take() else {
            return drag;
        };
        self.focused = false;
        self.modifiers = keyboard::Modifiers::empty();
        self.zooming.modifiers_changed(self.modifiers);
        self.hovered = None;
        self.star_hovered = None;
        Task::batch([drag, window::close(id)])
    }

    /// Puts a tray stop's held error in its card's panel, once the library
    /// is what the window shows.
    pub(crate) fn release_held_error(&mut self) -> Task<Message> {
        if self.main_window.is_none() || self.page_open() {
            return Task::none();
        }
        let Some((name, error)) = self.tray.held_error.take() else {
            return Task::none();
        };
        let open = if self.selected.as_deref() == Some(&name) && self.panel_open {
            Task::none()
        } else {
            self.update(Message::OpenCard(name.clone()))
        };
        if self.selected.as_deref() == Some(&name) {
            self.panel_error = Some(error);
        }
        open
    }

    /// The light poll's running set. What it no longer lists has stopped,
    /// so its play time is read again and the Play rows reorder.
    pub(crate) fn tray_polled(&mut self, running: BTreeSet<String>) -> Task<Message> {
        let stopped: Vec<String> = self
            .cards
            .iter()
            .filter(|card| self.tray_running(card) && !running.contains(&card.name))
            .map(|card| card.name.clone())
            .collect();
        self.tray.polled = Some(running);
        if stopped.is_empty() {
            return Task::none();
        }
        Task::batch(stopped.into_iter().map(|name| {
            let paths = self.paths.clone();
            let read = name.clone();
            crate::task::blocking_value(
                move || tray::last_played(&paths, &read),
                move |played| Message::TrayPlayed(name, played.ok().flatten()),
            )
        }))
    }

    pub(crate) fn tray_played(&mut self, name: String, played: Option<Timestamp>) {
        if let Some(played) = played {
            self.tray.played.insert(name, played);
        }
    }
}
