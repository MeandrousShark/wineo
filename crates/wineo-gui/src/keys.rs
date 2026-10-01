use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};

use crate::{App, CARD_ROW, Message, SCROLL_ID, SEARCH_ID};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    None,
    Move(Direction),
    GridFromSearch,
    OpenOrPlay,
    CloseLog,
    ClosePanel,
    ClearSearch,
    FocusSearch,
}

pub(crate) fn action(
    key: &Key,
    modifiers: Modifiers,
    search_focused: bool,
    log_open: bool,
    panel_open: bool,
    search_empty: bool,
) -> Action {
    match key.as_ref() {
        Key::Character(character) if modifiers.control() && character.eq_ignore_ascii_case("f") => {
            Action::FocusSearch
        }
        Key::Character("/") if !search_focused => Action::FocusSearch,
        Key::Named(Named::Escape) if log_open => Action::CloseLog,
        Key::Named(Named::Escape) if panel_open => Action::ClosePanel,
        Key::Named(Named::Escape) if !search_empty => Action::ClearSearch,
        Key::Named(Named::ArrowDown) if search_focused => Action::GridFromSearch,
        Key::Named(Named::ArrowLeft) if !search_focused => Action::Move(Direction::Left),
        Key::Named(Named::ArrowRight) if !search_focused => Action::Move(Direction::Right),
        Key::Named(Named::ArrowUp) if !search_focused => Action::Move(Direction::Up),
        Key::Named(Named::ArrowDown) if !search_focused => Action::Move(Direction::Down),
        Key::Named(Named::Enter) if !search_focused => Action::OpenOrPlay,
        _ => Action::None,
    }
}

impl App {
    pub(crate) fn handle_key(
        &mut self,
        key: Key,
        modifiers: Modifiers,
        search_focused: bool,
        captured: bool,
    ) -> Task<Message> {
        if self.close_prompt || self.flow.is_some() {
            return Task::none();
        }
        if self.settings.is_some() {
            return self.settings_key(&key, modifiers, captured);
        }
        if self.section != crate::sidebar::Section::Library {
            return Task::none();
        }
        self.search_focused = search_focused;
        if captured
            && !search_focused
            && matches!(
                key,
                Key::Named(
                    Named::ArrowLeft
                        | Named::ArrowRight
                        | Named::ArrowUp
                        | Named::ArrowDown
                        | Named::Enter
                )
            )
        {
            return Task::none();
        }
        match action(
            &key,
            modifiers,
            search_focused,
            self.log_view,
            self.panel_open,
            self.search.is_empty(),
        ) {
            Action::None => Task::none(),
            Action::FocusSearch => iced::widget::operation::focus(iced::widget::Id::new(SEARCH_ID))
                .chain(
                    iced::widget::operation::is_focused(iced::widget::Id::new(SEARCH_ID))
                        .map(Message::SearchFocusChanged),
                ),
            Action::CloseLog => {
                self.close_log();
                Task::none()
            }
            Action::ClosePanel => {
                self.close_panel();
                Task::none()
            }
            Action::ClearSearch => self.update(Message::SearchChanged(String::new())),
            Action::GridFromSearch => {
                let old = self.selected.clone();
                self.selected = self.filtered().first().map(|card| card.name.clone());
                if self.selected != old {
                    self.selection_changed();
                }
                iced::advanced::widget::operate(
                    iced::advanced::widget::operation::focusable::unfocus::<()>(),
                )
                .discard()
                .chain(Task::batch([
                    self.scroll_selection(),
                    self.load_visible_art(),
                    self.load_panel_art(),
                ]))
                .chain(
                    iced::widget::operation::is_focused(iced::widget::Id::new(SEARCH_ID))
                        .map(Message::SearchFocusChanged),
                )
            }
            Action::Move(direction) => {
                let old = self.selected.clone();
                let filtered = self.filtered();
                let current = self
                    .selected
                    .as_ref()
                    .and_then(|name| filtered.iter().position(|card| &card.name == name));
                let columns = self.columns();
                let next = move_index(current, filtered.len(), columns, direction);
                let next_name = next.map(|index| filtered[index].name.clone());
                self.selected = next_name;
                if self.selected != old {
                    self.selection_changed();
                }
                Task::batch([
                    self.scroll_selection(),
                    self.load_visible_art(),
                    self.load_panel_art(),
                ])
            }
            Action::OpenOrPlay => {
                if self.panel_open {
                    self.start_selected()
                } else if let Some(name) = self.selected.clone() {
                    self.update(Message::Select(name))
                } else {
                    Task::none()
                }
            }
        }
    }

