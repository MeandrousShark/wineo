//! The library's list layout: one row per game with a thumbnail and
//! columns, the header that sorts by them, and which columns fit.

use std::borrow::Cow;

use iced::widget::{Space, button, column, container, image, row, stack, svg, text};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Length, Padding, border};
use wineo_core::library::{LIST_COLUMN_MAX, LIST_COLUMN_MIN, ListColumn, SortKey};

use crate::gog_library::Badge;
use crate::grid::{card_area, install_bar, shows_identifier};
use crate::grid_motion::PADDING;
use crate::library::{Card, CardKind};
use crate::library_actions::{EYE_OFF, STAR, STAR_OUTLINE};
use crate::library_menu::icon;
use crate::slot::SlotId;
use crate::sort_control::{ARROW_DOWN, ARROW_UP};
use crate::theme::Palette;
use crate::{App, Message, ui};

pub(crate) const ROW_HEIGHT: f32 = 56.0;
/// The header above the rows, which stays put while they scroll.
pub(crate) const HEADER_HEIGHT: f32 = 32.0;
const THUMB_WIDTH: f32 = 32.0;
/// A 2:3 poster, like the grid's.
const THUMB_HEIGHT: f32 = 48.0;
/// The row's content inset from its selection border on either side.
const ROW_PADDING: f32 = 8.0;
const CELL_GAP: f32 = 16.0;
/// The thumbnail's gap to the title.
const THUMB_GAP: f32 = 12.0;
/// What the Title column keeps, thumbnail and marks included, before the
/// other columns start leaving.
pub(crate) const TITLE_MIN: f32 = 200.0;
const STATE_WIDTH: f32 = ListColumn::State.default_width() as f32;
const TITLE_SIZE: f32 = 14.0;
const CELL_SIZE: f32 = 13.0;
const IDENTIFIER_SIZE: f32 = 12.0;
const HEADER_SIZE: f32 = 12.0;
const DOT: f32 = 8.0;
const MARK_ICON: f32 = 16.0;
/// The star's click target, which stays in place with no star showing so
/// nothing moves when the hover brings it.
const STAR_BOX: f32 = 24.0;
const CHECK_CHIP: f32 = 22.0;
const SELECTED_BORDER: f32 = 2.0;
/// A header button's tint reaches this far past its label. The header's
/// cells grow by it on each side and its gaps and padding shrink to match,
/// so the labels still line up with the rows' cells.
pub(crate) const HEADER_INSET: f32 = 6.0;
/// What separates two header buttons, which a resize handle sits over.
pub(crate) const HEADER_GAP: f32 = CELL_GAP - 2.0 * HEADER_INSET;
const MARKS_GAP: f32 = 4.0;
/// How much the star grows at the top of its pop.
const STAR_LIFT: f32 = 4.0;
/// The sort arrow beside a header label, and its gap to it.
const ARROW_SIZE: f32 = 12.0;
const ARROW_GAP: f32 = 4.0;

/// A column after Title and State, which a narrow row drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Column {
    Playtime,
    LastPlayed,
    Source,
    Tags,
}

impl Column {
    /// Left to right. They leave from the right as the row narrows.
    pub(crate) const ALL: [Self; 4] = [Self::Playtime, Self::LastPlayed, Self::Source, Self::Tags];

    /// The width it has until it's resized.
    fn width(self) -> f32 {
        f32::from(self.list_column().default_width())
    }

    pub(crate) fn list_column(self) -> ListColumn {
        match self {
            Self::Playtime => ListColumn::Playtime,
            Self::LastPlayed => ListColumn::LastPlayed,
            Self::Source => ListColumn::Source,
            Self::Tags => ListColumn::Tags,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Playtime => "Playtime",
            Self::LastPlayed => "Last played",
            Self::Source => "Source",
            Self::Tags => "Tags",
        }
    }

    /// The sort a click on its header picks, like the Sort menu's row.
    pub(crate) fn sort(self) -> Option<SortKey> {
        match self {
            Self::Playtime => Some(SortKey::Playtime),
            Self::LastPlayed => Some(SortKey::LastPlayed),
            Self::Source => Some(SortKey::Source),
            Self::Tags => Some(SortKey::Tags),
        }
    }
}

