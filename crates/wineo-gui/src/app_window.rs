//! Message handlers for the window: resizing, closing, and canceling work on the way out.

use iced::Task;

use crate::{App, Message, sidebar};

impl App {
    pub(crate) fn close_requested(&mut self) -> Task<Message> {
        if self.foreground_busy() {
            self.close_prompt = true;
            Task::none()
        } else {
            for page in self.art_pages.values() {
                page.cancel_thumbnails();
            }
            self.cancel_remove_dialog();
            iced::exit()
        }
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
        if !self.foreground_busy() {
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
        self.load_visible_art()
    }
}
