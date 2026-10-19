//! The library's Filters popup: source and tag filters, Show hidden, and
//! Clear filters. None of it is saved, so each launch shows everything.

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Row, button, column, container, row, scrollable, text};
use iced::{Center, Element, Font, Point, Rectangle, Task, border, font};

use crate::grid::{
    ACTION_MENU_HEADER_HEIGHT, ACTION_MENU_MARGIN, ACTION_MENU_PADDING, ACTION_MENU_SPACING,
    ACTION_MENU_WIDTH, context_menu_style, menu_position,
};
use crate::library_menu::{CHEVRON, anchor_under, check_box, icon, menu_row};
use crate::theme::Palette;
use crate::ui::{self, ScrollStyle};
use crate::{App, Message, focus_button};

pub(crate) const FILTER_BUTTON_ID: &str = "library-filters";
const SHOW_HIDDEN_ID: &str = "library-filter-show-hidden";
const CLEAR_ID: &str = "library-filter-clear";
/// What the popup's placement assumes it needs at least, so a pointer near
/// the bottom of a short window still leaves room for a few rows.
const MIN_HEIGHT: f32 = 200.0;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click on the Filters button, which opens the popup at the cursor.
    ToggleMenu,
    /// Enter or Space on the focused Filters button.
    ToggleMenuFromKeyboard,
    /// The Filters button's bounds, found for the keyboard's open request
    /// with this number.
    OpenUnder(u64, Option<Rectangle>),
    /// A click outside the popup.
    CloseMenu,
    ToggleSource(&'static str),
    ToggleTag(String),
    ToggleShowHidden,
    /// An empty state's Show hidden, which only turns it on.
    ShowHidden,
    /// Clears the search and the source and tag filters, leaving the sort
    /// and Show hidden alone.
    Clear,
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The reply always arrives, so it can clear its request even when
        // an overlay is up; `OpenUnder` checks the window itself.
        !matches!(self, Self::OpenUnder(..))
    }
}

fn source_id(label: &str) -> String {
    format!("library-filter-source-{}", label.to_lowercase())
}

fn tag_id(tag: &str) -> String {
    format!("library-filter-tag-{tag}")
}

impl App {
    /// The popup shows only over the Library itself.
    pub(crate) fn filter_menu_shown(&self) -> bool {
        self.filter_menu
            && self.section == crate::sidebar::Section::Library
            && !self.panel_hidden()
            && self.hook_stop_confirm.is_none()
    }

    /// Clear filters has something to clear.
    pub(crate) fn filters_clearable(&self) -> bool {
        !self.search.is_empty() || self.filters.active() > 0
    }

    /// The sources and tags the popup offers: the library's, plus any chosen
    /// one it no longer has, so a filter never turns off behind your back
    /// and can still be turned off here.
    fn offered(&self) -> (Vec<&'static str>, Vec<String>) {
        let options = self.order.options(self.order_inputs());
        let mut sources = options.sources.clone();
        for chosen in &self.filters.sources {
            if !sources.contains(chosen) {
                sources.push(chosen);
            }
        }
        sources.sort_by_key(|label| label.to_lowercase());
        let mut tags = options.tags.clone();
        let mut gone = false;
        for chosen in &self.filters.tags {
            if !tags.contains(chosen) {
                tags.push(chosen.clone());
                gone = true;
            }
        }
        if gone {
            tags.sort_by_cached_key(|tag| (tag.to_lowercase(), tag.clone()));
        }
        (sources, tags)
    }