/// Each resizable column's width in use, which starts as the saved ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Widths {
    state: f32,
    /// In `Column::ALL`'s order.
    columns: [f32; 4],
}

impl Default for Widths {
    fn default() -> Self {
        Self {
            state: STATE_WIDTH,
            columns: Column::ALL.map(Column::width),
        }
    }
}

impl Widths {
    /// The defaults with `[view.list_columns]`'s widths over them.
    pub(crate) fn saved(saved: &std::collections::BTreeMap<ListColumn, u16>) -> Self {
        let mut widths = Self::default();
        for (column, width) in saved {
            widths.set(*column, f32::from(*width));
        }
        widths
    }

    pub(crate) fn of(&self, column: Column) -> f32 {
        self.columns[column as usize]
    }

    pub(crate) fn get(&self, column: ListColumn) -> f32 {
        match column {
            ListColumn::State => self.state,
            ListColumn::Playtime => self.of(Column::Playtime),
            ListColumn::LastPlayed => self.of(Column::LastPlayed),
            ListColumn::Source => self.of(Column::Source),
            ListColumn::Tags => self.of(Column::Tags),
        }
    }

    pub(crate) fn set(&mut self, column: ListColumn, width: f32) {
        let slot = match column {
            ListColumn::State => &mut self.state,
            ListColumn::Playtime => &mut self.columns[Column::Playtime as usize],
            ListColumn::LastPlayed => &mut self.columns[Column::LastPlayed as usize],
            ListColumn::Source => &mut self.columns[Column::Source as usize],
            ListColumn::Tags => &mut self.columns[Column::Tags as usize],
        };
        *slot = width;
    }

    /// The width as `library.toml` holds it. Widths in use are always whole.
    pub(crate) fn saved_width(&self, column: ListColumn) -> u16 {
        self.get(column) as u16
    }
}

/// The Title cell's width, thumbnail and marks included, in a row `width`
/// wide with `columns` beside it.
pub(crate) fn title_cell(widths: &Widths, columns: &[Column], width: f32) -> f32 {
    let others: f32 = columns
        .iter()
        .map(|column| CELL_GAP + widths.of(*column))
        .sum();
    width - 2.0 * ROW_PADDING - CELL_GAP - widths.state - others
}

/// The widest `column` can be made in a row `width` wide with `columns`
/// showing: as wide as leaves Title its minimum, but never narrower than
/// `from`, so a column already past that doesn't jump, and never past the
/// maximum. Written without `clamp`, which panics once Title is already
/// under its minimum and the bounds cross.
pub(crate) fn widest(
    widths: &Widths,
    column: ListColumn,
    columns: &[Column],
    width: f32,
    from: f32,
) -> f32 {
    let laid =
        column == ListColumn::State || columns.iter().any(|shown| shown.list_column() == column);
    let allowed = if laid {
        (widths.get(column) + title_cell(widths, columns, width) - TITLE_MIN).floor()
    } else {
        f32::from(LIST_COLUMN_MAX)
    };
    allowed.max(from).min(f32::from(LIST_COLUMN_MAX))
}

/// `wanted` held between the narrowest a column can be and `widest`, rounded
/// to a whole pixel.
pub(crate) fn bounded(wanted: f32, widest: f32) -> f32 {
    wanted.round().max(f32::from(LIST_COLUMN_MIN)).min(widest)
}

/// The columns a row `width` wide has room for besides Title and State.
pub(crate) fn columns(width: f32, widths: &Widths) -> &'static [Column] {
    let mut used = 2.0 * ROW_PADDING + TITLE_MIN + CELL_GAP + widths.state;
    let mut count = 0;
    for column in Column::ALL {
        used += CELL_GAP + widths.of(column);
        if used > width {
            break;
        }
        count += 1;
    }
    &Column::ALL[..count]
}

/// The store, or failing that what it runs on, as core words it for the
/// Source sort too.
pub(crate) fn source_label(card: &Card) -> &'static str {
    card.source.label().unwrap_or("-")
}

/// The poster star's three looks, which a row shows the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Star {
    None,
    /// The outline a hovered card offers to favorite it.
    Outline,
    Filled,
}

