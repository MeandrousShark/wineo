//! The library's sort control: the Sort menu with its keys and "Favorites
//! first", the direction button, and saving each change to `library.toml`.

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{button, column, container, row, scrollable, text, tooltip};
use iced::{Center, Element, Fill, Font, Point, Rectangle, Task, border, font};
use wineo_core::library::{self, GroupBy, RandomPool, SortKey, ViewLayout};
use wineo_core::paths::Paths;

use crate::grid::{
    ACTION_MENU_DIVIDER, ACTION_MENU_HEADER_HEIGHT, ACTION_MENU_MARGIN, ACTION_MENU_PADDING,
    ACTION_MENU_SPACING, ACTION_MENU_WIDTH, menu_position,
};
#[cfg(test)]
use crate::library_menu::bounds_of;
use crate::library_menu::{CHEVRON, anchor_under, check_box, icon, menu_row};
use crate::ui::{self, ScrollStyle};
use crate::{App, Message, focus_button, rescan};

pub(crate) const SORT_BUTTON_ID: &str = "library-sort";
pub(crate) const DIRECTION_BUTTON_ID: &str = "library-sort-direction";
const FAVORITES_FIRST_ID: &str = "library-sort-favorites-first";
const GROUP_ID: &str = "library-sort-group";
/// The Group by submenu's width, which fits its longest row.
const GROUP_MENU_WIDTH: f32 = 160.0;
/// The submenu's gap from the Sort menu's side.
const GROUP_MENU_GAP: f32 = 4.0;
const MENU_SCROLL_ID: &str = "library-sort-menu";
const NOTICE_TIME: std::time::Duration = std::time::Duration::from_secs(8);

pub(crate) const ARROW_DOWN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 2.5v11M3.5 9l4.5 4.5L12.5 9" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(crate) const ARROW_UP: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 13.5v-11M3.5 7l4.5-4.5L12.5 7" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click, Enter, or Space on the Sort button, which opens the menu
    /// under it once its bounds are found.
    ToggleMenu,
    /// The Sort button's bounds, found for the open request with this
    /// number.
    OpenUnder(u64, Option<Rectangle>),
    /// A click outside the menu.
    CloseMenu,
    Pick(SortKey),
    /// A click on a list column's header: its sort, or the other
    /// direction when it's the sort already.
    Header(SortKey),
    FlipDirection,
    ToggleFavoritesFirst,
    /// The Group by row, which opens its submenu.
    OpenGroups,
    /// A Group by submenu row.
    Group(GroupBy),
    /// A write finished, with the value it wrote.
    Saved(Field, Result<(), String>),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The reply always arrives, so it can clear its request even when
        // an overlay is up; `OpenUnder` checks the window itself.
        !matches!(self, Self::Saved(..) | Self::OpenUnder(..))
    }
}

/// One `[view]` write and the values written. Each write sets only its own
/// fields, so a window never writes back state it merely read. Picking a
/// key writes its direction with it, in one edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Sort(SortKey, bool),
    Descending(bool),
    FavoritesFirst(bool),
    /// The whole percent the grid's zoom is heading to.
    Zoom(u16),
    PosterOnly(bool),
    Random(RandomPool),
    Layout(ViewLayout),
    Group(GroupBy),
}

/// The value each field's write in flight is writing. One write per field
/// runs at a time; a change made meanwhile is written when it finishes.
#[derive(Debug, Default)]
pub(crate) struct Writes {
    pub(crate) sort: Option<(SortKey, bool)>,
    pub(crate) descending: Option<bool>,
    pub(crate) favorites_first: Option<bool>,
    pub(crate) zoom: Option<u16>,
    pub(crate) poster_only: Option<bool>,
    pub(crate) random: Option<RandomPool>,
    pub(crate) layout: Option<ViewLayout>,
    pub(crate) group: Option<GroupBy>,
    /// Fields whose write in flight a preset overtook. Applying a preset
    /// writes nothing (plan 0062, decision 16), so when the write lands it
    /// doesn't follow up with the preset's value, unless a control changed
    /// the field again meanwhile.
    overtaken: Vec<std::mem::Discriminant<Field>>,
}

