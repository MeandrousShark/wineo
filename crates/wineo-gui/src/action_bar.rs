//! The bar that floats at the bottom of the Library while games are marked:
//! how many, **Tags**, a favorite and a hide button, and an x that clears the
//! marks. **Tags** opens a popup above it with a box per tag across the
//! marked games and a field for a new one.

use std::collections::BTreeSet;

use iced::alignment::Vertical;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Row, button, column, container, row, scrollable, text, text_input, tooltip};
use iced::{Center, Element, Fill, Font, Padding, Point, Rectangle, Task, font};

use crate::grid::{
    ACTION_MENU_HEADER_HEIGHT, ACTION_MENU_MARGIN, ACTION_MENU_PADDING, ACTION_MENU_SPACING,
    ACTION_MENU_WIDTH, menu_position,
};
use crate::library_actions::Flag;
use crate::library_menu::{check_box, icon, menu_row, mixed_box};
use crate::theme::Palette;
use crate::ui::{self, ScrollStyle};
use crate::{App, Message, bulk_tags, focus_button};

pub(crate) const TAGS_BUTTON_ID: &str = "library-action-tags";
pub(crate) const FAVORITE_BUTTON_ID: &str = "library-action-favorite";
pub(crate) const HIDE_BUTTON_ID: &str = "library-action-hide";
pub(crate) const CLEAR_ID: &str = "library-action-clear";
pub(crate) const NEW_TAG_ID: &str = "library-tag-new";
const MENU_SCROLL_ID: &str = "library-tag-menu";
pub(crate) const CLEAR_LABEL: &str = "Clear selection";
/// The popup's distance above the Tags button.
const GAP: f32 = 4.0;
/// The bar's padding around its controls, top and bottom.
const BAR_PADDING: f32 = 8.0;
/// The bar's height: one row of controls and its padding.
pub(crate) const BAR_HEIGHT: f32 = ui::BUTTON_HEIGHT + 2.0 * BAR_PADDING;
/// The space between the last row of posters and the bar, once scrolled
/// to the end.
const BAR_CLEARANCE: f32 = 8.0;
/// The room the grid keeps under its last row while the bar shows, so the
/// row can scroll clear of it: the bar, the notes' margin below it, and
/// some space above.
pub(crate) const BAR_ROOM: f32 = crate::grid::NOTES_MARGIN + BAR_HEIGHT + BAR_CLEARANCE;

const CHEVRON_UP: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 10l4-4 4 4" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click, Enter, or Space on Tags, which opens the popup above it once
    /// its bounds are found.
    ToggleMenu,
    /// The Tags button's bounds, found for the open request with this number.
    OpenAbove(u64, Option<Rectangle>),
    /// A click outside the popup.
    CloseMenu,
    /// The bar's x.
    Clear,
    /// The bar's favorite or hide button: sets the flag on every marked game
    /// unless all have it, then clears it from all.
    ToggleFlag(Flag),
    /// A tag's row: adds it to every marked game unless all have it, then
    /// removes it from all.
    Toggle(String),
    /// The new tag field's text.
    Draft(String),
    /// Enter in the new tag field.
    Submit,
    /// A batch of writes finished.
    Saved(bulk_tags::Saved),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // The replies always arrive; `OpenAbove` checks the window itself.
        !matches!(self, Self::OpenAbove(..) | Self::Saved(_))
    }
}

/// How many of the marked games have a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Share {
    All,
    Some,
    None,
}

/// The open popup's own state, fresh each time it opens.
#[derive(Debug, Default)]
pub(crate) struct Popup {
    /// The Tags button's bounds, which the popup sits above.
    anchor: Rectangle,
    draft: String,
    /// Tags changed since it opened, kept listed after the last game with
    /// one loses it, so the row can be clicked back.
    touched: BTreeSet<String>,
}

fn row_id(tag: &str) -> String {
    format!("library-tag-row-{tag}")
}

/// Why `draft` can't be a tag, by the rules `wineo set` and `wineo tag` use.
/// Nothing typed yet is no problem, it just can't be added.
pub(crate) fn problem(draft: &str) -> Option<String> {
    if draft.is_empty() {
        return None;
    }
    wineo_core::config::validate_tags(&[draft.to_owned()]).err()
}

