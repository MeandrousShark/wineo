//! The library's Filters popup: store, platform, and tag filters, Show hidden, and
//! Clear filters. None of it is saved, so each launch shows everything.

use std::collections::BTreeSet;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Row, button, column, container, mouse_area, row, scrollable, text};
use iced::{Center, Element, Font, Point, Rectangle, Task, border};
use wineo_core::source::{Platform, Store};

use crate::grid::{
    ACTION_MENU_HEADER_HEIGHT, ACTION_MENU_MARGIN, ACTION_MENU_PADDING, ACTION_MENU_SPACING,
    ACTION_MENU_WIDTH, context_menu_style, menu_position,
};
use crate::library_menu::{CHECK, CHEVRON, anchor_under, check_box, icon, menu_row, menu_row_with};
use crate::theme::Palette;
use crate::ui::{self, ScrollStyle};
use crate::{App, Message, focus_button};

pub(crate) const FILTER_BUTTON_ID: &str = "library-filters";
const SHOW_HIDDEN_ID: &str = "library-filter-show-hidden";
const CLEAR_ID: &str = "library-filter-clear";
pub(crate) const MENU_SCROLL_ID: &str = "library-filter-menu";
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
    ToggleStore(Store),
    TogglePlatform(Platform),
    ToggleTag(String),
    /// A right-click, or X on the focused row, which excludes an off value
    /// and turns a chosen one off.
    ExcludeStore(Store),
    ExcludePlatform(Platform),
    ExcludeTag(String),
    ToggleShowHidden,
    /// An empty state's Show hidden, which only turns it on.
    ShowHidden,
    /// Clears the search and the store, platform, and tag filters, included
    /// and excluded, leaving the sort and Show hidden alone.
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
    /// The presets at the top of the popup.
    Preset(crate::presets::Event),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The reply always arrives, so it can clear its request even when
        // an overlay is up; `OpenUnder` checks the window itself.
        match self {
            Self::Preset(event) => event.is_input(),
            _ => !matches!(self, Self::OpenUnder(..) | Self::ButtonBounds(_)),
        }
    }
}

/// Where a popup row stands: off, included, or excluded, never two at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Off,
    Included,
    Excluded,
}

/// How a row was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    /// A click, Enter, or Space.
    Click,
    /// A right-click or X.
    Exclude,
}

impl Mark {
    fn of<T: Ord>(include: &BTreeSet<T>, exclude: &BTreeSet<T>, value: &T) -> Self {
        if include.contains(value) {
            Self::Included
        } else if exclude.contains(value) {
            Self::Excluded
        } else {
            Self::Off
        }
    }

    /// A click includes an off row and turns off one that's included or
    /// excluded, so clicking a ticked row unticks it as it always has.
    fn clicked(self) -> Self {
        match self {
            Self::Off => Self::Included,
            Self::Included | Self::Excluded => Self::Off,
        }
    }

    /// Excluding an off row excludes it; any press on a chosen row turns it
    /// off, so a right-click never flips an include straight to an exclude.
    fn excluded(self) -> Self {
        match self {
            Self::Off => Self::Excluded,
            Self::Included | Self::Excluded => Self::Off,
        }
    }

    fn after(self, press: Press) -> Self {
        match press {
            Press::Click => self.clicked(),
            Press::Exclude => self.excluded(),
        }
    }
}

/// Applies `press` to `value`'s row, moving it between the two sets.
fn press_row<T: Ord>(include: &mut BTreeSet<T>, exclude: &mut BTreeSet<T>, value: T, press: Press) {
    let next = Mark::of(include, exclude, &value).after(press);
    include.remove(&value);
    exclude.remove(&value);
    match next {
        Mark::Off => {}
        Mark::Included => {
            include.insert(value);
        }
        Mark::Excluded => {
            exclude.insert(value);
        }
    }
}

fn store_id(store: Store) -> String {
    format!("library-filter-store-{}", store.label().to_lowercase())
}

fn platform_id(platform: Platform) -> String {
    format!(
        "library-filter-platform-{}",
        platform.label().to_lowercase()
    )
}

/// What the popup offers, each group A to Z by label.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Offered {
    pub(crate) stores: Vec<Store>,
    pub(crate) platforms: Vec<Platform>,
    pub(crate) tags: Vec<String>,
}