impl Writes {
    fn busy(&self, field: Field) -> bool {
        match field {
            Field::Sort(..) => self.sort.is_some(),
            Field::Descending(_) => self.descending.is_some(),
            Field::FavoritesFirst(_) => self.favorites_first.is_some(),
            Field::Zoom(_) => self.zoom.is_some(),
            Field::PosterOnly(_) => self.poster_only.is_some(),
            Field::Random(_) => self.random.is_some(),
            Field::Layout(_) => self.layout.is_some(),
            Field::Group(_) => self.group.is_some(),
        }
    }

    /// Marks each of `fields` with a write in flight as overtaken.
    pub(crate) fn overtake(&mut self, fields: &[Field]) {
        for &field in fields {
            let kind = std::mem::discriminant(&field);
            if self.busy(field) && !self.overtaken.contains(&kind) {
                self.overtaken.push(kind);
            }
        }
    }

    /// Whether `field` was overtaken, which it then stops being.
    fn settle(&mut self, field: Field) -> bool {
        let kind = std::mem::discriminant(&field);
        let overtaken = self.overtaken.contains(&kind);
        self.overtaken.retain(|other| *other != kind);
        overtaken
    }

    fn start(&mut self, field: Field) {
        match field {
            Field::Sort(sort, descending) => self.sort = Some((sort, descending)),
            Field::Descending(value) => self.descending = Some(value),
            Field::FavoritesFirst(value) => self.favorites_first = Some(value),
            Field::Zoom(value) => self.zoom = Some(value),
            Field::PosterOnly(value) => self.poster_only = Some(value),
            Field::Random(value) => self.random = Some(value),
            Field::Layout(value) => self.layout = Some(value),
            Field::Group(value) => self.group = Some(value),
        }
    }

    /// A write is in flight, which closing the window waits for.
    pub(crate) fn pending(&self) -> bool {
        self.sort.is_some()
            || self.descending.is_some()
            || self.favorites_first.is_some()
            || self.zoom.is_some()
            || self.poster_only.is_some()
            || self.random.is_some()
            || self.layout.is_some()
            || self.group.is_some()
    }

    fn finish(&mut self, field: Field) {
        match field {
            Field::Sort(..) => self.sort = None,
            Field::Descending(_) => self.descending = None,
            Field::FavoritesFirst(_) => self.favorites_first = None,
            Field::Zoom(_) => self.zoom = None,
            Field::PosterOnly(_) => self.poster_only = None,
            Field::Random(_) => self.random = None,
            Field::Layout(_) => self.layout = None,
            Field::Group(_) => self.group = None,
        }
    }
}

pub(crate) fn label(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Name => "Name",
        SortKey::LastPlayed => "Last played",
        SortKey::Playtime => "Playtime",
        SortKey::Added => "Date added",
        SortKey::Source => "Source",
        SortKey::Tags => "Tags",
    }
}

/// The order the direction button currently gives, for its tooltip.
pub(crate) fn direction_label(sort: SortKey, descending: bool) -> &'static str {
    match (sort, descending) {
        (SortKey::Name | SortKey::Source | SortKey::Tags, false) => "A to Z",
        (SortKey::Name | SortKey::Source | SortKey::Tags, true) => "Z to A",
        (SortKey::Playtime, true) => "Most played first",
        (SortKey::Playtime, false) => "Least played first",
        (SortKey::LastPlayed, true) => "Most recent first",
        (SortKey::LastPlayed, false) => "Least recent first",
        (SortKey::Added, true) => "Newest first",
        (SortKey::Added, false) => "Oldest first",
    }
}

fn row_id(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Name => "library-sort-name",
        SortKey::LastPlayed => "library-sort-last-played",
        SortKey::Playtime => "library-sort-playtime",
        SortKey::Added => "library-sort-added",
        SortKey::Source => "library-sort-source",
        SortKey::Tags => "library-sort-tags",
    }
}