    /// The popup's focusable rows, in order.
    fn filter_menu_ids(&self) -> Vec<String> {
        let (sources, tags) = self.offered();
        let mut ids: Vec<String> = sources.iter().map(|label| source_id(label)).collect();
        ids.extend(tags.iter().map(|tag| tag_id(tag)));
        ids.push(SHOW_HIDDEN_ID.to_owned());
        if self.filters_clearable() {
            ids.push(CLEAR_ID.to_owned());
        }
        ids
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::ToggleMenu => {
            app.filter_opening.cancel();
            if app.filter_menu {
                app.filter_menu = false;
                return Task::none();
            }
            open_menu(app, app.menu_anchor)
        }
        Event::ToggleMenuFromKeyboard => {
            if app.filter_menu || app.filter_opening.pending.is_some() {
                app.filter_opening.cancel();
                app.filter_menu = false;
                return Task::none();
            }
            crate::library_menu::request_bounds(
                &mut app.filter_opening,
                FILTER_BUTTON_ID,
                |request, bounds| Message::Filter(Event::OpenUnder(request, bounds)),
            )
        }
        Event::OpenUnder(request, bounds) => {
            if !app.filter_opening.answer(request) || !app.library_menu_may_open() {
                return Task::none();
            }
            open_menu(app, bounds.map_or(app.menu_anchor, anchor_under))
        }
        Event::CloseMenu => {
            app.filter_opening.cancel();
            app.filter_menu = false;
            Task::none()
        }
        Event::ToggleSource(label) => {
            if !app.filters.sources.remove(label) {
                app.filters.sources.insert(label);
            }
            app.resorted()
        }
        Event::ToggleTag(tag) => {
            if !app.filters.tags.remove(&tag) {
                app.filters.tags.insert(tag);
            }
            app.resorted()
        }
        Event::ToggleShowHidden => {
            app.filters.show_hidden = !app.filters.show_hidden;
            app.resorted()
        }
        Event::ShowHidden => {
            if app.filters.show_hidden {
                return Task::none();
            }
            app.filters.show_hidden = true;
            app.resorted()
        }
        Event::Clear => {
            if !app.filters_clearable() {
                return Task::none();
            }
            app.search.clear();
            app.filters.sources.clear();
            app.filters.tags.clear();
            let resorted = app.resorted();
            if !app.filter_menu {
                return resorted;
            }
            // Its row is gone with nothing left to clear, so focus goes back
            // where the popup came from.
            app.filter_menu = false;
            Task::batch([
                resorted,
                iced::widget::operation::focus(iced::widget::Id::new(FILTER_BUTTON_ID)),
            ])
        }
    }
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.close_library_menus();
    app.filter_menu = true;
    app.filter_menu_position = position;
    // Focus moves into the popup, so a second Enter or Space toggles a row
    // instead of closing it again.
    let first = app.filter_menu_ids().swap_remove(0);
    iced::widget::operation::focus(iced::widget::Id::from(first))
}

/// Keys while the popup is open: Escape closes it, Tab and the up and down
/// arrows move through its rows, and the rows take Enter and Space
/// themselves. Nothing else reaches the grid behind it.
pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => {
            app.filter_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(FILTER_BUTTON_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            let reverse = *key == Key::Named(Named::ArrowUp)
                || (*key == Key::Named(Named::Tab) && modifiers.shift());
            iced::advanced::widget::operate(focus_button::cycle(&app.filter_menu_ids(), reverse))
                .discard()
        }
        _ => Task::none(),
    }
}

/// The Filters button's label, with how many source and tag filters are on.
pub(crate) fn button_label(app: &App) -> String {
    match app.filters.active() {
        0 => "Filters".to_owned(),
        count => format!("Filters ({count})"),
    }
}

/// The Filters button, for the toolbar after the direction button.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    let filters = button(
        row![
            text(button_label(app)).size(14),
            icon(CHEVRON, palette.muted, 16.0),
        ]
        .spacing(6)
        .align_y(Center),
    )
    .height(ui::BUTTON_HEIGHT)
    .padding([0, 12])
    .on_press(Message::Filter(Event::ToggleMenu))
    .style(move |_, status| ui::secondary_style(palette, status));
    focus_button::control(
        filters.into(),
        true,
        FILTER_BUTTON_ID,
        Message::Filter(Event::ToggleMenuFromKeyboard),
    )
}

fn heading(label: &str, palette: Palette) -> Element<'_, Message> {
    text(label)
        .size(12)
        .color(palette.muted)
        .height(ACTION_MENU_HEADER_HEIGHT)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}