impl App {
    /// The bar shows while anything is marked, over the Library itself.
    pub(crate) fn action_bar_shown(&self) -> bool {
        !self.marked.is_empty()
            && self.section == crate::sidebar::Section::Library
            && self.settings.is_none()
            && self.art_page.is_none()
            && self.tool_page.is_none()
            && self.portable_page.is_none()
    }

    /// How much of the grid's viewport bottom the bar covers, which the
    /// grid adds under its last row and scrolling into view keeps clear.
    pub(crate) fn bar_room(&self) -> f32 {
        if self.action_bar_shown() {
            BAR_ROOM
        } else {
            0.0
        }
    }

    /// The popup shows only with its bar, and like the toolbar's menus.
    pub(crate) fn tag_menu_shown(&self) -> bool {
        self.tag_menu
            && self.action_bar_shown()
            && !self.panel_hidden()
            && self.hook_stop_confirm.is_none()
    }

    /// How many of the marked games have `tag`.
    pub(crate) fn tag_share(&self, tag: &str) -> Share {
        let marked = self.marked_cards();
        let with = marked
            .iter()
            .filter(|card| bulk_tags::has_tag(card, tag))
            .count();
        if with == 0 {
            Share::None
        } else if with == marked.len() {
            Share::All
        } else {
            Share::Some
        }
    }

    /// Whether every marked game has `flag`, which the bar's button then
    /// undoes instead of setting.
    pub(crate) fn flag_all(&self, flag: Flag) -> bool {
        let marked = self.marked_cards();
        !marked.is_empty() && marked.iter().all(|card| flag.of(card))
    }

    /// Every tag in the library, A to Z as the Filters popup lists them,
    /// plus any changed since the popup opened.
    pub(crate) fn tag_rows(&self) -> Vec<String> {
        let mut tags = self.order.options(self.order_inputs()).tags.clone();
        let mut gone = false;
        for tag in &self.tag_popup.touched {
            if !tags.contains(tag) {
                tags.push(tag.clone());
                gone = true;
            }
        }
        if gone {
            tags.sort_by_cached_key(|tag| (tag.to_lowercase(), tag.clone()));
        }
        tags
    }

    /// The popup's focusable controls, in order.
    fn tag_menu_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.tag_rows().iter().map(|tag| row_id(tag)).collect();
        ids.push(NEW_TAG_ID.to_owned());
        ids
    }

    #[cfg(test)]
    pub(crate) fn tag_draft(&self) -> &str {
        &self.tag_popup.draft
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::ToggleMenu => {
            if app.tag_menu || app.tag_opening.pending.is_some() {
                app.tag_opening.cancel();
                app.tag_menu = false;
                return Task::none();
            }
            if !app.action_bar_shown() {
                return Task::none();
            }
            crate::library_menu::request_bounds(
                &mut app.tag_opening,
                TAGS_BUTTON_ID,
                |request, bounds| Message::Tags(Event::OpenAbove(request, bounds)),
            )
        }
        Event::OpenAbove(request, bounds) => {
            // Whatever took the window since the press keeps it.
            if !app.tag_opening.answer(request)
                || !app.library_menu_may_open()
                || !app.action_bar_shown()
            {
                return Task::none();
            }
            match bounds {
                Some(bounds) => open_menu(app, bounds),
                None => Task::none(),
            }
        }
        Event::CloseMenu => {
            app.tag_opening.cancel();
            app.tag_menu = false;
            Task::none()
        }
        Event::Clear => {
            app.clear_marks();
            Task::none()
        }
        Event::ToggleFlag(flag) => {
            if !app.action_bar_shown() {
                return Task::none();
            }
            let value = !app.flag_all(flag);
            app.flag_marked(flag, value)
        }
        Event::Toggle(tag) => {
            if !app.tag_menu_shown() {
                return Task::none();
            }
            let add = app.tag_share(&tag) != Share::All;
            app.tag_popup.touched.insert(tag.clone());
            app.tag_marked(&tag, add)
        }
        Event::Draft(draft) => {
            app.tag_popup.draft = draft;
            Task::none()
        }
        Event::Submit => {
            let draft = app.tag_popup.draft.clone();
            // A bad name stays in the field with its reason under it.
            if !app.tag_menu_shown() || draft.is_empty() || problem(&draft).is_some() {
                return Task::none();
            }
            app.tag_popup.draft.clear();
            app.tag_popup.touched.insert(draft.clone());
            app.tag_marked(&draft, true)
        }
        Event::Saved(saved) => app.batch_saved(saved),
    }
}