fn group_row_id(group: GroupBy) -> &'static str {
    match group {
        GroupBy::None => "library-group-none",
        GroupBy::Recent => "library-group-recent",
        GroupBy::Tag => "library-group-tag",
        GroupBy::Source => "library-group-source",
        GroupBy::State => "library-group-state",
    }
}

/// The sort rows and Favorites first, in order.
fn menu_ids() -> Vec<&'static str> {
    SortKey::ALL
        .into_iter()
        .map(row_id)
        .chain([FAVORITES_FIRST_ID])
        .collect()
}

/// Every focusable row of the menu, in order, with Group by's between the
/// sort rows and Favorites first.
fn cycle_ids() -> Vec<&'static str> {
    let mut ids = menu_ids();
    ids.insert(ids.len() - 1, GROUP_ID);
    ids
}

/// The Group by submenu's rows, in order.
fn group_ids() -> Vec<&'static str> {
    crate::sections::GROUPS
        .into_iter()
        .map(group_row_id)
        .collect()
}

/// Writes one field, then rereads the rescan's cache whatever happened.
pub(crate) fn save(paths: &Paths, watch: &rescan::Watch, field: Field) -> Result<(), String> {
    let result = match field {
        Field::Sort(sort, descending) => library::set_order(paths, sort, descending),
        Field::Descending(descending) => library::set_descending(paths, descending),
        Field::FavoritesFirst(first) => library::set_favorites_first(paths, first),
        Field::Zoom(zoom) => library::set_zoom(paths, zoom),
        Field::PosterOnly(poster_only) => library::set_poster_only(paths, poster_only),
        Field::Random(random) => library::set_random(paths, random),
        Field::Layout(layout) => library::set_layout(paths, layout),
        Field::Group(group) => library::set_group(paths, group),
    };
    watch.refresh_library(paths);
    result
        .map(|_| ())
        .map_err(|error| error_text(paths, &error))
}

/// How a failed `library.toml` write reads in a notice.
pub(crate) fn error_text(paths: &Paths, error: &wineo_core::error::Error) -> String {
    match error {
        // Its message already says what to do; the source is the raw
        // parse error, which Checkup shows.
        wineo_core::error::Error::LibraryUnreadable { .. } => error.to_string(),
        _ => paths.contract_text(&wineo_core::error::chain(error)),
    }
}

impl App {
    /// The menu shows only over the Library itself.
    pub(crate) fn sort_menu_shown(&self) -> bool {
        self.sort_menu
            && self.section == crate::sidebar::Section::Library
            && !self.panel_hidden()
            && self.hook_stop_confirm.is_none()
    }

    /// The window's current value of `field`'s field.
    fn current(&self, field: Field) -> Field {
        match field {
            Field::Sort(..) => Field::Sort(self.query.sort, self.query.descending),
            Field::Descending(_) => Field::Descending(self.query.descending),
            Field::FavoritesFirst(_) => Field::FavoritesFirst(self.query.favorites_first),
            Field::Zoom(_) => Field::Zoom(self.zoom),
            Field::PosterOnly(_) => Field::PosterOnly(self.poster_only),
            Field::Random(_) => Field::Random(self.random_pool),
            Field::Layout(_) => Field::Layout(self.layout),
            Field::Group(_) => Field::Group(self.group),
        }
    }

    /// Writes the window's value of `field` unless a write of it is in
    /// flight, which writes again when it finishes if the value moved on.
    pub(crate) fn save_field(&mut self, field: Field) -> Task<Message> {
        if self.view_writes.busy(field) {
            // A control changed it again, so the follow-up writes it.
            self.view_writes.settle(field);
            return Task::none();
        }
        let field = self.current(field);
        self.view_writes.start(field);
        let paths = self.paths.clone();
        let watch = self.shown.clone();
        crate::task::blocking_value(
            move || save(&paths, &watch, field),
            move |result| Message::Sort(Event::Saved(field, result.and_then(|saved| saved))),
        )
    }

