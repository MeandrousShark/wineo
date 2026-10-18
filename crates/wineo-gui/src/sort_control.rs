//! The library's sort control: the Sort menu with its keys and "Favorites
//! first", the direction button, and saving each change to `library.toml`.

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{
    Space, button, column, container, mouse_area, row, scrollable, svg, text, tooltip,
};
use iced::{Center, Color, Element, Fill, Font, Point, Rectangle, Task, border, font};
use wineo_core::library::{self, SortKey};
use wineo_core::paths::Paths;

use crate::grid::{
    ACTION_MENU_HEADER_HEIGHT, ACTION_MENU_MARGIN, ACTION_MENU_PADDING, ACTION_MENU_SPACING,
    ACTION_MENU_WIDTH, context_menu_style, menu_position,
};
use crate::theme::Palette;
use crate::ui::{self, ScrollStyle};
use crate::{App, Message, focus_button, rescan};

pub(crate) const SORT_BUTTON_ID: &str = "library-sort";
pub(crate) const DIRECTION_BUTTON_ID: &str = "library-sort-direction";
const FAVORITES_FIRST_ID: &str = "library-sort-favorites-first";
const NOTICE_TIME: std::time::Duration = std::time::Duration::from_secs(8);

const CHEVRON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 6l4 4 4-4" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const ARROW_DOWN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 2.5v11M3.5 9l4.5 4.5L12.5 9" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const ARROW_UP: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 13.5v-11M3.5 7l4.5-4.5L12.5 7" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CHECK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M3.5 8.5l3 3 6-6.5" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click on the Sort button, which opens the menu at the cursor.
    ToggleMenu,
    /// Enter or Space on the focused Sort button.
    ToggleMenuFromKeyboard,
    /// The Sort button's bounds, found for the keyboard's open request with
    /// this number.
    OpenUnder(u64, Option<Rectangle>),
    /// A click outside the menu.
    CloseMenu,
    Pick(SortKey),
    FlipDirection,
    ToggleFavoritesFirst,
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

/// One `[view]` field and the value written to it. Each write sets only its
/// own field, so a window never writes back state it merely read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Sort(SortKey),
    Descending(bool),
    FavoritesFirst(bool),
}

/// The value each field's write in flight is writing. One write per field
/// runs at a time; a change made meanwhile is written when it finishes.
#[derive(Debug, Default)]
pub(crate) struct Writes {
    pub(crate) sort: Option<SortKey>,
    pub(crate) descending: Option<bool>,
    pub(crate) favorites_first: Option<bool>,
}

impl Writes {
    fn busy(&self, field: Field) -> bool {
        match field {
            Field::Sort(_) => self.sort.is_some(),
            Field::Descending(_) => self.descending.is_some(),
            Field::FavoritesFirst(_) => self.favorites_first.is_some(),
        }
    }

    fn start(&mut self, field: Field) {
        match field {
            Field::Sort(value) => self.sort = Some(value),
            Field::Descending(value) => self.descending = Some(value),
            Field::FavoritesFirst(value) => self.favorites_first = Some(value),
        }
    }

    /// A write is in flight, which closing the window waits for.
    pub(crate) fn pending(&self) -> bool {
        self.sort.is_some() || self.descending.is_some() || self.favorites_first.is_some()
    }

    fn finish(&mut self, field: Field) {
        match field {
            Field::Sort(_) => self.sort = None,
            Field::Descending(_) => self.descending = None,
            Field::FavoritesFirst(_) => self.favorites_first = None,
        }
    }
}

/// The keyboard's request to open the menu while it looks up the button.
#[derive(Debug, Default)]
pub(crate) struct Opening {
    last: u64,
    pending: Option<u64>,
}

impl Opening {
    /// Drops the request in flight, so its reply is ignored.
    pub(crate) fn cancel(&mut self) {
        self.pending = None;
    }
}

pub(crate) fn label(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Name => "Name",
        SortKey::LastPlayed => "Last played",
        SortKey::Playtime => "Playtime",
        SortKey::Added => "Date added",
        SortKey::Source => "Source",
    }
}

