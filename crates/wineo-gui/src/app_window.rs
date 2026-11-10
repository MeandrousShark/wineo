//! Message handlers for the window: resizing, closing, and canceling work on the way out.

use std::time::Duration;

use iced::futures::FutureExt;
use iced::{Task, window};

use crate::{App, Message, OperationKind, portable_page, sidebar, snapshots_page};

/// How long a close waits for `library.toml` writes: one config lock wait,
/// about 5 s, and a margin. A write stalled past that is given up on.
pub(crate) const SORT_SAVE_WAIT: Duration = Duration::from_secs(6);

/// How long the exit waits after closing the window.
pub(crate) const EXIT_AFTER_CLOSE: Duration = Duration::from_millis(50);

/// How long an exit waits for the bus to take back the app's name and the
/// tray icon before it goes on anyway.
pub(crate) const LEAVE_WAIT: Duration = Duration::from_secs(1);

/// The window's size at 100%, in logical pixels.
const LAUNCH: iced::Size = iced::Size::new(1200.0, 800.0);

/// What iced multiplies the desktop's own scale by for an interface size in
/// percent.
pub(crate) fn factor(interface_size: u16) -> f32 {
    f32::from(interface_size) / 100.0
}

/// The launch size in the window's logical pixels, which a bigger interface
/// makes fewer.
pub(crate) fn launch_size(interface_size: u16) -> iced::Size {
    LAUNCH * (1.0 / factor(interface_size))
}

impl App {
    pub(crate) fn scale_factor(&self, window: window::Id) -> f32 {
        if self.main_window == Some(window) {
            factor(self.interface_size)
        } else {
            1.0
        }
    }

    /// A new scale factor changes the window's logical size without a
    /// `Resized`, so this asks for it. iced applies the factor before it
    /// answers.
    pub(crate) fn refresh_window_size(&self) -> Task<Message> {
        let Some(id) = self.main_window else {
            return Task::none();
        };
        window::size(id).map(move |size| Message::Window(id, window::Event::Resized(size)))
    }

    /// The close button: Keep running in the tray hides the window, even
    /// over foreground work since nothing stops, and otherwise it quits.
    pub(crate) fn close_requested(&mut self) -> Task<Message> {
        if self.keeps_running() {
            self.closing_to_the_tray();
            self.hide_window()
        } else {
            self.quit_requested()
        }
    }

    /// The tray's Quit, and the close button when it quits.
    pub(crate) fn quit_requested(&mut self) -> Task<Message> {
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
            self.start_leaving();
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
    /// Starting it again is harmless, and a forced close must still go
    /// through while a chained exit waits on a slow task.
    pub(crate) fn close_window(&mut self) -> Task<Message> {
        // Messages run until the loop stops, so late results must drop.
        self.exiting = true;
        self.steam_auto.cancel();
        let work = self.abandoned_work();
        self.watchdog.start(&work);
        self.close_then_exit()
    }

    /// The window goes first, so an exit stuck on work the watchdog is
    /// waiting out doesn't leave a frozen window up. The pause lets the
    /// destroy reach the compositor before the runtime stops.
    pub(crate) fn close_then_exit(&mut self) -> Task<Message> {
        self.start_leaving();
        let leave = self.leave_wait();
        let close = self.main_window.map_or_else(Task::none, window::close);
        close
            .chain(leave)
            .chain(Task::future(async { tokio::time::sleep(EXIT_AFTER_CLOSE).await }).discard())
            .chain(iced::exit())
    }

    /// Gives the bus name back and takes the tray icon down as soon as a
    /// close can't be taken back, so a relaunch while the exit waits on
    /// writes or stuck work starts a fresh app instead of calling `Show` on
    /// this one. Only the first call starts them; every exit path waits on
    /// the same result.
    pub(crate) fn start_leaving(&mut self) {
        if self.left {
            return;
        }
        self.left = true;
        let release = self.instance.as_ref().map(crate::instance::Instance::leave);
        let tray = self.tray.running.stop();
        if release.is_none() && tray.is_none() {
            return;
        }
        let both = async move {
            let release = async move {
                if let Some(release) = release {
                    release.await;
                }
            };
            let tray = async move {
                if let Some(tray) = tray {
                    tray.await;
                }
            };
            iced::futures::future::join(release, tray).await;
        };
        self.leaving = Some(both.boxed().shared());
    }

    /// Waits, bounded, for what `start_leaving` started. The runtime that
    /// serves the tray may hang once it's stopping, so this runs before the
    /// exit.
    fn leave_wait(&self) -> Task<Message> {
        let Some(leaving) = self.leaving.clone() else {
            return Task::none();
        };
        Task::future(async move {
            let _ = tokio::time::timeout(LEAVE_WAIT, leaving).await;
        })
        .discard()
    }

    /// What closing would leave unfinished, named for the line the watchdog
    /// prints. Read here because the watchdog thread can't see the app.
    pub(crate) fn abandoned_work(&self) -> Vec<String> {
        let mut work = Vec::new();
        if self.view_writes.pending() || self.flag_writes.pending() {
            work.push("a library write".to_owned());
        }
        if self.tag_writes.pending() {
            work.push("a tag write".to_owned());
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
        self.start_leaving();
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
        self.viewport_height = (size.height - 48.0 - self.header_room()).max(0.0);
        // Every frame of a drag would restart the springs.
        self.snap_grid();
        self.collapse.reset();
        let waited = self.motion_ended(true);
        Task::batch([self.load_visible_art(), waited])
    }

    /// A window event from the main window, as the runtime would send it.
    #[cfg(test)]
    pub(crate) fn window_event(&self, event: window::Event) -> Message {
        Message::Window(self.main_window.expect("boot opens the main window"), event)
    }
}