    fn sort_changed(&mut self, field: Field) -> Task<Message> {
        let resorted = self.resorted();
        Task::batch([resorted, self.save_field(field)])
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::ToggleMenu => {
            // A second press while the first waits for the button, or on
            // an open menu, closes it like any toggle.
            if app.sort_menu || app.sort_opening.pending.is_some() {
                app.sort_opening.cancel();
                app.sort_menu = false;
                return Task::none();
            }
            // The menu drops from the button, so it waits for where
            // the button is.
            crate::library_menu::request_bounds(
                &mut app.sort_opening,
                SORT_BUTTON_ID,
                |request, bounds| Message::Sort(Event::OpenUnder(request, bounds)),
            )
        }
        Event::OpenUnder(request, bounds) => {
            // Whatever took the window since the press keeps it.
            if !app.sort_opening.answer(request) || !app.library_menu_may_open() {
                return Task::none();
            }
            match bounds {
                Some(bounds) => open_menu(app, anchor_under(bounds)),
                None => Task::none(),
            }
        }
        Event::CloseMenu => {
            app.sort_opening.cancel();
            app.sort_menu = false;
            Task::none()
        }
        Event::Pick(sort) => {
            app.sort_menu = false;
            // The picked row is gone, so focus goes back where the menu
            // came from.
            let focus = iced::widget::operation::focus(iced::widget::Id::new(SORT_BUTTON_ID));
            if sort == app.query.sort {
                return focus;
            }
            // Each key starts in its own order, so Name to Last played
            // doesn't give the oldest first.
            app.query.sort = sort;
            app.query.descending = sort.starts_descending();
            Task::batch([
                focus,
                app.sort_changed(Field::Sort(sort, app.query.descending)),
            ])
        }
        Event::Header(sort) if sort == app.query.sort => {
            app.query.descending = !app.query.descending;
            app.sort_changed(Field::Descending(app.query.descending))
        }
        Event::Header(sort) => {
            app.sort_menu = false;
            app.query.sort = sort;
            app.query.descending = sort.starts_descending();
            app.sort_changed(Field::Sort(sort, app.query.descending))
        }
        Event::FlipDirection => {
            app.query.descending = !app.query.descending;
            app.sort_changed(Field::Descending(app.query.descending))
        }
        Event::ToggleFavoritesFirst => {
            app.group_menu = false;
            app.query.favorites_first = !app.query.favorites_first;
            app.sort_changed(Field::FavoritesFirst(app.query.favorites_first))
        }
        Event::OpenGroups => {
            if !app.sort_menu_shown() {
                return Task::none();
            }
            // A second press closes it, like the menu's own button.
            app.group_menu = !app.group_menu;
            if app.group_menu {
                iced::widget::operation::focus(iced::widget::Id::new(group_row_id(app.group)))
            } else {
                iced::widget::operation::focus(iced::widget::Id::new(GROUP_ID))
            }
        }
        Event::Group(group) => {
            app.sort_menu = false;
            app.group_menu = false;
            let focus = iced::widget::operation::focus(iced::widget::Id::new(SORT_BUTTON_ID));
            if group == app.group {
                return focus;
            }
            app.group = group;
            Task::batch([focus, app.sort_changed(Field::Group(group))])
        }
        Event::Saved(written, result) => {
            app.view_writes.finish(written);
            let overtaken = app.view_writes.settle(written);
            if app.exiting {
                return Task::none();
            }
            if let Err(error) = result {
                // The window keeps the view it chose.
                let what = match written {
                    Field::Zoom(_)
                    | Field::PosterOnly(_)
                    | Field::Random(_)
                    | Field::Layout(_)
                    | Field::Group(_) => "view",
                    _ => "sort",
                };
                app.feature_notice = Some((
                    format!("Couldn't save the {what}: {error}"),
                    std::time::Instant::now() + NOTICE_TIME,
                ));
            }
            // A drag saves only where it lets go, so the values it passes
            // through wait for its release.
            if app.current(written) == written
                || overtaken
                || (matches!(written, Field::Zoom(_)) && app.zooming.dragging())
            {
                Task::none()
            } else {
                app.save_field(written)
            }
        }
    }
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.close_library_menus();
    app.sort_menu = true;
    app.group_menu = false;
    app.sort_menu_position = position;
    // Focus moves into the menu, so a second Enter or Space picks a row
    // instead of closing it again.
    crate::library_menu::focus(row_id(app.query.sort), MENU_SCROLL_ID)
}

