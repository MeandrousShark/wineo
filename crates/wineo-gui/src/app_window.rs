//! Message handlers for the window: resizing, closing, and canceling work on the way out.

use std::time::Duration;

use iced::Task;

use crate::{App, Message, OperationKind, portable_page, sidebar, snapshots_page};

/// How long a close waits for `library.toml` writes: one config lock wait,
/// about 5 s, and a margin. A write stalled past that is given up on.
pub(crate) const SORT_SAVE_WAIT: Duration = Duration::from_secs(6);

impl App {
    pub(crate) fn close_requested(&mut self) -> Task<Message> {
        // A drag the close interrupts saves where it was, and the wait
        // below covers that write.
        let drag = self.end_zoom_drag();
        Task::batch([drag, self.close_decision()])
    }

    fn close_decision(&mut self) -> Task<Message> {
        if self.close_by.is_some() {
            // Asked again while waiting on library writes.
            self.exit_now()
        } else if self.foreground_busy() {
            self.close_prompt = true;
            Task::none()
        } else if self.library_writes_pending() {
            // The window closes once the file has the final view, or when
            // the wait runs out. Like any accepted close it takes the input,
            // but it shows nothing.
            self.closing = true;
            self.steam_auto.cancel();
            self.close_prompt = true;
            self.close_quietly = true;
            Task::none()
        } else {
            self.exit_now()
        }
    }

    pub(crate) fn exit_now(&mut self) -> Task<Message> {
        self.exiting = true;
        self.close_by = None;
        for page in self.art_pages.values() {
            page.cancel_thumbnails();
        }
        self.cancel_remove_dialog();
        self.close_window()
    }

    /// Closes the window and starts the watchdog. Every exit goes through
    /// here, so work still holding the runtime open can't keep the process.
    /// Starting it again is harmless.
    pub(crate) fn close_window(&mut self) -> Task<Message> {
        self.steam_auto.cancel();
        let work = self.abandoned_work();
        self.watchdog.start(&work);
        iced::exit()
    }

    /// What closing would leave unfinished, named for the line the watchdog
    /// prints. Read here because the watchdog thread can't see the app.
    pub(crate) fn abandoned_work(&self) -> Vec<String> {
        let mut work = Vec::new();
        if self.view_writes.pending() || self.flag_writes.pending() {
            work.push("a library write".to_owned());
        }
        if self.steam_auto.running() {
            work.push("a Steam import".to_owned());
        }
        if let Some(operation) = &self.operation {
            let name = &operation.name;
            work.push(if operation.after_hook {
                format!("the after hook of {name}")
            } else if operation.kind == OperationKind::Start {
                format!("starting {name}")
            } else {
                format!("stopping {name}")
            });
        }
        if let Some(page) = self.flow.as_ref().filter(|page| page.busy()) {
            work.push(page.work().to_owned());
        }
        if let Some(change) = &self.change {
            let reason = change.reason();
            work.push(reason.strip_prefix("Busy: ").unwrap_or(&reason).to_owned());
        }
        if self
            .snapshots_page
            .as_ref()
            .is_some_and(snapshots_page::Page::busy)
        {
            work.push("a snapshot".to_owned());
        }
        if self.portable_pages.values().any(portable_page::Page::busy) {
            work.push("a copy to portable".to_owned());
        }
        if self.tools_busy() {
            work.push("a tool".to_owned());
        }
        if self.art_busy() {
            work.push("an artwork write".to_owned());
        }
        if self.settings.as_ref().is_some_and(|page| page.saving) {
            work.push("a settings save".to_owned());
        }
        work
    }

    pub(crate) fn cancel_and_close(&mut self, event: Message) -> Task<Message> {
        let force = matches!(event, Message::ForceCloseWork);
        self.closing = true;
        self.steam_auto.cancel();
        self.cancel_management();
        self.cancel_features(force);
        for page in self.art_pages.values() {
            page.cancel_thumbnails();
        }
        for token in self.pending_art_cancels.values() {
            token.cancel();
        }
        self.cancel_checkup(force);
        if let Some(page) = &mut self.flow {
            page.cancel(force);
        }
        if let Some(operation) = &mut self.operation
            && let Some(token) = &operation.token
        {
            if force {
                token.force();
            } else {
                token.cancel();
            }
            operation.cancelled = true;
        }
        // Force stop doesn't wait for work that may never unwind: the
        // cancels above ran, and the watchdog bounds what is left.
        if force || !self.exit_waits() {
            self.close_window()
        } else {
            Task::none()
        }
    }

    pub(crate) fn window_resized(&mut self, size: iced::Size) -> Task<Message> {
        self.window_size = size;
        self.viewport_width = (size.width - sidebar::WIDTH).max(0.0);
        self.viewport_height = (size.height - 48.0).max(0.0);
        // Every frame of a drag would restart the springs.
        self.snap_grid();
        self.collapse.reset();
        let waited = self.motion_ended(true);
        Task::batch([self.load_visible_art(), waited])
    }
}