/// `listed` plus any `chosen` or `excluded` it lacks, sorted A to Z by
/// `label`.
fn with_chosen<T: Copy + Ord>(
    listed: &[T],
    chosen: &BTreeSet<T>,
    excluded: &BTreeSet<T>,
    label: fn(T) -> &'static str,
) -> Vec<T> {
    let mut values = listed.to_vec();
    for value in chosen.iter().chain(excluded) {
        if !values.contains(value) {
            values.push(*value);
        }
    }
    values.sort_by_key(|value| label(*value).to_lowercase());
    values
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
    /// plus any chosen or excluded one it no longer has, so a filter never
    /// turns off behind your back and can still be turned off here.
    pub(crate) fn offered(&self) -> Offered {
        let options = self.order.options(self.order_inputs());
        let stores = with_chosen(
            &options.stores,
            &self.filters.stores,
            &self.filters.not_stores,
            Store::label,
        );
        let platforms = with_chosen(
            &options.platforms,
            &self.filters.platforms,
            &self.filters.not_platforms,
            Platform::label,
        );
        let mut tags = options.tags.clone();
        let mut gone = false;
        for chosen in self.filters.tags.iter().chain(&self.filters.not_tags) {
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

    /// Every focusable control in the popup, in order: the presets part,
    /// then the filter rows.
    pub(crate) fn popup_ids(&self) -> Vec<String> {
        let mut ids = self.preset_ids();
        ids.extend(self.filter_menu_ids());
        ids
    }

    /// The popup's focusable filter rows, in order.
    fn filter_menu_ids(&self) -> Vec<String> {
        let offered = self.offered();
        let mut ids: Vec<String> = offered.stores.into_iter().map(store_id).collect();
        ids.extend(offered.platforms.into_iter().map(platform_id));
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
        Event::ToggleStore(store) => press_store(app, store, Press::Click),
        Event::TogglePlatform(platform) => press_platform(app, platform, Press::Click),
        Event::ToggleTag(tag) => press_tag(app, tag, Press::Click),
        Event::ExcludeStore(store) => press_store(app, store, Press::Exclude),
        Event::ExcludePlatform(platform) => press_platform(app, platform, Press::Exclude),
        Event::ExcludeTag(tag) => press_tag(app, tag, Press::Exclude),
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
            app.filters.clear();
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
        Event::Wheel(delta) => {
            app.interrupt_zoom();
            crate::filter_tab::wheel(delta, app.modifiers.shift())
        }
        Event::Preset(event) => crate::presets::update(app, event),
    }
}

fn press_store(app: &mut App, store: Store, press: Press) -> Task<Message> {
    let filters = &mut app.filters;
    press_row(&mut filters.stores, &mut filters.not_stores, store, press);
    app.resorted()
}

fn press_platform(app: &mut App, platform: Platform, press: Press) -> Task<Message> {
    let filters = &mut app.filters;
    press_row(
        &mut filters.platforms,
        &mut filters.not_platforms,
        platform,
        press,
    );
    app.resorted()
}

fn press_tag(app: &mut App, tag: String, press: Press) -> Task<Message> {
    let filters = &mut app.filters;
    press_row(&mut filters.tags, &mut filters.not_tags, tag, press);
    app.resorted()
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.close_library_menus();
    app.filter_menu = true;
    app.filter_menu_position = position;
    app.presets_opened();
    // Focus moves into the popup, so a second Enter or Space toggles a row
    // instead of closing it again. It lands on the first filter row as it
    // did before presets, which sit one Up or Shift+Tab above it.
    let first = app.filter_menu_ids().swap_remove(0);
    crate::library_menu::focus(first, MENU_SCROLL_ID)
}

/// Keys while the popup is open: Escape closes a preset's edit, or else
/// the popup, Tab and the up and down arrows move through its rows, and the
/// rows take Enter, Space, X, R, and Delete themselves. Nothing else reaches
/// the grid behind it.
pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => {
            if let Some(task) = app.cancel_preset_edit() {
                return task;
            }
            app.filter_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(FILTER_BUTTON_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            let reverse = *key == Key::Named(Named::ArrowUp)
                || (*key == Key::Named(Named::Tab) && modifiers.shift());
            crate::library_menu::cycle(&app.popup_ids(), reverse, MENU_SCROLL_ID)
        }
        _ => Task::none(),
    }
}

/// The Filters button's label: the active preset while the view matches
/// it, or else how many store, platform, and tag filters are on, included
/// or excluded.
pub(crate) fn button_label(app: &App) -> String {
    if let Some(name) = app.preset_shown() {
        let name = crate::tile_title::fit(name, crate::presets::BUTTON_NAME, 14.0, Font::DEFAULT);
        return format!("Filters ({name})");
    }
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

/// A box that shows an excluded row: the check box's shape, filled with the
/// danger color and holding a drawn x, so the two read as the same kind of mark.
fn exclude_box(palette: Palette) -> Element<'static, Message> {
    container(icon(
        crate::filter_tab::CROSS,
        ui::accent_ink(palette.danger),
        12.0,
    ))
    .width(16)
    .height(16)
    .center(16)
    .style(move |_| {
        container::Style::default()
            .background(palette.danger)
            .border(border::rounded(4).width(1).color(palette.danger))
    })
    .into()
}

/// The box in front of a store or platform row.
fn mark_box(mark: Mark, palette: Palette) -> Element<'static, Message> {
    match mark {
        Mark::Off => check_box(false, palette),
        Mark::Included => check_box(true, palette),
        Mark::Excluded => exclude_box(palette),
    }
}

