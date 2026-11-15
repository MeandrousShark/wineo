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
        let trashed = self.trashed_name();
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
                if available {
                    Task::none()
                } else {
                    self.tray_lost()
                }
            }
            Event::Open => self.user_show(),
            Event::Add => {
                let show = self.show_window();
                // Over a flow or page the window just shows; Add itself
                // refuses while anything else is busy. The chooser claims
                // the window, so a held error waits for its card.
                let add = if self.page_open() {
                    Task::none()
                } else {
                    self.update(Message::Add)
                };
                let release = if self.flow.is_none() {
                    self.release_held_error()
                } else {
                    Task::none()
                };
                Task::batch([show, add, release])
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

    /// Another launch for this root, which exits after this, so it's the
    /// user asking for the window like Open wineo.
    pub(crate) fn bus_show(&mut self) -> Task<Message> {
        // A close that's waiting on writes or work still ends in an exit, and
        // the name is already free for a relaunch to start fresh.
        // Accepted race: a Show taken just before `closing` is set still shows.
        if self.closing || self.exiting {
            return Task::none();
        }
        self.user_show()
    }

    /// Open wineo and a relaunch: the window as it was, and a tray stop's
    /// held error if its card's panel can open.
    fn user_show(&mut self) -> Task<Message> {
        let show = self.show_window();
        Task::batch([show, self.release_held_error()])
    }

    /// A page, flow, or dialog is over the library, which a tray click must
    /// never throw away.
    pub(crate) fn page_open(&self) -> bool {
        self.panel_hidden() || self.input_covered()
    }

    /// The environment the Undo toast just moved to the trash, whose card
    /// stays until the library reloads.
    pub(crate) fn trashed_name(&self) -> Option<&str> {
        self.toast.as_ref().and_then(crate::removal::Toast::trashed)
    }

    /// A tray click for a card the menu offered but the library no longer
    /// has, renamed, removed, or just trashed. The window only shows.
    pub(crate) fn tray_card_gone(&self, name: &str) -> bool {
        self.trashed_name() == Some(name) || !self.cards.iter().any(|card| card.name == name)
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
            self.show_held_error(&name);
            Task::none()
        } else {
            self.update(Message::OpenCard(name))
        };
        Task::batch([show, open])
    }

    /// The user's own show: the tray's click or menu, or a blocked tray
    /// Play or Stop. It shows the window where it was, or raises it if it's
    /// open.
    pub(crate) fn show_window(&mut self) -> Task<Message> {
        self.user_opened_window();
        self.open_window()
    }

    /// Opens the window where it was, or raises it if it's open. A raise
    /// can't pass the tray's activation token, since iced takes none, so a
    /// compositor may refuse to focus it.
    pub(crate) fn open_window(&mut self) -> Task<Message> {
        match self.main_window {
            Some(id) => window::gain_focus(id),
            None => {
                let (id, open) = crate::open_main_window(
                    self.windowed_size,
                    self.interface_size,
                    self.fullscreen.is_set(),
                );
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
        }
    }

    /// Closes the window and keeps running in the tray. The window's state
    /// stays, so it comes back where it was.
    pub(crate) fn hide_window(&mut self) -> Task<Message> {
        let drag = self.end_zoom_drag();
        let Some(id) = self.main_window.take() else {
            return drag;
        };
        self.focused = false;
        self.close_pad_gate();
        self.modifiers = keyboard::Modifiers::empty();
        self.zooming.modifiers_changed(self.modifiers);
        self.hovered = None;
        self.star_hovered = None;
        Task::batch([drag, window::close(id)])
    }

    /// Opens the panel of the card a tray stop's error is held for, when the
    /// window shows the library and nothing else is about to claim the
    /// panel. An error whose card is gone drops.
    pub(crate) fn release_held_error(&mut self) -> Task<Message> {
        let Some(name) = self.tray.held_error.as_ref().map(|(name, _)| name.clone()) else {
            return Task::none();
        };
        if self.tray_card_gone(&name) {
            self.tray.held_error = None;
            return Task::none();
        }
        if self.main_window.is_none() || self.page_open() {
            return Task::none();
        }
        let own_panel = self.selected.as_deref() == Some(&name);
        // Another card's open panel claims the window, so the error waits.
        if self.panel_open && !own_panel {
            return Task::none();
        }
        if own_panel && self.panel_open {
            self.show_held_error(&name);
            Task::none()
        } else {
            // Opening the panel shows it.
            self.update(Message::OpenCard(name))
        }
    }

    /// Shows a held error in `name`'s panel, which just opened. The error
    /// belongs to its card, so another card's panel leaves it held.
    pub(crate) fn show_held_error(&mut self, name: &str) {
        if self
            .tray
            .held_error
            .as_ref()
            .is_some_and(|(held, _)| held == name)
            && let Some((_, error)) = self.tray.held_error.take()
        {
            self.panel_error = Some(error);
        }
    }

    /// The light poll's running set. What it no longer lists has stopped,
    /// so its play time is read again and the Play rows reorder.
    pub(crate) fn tray_polled(
        &mut self,
        generation: u64,
        running: BTreeSet<String>,
    ) -> Task<Message> {
        let back = self.hidden_for_stopped(generation, &running);
        let stopped: Vec<String> = self
            .cards
            .iter()
            .filter(|card| self.tray_running(card) && !running.contains(&card.name))
            .map(|card| card.name.clone())
            .collect();
        self.tray.polled = Some(running);
        if stopped.is_empty() {
            return back;
        }
        let reads = Task::batch(stopped.into_iter().map(|name| {
            let paths = self.paths.clone();
            let read = name.clone();
            crate::task::blocking_value(
                move || tray::last_played(&paths, &read),
                move |played| Message::TrayPlayed(name, played.ok().flatten()),
            )
        }));
        Task::batch([back, reads])
    }

    pub(crate) fn tray_played(&mut self, name: String, played: Option<Timestamp>) {
        if let Some(played) = played {
            self.tray.played.insert(name, played);
        }
    }
}
