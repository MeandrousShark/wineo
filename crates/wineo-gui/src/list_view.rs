//! The library's list layout: one row per game with a thumbnail and
//! columns, the header that sorts by them, and which columns fit.

use iced::widget::{Space, button, column, container, image, row, stack, svg, text};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Length, Padding, border, font};
use wineo_core::library::SortKey;

use crate::gog_library::Badge;
use crate::grid::{card_area, install_bar, shows_identifier};
use crate::grid_motion::PADDING;
use crate::library::{Card, CardKind};
use crate::library_actions::{EYE_OFF, STAR, STAR_OUTLINE};
use crate::library_menu::{CHECK, icon};
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
const TITLE_MIN: f32 = 200.0;
const STATE_WIDTH: f32 = 128.0;
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
const CHECK_ICON: f32 = 14.0;
const SELECTED_BORDER: f32 = 2.0;
/// The GOG pill, as on the poster.
const PILL_SIZE: f32 = 11.0;
const PILL_PADDING: f32 = 2.0;
const PILL_SIDE: f32 = 8.0;
const MARKS_GAP: f32 = 4.0;
/// How much the star grows at the top of its pop.
const STAR_LIFT: f32 = 4.0;

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

    fn width(self) -> f32 {
        match self {
            Self::Playtime => 96.0,
            Self::LastPlayed => 112.0,
            Self::Source => 80.0,
            Self::Tags => 176.0,
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
            Self::Source | Self::Tags => None,
        }
    }
}

/// The columns a row `width` wide has room for besides Title and State.
pub(crate) fn columns(width: f32) -> &'static [Column] {
    let mut used = 2.0 * ROW_PADDING + TITLE_MIN + CELL_GAP + STATE_WIDTH;
    let mut count = 0;
    for column in Column::ALL {
        used += CELL_GAP + column.width();
        if used > width {
            break;
        }
        count += 1;
    }
    &Column::ALL[..count]
}

/// The store, or failing that what it runs on.
pub(crate) fn source_label(card: &Card) -> &'static str {
    card.source
        .store
        .map(wineo_core::source::Store::label)
        .or_else(|| {
            card.source
                .platform
                .map(wineo_core::source::Platform::label)
        })
        .unwrap_or("-")
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
pub(crate) struct RowFacts {
    pub(crate) title: String,
    /// The environment name, as the card's muted line under the title.
    pub(crate) identifier: Option<String>,
    pub(crate) state: &'static str,
    pub(crate) dot: Color,
    pub(crate) playtime: String,
    pub(crate) last_played: String,
    pub(crate) source: &'static str,
    pub(crate) tags: String,
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
    /// What the list's header takes from the scroll's height.
    pub(crate) fn header_room(&self) -> f32 {
        if self.layout == crate::grid_motion::ViewLayout::List {
            HEADER_HEIGHT
        } else {
            0.0
        }
    }

    pub(crate) fn row_facts(&self, card: &Card, palette: Palette) -> RowFacts {
        let hovered = self.hovered.as_ref() == Some(&card.name);
        let selected = self.selected.as_ref() == Some(&card.name);
        let dimmed = card.not_installed();
        // The poster's hover fade, so the row's hover and star ease the same.
        let (hover, _) = self.hover_fades.progress(&card.name, hovered, selected);
        RowFacts {
            title: card.display_title.clone(),
            identifier: shows_identifier(card).then(|| card.name.clone()),
            state: card.state_word(),
            dot: card.dot(palette),
            playtime: card.playtime().to_owned(),
            last_played: card.last_played.clone(),
            source: source_label(card),
            tags: card.tags.join(", "),
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
                .progress(&card.name)
                .map_or(0.0, crate::star_pop::lift),
            gog: crate::gog_library::badge(card.gog.as_ref(), self.gog_updates.status(&card.name)),
            eye: card.hidden,
            steam: card.kind == Some(CardKind::Proxied),
            install: card.install_progress(),
            faded: dimmed || card.hidden,
            gray: dimmed,
            selected,
            hovered,
            marked: self.marked.contains(&card.name),
        }
    }

