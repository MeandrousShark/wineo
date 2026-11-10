//! Hide to the tray (plan 0063): a start the app runs can close the window,
//! and it comes back once everything it hid for has stopped. When everything
//! stops can also open a window the user closed themselves.

use std::collections::BTreeSet;

use iced::Task;
use wineo_core::launch::StartOutcome;
use wineo_core::library::{OnStart, OnStop};

use crate::{App, Message};

impl App {
    /// A start the app ran, from the window or the tray, has finished. Only
    /// one wineo runs itself counts: AlreadyRunning and Steam's answers
    /// leave the window alone (answer 13), and a CLI start never gets here.
    pub(crate) fn hide_for_start(&mut self, name: &str, outcome: StartOutcome) -> Task<Message> {
        if outcome != StartOutcome::Started
            || !self.joins_the_hide()
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
        // Only Hide to the tray closes the window. Always show the window
        // joins a start with the window open too, so one closed mid-run
        // still comes back. A window that's already closed stays closed.
        if self.desktop.on_start == OnStart::Hide {
            self.hide_window()
        } else {
            Task::none()
        }
    }

    /// Whether a start has the window come back when it's over. Always show
    /// the window counts every start. Otherwise only a window a hide closed
    /// does: a hide closes an open window, and a window it closed already
    /// has everything that started since in its set.
    fn joins_the_hide(&self) -> bool {
        if self.desktop.on_stop == OnStop::Show {
            return true;
        }
        if self.main_window.is_some() {
            return self.desktop.on_start == OnStart::Hide;
        }
        !self.tray.hidden_for.is_empty()
    }

    /// The light poll's reading: when it's from since the last hide and
    /// nothing the window hid for still runs, the window comes back where
    /// it was. One that's open by then is left alone, not raised.
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
        if self.main_window.is_some() {
            self.tray.hidden_for.clear();
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