fn open_menu(app: &mut App, anchor: Rectangle) -> Task<Message> {
    app.close_library_menus();
    app.tag_menu = true;
    app.tag_popup = Popup {
        anchor,
        ..Popup::default()
    };
    // Focus moves into the popup, so a second Enter or Space toggles a row
    // instead of closing it again.
    let first = app.tag_menu_ids().swap_remove(0);
    crate::library_menu::focus(first, MENU_SCROLL_ID)
}

/// Keys while the popup is open: Escape closes it and gives Tags its focus
/// back, Tab and the up and down arrows move through its rows and field,
/// and the rows take Enter and Space themselves. Nothing else reaches the
/// grid behind it, so Esc closes the popup before it would clear the marks.
pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => {
            app.tag_menu = false;
            iced::widget::operation::focus(iced::widget::Id::new(TAGS_BUTTON_ID))
        }
        Key::Named(Named::Tab | Named::ArrowDown | Named::ArrowUp) => {
            let mut ids = app.tag_menu_ids();
            // Cycling a lone control unfocuses it, so with no tags the
            // field just keeps its focus.
            if ids.len() == 1 {
                return crate::library_menu::focus(ids.swap_remove(0), MENU_SCROLL_ID);
            }
            let reverse = *key == Key::Named(Named::ArrowUp)
                || (*key == Key::Named(Named::Tab) && modifiers.shift());
            crate::library_menu::cycle(&ids, reverse, MENU_SCROLL_ID)
        }
        _ => Task::none(),
    }
}

/// The bar's label for `count` marked games.
pub(crate) fn count_label(count: usize) -> String {
    format!("{count} selected")
}

/// The label of the bar's button for `flag`, which says what pressing it
/// does to the marked games: `all` of them have the flag already.
pub(crate) fn flag_label(flag: Flag, all: bool) -> &'static str {
    match (flag, all) {
        (Flag::Favorite, false) => "Favorite",
        (Flag::Favorite, true) => "Unfavorite",
        (Flag::Hidden, false) => "Hide",
        (Flag::Hidden, true) => "Show",
    }
}

/// A text-only button of the bar.
fn flag_button(
    palette: Palette,
    label: &'static str,
    id: &'static str,
    message: Message,
) -> Element<'static, Message> {
    let button = button(text(label).size(14).height(Fill).align_y(Center))
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 12])
        .on_press(message.clone())
        .style(move |_, status| ui::secondary_style(palette, status));
    focus_button::control(button.into(), true, id, message)
}

/// What the bar does to the marked games. Each action is one more button
/// here, before the x, and Tab visits them in this order.
fn actions(app: &App) -> Vec<Element<'static, Message>> {
    let palette = app.palette;
    let message = Message::Tags(Event::ToggleMenu);
    let tags = button(
        row![text("Tags").size(14), icon(CHEVRON_UP, palette.muted, 16.0)]
            .spacing(6)
            .height(Fill)
            .align_y(Center),
    )
    .height(ui::BUTTON_HEIGHT)
    .padding([0, 12])
    .on_press(message.clone())
    .style(move |_, status| ui::secondary_style(palette, status));
    vec![
        focus_button::control(tags.into(), true, TAGS_BUTTON_ID, message),
        flag_button(
            palette,
            flag_label(Flag::Favorite, app.flag_all(Flag::Favorite)),
            FAVORITE_BUTTON_ID,
            Message::Tags(Event::ToggleFlag(Flag::Favorite)),
        ),
        flag_button(
            palette,
            flag_label(Flag::Hidden, app.flag_all(Flag::Hidden)),
            HIDE_BUTTON_ID,
            Message::Tags(Event::ToggleFlag(Flag::Hidden)),
        ),
    ]
}

