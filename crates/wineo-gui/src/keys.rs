use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};

use crate::{App, Message, SEARCH_ID};

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
    ToggleFavorite,
    PickRandom,
    MarkAll,
    MarkSelected,
    ToggleLayout,
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
        Key::Character(character)
            if modifiers.control()
                && !(modifiers.alt() || modifiers.logo())
                && character.eq_ignore_ascii_case("l") =>
        {
            Action::ToggleLayout
        }
        Key::Character(character)
            if !search_focused
                && modifiers.control()
                && !(modifiers.alt() || modifiers.logo())
                && character.eq_ignore_ascii_case("a") =>
        {
            Action::MarkAll
        }
        Key::Named(Named::Space) | Key::Character(" ")
            if !search_focused && !(modifiers.control() || modifiers.alt() || modifiers.logo()) =>
        {
            Action::MarkSelected
        }
        // Shift is allowed, so Caps Lock doesn't matter.
        Key::Character(character)
            if !search_focused
                && !(modifiers.control() || modifiers.alt() || modifiers.logo())
                && character.eq_ignore_ascii_case("f") =>
        {
            Action::ToggleFavorite
        }
        Key::Character(character)
            if !search_focused
                && !(modifiers.control() || modifiers.alt() || modifiers.logo())
                && character.eq_ignore_ascii_case("r") =>
        {
            Action::PickRandom
        }
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
        if self.close_prompt
            || self.flow.is_some()
            || self.removal.is_some()
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
        {
            return Task::none();
        }
        if self.settings.is_some() {
            return self.settings_key(&key, modifiers, captured);
        }
        if self.art_page.is_some() {
            return self.art_key(&key, modifiers, captured);
        }
        if self.feature_page_open() {
            return self.feature_key(&key, modifiers, captured);
        }
        if self.section == crate::sidebar::Section::Library {
            self.search_focused = search_focused;
        }
        if self.sort_menu_shown() {
            return crate::sort_control::key(self, &key, modifiers);
        }
        if self.filter_menu_shown() {
            return crate::filter_control::key(self, &key, modifiers);
        }
        if self.random_menu_shown() {
            return crate::random_pick::key(self, &key, modifiers);
        }
        if self.more_menu_shown() {
            return crate::more_menu::key(self, &key, modifiers);
        }
        if self.tag_menu_shown() {
            return crate::action_bar::key(self, &key, modifiers);
        }
        if let Some(task) = crate::checkup::key(
            self,
            &key,
            modifiers,
            if captured {
                iced::event::Status::Captured
            } else {
                iced::event::Status::Ignored
            },
        ) {
            return task;
        }
        if self.section == crate::sidebar::Section::Downloads {
            return crate::downloads_page::key(self, &key, modifiers, captured);
        }
        if self.section != crate::sidebar::Section::Library {
            return Task::none();
        }
        // Leaving the drop-down's field closes it, keeping the text, so the
        // toolbar's menus can open again and nothing hangs over the grid.
        if self.search_dropdown_shown() && search_focused && !modifiers.control() {
            if key == Key::Named(Named::Tab) && !captured {
                self.search_open = false;
                // From the icon, which holds search's place in Tab's order.
                let walk = if modifiers.shift() {
                    iced::widget::operation::focus_previous()
                } else {
                    iced::widget::operation::focus_next()
                };
                return iced::widget::operation::focus(iced::widget::Id::new(
                    crate::search_drop::ICON_ID,
                ))
                .chain(walk);
            }
            // Down only leaves when there's a card to go to.
            if key == Key::Named(Named::ArrowDown) && !self.grid_places().ids.is_empty() {
                self.search_open = false;
            }
        }
        if key == Key::Named(Named::Tab)
            && let Some(task) = crate::removal::key(
                self,
                &key,
                modifiers,
                if captured {
                    iced::event::Status::Captured
                } else {
                    iced::event::Status::Ignored
                },
            )
        {
            return task;
        }
        if key == Key::Named(Named::Delete) && modifiers.is_empty() {
            return crate::removal::delete_key(self, search_focused, captured);
        }
        if key == Key::Named(Named::Tab) && !captured && !modifiers.control() {
            if self.filter_tab().is_some() {
                return crate::filter_tab::step(self, modifiers.shift());
            }
            let walk = if modifiers.shift() {
                iced::widget::operation::focus_previous()
            } else {
                iced::widget::operation::focus_next()
            };
            return walk.chain(self.version_reveal());
        }
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
            // The picker's builds take Up and Down to move their focus.
            return if matches!(key, Key::Named(Named::ArrowUp | Named::ArrowDown)) {
                self.version_reveal()
            } else {
                Task::none()
            };
        }
        // Marks clear after the log, version picker, results, and context
        // menu, and before the panel.
        if key == Key::Named(Named::Escape)
            && !self.marked.is_empty()
            && !(self.panel_log_visible()
                || self.version_visible()
                || self.checkup_visible()
                || self.action_menu.is_some())
        {
            self.clear_marks();
            return Task::none();
        }
        match action(
            &key,
            modifiers,
            search_focused,
            self.panel_log_visible() || self.version_visible(),
            self.panel_open,
            self.search.is_empty(),
        ) {
            Action::None => Task::none(),
            // Collapsed, search opens in its drop-down.
            Action::FocusSearch if self.search_icon => self.open_search(),
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
                self.interrupt_zoom();
                // With nothing in view there's nowhere to go, so the kept
                // selection and the search's focus both stay.
                if self.grid_places().ids.is_empty() {
                    return Task::none();
                }
                let old = self.selected.clone();
                if let Some(first) = self.grid_places().ids.first() {
                    self.set_cursor(first.clone());
                }
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
                self.interrupt_zoom();
                let old = self.selected.clone();
                let places = self.grid_places();
                // A selection the filters hide stays put when there's
                // nothing to move to.
                if places.ids.is_empty() {
                    return Task::none();
                }
                // The arrows walk slots, so stepping onto another copy of
                // the selected game moves the cursor and leaves the panel.
                // A cursor on a header starts from there.
                let current = match self.header_cursor().and_then(|key| {
                    places
                        .headers
                        .iter()
                        .position(|place| &place.header.key == key)
                }) {
                    Some(header) => Some(Spot::Header(header)),
                    None => self.selected_index(&places).map(Spot::Card),
                };
                if let Some(next) = walk(&places, current, direction) {
                    self.set_cursor(places.ids[next].clone());
                }
                if self.selected != old {
                    self.selection_changed();
                }
                Task::batch([
                    self.scroll_selection(),
                    self.load_visible_art(),
                    self.load_panel_art(),
                ])
            }
            // Same rule as Enter: only a shown card, and only when nothing
            // else has the keys.
            Action::ToggleFavorite => {
                if captured
                    || self.action_menu.is_some()
                    || self.version_visible()
                    || (self.checkup_visible() && !self.panel_log_visible())
                    || !self.selected_shown()
                {
                    return Task::none();
                }
                match self.selected.clone() {
                    Some(name) => self.toggle_flag(&name, crate::library_actions::Flag::Favorite),
                    None => Task::none(),
                }
            }
            // Same scope as favoriting: nothing else has the keys. A card
            // doesn't need to be selected.
            Action::PickRandom => {
                if captured
                    || self.action_menu.is_some()
                    || self.version_visible()
                    || (self.checkup_visible() && !self.panel_log_visible())
                {
                    return Task::none();
                }
                self.update(Message::Random(crate::random_pick::Event::Pick))
            }
            // Search has no use for Ctrl+L, but anything else that took it
            // keeps it.
            Action::ToggleLayout => {
                if captured && !search_focused {
                    return Task::none();
                }
                self.update(Message::Layout(crate::layout_switch::Event::Toggle))
            }
            // A captured Ctrl+A is the log's or the search's own select all.
            Action::MarkAll => {
                if captured || !self.marking_keys_free() {
                    return Task::none();
                }
                self.mark_all();
                Task::none()
            }
            // A focused button takes Space only when it isn't a repeat, so
            // whether anything has focus is asked rather than read from the
            // capture.
            Action::MarkSelected => {
                if captured || !self.marking_keys_free() || !self.selected_shown() {
                    return Task::none();
                }
                iced::advanced::widget::operate(
                    iced::advanced::widget::operation::focusable::count(),
                )
                .map(|count| Message::SpaceFocus(count.focused.is_some()))
            }
            Action::OpenOrPlay => {
                if self.action_menu.is_some()
                    || self.version_visible()
                    || (self.checkup_visible() && !self.panel_log_visible())
                    || !self.selected_shown()
                {
                    Task::none()
                } else if self.panel_open {
                    self.start_selected()
                } else if let Some(name) = self.selected.clone() {
                    self.update(Message::OpenCard(name))
                } else {
                    Task::none()
                }
            }
        }
    }

    /// The selected game has a copy in view: shown, and not only in
    /// collapsed sections (plan 0062, question 22).
    pub(crate) fn selected_shown(&self) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|name| self.grid_places().first(name).is_some())
    }

    pub(crate) fn selection_changed(&mut self) {
        self.close_log();
        self.close_checkup();
        self.target = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
            .and_then(|card| card.default_target.clone());
        self.panel_error = None;
        self.panel_note = None;
    }

    /// Brings the card the cursor is on into view, now or, while the grid
    /// moves, once it settles.
    pub(crate) fn scroll_selection(&mut self) -> Task<Message> {
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        if self.grid_motion.active() || self.zooming.moving() {
            // The copy is found again when it fires, so it follows the
            // cursor meanwhile.
            self.grid_scroll_after_settle = Some(name);
        }
        // A selection the grid doesn't show has no card to find.
        let Some(slot) = self.selected_slot() else {
            return Task::none();
        };
        let generation = self.grid_revision();
        crate::grid_motion::locate_card(&slot)
            .map(move |bounds| Message::GridCardLocated(slot.clone(), generation, bounds))
    }
}

