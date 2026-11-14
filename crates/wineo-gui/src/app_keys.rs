//! Message handlers for keys that no page or dialog consumed first.

use std::ops::ControlFlow;

use iced::{Task, keyboard};

use crate::{App, Early, Message, SEARCH_ID, downloads_page, flow, removal, sidebar};

/// Keys a flow takes once per press: a held number key, Esc, or Ctrl+Enter
/// would otherwise walk on across pages, and on into the library.
fn once_per_press(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    use iced::keyboard::{Key, key::Named};
    match key.as_ref() {
        Key::Named(Named::Escape) => true,
        Key::Named(Named::Enter) => modifiers.control(),
        Key::Character(text) => !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

impl App {
    /// A held key's repeat as the press it repeats, or None when it's a
    /// number key, Esc, or Ctrl+Enter and a flow is open or took its first
    /// press. Any other press forgets the held key.
    pub(crate) fn unrepeated(&mut self, message: Message) -> Option<Message> {
        match message {
            Message::KeyRepeat(key, modifiers, status) => {
                let held = once_per_press(&key, modifiers)
                    && (self.flow.is_some() || self.flow_held_key.as_ref() == Some(&key));
                (!held).then_some(Message::Key(key, modifiers, status))
            }
            Message::Key(key, modifiers, status) => {
                self.flow_held_key =
                    (self.flow.is_some() && once_per_press(&key, modifiers)).then(|| key.clone());
                Some(Message::Key(key, modifiers, status))
            }
            message => Some(message),
        }
    }

    pub(crate) fn flow_key(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        status: iced::event::Status,
    ) -> Early {
        use iced::keyboard::{Key, key::Named};
        if sidebar::shortcut(&key, modifiers).is_some() {
            return ControlFlow::Break(Task::none());
        }
        let generation = self.flow_generation;
        let width = self.flow_width();
        let new_choosing = matches!(&self.flow, Some(flow::Page::New(page)) if page.choosing());
        let own = if status == iced::event::Status::Captured {
            None
        } else {
            self.flow.as_mut().unwrap().key(&key, modifiers, width)
        };
        ControlFlow::Continue(if let Some(task) = own {
            task.map(move |event| Message::Flow(generation, event))
        } else {
            match key.as_ref() {
                Key::Character(choice) if matches!(self.flow, Some(flow::Page::Choose(_))) => {
                    let event = match choice {
                        "1" => flow::Event::New,
                        "2" => flow::Event::Prefix,
                        "3" => flow::Event::Heroic,
                        "4" => flow::Event::Steam,
                        "5" => flow::Event::Gog,
                        _ => return ControlFlow::Break(Task::none()),
                    };
                    self.flow_update(self.flow_generation, event)
                }
                Key::Character(digit) if new_choosing => {
                    let Some(event) = flow::new::Page::numbered(digit) else {
                        return ControlFlow::Break(Task::none());
                    };
                    self.flow_update(self.flow_generation, event.into())
                }
                Key::Named(Named::Enter) if modifiers.control() => {
                    let generation = self.flow_generation;
                    // Add's submit puts New in its place at once, so focus
                    // lands by the rows before and after, as in flow_update.
                    let before = self.flow.as_ref().and_then(flow::Page::rows);
                    let task = self
                        .flow
                        .as_mut()
                        .unwrap()
                        .submit(self.steamgriddb_available)
                        .map(move |event| Message::Flow(generation, event));
                    let after = self.flow.as_ref().and_then(flow::Page::rows);
                    let land = crate::focus_button::land(before.as_ref(), after.as_ref())
                        .unwrap_or_else(Task::none);
                    Task::batch([
                        task.chain(self.flow_update(generation, flow::Event::AfterSubmit)),
                        land,
                    ])
                }
                // The page's rows took the arrow and moved focus.
                Key::Named(Named::ArrowUp | Named::ArrowDown)
                    if status == iced::event::Status::Captured =>
                {
                    self.flow_rows_reveal()
                }
                Key::Named(Named::Tab) if modifiers.shift() => {
                    iced::widget::operation::focus_previous().chain(self.flow_rows_reveal())
                }
                Key::Named(Named::Tab) => {
                    iced::widget::operation::focus_next().chain(self.flow_rows_reveal())
                }
                Key::Named(Named::Escape)
                    if status != iced::event::Status::Captured && !self.close_prompt =>
                {
                    self.flow_update(self.flow_generation, flow::Event::Back)
                }
                _ => Task::none(),
            }
        })
    }

    /// Shows the open page's focused row, if one of its rows has focus.
    fn flow_rows_reveal(&self) -> Task<Message> {
        self.flow
            .as_ref()
            .and_then(flow::Page::rows)
            .map_or_else(Task::none, |rows| crate::focus_button::reveal(rows.ids))
    }

    pub(crate) fn page_key(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        status: iced::event::Status,
    ) -> Early {
        let results_tab = self.checkup_key_scope().is_some()
            && key == keyboard::Key::Named(keyboard::key::Named::Tab);
        if !results_tab && let Some(task) = removal::key(self, &key, modifiers, status) {
            return ControlFlow::Break(task);
        }
        // shortcuts::capture normally takes these first. This covers a
        // key an open overlay saw and left uncaptured.
        ControlFlow::Continue(if let Some(section) = sidebar::shortcut(&key, modifiers) {
            if status == iced::event::Status::Captured {
                Task::none()
            } else {
                self.navigate_section(section)
            }
        } else if self.settings.is_some() {
            self.settings_key(&key, modifiers, status == iced::event::Status::Captured)
        } else if self.art_page.is_some() {
            self.art_key(&key, modifiers, status == iced::event::Status::Captured)
        } else if self.feature_page_open() {
            self.feature_key(&key, modifiers, status == iced::event::Status::Captured)
        } else if self.section == sidebar::Section::Downloads {
            downloads_page::key(
                self,
                &key,
                modifiers,
                status == iced::event::Status::Captured,
            )
        } else if modifiers.control() && matches!(key.as_ref(), keyboard::Key::Character("n" | "N"))
        {
            self.update(Message::Add)
        } else {
            iced::widget::operation::is_focused(iced::widget::Id::new(SEARCH_ID))
                .map(move |focused| Message::KeyWithFocus(key.clone(), modifiers, status, focused))
        })
    }
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{Key, Modifiers, key::Named};

    use super::once_per_press;

    #[test]
    fn flows_take_digits_esc_and_ctrl_enter_once_per_press() {
        let none = Modifiers::empty();
        assert!(once_per_press(&Key::Character("3".into()), none));
        assert!(once_per_press(&Key::Named(Named::Escape), none));
        assert!(once_per_press(&Key::Named(Named::Enter), Modifiers::CTRL));
        assert!(!once_per_press(&Key::Named(Named::Enter), none));
        assert!(!once_per_press(&Key::Character("a".into()), none));
        assert!(!once_per_press(&Key::Character("".into()), none));
        assert!(!once_per_press(&Key::Named(Named::ArrowDown), none));
    }
}
