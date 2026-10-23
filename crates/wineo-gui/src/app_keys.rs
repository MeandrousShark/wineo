//! Message handlers for keys that no page or dialog consumed first.

use std::ops::ControlFlow;

use iced::{Task, keyboard};

use crate::{App, Early, Message, SEARCH_ID, downloads_page, flow, removal, sidebar};

impl App {
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
                Key::Named(Named::Enter) if modifiers.control() => {
                    let generation = self.flow_generation;
                    let task = self
                        .flow
                        .as_mut()
                        .unwrap()
                        .submit()
                        .map(move |event| Message::Flow(generation, event));
                    task.chain(self.flow_update(generation, flow::Event::AfterSubmit))
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