/// A tag that toggles its filter. Included, it has a check mark and the
/// accent tint and border the sidebar's selected item uses; excluded, a drawn
/// x with the same tint and border in the danger color. Either way it reads
/// as chosen without color alone, and the focus ring stays outside it.
fn chip(tag: String, mark: Mark, palette: Palette) -> Element<'static, Message> {
    let id = tag_id(&tag);
    let message = Message::Filter(Event::ToggleTag(tag.clone()));
    let exclude = Message::Filter(Event::ExcludeTag(tag.clone()));
    let label: Element<'static, Message> = match mark {
        Mark::Included => row![icon(CHECK, palette.primary, 12.0), text(tag).size(13)]
            .spacing(4)
            .align_y(Center)
            .into(),
        Mark::Excluded => row![
            icon(crate::filter_tab::CROSS, palette.danger, 12.0),
            text(tag).size(13)
        ]
        .spacing(4)
        .align_y(Center)
        .into(),
        Mark::Off => text(tag).size(13).into(),
    };
    let chip = button(label)
        .padding([4, 10])
        .on_press(message.clone())
        .style(move |_, status| {
            let mut style = context_menu_style(palette, false, status);
            style.border = border::rounded(12).width(1).color(match mark {
                Mark::Included => palette.primary,
                Mark::Excluded => palette.danger,
                Mark::Off => palette.border,
            });
            let tint = match status {
                button::Status::Hovered | button::Status::Pressed => 0.24,
                _ => 0.15,
            };
            match mark {
                Mark::Included => {
                    style.background = Some(palette.primary.scale_alpha(tint).into());
                }
                Mark::Excluded => {
                    style.background = Some(palette.danger.scale_alpha(tint).into());
                }
                Mark::Off => {}
            }
            style
        });
    let control = focus_button::excludable(chip.into(), id, message, exclude.clone());
    mouse_area(control).on_right_press(exclude).into()
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
                .font(crate::fonts::semibold())
                .color(palette.foreground)
                .height(ACTION_MENU_HEADER_HEIGHT)
                .align_y(Center),
            heading("Presets", palette),
        ]
        .spacing(ACTION_MENU_SPACING);
        for item in self.presets_view() {
            rows = rows.push(item);
        }
        rows = rows.push(ui::separator(palette));
        // Each row: its label, mark, id, and what a click and an exclude send.
        let filters = &self.filters;
        let stores = stores.into_iter().map(|store| {
            (
                store.label(),
                Mark::of(&filters.stores, &filters.not_stores, &store),
                store_id(store),
                Event::ToggleStore(store),
                Event::ExcludeStore(store),
            )
        });
        let platforms = platforms.into_iter().map(|platform| {
            (
                platform.label(),
                Mark::of(&filters.platforms, &filters.not_platforms, &platform),
                platform_id(platform),
                Event::TogglePlatform(platform),
                Event::ExcludePlatform(platform),
            )
        });
        let groups = [
            ("Store", stores.collect::<Vec<_>>()),
            ("Platform", platforms.collect()),
        ];
        for (title, values) in groups {
            if values.is_empty() {
                continue;
            }
            rows = rows.push(heading(title, palette));
            for (label, mark, id, toggle, exclude) in values {
                rows = rows.push(menu_row_with(
                    row![mark_box(mark, palette), text(label).size(14)]
                        .spacing(8)
                        .align_y(Center)
                        .into(),
                    palette,
                    false,
                    id,
                    Message::Filter(toggle),
                    Some(Message::Filter(exclude)),
                ));
            }
        }
        if !tags.is_empty() {
            rows = rows.push(heading("Tags", palette));
            let chips = tags.into_iter().map(|tag| {
                let mark = Mark::of(&self.filters.tags, &self.filters.not_tags, &tag);
                chip(tag, mark, palette)
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
            .push(ui::separator(palette))
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
            .push(ui::separator(palette))
            .push(focus_button::control(
                clear.into(),
                clear_enabled,
                CLEAR_ID,
                clear_message,
            ))
            .push(
                text("Right-click or press X to exclude.")
                    .size(12)
                    .color(palette.muted),
            );
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
