//! What a controller press does in the app: the pad gate, the filter that
//! keeps pad presses to the Library, and the key each press stands for.
//!
//! Pad presses arrive as ignored keys, which skip the `captured` guards a
//! real key meets in an open overlay. So the filter is an allow-list: only
//! the Library with nothing over it takes them, and an overlay added later
//! is closed to the pad until `library_overlay_open` names it.

use std::time::{Duration, Instant};

use iced::advanced::widget::operation::{Focusable, Outcome, TextInput};
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Rectangle, Task, Vector};
use wineo_core::library::PadAction;

use crate::keys::Direction;
use crate::pad::{self, Input};
use crate::{App, Message, sidebar};

/// The key a pad action stands for, so the pad never does something a key
/// can't. Back and Fullscreen aren't here: they skip the filter.
pub(crate) fn twin(action: PadAction) -> Option<(Key, Modifiers)> {
    let none = Modifiers::empty();
    Some(match action {
        PadAction::Confirm => (Key::Named(Named::Enter), none),
        PadAction::Favorite => (Key::Character("f".into()), none),
        PadAction::Mark => (Key::Named(Named::Space), none),
        PadAction::PageLeft => (Key::Named(Named::PageUp), none),
        PadAction::PageRight => (Key::Named(Named::PageDown), none),
        PadAction::First => (Key::Named(Named::Home), none),
        PadAction::Last => (Key::Named(Named::End), none),
        PadAction::Layout => (Key::Character("l".into()), Modifiers::CTRL),
        PadAction::Random => (Key::Character("r".into()), none),
        PadAction::Back | PadAction::Fullscreen => return None,
    })
}

fn arrow(direction: Direction) -> Key {
    Key::Named(match direction {
        Direction::Left => Named::ArrowLeft,
        Direction::Right => Named::ArrowRight,
        Direction::Up => Named::ArrowUp,
        Direction::Down => Named::ArrowDown,
    })
}

/// Back is exactly the mouse Back button (`event_message`), so it backs out
/// of whatever Esc closes and no further.
pub(crate) fn back_message() -> Message {
    Message::Key(
        Key::Named(Named::Escape),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    )
}

/// What Start sends: F11, past the filter. It lands on F11's global arm in
/// `update` (`toggle_fullscreen`), after the close prompt, which takes it.
pub(crate) fn start_message() -> Message {
    Message::Key(
        Key::Named(Named::F11),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    )
}

/// A press in the window that follows any close by less than this is taken
/// as the trailing press of the input that closed it, such as the double
/// click or Enter that launched a game: iced hands a widget's message to
/// `update` before the press reaches the subscription that reports it. It
/// applies after losing focus or hiding too, where no press follows.
pub(crate) const PRESS_GRACE: Duration = Duration::from_millis(300);

/// Whether pad presses count. It opens on a real focus at once, or on a
/// press in the window, and shuts when the window loses focus, hides, or
/// launches something.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gate {
    open: bool,
    closed_at: Option<Instant>,
}

impl Gate {
    pub(crate) fn is_open(self) -> bool {
        self.open
    }

    pub(crate) fn close(&mut self, now: Instant) {
        self.open = false;
        self.closed_at = Some(now);
    }

    pub(crate) fn focus(&mut self) {
        self.open = true;
    }

    /// A press opens it too, since some compositors never send focus,
    /// unless it comes within the grace of any close.
    pub(crate) fn press(&mut self, now: Instant) {
        if self
            .closed_at
            .is_none_or(|at| now.saturating_duration_since(at) >= PRESS_GRACE)
        {
            self.open = true;
        }
    }
}

