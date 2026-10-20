//! Message handlers for the window: resizing, closing, and canceling work on the way out.

use std::time::Duration;

use iced::Task;

use crate::{App, Message, sidebar};

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
        iced::exit()
    }

    pub(crate) fn cancel_and_close(&mut self, event: Message) -> Task<Message> {
        let force = matches!(event, Message::ForceCloseWork);
        self.closing = true;
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
        if !self.exit_waits() {
            iced::exit()
        } else {
            Task::none()
        }
    }

    pub(crate) fn window_resized(&mut self, size: iced::Size) -> Task<Message> {
        self.window_size = size;
        self.viewport_width = (size.width - sidebar::WIDTH).max(0.0);
        self.viewport_height = (size.height - 48.0).max(0.0);
        self.grid_motion
            .snap(self.panel_open, self.filtered().len());
        self.collapse.reset();
        let waited = self.motion_ended(true);
        Task::batch([self.load_visible_art(), waited])
    }
}