/// The order the direction button currently gives, for its tooltip.
pub(crate) fn direction_label(sort: SortKey, descending: bool) -> &'static str {
    match (sort, descending) {
        (SortKey::Name, false) => "A to Z",
        (SortKey::Name, true) => "Z to A",
        (SortKey::Source, false) => "Sources A to Z",
        (SortKey::Source, true) => "Sources Z to A",
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
    }
}

/// The menu's focusable rows, in order.
fn menu_ids() -> Vec<&'static str> {
    SortKey::ALL
        .into_iter()
        .map(row_id)
        .chain([FAVORITES_FIRST_ID])
        .collect()
}

/// Writes one field, then rereads the rescan's cache whatever happened.
pub(crate) fn save(paths: &Paths, watch: &rescan::Watch, field: Field) -> Result<(), String> {
    let result = match field {
        Field::Sort(sort) => library::set_sort(paths, sort),
        Field::Descending(descending) => library::set_descending(paths, descending),
        Field::FavoritesFirst(first) => library::set_favorites_first(paths, first),
    };
    watch.refresh_library(paths);
    result.map(|_| ()).map_err(|error| error.to_string())
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
            Field::Sort(_) => Field::Sort(self.query.sort),
            Field::Descending(_) => Field::Descending(self.query.descending),
            Field::FavoritesFirst(_) => Field::FavoritesFirst(self.query.favorites_first),
        }
    }

    /// Writes the window's value of `field` unless a write of it is in
    /// flight, which writes again when it finishes if the value moved on.
    fn save_field(&mut self, field: Field) -> Task<Message> {
        if self.view_writes.busy(field) {
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
            app.sort_opening.cancel();
            if app.sort_menu {
                app.sort_menu = false;
                return Task::none();
            }
            open_menu(app, app.menu_anchor)
        }
        Event::ToggleMenuFromKeyboard => {
            // A second press while the first waits for the button, or on
            // an open menu, closes it like any toggle.
            if app.sort_menu || app.sort_opening.pending.is_some() {
                app.sort_opening.cancel();
                app.sort_menu = false;
                return Task::none();
            }
            // The pointer may be anywhere, so the menu waits for where
            // the button is.
            app.sort_opening.last += 1;
            let request = app.sort_opening.last;
            app.sort_opening.pending = Some(request);
            iced::advanced::widget::operate(bounds_of(SORT_BUTTON_ID))
                .map(move |bounds| Message::Sort(Event::OpenUnder(request, bounds)))
        }
        Event::OpenUnder(request, bounds) => {
            if app.sort_opening.pending != Some(request) {
                return Task::none();
            }
            app.sort_opening.cancel();
            // Whatever took the window since the press keeps it.
            if app.exiting
                || app.closing
                || app.sort_menu
                || app.action_menu.is_some()
                || app.hook_stop_confirm.is_some()
                || app.section != crate::sidebar::Section::Library
                || app.panel_hidden()
            {
                return Task::none();
            }
            open_menu(app, bounds.map_or(app.menu_anchor, menu_anchor_under))
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
            app.query.sort = sort;
            Task::batch([focus, app.sort_changed(Field::Sort(sort))])
        }
        Event::FlipDirection => {
            app.query.descending = !app.query.descending;
            app.sort_changed(Field::Descending(app.query.descending))
        }
        Event::ToggleFavoritesFirst => {
            app.query.favorites_first = !app.query.favorites_first;
            app.sort_changed(Field::FavoritesFirst(app.query.favorites_first))
        }
        Event::Saved(written, result) => {
            app.view_writes.finish(written);
            if app.exiting {
                return Task::none();
            }
            if let Err(error) = result {
                // The window keeps the view it chose.
                app.feature_notice = Some((
                    format!("Couldn't save the sort: {error}"),
                    std::time::Instant::now() + NOTICE_TIME,
                ));
            }
            if app.current(written) == written {
                Task::none()
            } else {
                app.save_field(written)
            }
        }
    }
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.action_menu = None;
    app.sort_menu = true;
    app.sort_menu_position = position;
    // Focus moves into the menu, so a second Enter or Space picks a row
    // instead of closing it again.
    iced::widget::operation::focus(iced::widget::Id::new(row_id(app.query.sort)))
}

/// The pointer position that puts the menu a small gap under the button's
/// left edge. `menu_position` offsets a pointer by 4 px each way.
fn menu_anchor_under(button: Rectangle) -> Point {
    Point::new(button.x - 4.0, button.y + button.height)
}