impl App {
    /// The time the pad gate goes by.
    pub(crate) fn pad_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = self.pad_clock {
            return now;
        }
        Instant::now()
    }

    /// A confirm, the close prompt, or a flow takes every key.
    pub(crate) fn dialog_open(&self) -> bool {
        self.close_prompt
            || self.flow.is_some()
            || self.removal.is_some()
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
    }

    /// The panel or its menu takes the library's letter keys and Enter: the
    /// context menu, the version picker, or checkup results not under the
    /// log.
    pub(crate) fn panel_takes_keys(&self) -> bool {
        self.action_menu.is_some()
            || self.version_visible()
            || (self.checkup_visible() && !self.panel_log_visible())
    }

    /// Anything over the Library: a dialog or flow, a page, a menu, or a
    /// view that takes the panel's place. Each part is a guard `handle_key`
    /// uses too.
    pub(crate) fn library_overlay_open(&self) -> bool {
        self.dialog_open()
            || self.settings.is_some()
            || self.art_page.is_some()
            || self.feature_page_open()
            || self.library_menu_shown()
            || self.panel_takes_keys()
            || self.panel_log_visible()
            || self.checkup_visible()
    }

    /// The gate is open and nothing is closing.
    pub(crate) fn pad_gate_open(&self) -> bool {
        self.pad_gate.is_open() && !self.closing && !self.exiting && self.main_window.is_some()
    }

    /// Whether a pad press other than Back and Start acts: the Library,
    /// with the window really focused and nothing over it.
    pub(crate) fn pad_drives_library(&self) -> bool {
        self.pad_gate_open()
            && self.section == sidebar::Section::Library
            && !self.library_overlay_open()
    }

    /// Closes the gate until the next real focus, since gilrs reads the pad
    /// whichever window has focus.
    pub(crate) fn close_pad_gate(&mut self) {
        self.pad_gate.close(self.pad_now());
        self.pads.release_all();
    }

    pub(crate) fn pad_event(&mut self, event: pad::PadEvent) -> Task<Message> {
        let connection = matches!(
            event,
            pad::PadEvent::Connected(..) | pad::PadEvent::Disconnected(_)
        );
        let input = self.pads.event(event, self.pad_now());
        if connection && let Some(page) = &mut self.settings {
            page.pads = self.pads.names();
        }
        if !self.input.controller {
            self.pads.release_all();
            return Task::none();
        }
        match input {
            Some(input) => self.pad_input(input),
            None => Task::none(),
        }
    }

    pub(crate) fn pad_repeat(&mut self, generation: u64) -> Task<Message> {
        if !self.pad_wake.fired(generation) {
            return Task::none();
        }
        // The clock moves on either way, so a slow check skips a repeat
        // instead of piling them up.
        match self.pads.repeat(self.pad_now()) {
            Some(input) if self.input.controller && !self.pad_checking => self.pad_input(input),
            _ => Task::none(),
        }
    }

    fn pad_input(&mut self, input: Input) -> Task<Message> {
        if !self.pad_gate_open() {
            self.pads.release_all();
            return Task::none();
        }
        let (Input::Button(_, style) | Input::Move(_, style)) = input;
        self.pad_hint = Some(style);
        let (key, modifiers) = match input {
            Input::Move(direction, _) => (arrow(direction), Modifiers::empty()),
            Input::Button(button, _) => match self.input.bindings().action(button) {
                None => return Task::none(),
                Some(PadAction::Back) => return self.update(back_message()),
                Some(PadAction::Fullscreen) => return self.update(start_message()),
                Some(action) => twin(action).expect("Back and Fullscreen are handled above"),
            },
        };
        if !self.pad_drives_library() {
            return Task::none();
        }
        self.pad_checking = true;
        iced::advanced::widget::operate(TextFocus::default())
            .map(move |focused| Message::PadKey(key.clone(), modifiers, focused))
    }

    /// The key a pad press stands for, once whether a text field has focus
    /// is known. A focused field would take the key, but a pad key arrives
    /// ignored and would reach the library behind it. The gate and overlays
    /// are asked again, since either can change while the check runs.
    pub(crate) fn pad_key(
        &mut self,
        key: Key,
        modifiers: Modifiers,
        text_focused: bool,
    ) -> Task<Message> {
        self.pad_checking = false;
        if text_focused || !self.pad_drives_library() {
            return Task::none();
        }
        self.update(Message::Key(key, modifiers, iced::event::Status::Ignored))
    }

    /// The hint bar's room under the grid's last row while it shows.
    pub(crate) fn pad_hint_shown(&self) -> bool {
        self.pad_hint.is_some()
            && self.input.controller
            && self.section == sidebar::Section::Library
            && !self.panel_hidden_for_pad()
    }

    fn panel_hidden_for_pad(&self) -> bool {
        self.settings.is_some()
            || self.art_page.is_some()
            || self.tool_page.is_some()
            || self.portable_page.is_some()
    }

    pub(crate) fn pad_hint_view(&self) -> Option<iced::Element<'_, Message>> {
        let style = self.pad_hint.filter(|_| self.pad_hint_shown())?;
        Some(pad::glyph::bar(self.pad_hints(), style, self.palette))
    }

    /// The bar's hints. A panel a coverflow card hides isn't open to A,
    /// which opens the card's section instead.
    pub(crate) fn pad_hints(&self) -> Vec<(wineo_core::library::PadButton, &'static str)> {
        pad::glyph::hints(
            &self.input.bindings(),
            self.panel_open && !self.panel_hidden_by_card(),
            self.header_cursor().is_some(),
        )
    }

    /// A tool run started, by its button, Enter, or any other route: its
    /// window takes the pad, like a game's. Every route goes through
    /// `update`, which asks after each message.
    pub(crate) fn sync_tool_starts(&mut self) {
        let mut started = false;
        for page in self.tool_pages.values_mut() {
            started |= page.take_started();
        }
        if started {
            self.close_pad_gate();
        }
    }

    /// The saved `[input]`, from the global Settings page's base whenever it
    /// reads `library.toml`. The reader starts the first time it's on.
    pub(crate) fn sync_input(&mut self) {
        if let Some(input) = self.settings.as_ref().and_then(|page| page.input()) {
            self.input = input;
        }
        if self.input.controller {
            self.pad_reading = true;
        } else {
            self.pads.release_all();
            self.pad_hint = None;
        }
    }
}

/// Finds whether a text field has focus. A text input reports itself as a
/// text input and then as a focusable, so the pair names one widget.
#[derive(Default)]
pub(crate) struct TextFocus {
    text_input: bool,
    found: bool,
}

impl Operation<bool> for TextFocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<bool>)) {
        operate(self);
    }

    fn container(&mut self, _id: Option<&Id>, _bounds: Rectangle) {
        self.text_input = false;
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
        _translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.text_input = false;
    }

    fn text_input(&mut self, _id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn TextInput) {
        self.text_input = true;
    }

    fn focusable(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if self.text_input && state.is_focused() {
            self.found = true;
        }
        self.text_input = false;
    }

    fn finish(&self) -> Outcome<bool> {
        Outcome::Some(self.found)
    }
}

#[cfg(test)]
mod tests;
