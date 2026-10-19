//! The library's Filters popup: store, platform, and tag filters, Show hidden, and
//! Clear filters. None of it is saved, so each launch shows everything.

use std::collections::BTreeSet;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Row, button, column, container, row, scrollable, text};
use iced::{Center, Element, Font, Point, Rectangle, Task, border, font};

use crate::grid::{
    ACTION_MENU_HEADER_HEIGHT, ACTION_MENU_MARGIN, ACTION_MENU_PADDING, ACTION_MENU_SPACING,
    ACTION_MENU_WIDTH, context_menu_style, menu_position,
};
use crate::library_menu::{CHECK, CHEVRON, anchor_under, check_box, icon, menu_row};
use crate::theme::Palette;
use crate::ui::{self, ScrollStyle};
use crate::{App, Message, focus_button};

pub(crate) const FILTER_BUTTON_ID: &str = "library-filters";
const SHOW_HIDDEN_ID: &str = "library-filter-show-hidden";
const CLEAR_ID: &str = "library-filter-clear";
const MENU_SCROLL_ID: &str = "library-filter-menu";
/// What the popup's placement assumes it needs at least, so a button near
/// the bottom of a short window still leaves room for a few rows.
const MIN_HEIGHT: f32 = 200.0;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click, Enter, or Space on the Filters button or the filter tab's
    /// +N, which opens the popup under the Filters button once its bounds
    /// are found.
    ToggleMenu,
    /// The Filters button's bounds, found for the open request with this
    /// number.
    OpenUnder(u64, Option<Rectangle>),
    /// A click outside the popup.
    CloseMenu,
    ToggleStore(&'static str),
    TogglePlatform(&'static str),
    ToggleTag(String),
    ToggleShowHidden,
    /// An empty state's Show hidden, which only turns it on.
    ShowHidden,
    /// Clears the search and the store, platform, and tag filters, leaving the sort
    /// and Show hidden alone.
    Clear,
    /// Where the Filters button is now, for the filter tab.
    ButtonBounds(Rectangle),
    /// A filter tab chip's x.
    Remove(crate::filter_tab::Chip),
    /// Enter or Space on a filter tab chip's x.
    RemoveFromKeyboard(crate::filter_tab::Chip),
    /// The filter tab's x, which leaves the search alone.
    ClearChips,
    /// Enter or Space on the filter tab's x.
    ClearChipsFromKeyboard,
    /// A wheel turn over the filter tab, which scrolls the grid under it.
    Wheel(iced::mouse::ScrollDelta),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The reply always arrives, so it can clear its request even when
        // an overlay is up; `OpenUnder` checks the window itself.
        !matches!(self, Self::OpenUnder(..) | Self::ButtonBounds(_))
    }
}

fn store_id(label: &str) -> String {
    format!("library-filter-store-{}", label.to_lowercase())
}

fn platform_id(label: &str) -> String {
    format!("library-filter-platform-{}", label.to_lowercase())
}

/// What the popup offers, each group A to Z.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Offered {
    pub(crate) stores: Vec<&'static str>,
    pub(crate) platforms: Vec<&'static str>,
    pub(crate) tags: Vec<String>,
}

/// `listed` plus any `chosen` it lacks, sorted A to Z.
fn with_chosen(listed: &[&'static str], chosen: &BTreeSet<&'static str>) -> Vec<&'static str> {
    let mut labels = listed.to_vec();
    for label in chosen {
        if !labels.contains(label) {
            labels.push(label);
        }
    }
    labels.sort_by_key(|label| label.to_lowercase());
    labels
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

    /// What the popup offers: the library's stores, platforms, and tags,
    /// plus any chosen one it no longer has, so a filter never turns off
    /// behind your back and can still be turned off here.
    pub(crate) fn offered(&self) -> Offered {
        let options = self.order.options(self.order_inputs());
        let stores = with_chosen(&options.stores, &self.filters.stores);
        let platforms = with_chosen(&options.platforms, &self.filters.platforms);
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
        Offered {
            stores,
            platforms,
            tags,
        }
    }

    /// The popup's focusable rows, in order.
    fn filter_menu_ids(&self) -> Vec<String> {
        let offered = self.offered();
        let mut ids: Vec<String> = offered.stores.iter().map(|label| store_id(label)).collect();
        ids.extend(offered.platforms.iter().map(|label| platform_id(label)));
        ids.extend(offered.tags.iter().map(|tag| tag_id(tag)));
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
            match bounds {
                Some(bounds) => open_menu(app, anchor_under(bounds)),
                None => Task::none(),
            }
        }
        Event::CloseMenu => {
            app.filter_opening.cancel();
            app.filter_menu = false;
            Task::none()
        }
        Event::ToggleStore(label) => {
            if !app.filters.stores.remove(label) {
                app.filters.stores.insert(label);
            }
            app.resorted()
        }
        Event::TogglePlatform(label) => {
            if !app.filters.platforms.remove(label) {
                app.filters.platforms.insert(label);
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
            app.filters.stores.clear();
            app.filters.platforms.clear();
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
        Event::ButtonBounds(bounds) => {
            app.filter_button = Some(bounds);
            Task::none()
        }
        Event::Remove(chip) => crate::filter_tab::remove(app, &chip, false),
        Event::RemoveFromKeyboard(chip) => crate::filter_tab::remove(app, &chip, true),
        Event::ClearChips => crate::filter_tab::clear(app, false),
        Event::ClearChipsFromKeyboard => crate::filter_tab::clear(app, true),
        Event::Wheel(delta) => crate::filter_tab::wheel(delta, app.modifiers.shift()),
    }
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.close_library_menus();
    app.filter_menu = true;
    app.filter_menu_position = position;
    // Focus moves into the popup, so a second Enter or Space toggles a row
    // instead of closing it again.
    let first = app.filter_menu_ids().swap_remove(0);
    crate::library_menu::focus(first, MENU_SCROLL_ID)
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
            crate::library_menu::cycle(&app.filter_menu_ids(), reverse, MENU_SCROLL_ID)
        }
        _ => Task::none(),
    }
}