    /// A list row `width` wide at `opacity`, as it fades in or out.
    pub(crate) fn row_drawn<'a>(
        &'a self,
        card: &'a Card,
        opacity: f32,
        width: f32,
    ) -> Element<'a, Message> {
        let alpha = self.collapse.alpha() * opacity;
        let palette = self.palette.faded(alpha);
        let facts = self.row_facts(card, palette);
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
            stack![thumb, check(self.palette, alpha)].into()
        } else {
            thumb
        };
        let fitted = row_text(&facts, width);
        let mut title = column![
            text(fitted.title)
                .size(TITLE_SIZE)
                .font(SEMIBOLD)
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
            marks = marks.push(pill(badge, palette, alpha));
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
            &card.name,
            facts.star,
            facts.hovered,
            facts.hover,
            facts.lift,
            palette,
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
        let mut cells = row![title_cell, container(state).width(STATE_WIDTH).clip(true)]
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
                .width(column.width())
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
        card_area(content, card)
    }

    /// The column names over the rows, at the rows' width. Title,
    /// Playtime, and Last played sort like the Sort menu's rows, and a
    /// second click flips the direction.
    pub(crate) fn list_header(&self) -> Element<'_, Message> {
        let width = self.slot_width();
        let mut cells = row![
            self.header_cell("Title", Some(SortKey::Name), Length::Fill),
            self.header_cell("State", None, Length::Fixed(STATE_WIDTH)),
        ]
        .spacing(CELL_GAP)
        .align_y(Center);
        for column in columns(width) {
            cells = cells.push(self.header_cell(
                column.label(),
                column.sort(),
                Length::Fixed(column.width()),
            ));
        }
        container(
            container(cells)
                .padding([0.0, ROW_PADDING])
                .width(width)
                .height(HEADER_HEIGHT)
                .center_y(HEADER_HEIGHT),
        )
        .padding([0.0, PADDING])
        .width(Fill)
        .into()
    }

    fn header_cell(
        &self,
        label: &'static str,
        sort: Option<SortKey>,
        width: Length,
    ) -> Element<'static, Message> {
        let palette = self.palette;
        let active = sort.is_some_and(|sort| sort == self.query.sort);
        let label = text(label)
            .size(HEADER_SIZE)
            .font(SEMIBOLD)
            .wrapping(text::Wrapping::None);
        let Some(sort) = sort else {
            return container(label.color(palette.muted))
                .width(width)
                .clip(true)
                .into();
        };
        // Colored by the button's style, so hover and press reach it.
        let mut content = row![label].spacing(4).align_y(Center);
        if active {
            let arrow = if self.query.descending {
                ARROW_DOWN
            } else {
                ARROW_UP
            };
            content = content.push(icon(arrow, palette.foreground, 12.0));
        }
        let cell = button(content)
            .padding([2, 0])
            .on_press(Message::Sort(crate::sort_control::Event::Header(sort)))
            .style(move |_, status| header_style(palette, status, active));
        container(cell).width(width).clip(true).into()
    }
}

const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..Font::DEFAULT
};

/// A row's text cut to its cells with an ellipsis, so nothing clips
/// mid-glyph.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowText {
    pub(crate) title: String,
    pub(crate) identifier: Option<String>,
    pub(crate) state: String,
    pub(crate) cells: Vec<(Column, String)>,
}