/// A tag that toggles its filter, tinted while it's on.
fn chip(tag: String, on: bool, palette: Palette) -> Element<'static, Message> {
    let id = tag_id(&tag);
    let message = Message::Filter(Event::ToggleTag(tag.clone()));
    let chip = button(text(tag).size(13))
        .padding([4, 10])
        .on_press(message.clone())
        .style(move |_, status| {
            let mut style = context_menu_style(palette, false, status);
            style.border = border::rounded(12).width(1).color(if on {
                palette.primary
            } else {
                palette.border
            });
            if on && matches!(status, button::Status::Active) {
                style.background = Some(palette.primary.scale_alpha(0.16).into());
            }
            style
        });
    focus_button::control(chip.into(), true, id, message)
}

impl App {
    /// The open Filters popup over the page, placed like the Sort menu. It
    /// grows with its content up to the window's bottom edge, then scrolls.
    pub(crate) fn filter_menu_view(&self) -> [Element<'_, Message>; 2] {
        let palette = self.palette;
        let position = menu_position(self.filter_menu_position, self.window_size, MIN_HEIGHT);
        let max_height = (self.window_size.height - position.y - ACTION_MENU_MARGIN).max(0.0);
        let (sources, tags) = self.offered();
        let counts = self.order.counts(self.order_inputs());
        let mut rows = column![
            text("Filters")
                .size(14)
                .font(Font {
                    weight: font::Weight::Semibold,
                    ..Font::DEFAULT
                })
                .color(palette.foreground)
                .height(ACTION_MENU_HEADER_HEIGHT)
                .align_y(Center),
        ]
        .spacing(ACTION_MENU_SPACING);
        if !sources.is_empty() {
            rows = rows.push(heading("Source", palette));
            for label in sources {
                rows = rows.push(menu_row(
                    row![
                        check_box(self.filters.sources.contains(label), palette),
                        text(label).size(14),
                    ]
                    .spacing(8)
                    .align_y(Center)
                    .into(),
                    palette,
                    false,
                    source_id(label),
                    Message::Filter(Event::ToggleSource(label)),
                ));
            }
        }
        if !tags.is_empty() {
            rows = rows.push(heading("Tags", palette));
            let chips = tags.into_iter().map(|tag| {
                let on = self.filters.tags.contains(&tag);
                chip(tag, on, palette)
            });
            rows = rows.push(
                container(
                    Row::with_children(chips)
                        .spacing(6)
                        .wrap()
                        .vertical_spacing(6),
                )
                .padding([4, 0]),
            );
        }
        let clear_enabled = self.filters_clearable();
        let clear_message = Message::Filter(Event::Clear);
        let clear = button(
            container(text("Clear filters").size(14))
                .height(iced::Fill)
                .center_y(iced::Fill),
        )
        .width(iced::Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press_maybe(clear_enabled.then(|| clear_message.clone()))
        .style(move |_, status| context_menu_style(palette, false, status));
        rows = rows
            .push(ui::separator(palette).padding([3, 0]))
            .push(menu_row(
                row![
                    check_box(self.filters.show_hidden, palette),
                    text(format!("Show hidden ({})", counts.hidden)).size(14),
                ]
                .spacing(8)
                .align_y(Center)
                .into(),
                palette,
                false,
                SHOW_HIDDEN_ID,
                Message::Filter(Event::ToggleShowHidden),
            ))
            .push(ui::separator(palette).padding([3, 0]))
            .push(focus_button::control(
                clear.into(),
                clear_enabled,
                CLEAR_ID,
                clear_message,
            ));
        let menu = container(scrollable(rows).wineo_style(palette))
            .padding(ACTION_MENU_PADDING)
            .width(ACTION_MENU_WIDTH)
            .max_height(max_height)
            .style(move |_| crate::library_menu::surface_style(palette));
        crate::library_menu::layers(menu.into(), position, Message::Filter(Event::CloseMenu))
    }
}

#[cfg(test)]
pub(crate) mod tests;
