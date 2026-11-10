//! The toolbar's More button and its menu, which hold the controls a narrow
//! toolbar folds away. Each row sends what its toolbar control sends.

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Column, button, container, row, text};
use iced::{Center, Element, Fill, Point, Rectangle, Task};

use crate::grid::{ACTION_MENU_DIVIDER, ACTION_MENU_PADDING, ACTION_MENU_SPACING, menu_position};
use crate::grid_motion::ViewLayout;
use crate::library_menu::{icon, menu_row};
use crate::toolbar_squeeze::{Control, Fit, Folds};
use crate::{App, Message, focus_button, ui, zoom_control};

pub(crate) const MORE_ID: &str = "library-more";
pub(crate) const FILTERS_ID: &str = "library-more-filters";
pub(crate) const RANDOM_ID: &str = "library-more-random";
pub(crate) const LAYOUT_ID: &str = "library-more-layout";
pub(crate) const ZOOM_ID: &str = "library-more-zoom";
pub(crate) const TITLES_ID: &str = "library-more-titles";
pub(crate) const LABEL: &str = "More";
pub(crate) const WIDTH: f32 = crate::grid::ACTION_MENU_WIDTH;

/// Three dots in a row.
const DOTS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="3" cy="8" r="1.4" fill="#000"/><circle cx="8" cy="8" r="1.4" fill="#000"/><circle cx="13" cy="8" r="1.4" fill="#000"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click, Enter, or Space on More, which opens the menu under it once
    /// its bounds are found.
    ToggleMenu,
    /// More's bounds, found for the open request with this number.
    OpenUnder(u64, Option<Rectangle>),
    /// A click outside the menu.
    CloseMenu,
    /// The toolbar's laid out folds and search, when they change.
    Fitted(Fit),
    /// The Filters row, which opens the Filters popup under More.
    OpenFilters,
    /// The Pick one at random row.
    Pick,
    /// The Menu key or Shift+F10 on that row, for the dice's options.
    RandomOptions,
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The reply always arrives so it can clear its request, and the
        // folds follow the layout whatever is open.
        !matches!(self, Self::OpenUnder(..) | Self::Fitted(_))
    }
}

/// The toolbar focus control a folded control has there.
fn toolbar_id(control: Control) -> &'static str {
    match control {
        Control::Filters => crate::filter_control::FILTER_BUTTON_ID,
        Control::Dice => crate::random_pick::BUTTON_ID,
        Control::Layout => crate::layout_switch::ID,
        Control::Zoom => zoom_control::SLIDER_ID,
        Control::Titles => zoom_control::POSTER_ONLY_ID,
    }
}

/// The point that puts a menu `width` wide a small gap under `button`,
/// right edges aligned. `menu_position` offsets the point by 4 px each way.
pub(crate) fn anchor_under_right(button: Rectangle, width: f32) -> Point {
    Point::new(
        button.x + button.width - width - 4.0,
        button.y + button.height,
    )
}

impl App {
    /// The menu shows only over the Library itself.
    pub(crate) fn more_menu_shown(&self) -> bool {
        self.more_menu
            && self.section == crate::sidebar::Section::Library
            && !self.panel_hidden()
            && self.hook_stop_confirm.is_none()
    }

    /// The More menu is up and takes its rows' input, which the menu guard
    /// on the layout switch and zoom would otherwise refuse.
    pub(crate) fn more_menu_input(&self) -> bool {
        self.more_menu_shown() && !self.exiting && !self.closing
    }

    /// Zoom input the More menu's slider and Hide titles row send. The
    /// zoom keys stay off while it's open, like under any other menu.
    pub(crate) fn more_menu_zoom(&self, event: &crate::zoom::Event) -> bool {
        use crate::zoom::Event as Zoom;
        self.more_menu_input()
            && self.layout == ViewLayout::Grid
            && matches!(
                event,
                Zoom::Step(_)
                    | Zoom::Drag(_)
                    | Zoom::DragEnded
                    | Zoom::Wheel(_)
                    | Zoom::TogglePosterOnly
            )
    }