/// Finds the bounds of the focus control with `id`, if it is on screen.
fn bounds_of(id: &'static str) -> impl iced::advanced::widget::Operation<Option<Rectangle>> {
    struct Bounds {
        id: iced::widget::Id,
        found: Option<Rectangle>,
    }

    impl iced::advanced::widget::Operation<Option<Rectangle>> for Bounds {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<Option<Rectangle>>),
        ) {
            operate(self);
        }

        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if id == Some(&self.id) {
                self.found = Some(bounds);
            }
        }

        fn finish(&self) -> iced::advanced::widget::operation::Outcome<Option<Rectangle>> {
            iced::advanced::widget::operation::Outcome::Some(self.found)
        }
    }

    Bounds {
        id: iced::widget::Id::new(id),
        found: None,
    }
}

/// Keys while the menu is open: Escape closes it, Tab and the up and down
/// arrows move through its rows, and the rows take Enter and Space
/// themselves. Nothing else reaches the grid behind it.
pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => {
            app.sort_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(SORT_BUTTON_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            let reverse = *key == Key::Named(Named::ArrowUp)
                || (*key == Key::Named(Named::Tab) && modifiers.shift());
            iced::advanced::widget::operate(focus_button::cycle(&menu_ids(), reverse)).discard()
        }
        _ => Task::none(),
    }
}

fn icon(bytes: &'static [u8], color: Color, size: f32) -> svg::Svg<'static> {
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
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
            Message::Sort(Event::ToggleMenuFromKeyboard)
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

const MENU_ROWS: usize = SortKey::ALL.len() + 1;

fn menu_height(window_height: f32) -> f32 {
    let items = 1 + MENU_ROWS + 1;
    let content = ACTION_MENU_HEADER_HEIGHT
        + MENU_ROWS as f32 * ui::BUTTON_HEIGHT
        + 7.0
        + (items - 1) as f32 * ACTION_MENU_SPACING;
    (content + 2.0 * ACTION_MENU_PADDING).min((window_height - 2.0 * ACTION_MENU_MARGIN).max(0.0))
}

fn menu_row<'a>(
    content: Element<'a, Message>,
    palette: Palette,
    chosen: bool,
    id: &'static str,
    message: Message,
) -> Element<'a, Message> {
    let row = button(container(content).height(Fill).center_y(Fill))
        .width(Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press(message.clone())
        .style(move |_, status| {
            let mut style = context_menu_style(palette, false, status);
            if chosen && status == button::Status::Active {
                style.background = Some(palette.primary.scale_alpha(0.12).into());
            }
            style
        });
    focus_button::control(row.into(), true, id, message)
}

/// A box that shows whether "Favorites first" is on.
fn check_box(on: bool, palette: Palette) -> Element<'static, Message> {
    let mark: Element<'static, Message> = if on {
        icon(CHECK, ui::accent_ink(palette.primary), 12.0).into()
    } else {
        Space::new().into()
    };
    container(mark)
        .width(16)
        .height(16)
        .center(16)
        .style(move |_| {
            let style =
                container::Style::default().border(border::rounded(4).width(1).color(if on {
                    palette.primary
                } else {
                    palette.muted
                }));
            if on {
                style.background(palette.primary)
            } else {
                style
            }
        })
        .into()
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
        let menu = container(scrollable(rows).wineo_style(palette).height(Fill))
            .padding(ACTION_MENU_PADDING)
            .width(ACTION_MENU_WIDTH)
            .height(height)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(
                        border::rounded(ui::SECTION_RADIUS)
                            .width(1)
                            .color(palette.border),
                    )
                    .shadow(iced::Shadow {
                        color: Color::BLACK.scale_alpha(0.2),
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 14.0,
                    })
            });
        [
            mouse_area(Space::new().width(Fill).height(Fill))
                .on_press(Message::Sort(Event::CloseMenu))
                .on_right_press(Message::Sort(Event::CloseMenu))
                .into(),
            container(menu)
                .padding(iced::Padding {
                    top: position.y,
                    left: position.x,
                    right: 0.0,
                    bottom: 0.0,
                })
                .width(Fill)
                .height(Fill)
                .into(),
        ]
    }
}

#[cfg(test)]
mod tests;
