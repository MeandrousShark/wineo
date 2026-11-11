//! Search collapsed to an icon, once even everything folded into More
//! leaves the toolbar too wide: the icon opens the same field in a
//! drop-down under it, and the library filters live as you type.

use iced::widget::{button, container};
use iced::{Element, Fill, Task};

use crate::library_menu::icon;
use crate::{App, Message, SEARCH_ID, focus_button, ui, zoom_control};

pub(crate) const ICON_ID: &str = "library-search-icon";
pub(crate) const LABEL: &str = "Search";

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click, Enter, or Space on the icon.
    Toggle,
    /// A press outside the open drop-down.
    Close,
    /// Search collapsed while its field had focus.
    CollapsedWithFocus,
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        !matches!(self, Self::CollapsedWithFocus)
    }
}

impl App {
    /// The drop-down shows only over the Library, while search is an icon.
    pub(crate) fn search_dropdown_shown(&self) -> bool {
        self.search_open
            && self.search_icon
            && self.section == crate::sidebar::Section::Library
            && !self.panel_hidden()
            && self.hook_stop_confirm.is_none()
    }

    /// Opens the drop-down with the field focused, as the icon, `/`, and
    /// Ctrl+F do. Like any toolbar menu it waits for the others to close.
    pub(crate) fn open_search(&mut self) -> Task<Message> {
        if !self.search_open {
            if !self.library_menu_may_open() {
                return Task::none();
            }
            self.close_library_menus();
            self.search_open = true;
        }
        iced::widget::operation::focus(iced::widget::Id::new(SEARCH_ID)).chain(
            iced::widget::operation::is_focused(iced::widget::Id::new(SEARCH_ID))
                .map(Message::SearchFocusChanged),
        )
    }

    /// Closes the drop-down, keeping its text, and gives the icon focus.
    pub(crate) fn close_search(&mut self) -> Task<Message> {
        self.search_open = false;
        self.search_focused = false;
        iced::widget::operation::focus(iced::widget::Id::new(ICON_ID))
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Toggle if app.search_open => app.close_search(),
        Event::Toggle => app.open_search(),
        Event::Close => app.close_search(),
        Event::CollapsedWithFocus => {
            if !app.library_menu_open() && app.action_menu.is_none() {
                app.search_open = true;
            }
            Task::none()
        }
    }
}

/// The icon's tooltip: the search text, or "Search" with none.
pub(crate) fn tooltip_label(app: &App) -> &str {
    if app.search.is_empty() {
        LABEL
    } else {
        &app.search
    }
}

/// The icon, in search's place once it collapses. While open, or while
/// it holds text, it shows the selected look, and its tooltip shows the
/// text.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    let selected = app.search_open || !app.search.is_empty();
    let search = button(
        container(icon(
            crate::library_states::SEARCH_ICON,
            palette.foreground,
            16.0,
        ))
        .center(Fill),
    )
    .width(ui::BUTTON_HEIGHT)
    .height(ui::BUTTON_HEIGHT)
    .padding(0)
    .on_press(Message::Search(Event::Toggle))
    .style(move |_, status| {
        let mut style = ui::secondary_style(palette, status);
        if selected {
            style.background = Some(
                palette
                    .primary
                    .scale_alpha(zoom_control::selected_tint(status))
                    .into(),
            );
            style.border = style.border.color(palette.primary);
        }
        style
    });
    focus_button::control(
        zoom_control::with_tooltip(search.into(), tooltip_label(app), palette),
        true,
        ICON_ID,
        Message::Search(Event::Toggle),
    )
}

#[cfg(test)]
mod tests;