/// Where the cursor is: on a card's slot, or on a section's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spot {
    Card(usize),
    Header(usize),
}

/// The slot an arrow moves the cursor to from `current`, the first slot
/// without one. Left and Right follow the shown order and wrap; from a
/// header they go to the cards either side of it. Up and Down go to the
/// nearest row above or below by where the slots sit, so they cross
/// section headers, and land on the slot whose middle is nearest the
/// cursor's, the leftmost on a tie; from a header that's the row's first.
/// With no row there, the cursor stays: `None` from a header.
pub(crate) fn walk(
    places: &crate::grid_motion::Places,
    current: Option<Spot>,
    direction: Direction,
) -> Option<usize> {
    let count = places.slots.len();
    if count == 0 {
        return None;
    }
    let (here, middle, before, after) = match current {
        None => return Some(0),
        Some(Spot::Card(index)) => {
            let here = places.slots[index];
            (here, here.center_x(), index + count - 1, index + 1)
        }
        Some(Spot::Header(header)) => {
            let place = places.headers.get(header)?;
            let at = place.header.at;
            (place.slot, place.slot.x, at + count - 1, at)
        }
    };
    let stay = match current {
        Some(Spot::Card(index)) => Some(index),
        _ => None,
    };
    let row = match direction {
        Direction::Left => return Some(before % count),
        Direction::Right => return Some(after % count),
        Direction::Up => places
            .slots
            .iter()
            .map(|slot| slot.y)
            .filter(|y| *y < here.y)
            .reduce(f32::max),
        Direction::Down => places
            .slots
            .iter()
            .map(|slot| slot.y)
            .filter(|y| *y > here.y)
            .reduce(f32::min),
    };
    let Some(row) = row else {
        return stay;
    };
    places
        .slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.y == row)
        .min_by(|(_, a), (_, b)| {
            (a.center_x() - middle)
                .abs()
                .total_cmp(&(b.center_x() - middle).abs())
        })
        .map(|(index, _)| index)
}

/// The arrows on one uniform grid, as the art picker's are. `walk` moves
/// the same way over a flat library.
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