    fn selection_changed(&mut self) {
        self.close_log();
        self.target = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
            .and_then(|card| card.default_target.clone());
        self.panel_error = None;
        self.panel_note = None;
    }

    fn scroll_selection(&mut self) -> Task<Message> {
        let filtered = self.filtered();
        let Some(index) = self
            .selected
            .as_ref()
            .and_then(|name| filtered.iter().position(|card| &card.name == name))
        else {
            return Task::none();
        };
        let top = (index / self.columns()) as f32 * self.row_pitch + 16.0;
        let bottom = top + CARD_ROW;
        let offset = if top < self.scroll_offset {
            top
        } else if bottom > self.scroll_offset + self.viewport_height {
            bottom - self.viewport_height
        } else {
            return Task::none();
        };
        self.scroll_offset = offset.max(0.0);
        iced::widget::operation::scroll_to(
            iced::widget::Id::new(SCROLL_ID),
            iced::widget::scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll_offset,
            },
        )
    }
}

pub(crate) fn move_index(
    selected: Option<usize>,
    count: usize,
    columns: usize,
    direction: Direction,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let Some(index) = selected else {
        return Some(0);
    };
    let columns = columns.max(1);
    let column = index % columns;
    let row = index / columns;
    let last_row = (count - 1) / columns;
    Some(match direction {
        Direction::Left => (index + count - 1) % count,
        Direction::Right => (index + 1) % count,
        Direction::Up if row > 0 => (row - 1) * columns + column,
        Direction::Up => index,
        Direction::Down if row < last_row => ((row + 1) * columns + column).min(count - 1),
        Direction::Down => index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movement_wraps_and_clamps_partial_rows() {
        assert_eq!(move_index(None, 5, 3, Direction::Down), Some(0));
        assert_eq!(move_index(Some(2), 5, 3, Direction::Down), Some(4));
        assert_eq!(move_index(Some(3), 5, 3, Direction::Down), Some(3));
        assert_eq!(move_index(Some(3), 5, 3, Direction::Up), Some(0));
        assert_eq!(move_index(Some(2), 5, 3, Direction::Up), Some(2));
        assert_eq!(move_index(Some(2), 5, 3, Direction::Right), Some(3));
        assert_eq!(move_index(Some(0), 5, 3, Direction::Left), Some(4));
        assert_eq!(move_index(Some(4), 5, 3, Direction::Right), Some(0));
        assert_eq!(move_index(Some(0), 0, 3, Direction::Right), None);
        assert_eq!(move_index(Some(1), 3, 1, Direction::Down), Some(2));
        assert_eq!(move_index(Some(3), 7, 4, Direction::Down), Some(6));
        assert_eq!(move_index(Some(4), 7, 4, Direction::Down), Some(4));
        let down = move_index(Some(3), 6, 5, Direction::Down);
        assert_eq!(down, Some(5));
        assert_eq!(move_index(down, 6, 5, Direction::Up), Some(0));
    }
    #[test]
    fn escape_order_and_search_focus() {
        let esc = Key::Named(Named::Escape);
        assert_eq!(
            action(&esc, Modifiers::empty(), false, true, true, false),
            Action::CloseLog
        );
        assert_eq!(
            action(&esc, Modifiers::empty(), false, false, true, false),
            Action::ClosePanel
        );
        assert_eq!(
            action(&esc, Modifiers::empty(), false, false, false, false),
            Action::ClearSearch
        );
        assert_eq!(
            action(
                &Key::Named(Named::ArrowLeft),
                Modifiers::empty(),
                true,
                false,
                false,
                false
            ),
            Action::None
        );
        assert_eq!(
            action(
                &Key::Named(Named::ArrowDown),
                Modifiers::empty(),
                true,
                false,
                false,
                false
            ),
            Action::GridFromSearch
        );
    }
}