/// What a row shows, from the same card data a poster draws.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowFacts<'a> {
    pub(crate) title: &'a str,
    /// The environment name, as the card's muted line under the title.
    pub(crate) identifier: Option<&'a str>,
    pub(crate) state: &'static str,
    pub(crate) dot: Color,
    pub(crate) playtime: &'a str,
    pub(crate) last_played: &'a str,
    pub(crate) source: &'static str,
    /// Joined only when the Tags column shows.
    pub(crate) tags: &'a [String],
    pub(crate) star: Star,
    /// How far the hover has faded in, 0 to 1, as on the poster.
    pub(crate) hover: f32,
    /// How far the star has grown in its pop, 0 to 1.
    pub(crate) lift: f32,
    /// The GOG pill: an update waiting, or one to finish.
    pub(crate) gog: Option<Badge>,
    /// The crossed eye of a hidden game.
    pub(crate) eye: bool,
    pub(crate) steam: bool,
    pub(crate) install: Option<u8>,
    /// Hidden or not installed: the thumbnail fades and the title mutes.
    pub(crate) faded: bool,
    /// Not installed: the thumbnail is the grayscale art.
    pub(crate) gray: bool,
    pub(crate) selected: bool,
    pub(crate) hovered: bool,
    pub(crate) marked: bool,
}

