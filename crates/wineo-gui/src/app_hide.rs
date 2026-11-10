//! Hide to the tray (plan 0063): a start the app runs closes the window,
//! and it comes back once everything it hid for has stopped.

use std::collections::BTreeSet;

use iced::Task;
use wineo_core::launch::StartOutcome;
use wineo_core::library::OnStart;

use crate::{App, Message};

impl App {
    /// A start the app ran, from the window or the tray, has finished. Only
    /// one wineo runs itself hides the window: AlreadyRunning and Steam's
    /// answers leave it alone (answer 13), and a CLI start never gets here.
    pub(crate) fn hide_for_start(&mut self, name: &str, outcome: StartOutcome) -> Task<Message> {
        if outcome != StartOutcome::Started
            || self.desktop.on_start != OnStart::Hide
            // Without a tray to come back from, the window stays.
            || !self.tray_shown()
            // A close question needs the window to be answered.
            || self.close_prompt
            || self.closing
            || self.exiting
        {
            return Task::none();
        }
        self.tray.hidden_for.insert(name.to_owned());
        // The poll starts over, so even a run that's over before its next
        // reading brings the window back, and a reading from before this
        // start can't.
        self.tray.poll_generation += 1;
        // A window that's already hidden stays hidden.
        self.hide_window()
    }

    /// The light poll's reading: when it's from since the last hide and
    /// nothing the window hid for still runs, the window comes back where
    /// it was.
    pub(crate) fn hidden_for_stopped(
        &mut self,
        generation: u64,
        running: &BTreeSet<String>,
    ) -> Task<Message> {
        if generation != self.tray.poll_generation
            || self.tray.hidden_for.is_empty()
            || !self.tray.hidden_for.is_disjoint(running)
        {
            return Task::none();
        }
        self.come_back()
    }

    /// The tray host went away. A hidden window, whatever hid it, comes
    /// back at once, so the app is never left running unreachable.
    pub(crate) fn tray_lost(&mut self) -> Task<Message> {
        if self.main_window.is_some() {
            return Task::none();
        }
        self.come_back()
    }

    /// The user opened the window themselves while it was hidden for a
    /// start (answer A to card 732): that ends the hide, so the game
    /// stopping changes nothing and the next start hides as usual.
    pub(crate) fn user_opened_window(&mut self) {
        if self.main_window.is_none() {
            self.tray.hidden_for.clear();
        }
    }

    /// Like any show, it lets a tray stop's held error out.
    fn come_back(&mut self) -> Task<Message> {
        self.tray.hidden_for.clear();
        let open = self.open_window();
        Task::batch([open, self.release_held_error()])
    }
}

#[cfg(test)]
mod tests;