/// Keys while the menu is open: Escape closes it, Tab and the up and down
/// arrows move through its rows, and the rows take Enter and Space
/// themselves. Nothing else reaches the grid behind it.
pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    // The submenu has the keys while it's open: Escape and Left go back to
    // its row, and the rest stay among its own rows.
    if app.group_menu {
        return match key {
            Key::Named(Named::Escape | Named::ArrowLeft) => {
                app.group_menu = false;
                iced::widget::operation::focus(iced::widget::Id::new(GROUP_ID))
            }
            Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
                let reverse = *key == Key::Named(Named::ArrowUp)
                    || (*key == Key::Named(Named::Tab) && modifiers.shift());
                iced::advanced::widget::operate(focus_button::cycle(&group_ids(), reverse))
                    .discard()
            }
            _ => Task::none(),
        };
    }
    match key {
        Key::Named(Named::Escape) => {
            app.sort_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(SORT_BUTTON_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            let reverse = *key == Key::Named(Named::ArrowUp)
                || (*key == Key::Named(Named::Tab) && modifiers.shift());
            crate::library_menu::cycle(&cycle_ids(), reverse, MENU_SCROLL_ID)
        }
        _ => Task::none(),
    }
}

/// The Sort button and the direction button, for the toolbar beside the
/// search box.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    let sort = button(
        row![
            text(format!("Sort: {}", label(app.query.sort))).size(14),
            icon(CHEVRON, palette.muted, 16.0),
        ]
        .spacing(6)
        .height(Fill)
        .align_y(Center),
    )
    .height(ui::BUTTON_HEIGHT)
    .padding([0, 12])
    .on_press(Message::Sort(Event::ToggleMenu))
    .style(move |_, status| ui::secondary_style(palette, status));
    let arrow = if app.query.descending {
        ARROW_DOWN
    } else {
        ARROW_UP
    };
    let flip = button(container(icon(arrow, palette.foreground, 16.0)).center(Fill))
        .width(ui::BUTTON_HEIGHT)
        .height(ui::BUTTON_HEIGHT)
        .padding(0)
        .on_press(Message::Sort(Event::FlipDirection))
        .style(move |_, status| ui::secondary_style(palette, status));
    let flip = tooltip(
        flip,
        container(text(direction_label(app.query.sort, app.query.descending)).size(13))
            .padding([4, 8]),
        tooltip::Position::Bottom,
    )
    .style(move |_| {
        container::Style::default()
            .background(palette.surface)
            .border(border::rounded(8).width(1).color(palette.border))
    })
    .gap(6);
    row![
        focus_button::control(
            sort.into(),
            true,
            SORT_BUTTON_ID,
            Message::Sort(Event::ToggleMenu)
        ),
        focus_button::control(
            flip.into(),
            true,
            DIRECTION_BUTTON_ID,
            Message::Sort(Event::FlipDirection),
        ),
    ]
    .spacing(8)
    .align_y(Center)
    .into()
}

/// The sort rows, Group by, and Favorites first.
const MENU_ROWS: usize = SortKey::ALL.len() + 2;

fn menu_height(window_height: f32) -> f32 {
    let items = 1 + MENU_ROWS + 1;
    let content = ACTION_MENU_HEADER_HEIGHT
        + MENU_ROWS as f32 * ui::BUTTON_HEIGHT
        + ACTION_MENU_DIVIDER
        + (items - 1) as f32 * ACTION_MENU_SPACING;
    (content + 2.0 * ACTION_MENU_PADDING).min((window_height - 2.0 * ACTION_MENU_MARGIN).max(0.0))
}