impl App {
    /// The columns the list shows besides Title and State: the ones that fit,
    /// or while a handle is dragged, the ones that showed when it started.
    pub(crate) fn list_columns(&self) -> &'static [Column] {
        match &self.column_drag {
            Some(drag) => drag.held,
            None => columns(self.slot_width(), &self.list_widths),
        }
    }

    /// What the list's header takes from the scroll's height.
    pub(crate) fn header_room(&self) -> f32 {
        if self.layout == crate::grid_motion::ViewLayout::List {
            HEADER_HEIGHT
        } else {
            0.0
        }
    }

    /// What `card`'s row in `slot` shows.
    pub(crate) fn row_facts<'a>(
        &self,
        slot: &SlotId,
        card: &'a Card,
        palette: Palette,
    ) -> RowFacts<'a> {
        let hovered = self.hovered.as_ref() == Some(slot);
        let selected = self.selected.as_ref() == Some(&card.name);
        let dimmed = card.not_installed();
        // The poster's hover fade, so the row's hover and star ease the same.
        let (hover, _) = self.hover_fades.progress(slot, hovered, selected);
        RowFacts {
            title: &card.display_title,
            identifier: shows_identifier(card).then_some(card.name.as_str()),
            state: card.state_word(),
            dot: card.dot(palette),
            playtime: card.playtime(),
            last_played: &card.last_played,
            source: source_label(card),
            tags: &card.tags,
            star: if card.favorite {
                Star::Filled
            } else if hovered || hover > 0.0 {
                Star::Outline
            } else {
                Star::None
            },
            hover,
            lift: self
                .star_pops
                .progress(slot)
                .map_or(0.0, crate::star_pop::lift),
            gog: crate::gog_library::badge(card.gog.as_ref(), self.gog_updates.status(&card.name)),
            eye: card.hidden,
            steam: card.kind == Some(CardKind::Proxied),
            install: card.install_progress(),
            faded: dimmed || card.hidden,
            gray: dimmed,
            selected,
            hovered,
            marked: self.marked.contains(slot.name()),
        }
    }

    /// `card`'s row in `slot`, `width` wide with `columns`, the ones
    /// `list_columns` gives or, for a row fading out, `columns(width)`, at
    /// `opacity`, as it fades in or out.
    pub(crate) fn row_drawn<'a>(
        &'a self,
        slot: &SlotId,
        card: &'a Card,
        opacity: f32,
        width: f32,
        columns: &[Column],
    ) -> Element<'a, Message> {
        let alpha = self.collapse.alpha() * opacity;
        let palette = self.palette.faded(alpha);
        let facts = self.row_facts(slot, card, palette);
        let dim = if facts.faded {
            crate::panel::DIMMED_OPACITY
        } else {
            1.0
        };
        let art = card.art.as_ref().and_then(|art| {
            self.images
                .shown(&(art.clone(), crate::artwork::Kind::Grid), facts.gray)
        });
        let thumb: Element<'_, Message> = match art {
            Some(handle) => image(handle.clone())
                .width(THUMB_WIDTH)
                .height(THUMB_HEIGHT)
                .content_fit(ContentFit::Cover)
                .border_radius(3)
                .opacity(alpha * dim)
                .into(),
            None => container(Space::new())
                .width(THUMB_WIDTH)
                .height(THUMB_HEIGHT)
                .style(move |_| {
                    container::Style::default()
                        .background(palette.surface.scale_alpha(dim))
                        .border(border::rounded(3).width(1).color(palette.border))
                })
                .into(),
        };
        let thumb: Element<'_, Message> = if facts.marked {
            let check = crate::multi_select::check(self.palette, alpha, CHECK_CHIP);
            stack![
                thumb,
                container(check)
                    .center_x(THUMB_WIDTH)
                    .center_y(THUMB_HEIGHT)
            ]
            .into()
        } else {
            thumb
        };
        let widths = &self.list_widths;
        let fitted = row_text(&facts, columns, width, widths);
        let mut title = column![
            text(fitted.title)
                .size(TITLE_SIZE)
                .font(crate::fonts::semibold())
                .color(if facts.faded {
                    palette.muted
                } else {
                    palette.foreground
                })
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2);
        if let Some(identifier) = fitted.identifier {
            title = title.push(
                text(identifier)
                    .size(IDENTIFIER_SIZE)
                    .color(palette.muted)
                    .wrapping(text::Wrapping::None),
            );
        }
        let mut marks = row![].spacing(MARKS_GAP).align_y(Center);
        if let Some(badge) = facts.gog {
            marks = marks
                .push(crate::grid::gog_pill(badge, palette, alpha, false).width(pill_width(badge)));
        }
        if facts.steam {
            marks = marks.push(
                svg(svg::Handle::from_memory(include_bytes!(
                    "../assets/steam.svg"
                )))
                .width(MARK_ICON)
                .height(MARK_ICON)
                .opacity(alpha * dim),
            );
        }
        if facts.eye {
            marks = marks.push(icon(EYE_OFF, palette.muted, MARK_ICON).opacity(alpha));
        }
        marks = marks.push(star(
            slot,
            facts.star,
            facts.hovered,
            facts.hover,
            facts.lift,
            palette,
            alpha,
        ));
        let title_cell = row![thumb, container(title).width(Fill).clip(true), marks,]
            .spacing(THUMB_GAP)
            .align_y(Center)
            .width(Fill);
        let state = row![
            container(Space::new())
                .width(DOT)
                .height(DOT)
                .style(move |_| container::Style::default()
                    .background(facts.dot)
                    .border(border::rounded(DOT / 2.0))),
            text(fitted.state)
                .size(CELL_SIZE)
                .color(palette.foreground)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(8)
        .align_y(Center);
        let mut cells = row![title_cell, container(state).width(widths.state).clip(true)]
            .spacing(CELL_GAP)
            .align_y(Center);
        for (column, value) in fitted.cells {
            cells = cells.push(
                container(
                    text(value)
                        .size(CELL_SIZE)
                        .color(palette.muted)
                        .wrapping(text::Wrapping::None),
                )
                .width(widths.of(column))
                .clip(true),
            );
        }
        let background = if facts.marked {
            Some(
                self.palette
                    .primary
                    .scale_alpha(crate::multi_select::WASH * alpha),
            )
        } else if facts.hover > 0.0 {
            Some(palette.surface.scale_alpha(facts.hover))
        } else {
            None
        };
        let border_color = if facts.selected {
            palette.primary
        } else {
            Color::TRANSPARENT
        };
        let body = container(cells)
            .padding([0.0, ROW_PADDING])
            .width(width)
            .height(ROW_HEIGHT)
            .center_y(ROW_HEIGHT)
            .style(move |_| container::Style {
                background: background.map(Into::into),
                border: border::rounded(6)
                    .width(SELECTED_BORDER)
                    .color(border_color),
                ..container::Style::default()
            });
        let content: Element<'_, Message> = match facts.install {
            Some(percent) => stack![
                body,
                container(install_bar(percent, palette, alpha))
                    .width(Fill)
                    .height(Fill)
                    .align_bottom(Fill)
                    .padding(Padding {
                        left: ROW_PADDING,
                        right: ROW_PADDING,
                        bottom: SELECTED_BORDER,
                        top: 0.0,
                    }),
            ]
            .width(width)
            .height(ROW_HEIGHT)
            .into(),
            None => body.into(),
        };
        card_area(content, slot)
    }

    /// The column names over the rows, at the rows' width. Every one but
    /// State sorts like the Sort menu's rows, and a second click flips the
    /// direction. A handle on the left edge of each column but Title resizes
    /// it.
    pub(crate) fn list_header(&self) -> Element<'_, Message> {
        let width = self.slot_width();
        let widths = &self.list_widths;
        let shown = self.list_columns();
        let mut cells = row![
            self.header_cell(
                "Title",
                Some(SortKey::Name),
                Length::Fill,
                title_cell(widths, shown, width),
            ),
            self.header_cell(
                "State",
                None,
                Length::Fixed(widths.state + 2.0 * HEADER_INSET),
                widths.state,
            ),
        ]
        .spacing(HEADER_GAP)
        .align_y(Center);
        let mut handles = vec![ListColumn::State];
        for column in shown {
            cells = cells.push(self.header_cell(
                column.label(),
                column.sort(),
                Length::Fixed(widths.of(*column) + 2.0 * HEADER_INSET),
                widths.of(*column),
            ));
            handles.push(column.list_column());
        }
        let inner = container(cells)
            .padding([0.0, ROW_PADDING - HEADER_INSET])
            .width(width)
            .height(HEADER_HEIGHT)
            .center_y(HEADER_HEIGHT);
        container(crate::column_resize::Handles::new(
            inner.into(),
            handles,
            self.column_drag.as_ref().map(|drag| drag.column),
            self.palette,
        ))
        .padding([0.0, PADDING])
        .width(Fill)
        .id(iced::widget::Id::new(crate::column_resize::HEADER_ID))
        .into()
    }

    /// A header cell `width` wide whose label has `room` beside the sort
    /// arrow, cut with an ellipsis so it never reaches past the cell.
    fn header_cell(
        &self,
        label: &'static str,
        sort: Option<SortKey>,
        width: Length,
        room: f32,
    ) -> Element<'static, Message> {
        let palette = self.palette;
        let active = sort.is_some_and(|sort| sort == self.query.sort);
        let room = if active {
            room - ARROW_GAP - ARROW_SIZE
        } else {
            room
        };
        let label = text(crate::tile_title::fit(
            label,
            room,
            HEADER_SIZE,
            crate::fonts::semibold(),
        ))
        .size(HEADER_SIZE)
        .font(crate::fonts::semibold())
        .wrapping(text::Wrapping::None);
        let Some(sort) = sort else {
            return container(label.color(palette.muted))
                .padding([0.0, HEADER_INSET])
                .width(width)
                .clip(true)
                .into();
        };
        // Colored by the button's style, so hover and press reach it.
        let mut content = row![label].spacing(ARROW_GAP).align_y(Center);
        if active {
            let arrow = if self.query.descending {
                ARROW_DOWN
            } else {
                ARROW_UP
            };
            content = content.push(icon(arrow, palette.foreground, ARROW_SIZE));
        }
        let cell = button(content)
            .padding([2.0, HEADER_INSET])
            .on_press(Message::Sort(crate::sort_control::Event::Header(sort)))
            .style(move |_, status| header_style(palette, status, active));
        container(cell).width(width).clip(true).into()
    }
}