impl App {
    /// The bar, for the bottom of the Library's notes.
    pub(crate) fn action_bar_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let message = Message::Tags(Event::Clear);
        let clear = button(
            container(icon(crate::filter_tab::CROSS, palette.foreground, 16.0)).center(Fill),
        )
        .width(ui::BUTTON_HEIGHT)
        .height(ui::BUTTON_HEIGHT)
        .padding(0)
        .on_press(message.clone())
        .style(move |_, status| ui::secondary_style(palette, status));
        let clear = focus_button::control(
            crate::zoom_control::with_tooltip_at(
                clear.into(),
                CLEAR_LABEL,
                palette,
                tooltip::Position::Top,
            ),
            true,
            CLEAR_ID,
            message,
        );
        let count = text(count_label(self.marked.len()))
            .size(14)
            .color(palette.foreground);
        let content = Row::with_children(
            std::iter::once(container(count).padding([0, 4]).into())
                .chain(actions(self))
                .chain(std::iter::once(clear)),
        )
        .spacing(8)
        .align_y(Center);
        container(content)
            .padding(Padding {
                top: BAR_PADDING,
                right: BAR_PADDING,
                bottom: BAR_PADDING,
                left: 12.0,
            })
            .height(BAR_HEIGHT)
            .style(move |_| crate::library_menu::surface_style(palette))
            .into()
    }

    /// The open tag popup, above the Tags button, left edges aligned. It
    /// grows with its content up to the window's top edge, then scrolls.
    pub(crate) fn tag_menu_view(&self) -> [Element<'_, Message>; 2] {
        let palette = self.palette;
        let anchor = self.tag_popup.anchor;
        let left = menu_position(Point::new(anchor.x - 4.0, 0.0), self.window_size, 0.0).x;
        let bottom = (self.window_size.height - anchor.y + GAP).max(0.0);
        let max_height = (anchor.y - GAP - ACTION_MENU_MARGIN).max(0.0);
        let mut rows = column![
            text("Tags")
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
        for tag in self.tag_rows() {
            let mark = match self.tag_share(&tag) {
                Share::All => check_box(true, palette),
                Share::Some => mixed_box(palette),
                Share::None => check_box(false, palette),
            };
            let id = row_id(&tag);
            rows = rows.push(menu_row(
                row![mark, text(tag.clone()).size(14)]
                    .spacing(8)
                    .align_y(Center)
                    .into(),
                palette,
                false,
                id,
                Message::Tags(Event::Toggle(tag)),
            ));
        }
        let draft = &self.tag_popup.draft;
        let field = text_input("New tag", draft)
            .id(iced::widget::Id::new(NEW_TAG_ID))
            .on_input(|draft| Message::Tags(Event::Draft(draft)))
            .on_submit(Message::Tags(Event::Submit))
            .size(14)
            .padding([6, 10])
            .style(move |_, status| crate::chips::input_style(palette, status));
        rows = rows
            .push(ui::separator(palette).padding([3, 0]))
            .push(field);
        if let Some(problem) = problem(draft) {
            rows = rows.push(text(problem).size(12).color(palette.danger));
        }
        let menu = container(
            scrollable(rows)
                .id(iced::widget::Id::new(MENU_SCROLL_ID))
                .wineo_style(palette),
        )
        .padding(ACTION_MENU_PADDING)
        .width(ACTION_MENU_WIDTH)
        .max_height(max_height)
        .style(move |_| crate::library_menu::surface_style(palette));
        let close = Message::Tags(Event::CloseMenu);
        [
            iced::widget::mouse_area(iced::widget::Space::new().width(Fill).height(Fill))
                .on_press(close.clone())
                .on_right_press(close)
                .into(),
            container(menu)
                .padding(Padding {
                    top: 0.0,
                    left,
                    right: 0.0,
                    bottom,
                })
                .width(Fill)
                .height(Fill)
                .align_y(Vertical::Bottom)
                .into(),
        ]
    }
}

#[cfg(test)]
mod tests;