impl App {
    /// The open Sort menu over the page, placed like the action menu.
    pub(crate) fn sort_menu_view(&self) -> [Element<'_, Message>; 2] {
        let palette = self.palette;
        let height = menu_height(self.window_size.height);
        let position = menu_position(self.sort_menu_position, self.window_size, height);
        let mut rows = column![
            text("Sort by")
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
        for sort in SortKey::ALL {
            rows = rows.push(menu_row(
                text(label(sort)).size(14).into(),
                palette,
                sort == self.query.sort,
                row_id(sort),
                Message::Sort(Event::Pick(sort)),
            ));
        }
        rows = rows
            .push(ui::separator(palette).padding([3, 0]))
            .push(menu_row(
                row![
                    text("Group by").size(14).width(Fill),
                    text(crate::sections::label(self.group))
                        .size(14)
                        .color(palette.muted),
                    icon(crate::sections::CHEVRON_RIGHT, palette.muted, 16.0),
                ]
                .spacing(6)
                .align_y(Center)
                .into(),
                palette,
                self.group_menu,
                GROUP_ID,
                Message::Sort(Event::OpenGroups),
            ))
            .push(menu_row(
                row![
                    check_box(self.query.favorites_first, palette),
                    text("Favorites first").size(14),
                ]
                .spacing(8)
                .align_y(Center)
                .into(),
                palette,
                false,
                FAVORITES_FIRST_ID,
                Message::Sort(Event::ToggleFavoritesFirst),
            ));
        let menu = container(
            scrollable(rows)
                .id(iced::widget::Id::new(MENU_SCROLL_ID))
                .wineo_style(palette)
                .height(Fill),
        )
        .padding(ACTION_MENU_PADDING)
        .width(ACTION_MENU_WIDTH)
        .height(height)
        .style(move |_| crate::library_menu::surface_style(palette));
        crate::library_menu::layers(menu.into(), position, Message::Sort(Event::CloseMenu))
    }

    /// The open Group by submenu, beside its row: on the Sort menu's right,
    /// or its left when the window has no room there, kept inside it.
    pub(crate) fn group_menu_view(&self) -> Option<Element<'_, Message>> {
        if !self.group_menu {
            return None;
        }
        let palette = self.palette;
        let height = 2.0 * ACTION_MENU_PADDING
            + crate::sections::GROUPS.len() as f32 * ui::BUTTON_HEIGHT
            + (crate::sections::GROUPS.len() - 1) as f32 * ACTION_MENU_SPACING;
        let menu = menu_position(
            self.sort_menu_position,
            self.window_size,
            menu_height(self.window_size.height),
        );
        // The submenu's top, which lines its first row up with Group by.
        // Both menus have the same padding, so what's left above the row is
        // the heading, the sort rows, and the separator.
        let top = menu.y
            + ACTION_MENU_HEADER_HEIGHT
            + SortKey::ALL.len() as f32 * (ui::BUTTON_HEIGHT + ACTION_MENU_SPACING)
            + ACTION_MENU_SPACING
            + ACTION_MENU_DIVIDER
            + ACTION_MENU_SPACING;
        let right = menu.x + ACTION_MENU_WIDTH + GROUP_MENU_GAP;
        let x = if right + GROUP_MENU_WIDTH + ACTION_MENU_MARGIN <= self.window_size.width {
            right
        } else {
            (menu.x - GROUP_MENU_GAP - GROUP_MENU_WIDTH).max(ACTION_MENU_MARGIN)
        };
        let y = top.clamp(
            ACTION_MENU_MARGIN,
            (self.window_size.height - height - ACTION_MENU_MARGIN).max(ACTION_MENU_MARGIN),
        );
        let rows = crate::sections::GROUPS.into_iter().map(|group| {
            menu_row(
                text(crate::sections::label(group)).size(14).into(),
                palette,
                group == self.group,
                group_row_id(group),
                Message::Sort(Event::Group(group)),
            )
        });
        let submenu = container(column(rows).spacing(ACTION_MENU_SPACING))
            .padding(ACTION_MENU_PADDING)
            .width(GROUP_MENU_WIDTH)
            .height(height)
            .style(move |_| crate::library_menu::surface_style(palette));
        Some(
            container(submenu)
                .padding(iced::Padding {
                    top: y,
                    left: x,
                    right: 0.0,
                    bottom: 0.0,
                })
                .width(Fill)
                .height(Fill)
                .into(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests;