/// The Filters button's label, with how many store, platform, and tag filters are on.
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
        .height(iced::Fill)
        .align_y(Center),
    )
    .height(ui::BUTTON_HEIGHT)
    .padding([0, 12])
    .on_press(Message::Filter(Event::ToggleMenu))
    .style(move |_, status| ui::secondary_style(palette, status));
    crate::filter_tab::track(focus_button::control(
        filters.into(),
        true,
        FILTER_BUTTON_ID,
        Message::Filter(Event::ToggleMenu),
    ))
}

fn heading(label: &str, palette: Palette) -> Element<'_, Message> {
    text(label)
        .size(12)
        .color(palette.muted)
        .height(ACTION_MENU_HEADER_HEIGHT)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}

/// A tag that toggles its filter. While it's on it has a check mark and
/// the accent tint and border the sidebar's selected item uses, so it reads
/// as chosen without color alone, and the focus ring stays outside it.
fn chip(tag: String, on: bool, palette: Palette) -> Element<'static, Message> {
    let id = tag_id(&tag);
    let message = Message::Filter(Event::ToggleTag(tag.clone()));
    let label: Element<'static, Message> = if on {
        row![icon(CHECK, palette.primary, 12.0), text(tag).size(13)]
            .spacing(4)
            .align_y(Center)
            .into()
    } else {
        text(tag).size(13).into()
    };
    let chip = button(label)
        .padding([4, 10])
        .on_press(message.clone())
        .style(move |_, status| {
            let mut style = context_menu_style(palette, false, status);
            style.border = border::rounded(12).width(1).color(if on {
                palette.primary
            } else {
                palette.border
            });
            if on {
                let tint = match status {
                    button::Status::Hovered | button::Status::Pressed => 0.24,
                    _ => 0.15,
                };
                style.background = Some(palette.primary.scale_alpha(tint).into());
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
        let Offered {
            stores,
            platforms,
            tags,
        } = self.offered();
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
        type Group<'a> = (
            &'a str,
            Vec<&'static str>,
            &'a BTreeSet<&'static str>,
            fn(&str) -> String,
            fn(&'static str) -> Event,
        );
        let groups: [Group<'_>; 2] = [
            (
                "Store",
                stores,
                &self.filters.stores,
                store_id,
                Event::ToggleStore,
            ),
            (
                "Platform",
                platforms,
                &self.filters.platforms,
                platform_id,
                Event::TogglePlatform,
            ),
        ];
        for (title, labels, checked, id, toggle) in groups {
            if labels.is_empty() {
                continue;
            }
            rows = rows.push(heading(title, palette));
            for label in labels {
                rows = rows.push(menu_row(
                    row![
                        check_box(checked.contains(label), palette),
                        text(label).size(14),
                    ]
                    .spacing(8)
                    .align_y(Center)
                    .into(),
                    palette,
                    false,
                    id(label),
                    Message::Filter(toggle(label)),
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
        let menu = container(
            scrollable(rows)
                .id(iced::widget::Id::new(MENU_SCROLL_ID))
                .wineo_style(palette),
        )
        .padding(ACTION_MENU_PADDING)
        .width(ACTION_MENU_WIDTH)
        .max_height(max_height)
        .style(move |_| crate::library_menu::surface_style(palette));
        crate::library_menu::layers(menu.into(), position, Message::Filter(Event::CloseMenu))
    }
}

#[cfg(test)]
pub(crate) mod tests;
