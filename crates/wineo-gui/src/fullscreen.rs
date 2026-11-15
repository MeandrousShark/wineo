//! Full screen: F11 toggles it in any layout, and "Open coverflow in full
//! screen" makes coverflow enter it. The app tracks who owns it, so leaving
//! coverflow only leaves a full screen the switch started.

use iced::{Task, window};

use crate::grid_motion::ViewLayout;
use crate::{App, Message};

/// Who made the window full screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Owner {
    /// Windowed.
    #[default]
    Nobody,
    /// F11, or Start on a pad.
    User,
    /// Entering coverflow with the switch on.
    Switch,
}

impl Owner {
    pub(crate) fn is_set(self) -> bool {
        self != Self::Nobody
    }
}

impl App {
    /// F11, and Start on a pad: leaves full screen whoever started it, and
    /// otherwise enters it as the user's.
    pub(crate) fn toggle_fullscreen(&mut self) -> Task<Message> {
        let owner = if self.fullscreen.is_set() {
            Owner::Nobody
        } else {
            Owner::User
        };
        self.set_fullscreen(owner)
    }

    /// With no window (hidden to the tray) only the owner changes, and the
    /// reopen follows it.
    fn set_fullscreen(&mut self, owner: Owner) -> Task<Message> {
        self.fullscreen = owner;
        let Some(id) = self.main_window else {
            return Task::none();
        };
        let mode = if owner.is_set() {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        };
        window::set_mode(id, mode)
    }

    /// Who owns the first window: the switch, when the saved layout is
    /// coverflow. A start in grid or list opens a normal window.
    pub(crate) fn fullscreen_at_start(&self) -> Owner {
        if self.layout == ViewLayout::Coverflow && self.coverflow_fullscreen {
            Owner::Switch
        } else {
            Owner::Nobody
        }
    }

    /// Runs after every layout change, whichever way it came (the switch,
    /// Ctrl+L, a preset, or a pad). Already full screen keeps its owner.
    pub(crate) fn fullscreen_after_layout(&mut self, from: ViewLayout) -> Task<Message> {
        let coverflow = ViewLayout::Coverflow;
        if from != coverflow && self.layout == coverflow {
            if self.coverflow_fullscreen && !self.fullscreen.is_set() {
                return self.set_fullscreen(Owner::Switch);
            }
        } else if from == coverflow && self.layout != coverflow && self.fullscreen == Owner::Switch
        {
            return self.set_fullscreen(Owner::Nobody);
        }
        Task::none()
    }

    /// Follows the saved switch while the global Settings page has read it:
    /// turning it on in coverflow enters now, and turning it off leaves a
    /// full screen it owns.
    pub(crate) fn sync_coverflow_fullscreen(&mut self) -> Task<Message> {
        let Some(on) = self
            .settings
            .as_ref()
            .and_then(|page| page.coverflow_fullscreen())
        else {
            return Task::none();
        };
        if on == self.coverflow_fullscreen {
            return Task::none();
        }
        self.coverflow_fullscreen = on;
        if on && self.layout == ViewLayout::Coverflow && !self.fullscreen.is_set() {
            self.set_fullscreen(Owner::Switch)
        } else if !on && self.fullscreen == Owner::Switch {
            self.set_fullscreen(Owner::Nobody)
        } else {
            Task::none()
        }
    }

    /// A compositor keybind can leave full screen without telling the app,
    /// so while an owner is set each resize asks the window for its mode.
    /// The answer comes after the resize, so `window_resized` doesn't store
    /// the size until then.
    pub(crate) fn read_window_mode(&self, id: window::Id, size: iced::Size) -> Task<Message> {
        if !self.fullscreen.is_set() {
            return Task::none();
        }
        window::mode(id).map(move |mode| Message::WindowMode(id, mode, size))
    }

    /// A windowed answer means the compositor ended full screen, so nobody
    /// owns it and the resize that prompted the question was a windowed one.
    pub(crate) fn window_mode_read(
        &mut self,
        id: window::Id,
        mode: window::Mode,
        size: iced::Size,
    ) {
        if self.main_window == Some(id)
            && self.fullscreen.is_set()
            && mode == window::Mode::Windowed
        {
            self.fullscreen = Owner::Nobody;
            self.windowed_size = size;
        }
    }
}

#[cfg(test)]
mod tests;