/// The room the title's text has in a row `width` wide, after the
/// thumbnail, the other columns, and the marks beside it.
pub(crate) fn title_room(facts: &RowFacts, width: f32) -> f32 {
    let others: f32 = columns(width)
        .iter()
        .map(|column| CELL_GAP + column.width())
        .sum();
    let cell = width - 2.0 * ROW_PADDING - CELL_GAP - STATE_WIDTH - others;
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

/// The state's room beside its dot.
pub(crate) const STATE_ROOM: f32 = STATE_WIDTH - DOT - 8.0;

pub(crate) fn row_text(facts: &RowFacts, width: f32) -> RowText {
    use crate::tile_title::fit;
    let room = title_room(facts, width);
    RowText {
        title: fit(&facts.title, room, TITLE_SIZE, SEMIBOLD),
        identifier: facts
            .identifier
            .as_ref()
            .map(|identifier| fit(identifier, room, IDENTIFIER_SIZE, Font::DEFAULT)),
        state: fit(facts.state, STATE_ROOM, CELL_SIZE, Font::DEFAULT),
        cells: columns(width)
            .iter()
            .map(|column| {
                let value = match column {
                    Column::Playtime => facts.playtime.as_str(),
                    Column::LastPlayed => facts.last_played.as_str(),
                    Column::Source => facts.source,
                    Column::Tags => facts.tags.as_str(),
                };
                (
                    *column,
                    fit(value, column.width(), CELL_SIZE, Font::DEFAULT),
                )
            })
            .collect(),
    }
}

fn pill_width(badge: Badge) -> f32 {
    crate::tile_title::line_width(badge.label(), PILL_SIZE, Font::DEFAULT) + 2.0 * PILL_SIDE
}

/// The GOG pill in its semantic color, as the poster draws it.
fn pill(badge: Badge, palette: Palette, alpha: f32) -> Element<'static, Message> {
    let color = badge_color(badge, palette);
    let ink = ui::accent_ink(color);
    container(
        text(badge.label())
            .size(PILL_SIZE)
            .color(ink.scale_alpha(alpha))
            .wrapping(text::Wrapping::None),
    )
    .width(pill_width(badge))
    .padding([PILL_PADDING, PILL_SIDE])
    .style(move |_| {
        container::Style::default()
            .background(color)
            .border(border::rounded(8))
    })
    .into()
}

/// The poster pill's color: primary for an update waiting, warning for one
/// to finish.
pub(crate) fn badge_color(badge: Badge, palette: Palette) -> Color {
    match badge {
        Badge::UpdateAvailable => palette.primary,
        Badge::FinishUpdate | Badge::FinishInstall => palette.warning,
    }
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
    name: &str,
    look: Star,
    hovered: bool,
    hover: f32,
    lift: f32,
    palette: Palette,
) -> Element<'a, Message> {
    let (bytes, color, strength) = match look {
        Star::None => return Space::new().width(STAR_BOX).height(STAR_BOX).into(),
        Star::Outline => (STAR_OUTLINE, palette.muted, hover.max(f32::from(hovered))),
        Star::Filled => (STAR, palette.primary, 1.0),
    };
    let size = MARK_ICON + STAR_LIFT * lift;
    // A star fading out with the hover takes no presses.
    let press = (look == Star::Filled || hovered)
        .then(|| Message::Mark(crate::library_actions::Event::PosterStar(name.to_owned())));
    button(container(icon(bytes, color, size).opacity(strength)).center(STAR_BOX))
        .width(STAR_BOX)
        .height(STAR_BOX)
        .padding(0)
        .on_press_maybe(press)
        .style(move |_, status| star_style(palette, status))
        .into()
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

/// A marked row's check, centered on its thumbnail like the poster's on
/// its art.
fn check(palette: Palette, alpha: f32) -> Element<'static, Message> {
    let fill = palette.primary;
    // The renderer's tint ignores its alpha, so the icon fades by opacity.
    let chip = container(icon(CHECK, ui::accent_ink(fill), CHECK_ICON).opacity(alpha))
        .width(CHECK_CHIP)
        .height(CHECK_CHIP)
        .center(CHECK_CHIP)
        .style(move |_| {
            container::Style::default()
                .background(fill.scale_alpha(alpha))
                .border(border::rounded(CHECK_CHIP / 2.0))
        });
    container(chip)
        .center_x(THUMB_WIDTH)
        .center_y(THUMB_HEIGHT)
        .into()
}

#[cfg(test)]
mod tests;