    /// Where focus goes back to from the Filters popup: its button, or
    /// More while it's folded.
    pub(crate) fn filters_home(&self) -> &'static str {
        self.home(Control::Filters)
    }

    /// Where focus goes back to from the dice's menu.
    pub(crate) fn dice_home(&self) -> &'static str {
        self.home(Control::Dice)
    }

    fn home(&self, control: Control) -> &'static str {
        if self.toolbar_folds.contains(control) {
            MORE_ID
        } else {
            toolbar_id(control)
        }
    }

    /// The menu's rows, in toolbar order: what's folded, or while it's
    /// open what was folded since it opened, so a row in use never leaves
    /// under the pointer or the keys. Zoom and Hide titles show only in the
    /// grid, as on the toolbar.
    fn more_controls(&self) -> impl Iterator<Item = Control> + '_ {
        let rows = if self.more_menu {
            self.more_rows
        } else {
            self.toolbar_folds
        };
        rows.iter().filter(|control| {
            self.layout == ViewLayout::Grid || !matches!(control, Control::Zoom | Control::Titles)
        })
    }

    /// The menu's focusable rows, in order. A dice row with nothing to pick
    /// from is off, like the dice.
    pub(crate) fn more_ids(&self) -> Vec<&'static str> {
        self.more_controls()
            .filter_map(|control| match control {
                Control::Filters => Some(FILTERS_ID),
                Control::Dice => self.random_enabled().then_some(RANDOM_ID),
                Control::Layout => Some(LAYOUT_ID),
                Control::Zoom => Some(ZOOM_ID),
                Control::Titles => Some(TITLES_ID),
            })
            .collect()
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::ToggleMenu => {
            if app.more_menu || app.more_opening.pending.is_some() {
                app.more_opening.cancel();
                app.more_menu = false;
                return Task::none();
            }
            crate::library_menu::request_bounds(
                &mut app.more_opening,
                MORE_ID,
                |request, bounds| Message::More(Event::OpenUnder(request, bounds)),
            )
        }
        Event::OpenUnder(request, bounds) => {
            if !app.more_opening.answer(request) || !app.library_menu_may_open() {
                return Task::none();
            }
            match bounds {
                Some(bounds) => open_menu(app, anchor_under_right(bounds, WIDTH)),
                None => Task::none(),
            }
        }
        Event::CloseMenu => {
            app.more_opening.cancel();
            app.more_menu = false;
            Task::none()
        }
        Event::Fitted(fit) => {
            app.search_icon = fit.search_icon;
            // The field is back in the toolbar, with its text and focus.
            if !fit.search_icon {
                app.search_open = false;
            }
            folded(app, fit.folds)
        }
        Event::OpenFilters => {
            if !app.more_menu_shown() {
                return Task::none();
            }
            app.more_menu = false;
            crate::filter_control::update(app, crate::filter_control::Event::ToggleMenu)
        }
        Event::Pick => {
            if !app.more_menu_shown() {
                return Task::none();
            }
            app.more_menu = false;
            // The row is gone, so focus goes back to More, as the dice
            // keeps it after its own pick. The pick goes through the app's
            // update, so whatever blocks the dice blocks the row too.
            let focus = iced::widget::operation::focus(iced::widget::Id::new(MORE_ID));
            Task::batch([
                focus,
                app.update(Message::Random(crate::random_pick::Event::Pick)),
            ])
        }
        Event::RandomOptions => {
            if !app.more_menu_shown() {
                return Task::none();
            }
            app.more_menu = false;
            app.update(Message::Random(crate::random_pick::Event::OpenMenu))
        }
    }
}

/// The toolbar laid out with new folds. An open menu gains the rows of
/// controls that just folded and keeps the rest; once nothing is folded
/// More is gone, so the menu closes and focus goes to the first of its
/// controls back on the toolbar.
fn folded(app: &mut App, folds: Folds) -> Task<Message> {
    app.toolbar_folds = folds;
    if !app.more_menu {
        return Task::none();
    }
    for control in folds.iter() {
        app.more_rows = app.more_rows.with(control);
    }
    if !folds.is_empty() {
        return Task::none();
    }
    app.more_menu = false;
    match app.more_rows.iter().next() {
        Some(control) => iced::widget::operation::focus(iced::widget::Id::new(toolbar_id(control))),
        None => Task::none(),
    }
}

fn open_menu(app: &mut App, position: Point) -> Task<Message> {
    app.close_library_menus();
    app.more_menu = true;
    app.more_rows = app.toolbar_folds;
    app.more_menu_position = position;
    // Focus moves into the menu, so a second Enter or Space acts on a row
    // instead of closing it again.
    match app.more_ids().first() {
        Some(first) => iced::widget::operation::focus(iced::widget::Id::new(first)),
        None => Task::none(),
    }
}