/// A row's text cut to its cells with an ellipsis, so nothing clips
/// mid-glyph.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowText {
    pub(crate) title: String,
    pub(crate) identifier: Option<String>,
    pub(crate) state: String,
    pub(crate) cells: Vec<(Column, String)>,
}

/// The room the title's text has in a row `width` wide with `columns`,
/// after the thumbnail, the other columns, and the marks beside it.
pub(crate) fn title_room(facts: &RowFacts, columns: &[Column], width: f32, widths: &Widths) -> f32 {
    let cell = title_cell(widths, columns, width);
    let mut marks = STAR_BOX;
    if facts.steam {
        marks += MARK_ICON + MARKS_GAP;
    }
    if facts.eye {
        marks += MARK_ICON + MARKS_GAP;
    }
    if let Some(badge) = facts.gog {
        marks += pill_width(badge) + MARKS_GAP;
    }
    (cell - THUMB_WIDTH - 2.0 * THUMB_GAP - marks).max(0.0)
}

/// The state's room beside its dot in a State column `width` wide.
pub(crate) fn state_room(width: f32) -> f32 {
    width - DOT - 8.0
}

/// The row's text for a row `width` wide with `columns`, the ones
/// `columns(width, widths)` gives.
pub(crate) fn row_text(
    facts: &RowFacts,
    columns: &[Column],
    width: f32,
    widths: &Widths,
) -> RowText {
    use crate::tile_title::fit;
    let room = title_room(facts, columns, width, widths);
    RowText {
        title: fit(facts.title, room, TITLE_SIZE, crate::fonts::semibold()),
        identifier: facts
            .identifier
            .map(|identifier| fit(identifier, room, IDENTIFIER_SIZE, Font::DEFAULT)),
        state: fit(
            facts.state,
            state_room(widths.state),
            CELL_SIZE,
            Font::DEFAULT,
        ),
        cells: columns
            .iter()
            .map(|column| {
                let value = match column {
                    Column::Playtime => Cow::Borrowed(facts.playtime),
                    Column::LastPlayed => Cow::Borrowed(facts.last_played),
                    Column::Source => Cow::Borrowed(facts.source),
                    Column::Tags => match facts.tags {
                        [tag] => Cow::Borrowed(tag.as_str()),
                        tags => Cow::Owned(tags.join(", ")),
                    },
                };
                (
                    *column,
                    fit(&value, widths.of(*column), CELL_SIZE, Font::DEFAULT),
                )
            })
            .collect(),
    }
}

