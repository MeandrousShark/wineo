//! The library's list layout: one row per game with a thumbnail and
//! columns, the header that sorts by them, and which columns fit.

use iced::widget::{Space, button, column, container, image, row, stack, svg, text};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Length, Padding, border, font};
use wineo_core::library::SortKey;

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
        let dimmed = card.not_installed();
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
            } else if hovered {
                Star::Outline
            } else {
                Star::None
            },
            eye: card.hidden,
            steam: card.kind == Some(CardKind::Proxied),
            install: card.install_progress(),
            faded: dimmed || card.hidden,
            gray: dimmed,
            selected: self.selected.as_ref() == Some(&card.name),
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
        let semibold = Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        };
        let mut title = column![
            text(facts.title)
                .size(TITLE_SIZE)
                .font(semibold)
                .color(if facts.faded {
                    palette.muted
                } else {
                    palette.foreground
                })
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2);
        if let Some(identifier) = facts.identifier {
            title = title.push(
                text(identifier)
                    .size(IDENTIFIER_SIZE)
                    .color(palette.muted)
                    .wrapping(text::Wrapping::None),
            );
        }
        let mut marks = row![].spacing(4).align_y(Center);
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
        marks = marks.push(star(&card.name, facts.star, palette));
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
            text(facts.state)
                .size(CELL_SIZE)
                .color(palette.foreground)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(8)
        .align_y(Center);
        let mut cells = row![title_cell, container(state).width(STATE_WIDTH).clip(true)]
            .spacing(CELL_GAP)
            .align_y(Center);
        for column in columns(width) {
            let value = match column {
                Column::Playtime => facts.playtime.clone(),
                Column::LastPlayed => facts.last_played.clone(),
                Column::Source => facts.source.to_owned(),
                Column::Tags => facts.tags.clone(),
            };
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
        } else if facts.hovered {
            Some(palette.surface)
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
        let semibold = Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        };
        let active = sort.is_some_and(|sort| sort == self.query.sort);
        let mut content = row![
            text(label)
                .size(HEADER_SIZE)
                .font(semibold)
                .color(if active {
                    palette.foreground
                } else {
                    palette.muted
                })
                .wrapping(text::Wrapping::None),
        ]
        .spacing(4)
        .align_y(Center);
        if active {
            let arrow = if self.query.descending {
                ARROW_DOWN
            } else {
                ARROW_UP
            };
            content = content.push(icon(arrow, palette.foreground, 12.0));
        }
        let Some(sort) = sort else {
            return container(content).width(width).clip(true).into();
        };
        let cell = button(content)
            .padding(0)
            .on_press(Message::Sort(crate::sort_control::Event::Header(sort)))
            .style(move |_, status| {
                let mut style = button::Style::default();
                if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    style.text_color = palette.foreground;
                }
                style
            });
        container(cell).width(width).clip(true).into()
    }
}

/// The row's star: a favorite's filled one, or the outline while hovered,
/// pressed the way the poster's is. Its room stays with none showing.
fn star<'a>(name: &str, look: Star, palette: Palette) -> Element<'a, Message> {
    let (bytes, color) = match look {
        Star::None => return Space::new().width(STAR_BOX).height(STAR_BOX).into(),
        Star::Outline => (STAR_OUTLINE, palette.muted),
        Star::Filled => (STAR, palette.primary),
    };
    button(container(icon(bytes, color, MARK_ICON)).center(STAR_BOX))
        .width(STAR_BOX)
        .height(STAR_BOX)
        .padding(0)
        .on_press(Message::Mark(crate::library_actions::Event::PosterStar(
            name.to_owned(),
        )))
        .style(|_, _| button::Style::default())
        .into()
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