/// Keys while the menu is open: Escape closes it and gives More its focus
/// back, Tab and the up and down arrows move through its rows, and the rows
/// take Enter, Space, Left, and Right themselves. Nothing else reaches the
/// grid behind it.
pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => {
            app.more_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(MORE_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            let reverse = *key == Key::Named(Named::ArrowUp)
                || (*key == Key::Named(Named::Tab) && modifiers.shift());
            iced::advanced::widget::operate(focus_button::cycle(&app.more_ids(), reverse)).discard()
        }
        _ => Task::none(),
    }
}

/// The More button, at the end of the toolbar while anything is folded. It
/// shows the selected look while its menu is open.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    let open = app.more_menu;
    let more = button(container(icon(DOTS, palette.foreground, 16.0)).center(Fill))
        .width(ui::BUTTON_HEIGHT)
        .height(ui::BUTTON_HEIGHT)
        .padding(0)
        .on_press(Message::More(Event::ToggleMenu))
        .style(move |_, status| {
            let mut style = ui::secondary_style(palette, status);
            if open {
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
        zoom_control::with_tooltip(more.into(), LABEL, palette),
        true,
        MORE_ID,
        Message::More(Event::ToggleMenu),
    )
}

/// A row that isn't a button: its label, with its control at the right.
fn control_row<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    container(
        row![text(label).size(14).width(Fill), control]
            .spacing(8)
            .align_y(Center),
    )
    .height(ui::BUTTON_HEIGHT)
    .center_y(ui::BUTTON_HEIGHT)
    .padding([0, 10])
    .into()
}

impl App {
    /// The open More menu over the page, under More with right edges
    /// aligned.
    pub(crate) fn more_menu_view(&self) -> [Element<'_, Message>; 2] {
        let palette = self.palette;
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        let mut dividers = 0;
        for control in self.more_controls() {
            let item = match control {
                Control::Filters => menu_row(
                    row![
                        text(crate::filter_control::button_label(self))
                            .size(14)
                            .width(Fill),
                        icon(crate::sections::CHEVRON_RIGHT, palette.muted, 16.0),
                    ]
                    .spacing(6)
                    .align_y(Center)
                    .into(),
                    palette,
                    false,
                    FILTERS_ID,
                    Message::More(Event::OpenFilters),
                ),
                Control::Dice => self.random_row(),
                Control::Layout => {
                    control_row("Layout", crate::layout_switch::segment(self, LAYOUT_ID))
                }
                Control::Zoom => control_row("Zoom", zoom_control::slider(self, ZOOM_ID)),
                Control::Titles => {
                    if !rows.is_empty() {
                        rows.push(ui::separator(palette).into());
                        dividers += 1;
                    }
                    menu_row(
                        row![
                            zoom_control::titles_icon(self.poster_only, palette),
                            text(zoom_control::toggle_label(self.poster_only)).size(14),
                        ]
                        .spacing(8)
                        .align_y(Center)
                        .into(),
                        palette,
                        false,
                        TITLES_ID,
                        Message::Zoom(crate::zoom::Event::TogglePosterOnly),
                    )
                }
            };
            rows.push(item);
        }
        let count = rows.len();
        let buttons = count - dividers;
        let height = 2.0 * ACTION_MENU_PADDING
            + buttons as f32 * ui::BUTTON_HEIGHT
            + dividers as f32 * ACTION_MENU_DIVIDER
            + count.saturating_sub(1) as f32 * ACTION_MENU_SPACING;
        let position = menu_position(self.more_menu_position, self.window_size, height);
        let menu = container(Column::with_children(rows).spacing(ACTION_MENU_SPACING))
            .padding(ACTION_MENU_PADDING)
            .width(WIDTH)
            .height(height)
            .style(move |_| crate::library_menu::surface_style(palette));
        crate::library_menu::layers(menu.into(), position, Message::More(Event::CloseMenu))
    }

    /// The Pick one at random row: the die, muted with nothing to pick
    /// from, and the dice's options on the Menu key or Shift+F10.
    fn random_row(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let enabled = self.random_enabled();
        let ink = if enabled {
            palette.foreground
        } else {
            palette.muted
        };
        let message = Message::More(Event::Pick);
        let row = button(
            container(
                row![
                    icon(crate::random_pick::DICE, ink, 16.0),
                    text("Pick one at random").size(14).color(ink),
                ]
                .spacing(8)
                .align_y(Center),
            )
            .height(Fill)
            .center_y(Fill),
        )
        .width(Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press_maybe(enabled.then(|| message.clone()))
        .style(move |_, status| crate::grid::context_menu_style(palette, false, status));
        focus_button::with_menu(
            row.into(),
            enabled,
            RANDOM_ID,
            message,
            Message::More(Event::RandomOptions),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests;