fn pill_width(badge: Badge) -> f32 {
    use crate::grid::{GOG_PILL_SIDE, GOG_PILL_SIZE};
    crate::tile_title::line_width(badge.label(), GOG_PILL_SIZE, Font::DEFAULT) + 2.0 * GOG_PILL_SIDE
}

/// A sortable header's look: a list row's hover and press, with the label
/// muted until it's the sort or the pointer is on it.
pub(crate) fn header_style(
    palette: Palette,
    status: button::Status,
    active: bool,
) -> button::Style {
    let mut style = ui::list_row_style(palette, status);
    if !active && status == button::Status::Active {
        style.text_color = palette.muted;
    }
    style
}

/// The row's star: a favorite's filled one, or the outline fading with the
/// hover, pressed the way the poster's is and growing in its pop. Its room
/// stays with none showing.
fn star<'a>(
    slot: &SlotId,
    look: Star,
    hovered: bool,
    hover: f32,
    lift: f32,
    palette: Palette,
    alpha: f32,
) -> Element<'a, Message> {
    let (bytes, color) = match look {
        Star::None => return Space::new().width(STAR_BOX).height(STAR_BOX).into(),
        Star::Outline => (STAR_OUTLINE, palette.muted),
        Star::Filled => (STAR, palette.primary),
    };
    // The renderer's tint ignores its alpha, so the icon fades by opacity.
    let strength = star_strength(look, hover) * alpha;
    let size = MARK_ICON + STAR_LIFT * lift;
    // A star fading out with the hover takes no presses.
    let press = (look == Star::Filled || hovered)
        .then(|| Message::Mark(crate::library_actions::Event::PosterStar(slot.clone())));
    button(container(icon(bytes, color, size).opacity(strength)).center(STAR_BOX))
        .width(STAR_BOX)
        .height(STAR_BOX)
        .padding(0)
        .on_press_maybe(press)
        .style(move |_, status| star_style(palette, status))
        .into()
}

/// How strongly the star draws before the row's fade: a favorite's at full
/// strength, the outline with the hover's fade in and out, as the poster's.
pub(crate) fn star_strength(look: Star, hover: f32) -> f32 {
    match look {
        Star::None => 0.0,
        Star::Outline => hover,
        Star::Filled => 1.0,
    }
}

/// A list row's hover and press, round to fit the star.
pub(crate) fn star_style(palette: Palette, status: button::Status) -> button::Style {
    let style = ui::list_row_style(palette, status);
    button::Style {
        border: border::rounded(STAR_BOX / 2.0)
            .width(style.border.width)
            .color(style.border.color),
        ..style
    }
}

#[cfg(test)]
mod tests;
