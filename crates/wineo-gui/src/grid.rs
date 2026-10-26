use iced::advanced::text::Paragraph as _;
use iced::widget::{
    Space, button, column, container, image, mouse_area, row, scrollable, stack, svg, text,
    text_input,
};
use iced::{
    Center, Color, ContentFit, Element, Fill, FillPortion, Font, Size, border, font, mouse,
};

use crate::grid_motion::{CardText, GridLayout};
use crate::library::Card;
use crate::library_actions::{Event, Flag};
use crate::theme::Palette;
use crate::ui::{self, ScrollStyle, secondary_button};
use crate::wheel;
use crate::{App, Message, SCROLL_ID, SEARCH_ID, WHEEL_STEP_SCALE};

pub(crate) const ACTION_MENU_WIDTH: f32 = 248.0;
const ACTION_MENU_MAX_HEIGHT: f32 = 600.0;
pub(crate) const ACTION_MENU_MARGIN: f32 = 8.0;
pub(crate) const ACTION_MENU_PADDING: f32 = 12.0;
pub(crate) const ACTION_MENU_SPACING: f32 = 4.0;
pub(crate) const ACTION_MENU_HEADER_HEIGHT: f32 = 20.0;
const INSTALL_BAR_HEIGHT: f32 = 4.0;
const CARD_TITLE_SIZE: f32 = 14.0;
const CARD_SPACING: f32 = 4.0;
/// The identifier line and the state and playtime row.
const CARD_CONTEXT_SIZE: f32 = 12.0;
const CARD_DOT: f32 = 8.0;
const CARD_META_SPACING: f32 = 8.0;
const GOG_PILL_SIZE: f32 = 11.0;
/// The poster's corner layers sit this far from the art's edges.
const CORNER_PADDING: f32 = 8.0;
/// Room the top-left corner leaves for the marks' column on the right:
/// its margin, a mark, and the gap the chip and pill keep.
const MARKS_ROOM: f32 = CORNER_PADDING + crate::library_actions::MARK_SIZE + 4.0;
const GOG_PILL_PADDING: f32 = 2.0;
const GOG_PILL_SIDE: f32 = 8.0;
const GOG_BADGE_ICON: f32 = 14.0;
/// The poster-only title's margin from the art's edges.
const OVERLAY_PADDING: f32 = 8.0;
/// How far the overlay's gradient fades in above the title.
const OVERLAY_FADE: f32 = 24.0;
/// The gradient's darkness where the title starts and at the bottom edge.
const OVERLAY_SHADE: f32 = 0.75;
const OVERLAY_SHADE_BOTTOM: f32 = 0.85;
/// A hidden or uninstalled card's overlay title: white at this opacity.
const OVERLAY_MUTED: f32 = 0.6;
/// Room the title leaves on the right for the Steam mark: the mark and its
/// margin, and a small gap.
const STEAM_MARK_ROOM: f32 = 8.0 + 20.0 + 4.0;
pub(crate) const ADD_ID: &str = "library-add";
/// The search box's width, which a narrow window shrinks down to the minimum.
pub(crate) const SEARCH_WIDTH: f32 = 320.0;
pub(crate) const SEARCH_MIN_WIDTH: f32 = 160.0;

/// The identifier shows as muted context only when a title replaces it.
fn shows_identifier(card: &Card) -> bool {
    card.name != card.display_title
}

/// A card's laid out height, from the same sizes `App::card` builds it with,
/// so lazy art loading can follow the grid's real row geometry.
pub(crate) fn card_height(card: &Card, layout: GridLayout) -> f32 {
    let line = context_line();
    let text = layout.text();
    if text == CardText::PosterOnly {
        return layout.poster_height();
    }
    // Summed in the column's order, so the float rounding matches too.
    let mut height =
        layout.poster_height() + CARD_SPACING + crate::tile_title::two_line_height(CARD_TITLE_SIZE);
    if text == CardText::Title {
        return height;
    }
    if shows_identifier(card) {
        height += CARD_SPACING;
        height += line;
    }
    height += CARD_SPACING;
    height + meta_height(card, layout)
}

/// The identifier line's height.
pub(crate) fn context_line() -> f32 {
    iced::widget::text::LineHeight::default()
        .to_absolute(CARD_CONTEXT_SIZE.into())
        .0
}

/// The state and playtime row's height. Its labels wrap when they outgrow
/// the card, and a row hands each child whatever width the ones before it
/// left, so this measures them in that order.
fn meta_height(card: &Card, layout: GridLayout) -> f32 {
    let mut available = layout.card_width() - 2.0 * CARD_META_SPACING - CARD_DOT;
    let mut height = CARD_DOT;
    for label in [card.state_word(), card.playtime()] {
        let size =
            crate::tile_title::wrapped_size(label, available, CARD_CONTEXT_SIZE, Font::DEFAULT);
        available -= size.width;
        height = height.max(size.height);
    }
    height
}

/// Where a card's identifier and state row are built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Details {
    /// In the card's own column, for tests that look at a card whole. The
    /// grid draws every card's details apart.
    #[cfg(test)]
    Inline,
    /// Apart, for the grid to draw in a band under the card's title. The
    /// card keeps their room.
    Apart,
}

/// The identifier, when shown, and the state and playtime row, at the
/// card's width. The row wraps unless `wrap` is off.
fn card_details<'a>(
    card: &'a Card,
    layout: GridLayout,
    palette: Palette,
    caption: f32,
    wrap: bool,
) -> Vec<Element<'a, Message>> {
    let wrapping = if wrap {
        iced::widget::text::Wrapping::default()
    } else {
        iced::widget::text::Wrapping::None
    };
    let dot_color = card.dot(palette).scale_alpha(caption);
    let meta = row![
        container(Space::new())
            .width(CARD_DOT)
            .height(CARD_DOT)
            .style(move |_| container::Style::default()
                .background(dot_color)
                .border(border::rounded(4))),
        text(card.state_word())
            .size(CARD_CONTEXT_SIZE)
            .color(palette.muted.scale_alpha(caption))
            .wrapping(wrapping),
        text(card.playtime())
            .size(CARD_CONTEXT_SIZE)
            .color(palette.muted.scale_alpha(caption))
            .wrapping(wrapping),
    ]
    .spacing(CARD_META_SPACING)
    .align_y(Center);
    let mut details = Vec::with_capacity(2);
    if shows_identifier(card) {
        details.push(
            text(truncate_name(&card.name, layout.card_width()))
                .size(CARD_CONTEXT_SIZE)
                .color(palette.muted.scale_alpha(caption))
                .wrapping(iced::widget::text::Wrapping::None)
                .into(),
        );
    }
    details.push(meta.into());
    details
}

impl App {
    pub(crate) fn grid(&self) -> Element<'_, Message> {
        let layout = self.grid_layout();
        let progress = self.details_progress();
        let filtered = self.filtered();
        let scene = (!self.collapse.idle()).then(|| crate::grid_motion::Scene {
            collapse: &self.collapse,
            posters: filtered
                .iter()
                .enumerate()
                .map(|(index, card)| {
                    self.collapse
                        .fell(index)
                        .then(|| self.loose_poster(card))
                        .flatten()
                })
                .collect(),
            on_press: self.collapse.restorable().then_some(Message::Rebuild),
        });
        let cards = filtered
            .into_iter()
            .map(|card| {
                let opacity = self.grid_motion.opacity(&card.name);
                crate::grid_motion::GridCard {
                    name: card.name.clone(),
                    card: self.card_drawn(card, opacity, layout, Details::Apart),
                    details: self.details_apart(card, opacity, layout, progress),
                }
            })
            .collect();
        // Only the leaving cards near the view are worth building every
        // frame; the held height already covers the rest.
        let (top, bottom) = (
            self.scroll_offset - self.viewport_height,
            self.scroll_offset + 2.0 * self.viewport_height,
        );
        let by_name: std::collections::HashMap<&str, &Card> =
            if self.grid_motion.leaving().is_empty() {
                Default::default()
            } else {
                self.cards
                    .iter()
                    .map(|card| (card.name.as_str(), card))
                    .collect()
            };
        let leaving = self
            .grid_motion
            .leaving()
            .iter()
            .filter(|leaving| {
                leaving.bounds.y <= bottom && leaving.bounds.y + leaving.bounds.height >= top
            })
            .filter_map(|leaving| {
                let card = by_name.get(leaving.name.as_str())?;
                Some(crate::grid_motion::Outgoing {
                    name: leaving.name.clone(),
                    bounds: leaving.bounds,
                    card: self.card_drawn(card, leaving.opacity, leaving.layout, Details::Apart),
                    details: self.details_apart(card, leaving.opacity, leaving.layout, progress),
                })
            })
            .collect();
        crate::grid_motion::grid(
            cards,
            leaving,
            &self.grid_motion,
            self.panel_open,
            scene,
            self.grid_layout(),
            crate::details_fade::DetailsFade::drift(progress),
        )
    }
    /// The details the grid draws under `card`'s title at the details
    /// `progress`: with full text, and on the way out until they've faded.
    fn details_apart<'a>(
        &'a self,
        card: &'a Card,
        opacity: f32,
        layout: GridLayout,
        progress: f32,
    ) -> Option<crate::grid_motion::CardDetails<'a, Message>> {
        match layout.text() {
            CardText::Full => {}
            CardText::Title if progress > 0.0 => {}
            _ => return None,
        }
        let alpha = self.collapse.alpha() * opacity;
        // Apart from the title, which keeps the card's own strength.
        let caption = self.collapse.caption_alpha() * opacity * progress;
        // One line while fading, since the row is built at each frame's
        // zoom and would rewrap mid-crossing; the band clips it at the
        // card's edge. The card's spacer keeps the wrapped height.
        let wrap = !self.details_fade.active();
        let details = card_details(card, layout, self.palette.faded(alpha), caption, wrap);
        Some(crate::grid_motion::CardDetails {
            element: iced::widget::Column::with_children(details)
                .spacing(CARD_SPACING)
                .width(layout.card_width())
                .into(),
            top: layout.poster_height()
                + CARD_SPACING
                + crate::tile_title::two_line_height(CARD_TITLE_SIZE)
                + CARD_SPACING,
        })
    }
    /// The poster a collapsed card shows on its own, or a plain surface
    /// when it has no art.
    fn loose_poster(&self, card: &Card) -> Option<(iced::widget::image::Handle, f32)> {
        let placeholder = self.collapse.placeholder()?;
        let dimmed = card.not_installed();
        let art = card.art.as_ref().and_then(|art| {
            self.images
                .shown(&(art.clone(), crate::artwork::Kind::Grid), dimmed)
        });
        let opacity = if dimmed {
            crate::panel::DIMMED_OPACITY
        } else {
            1.0
        };
        Some((art.unwrap_or(placeholder).clone(), opacity))
    }
    #[cfg(test)]
    pub(crate) fn card<'a>(&'a self, card: &'a Card) -> Element<'a, Message> {
        self.card_drawn(card, 1.0, self.grid_layout(), Details::Inline)
    }

    /// A card drawn at `opacity` and `layout`, as it fades in or out of the
    /// grid.
    pub(crate) fn card_drawn<'a>(
        &'a self,
        card: &'a Card,
        opacity: f32,
        layout: GridLayout,
        details: Details,
    ) -> Element<'a, Message> {
        let alpha = self.collapse.alpha() * opacity;
        let caption = self.collapse.caption_alpha() * opacity;
        let palette = self.palette.faded(alpha);
        let hovered = self.hovered.as_ref() == Some(&card.name);
        let selected = self.selected.as_ref() == Some(&card.name);
        let (star, shade) = self.hover_fades.progress(&card.name, hovered, selected);
        let glow = self
            .hover_fades
            .glow(&card.name, self.glowing_star() == Some(card.name.as_str()));
        let dimmed = card.not_installed();
        let art = card.art.as_ref().and_then(|art| {
            self.images
                .shown(&(art.clone(), crate::artwork::Kind::Grid), dimmed)
        });
        // A hidden card fades without the grayscale, which stays the sign of
        // a game that isn't installed.
        let faded = dimmed || card.hidden;
        let text_mode = layout.text();
        // In poster-only, art that's still loading or failed to decode
        // leaves its tile blank and shows the title over it like the art
        // will, so the title doesn't vanish when the art arrives.
        let tile_title = text_mode != CardText::PosterOnly || card.art.is_none();
        let overlay = text_mode == CardText::PosterOnly && card.art.is_some() && shade > 0.0;
        let poster: Element<'_, Message> = match art {
            Some(handle) => image(handle.clone())
                .width(Fill)
                .height(Fill)
                .content_fit(ContentFit::Cover)
                .border_radius(4)
                .opacity(
                    alpha
                        * if faded {
                            crate::panel::DIMMED_OPACITY
                        } else if hovered {
                            0.85_f32
                        } else {
                            1.0_f32
                        },
                )
                .into(),
            None => container(if tile_title {
                Element::from(
                    text(&card.display_title)
                        .size(layout.tile_title_size())
                        .color(palette.muted)
                        .center(),
                )
            } else {
                Space::new().into()
            })
            .width(Fill)
            .height(Fill)
            .padding(12)
            .center(Fill)
            .style(move |_| {
                container::Style::default().background(palette.surface.scale_alpha(if faded {
                    crate::panel::DIMMED_OPACITY
                } else if hovered {
                    0.85
                } else {
                    1.0
                }))
            })
            .into(),
        };
        let title_font = Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        };
        let steam_mark = card.kind == Some(crate::library::CardKind::Proxied);
        let mut layers = stack![poster];
        if overlay {
            layers = layers.push(title_overlay(
                &card.display_title,
                layout,
                steam_mark,
                // Muted when faded, like the title under the poster, but in
                // white so it reads on the dark gradient in both themes.
                Color::WHITE.scale_alpha(alpha * shade * if faded { OVERLAY_MUTED } else { 1.0 }),
                alpha * shade,
            ));
        }
        if steam_mark {
            let mark = svg(svg::Handle::from_memory(include_bytes!(
                "../assets/steam.svg"
            )))
            .width(20)
            .height(20)
            .opacity(
                alpha
                    * if faded {
                        crate::panel::DIMMED_OPACITY
                    } else {
                        1.0_f32
                    },
            );
            layers = layers.push(
                container(mark)
                    .width(Fill)
                    .height(Fill)
                    .align_right(Fill)
                    .align_bottom(Fill)
                    .padding(8),
            );
        }
        let chip = (text_mode.chip() && !card.idle())
            .then(|| crate::library_actions::status_chip(card.dot(palette), alpha));
        // The marks draw above the corner, so it keeps clear of their
        // column on every card, even with no mark showing, and nothing
        // shifts when the hover star appears. A pill squeezed by them wraps
        // between words and clips rather than spill under a mark.
        let room = layout.art_frame().width
            - CORNER_PADDING
            - MARKS_ROOM
            - if chip.is_some() {
                crate::library_actions::MARK_SIZE + 4.0
            } else {
                0.0
            };
        let gog = crate::gog_library::badge(card.gog.as_ref(), self.gog_updates.status(&card.name))
            .map(|badge| {
                let color = match badge {
                    crate::gog_library::Badge::UpdateAvailable => palette.primary,
                    crate::gog_library::Badge::FinishUpdate
                    | crate::gog_library::Badge::FinishInstall => palette.warning,
                };
                let ink = ui::accent_ink(color);
                if !pill_fits(badge.label(), room) {
                    return Gog::Badge(gog_badge(color, ink, alpha));
                }
                Gog::Pill(
                    container(
                        text(badge.label())
                            .size(GOG_PILL_SIZE)
                            .color(ink.scale_alpha(alpha)),
                    )
                    .padding([GOG_PILL_PADDING, GOG_PILL_SIDE])
                    .clip(true)
                    .style(move |_| {
                        container::Style::default()
                            .background(color)
                            .border(border::rounded(8))
                    })
                    .into(),
                )
            });
        // Below about 63% three 24 px circles and their margins outgrow the
        // art, so a badge may give up its gap to the marks, never more.
        let right = match &gog {
            Some(Gog::Badge(_)) => CORNER_PADDING + crate::library_actions::MARK_SIZE,
            _ => MARKS_ROOM,
        };
        let corner: Option<Element<'_, Message>> = match (chip, gog) {
            (Some(chip), Some(Gog::Pill(pill))) => {
                // The chip keeps the corner, mirroring the marks, and a
                // one-line pill centers on it. On a small poster the pill
                // wraps, growing down instead of pushing the chip.
                let line = iced::widget::text::LineHeight::default()
                    .to_absolute(GOG_PILL_SIZE.into())
                    .0
                    + 2.0 * GOG_PILL_PADDING;
                let pill = container(pill).padding(iced::Padding {
                    top: ((crate::library_actions::MARK_SIZE - line) / 2.0).max(0.0),
                    ..iced::Padding::ZERO
                });
                Some(row![chip, pill].spacing(4).into())
            }
            (Some(chip), Some(Gog::Badge(badge))) => Some(row![chip, badge].spacing(4).into()),
            (Some(chip), None) => Some(chip),
            (None, Some(Gog::Pill(gog) | Gog::Badge(gog))) => Some(gog),
            (None, None) => None,
        };
        if let Some(corner) = corner {
            layers = layers.push(
                container(corner)
                    .width(Fill)
                    .height(Fill)
                    .align_left(Fill)
                    .align_top(Fill)
                    .padding(iced::Padding {
                        right,
                        ..iced::Padding::new(CORNER_PADDING)
                    }),
            );
        }
        let pop = self.star_pops.progress(&card.name);
        if let Some(t) = pop {
            layers = layers.push(crate::library_actions::star_ring(
                t,
                self.palette.primary,
                alpha,
            ));
        }
        let look = crate::library_actions::StarLook {
            fade: star,
            glow,
            primary: self.palette.primary,
            lift: pop.map_or(0.0, crate::star_pop::lift),
        };
        if let Some(marks) = crate::library_actions::poster_marks_faded(card, alpha, hovered, look)
        {
            layers = layers.push(marks);
        }
        if let Some(percent) = card.install_progress() {
            layers = layers.push(
                container(install_bar(percent, palette, alpha))
                    .width(Fill)
                    .height(Fill)
                    .align_bottom(Fill),
            );
        }
        let framed = container(container(layers).clip(true))
            .width(layout.card_width())
            .height(layout.poster_height())
            .padding(2)
            .style(move |_| {
                container::Style::default().border(border::rounded(6).width(2).color(if selected {
                    palette.primary
                } else {
                    Color::TRANSPARENT
                }))
            });
        if text_mode == CardText::PosterOnly {
            return card_area(column![framed].width(layout.card_width()), card);
        }
        let label = crate::tile_title::lines(
            &card.display_title,
            layout.card_width(),
            CARD_TITLE_SIZE,
            title_font,
        )
        .join("\n");
        let mut content = column![
            framed,
            text(label)
                .size(CARD_TITLE_SIZE)
                .height(crate::tile_title::two_line_height(CARD_TITLE_SIZE))
                .font(title_font)
                .color(
                    if faded {
                        palette.muted
                    } else {
                        palette.foreground
                    }
                    .scale_alpha(caption)
                )
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(CARD_SPACING)
        .width(layout.card_width());
        if text_mode == CardText::Title {
            return card_area(content, card);
        }
        match details {
            #[cfg(test)]
            Details::Inline => {
                for child in card_details(card, layout, palette, caption, true) {
                    content = content.push(child);
                }
            }
            // A spacer for each row, summed in `card_height`'s order.
            Details::Apart => {
                if shows_identifier(card) {
                    content = content.push(Space::new().height(context_line()));
                }
                content = content.push(Space::new().height(meta_height(card, layout)));
            }
        }
        card_area(content, card)
    }
    /// How wide the Add, import, and clone dialog is in the current window.
    pub(crate) fn flow_width(&self) -> f32 {
        let widest = if matches!(self.flow, Some(crate::flow::Page::Choose(_))) {
            680.0
        } else {
            900.0
        };
        (self.window_size.width - 40.0).clamp(280.0, widest)
    }
    pub(crate) fn view(&self) -> Element<'_, Message> {
        let content = crate::shortcuts::capture_with_undo(
            self.sidebar_view(),
            Message::Section,
            (self.toast.is_some()
                && self.removal.is_none()
                && !self.trash_confirm_open()
                && !self.downloads_confirm_open()
                && self.hook_stop_confirm.is_none()
                && self.flow.is_none()
                && self.settings.is_none()
                && self.art_page.is_none()
                && !self.close_prompt)
                .then_some(Message::Undo),
            self.zoom_allowed()
                .then_some(|key| Message::Zoom(crate::zoom::Event::Key(key))),
        );
        crate::menu_anchor::track(content, Message::MenuAnchor)
    }

    fn sidebar_view(&self) -> Element<'_, Message> {
        // Keep the page at a stable tree path so overlays preserve scroll and focus state.
        let mut page = stack![
            row![self.sidebar(), self.page_view()]
                .width(Fill)
                .height(Fill),
            // Always a layer, so the ones above keep their places.
            self.filter_tab_view(),
        ];
        if let Some((message, _)) = &self.feature_notice {
            let palette = self.palette;
            page = page.push(
                container(
                    container(text(message).color(palette.foreground))
                        .padding(12)
                        .style(move |_| {
                            container::Style::default()
                                .background(palette.surface)
                                .border(border::rounded(8).width(1).color(palette.border))
                        }),
                )
                .padding(16)
                .align_bottom(Fill)
                .center_x(Fill),
            );
        }
        if self.toast.is_some() {
            page = page.push(
                container(self.toast_view())
                    .padding(16)
                    .align_bottom(Fill)
                    .center_x(Fill),
            );
        }
        if self.removal.is_some() {
            page = page.push(iced::widget::opaque(
                mouse_area(container(self.remove_view()).center(Fill).style(|_| {
                    container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                }))
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.hook_stop_confirm.is_some() {
            page = page.push(iced::widget::opaque(
                mouse_area(container(self.hook_stop_view()).center(Fill).style(|_| {
                    container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                }))
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.trash_confirm_open() {
            page = page.push(iced::widget::opaque(
                mouse_area(
                    container(self.trash_confirm_view())
                        .center(Fill)
                        .style(|_| {
                            container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                        }),
                )
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.downloads_confirm_open() {
            page = page.push(iced::widget::opaque(
                mouse_area(
                    container(self.downloads_confirm_view())
                        .center(Fill)
                        .style(|_| {
                            container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                        }),
                )
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if let Some(flow) = &self.flow {
            let palette = self.palette;
            let generation = self.flow_generation;
            // Fits all four Add choices; smaller windows scroll them.
            let height = if matches!(flow, crate::flow::Page::Choose(_)) {
                560.0
            } else {
                720.0
            };
            let width = self.flow_width();
            let dialog = container(
                flow.view(palette, width, self.steamgriddb_available)
                    .map(move |event| Message::Flow(generation, event)),
            )
            .width(width)
            .height((self.window_size.height - 40.0).clamp(240.0, height))
            .clip(true)
            .style(move |_| {
                container::Style::default()
                    .background(palette.background)
                    .border(border::rounded(12).width(1).color(palette.border))
            });
            page = page.push(iced::widget::opaque(
                mouse_area(container(dialog).center(Fill).style(|_| {
                    container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                }))
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.close_prompt && self.close_quietly {
            return page
                .push(iced::widget::opaque(
                    mouse_area(Space::new().width(Fill).height(Fill))
                        .on_scroll(|_| Message::OverlayInput),
                ))
                .into();
        }
        if self.close_prompt {
            let message = if self.closing {
                if self.feature_finishing() {
                    "Finishing changes..."
                } else {
                    "Canceling and waiting for cleanup..."
                }
            } else {
                "An operation is still running. Cancel it and close wineo?"
            };
            let controls = if self.closing
                && (self.change.is_some() || self.art_busy() || self.filesystem_busy())
                && !self.checkup_force_available()
            {
                row![text("Waiting for the change to finish...")]
            } else if self.closing
                && (self
                    .tool_pages
                    .values()
                    .any(crate::tools_page::Page::force_available)
                    || self.operation.is_some()
                    || self.flow.as_ref().is_some_and(crate::flow::Page::busy))
            {
                row![secondary_button("Force stop", self.palette).on_press(Message::ForceCloseWork)]
            } else if self.closing {
                row![text("Waiting for cleanup...")]
            } else {
                row![
                    secondary_button("Keep working", self.palette).on_press(Message::KeepWorking),
                    secondary_button("Cancel and close", self.palette)
                        .on_press(Message::CancelAndClose),
                ]
            }
            .spacing(12);
            let palette = self.palette;
            let prompt = container(column![text(message), controls].spacing(16))
                .padding(24)
                .style(move |_| {
                    container::Style::default()
                        .background(palette.surface)
                        .border(border::rounded(8).width(1).color(palette.border))
                });
            return page
                .push(iced::widget::opaque(
                    container(prompt)
                        .center(Fill)
                        .width(Fill)
                        .height(Fill)
                        .style(|_| {
                            container::Style::default().background(Color::BLACK.scale_alpha(0.6))
                        }),
                ))
                .into();
        }
        if self.sort_menu_shown() {
            let [backdrop, menu] = self.sort_menu_view();
            return page.push(backdrop).push(menu).into();
        }
        if self.filter_menu_shown() {
            let [backdrop, menu] = self.filter_menu_view();
            return page.push(backdrop).push(menu).into();
        }
        if let Some(name) = &self.action_menu {
            let palette = self.palette;
            let wine_actions = self.wine_actions_with(&self.facts, name);
            let transfer_actions = self.transfer_actions(name);
            let uninstall_action = self.uninstall_action(name);
            let update_action = self.gog_update_action(name);
            let version_action = self
                .cards
                .iter()
                .any(|card| card.name == *name && crate::version_picker::has_version(card));
            let removal_reason = self.remove_reason_with(&self.facts, name);
            let card = self.cards.iter().find(|card| card.name == *name);
            let favorite = card.is_some_and(|card| card.favorite);
            let hidden = card.is_some_and(|card| card.hidden);
            let menu_buttons = 6
                + usize::from(wine_actions) * 3
                + usize::from(transfer_actions) * 2
                + usize::from(uninstall_action)
                + usize::from(update_action)
                + usize::from(version_action);
            let menu_dividers = 2 + usize::from(wine_actions);
            let body_content_height =
                action_menu_body_height(menu_buttons, menu_dividers, removal_reason.as_deref());
            let menu_height = action_menu_height(self.window_size.height, body_content_height);
            let position = menu_position(self.action_menu_position, self.window_size, menu_height);
            let mut scroll_items = column![
                context_menu_button(
                    Flag::Favorite.action_label(favorite),
                    palette,
                    false,
                    Some(Message::Mark(Event::Toggle(name.clone(), Flag::Favorite))),
                ),
                context_menu_button(
                    Flag::Hidden.action_label(hidden),
                    palette,
                    false,
                    Some(Message::Mark(Event::Toggle(name.clone(), Flag::Hidden))),
                ),
                ui::separator(palette).padding([3, 0]),
                context_menu_button(
                    "Settings",
                    palette,
                    false,
                    Some(Message::ContextSettings(name.clone())),
                ),
                context_menu_button(
                    "Art...",
                    palette,
                    false,
                    Some(Message::ContextArt(name.clone())),
                ),
            ]
            .spacing(ACTION_MENU_SPACING);
            if update_action {
                let enabled = self
                    .cards
                    .iter()
                    .find(|card| &card.name == name)
                    .is_some_and(|card| self.gog_update_enabled_with(&self.facts, card));
                scroll_items = scroll_items.push(context_menu_button(
                    "Update",
                    palette,
                    false,
                    enabled.then(|| Message::GogUpdate(name.clone())),
                ));
            }
            if version_action {
                scroll_items = scroll_items.push(context_menu_button(
                    "Version...",
                    palette,
                    false,
                    Some(Message::Version(crate::version_picker::Event::Open(
                        name.clone(),
                    ))),
                ));
            }
            if uninstall_action {
                // Steam's own dialog asks before anything is removed.
                scroll_items = scroll_items.push(context_menu_button(
                    "Uninstall",
                    palette,
                    false,
                    self.steam_request
                        .is_none()
                        .then(|| Message::Uninstall(name.clone())),
                ));
            }
            if wine_actions {
                scroll_items = scroll_items.push(context_menu_button(
                    "Snapshots...",
                    palette,
                    false,
                    Some(Message::SnapshotsEnvironment(name.clone())),
                ));
            }
            if transfer_actions {
                scroll_items = scroll_items.push(context_menu_button(
                    "Copy to portable folder...",
                    palette,
                    false,
                    Some(Message::CopyPortable(name.clone())),
                ));
            }
            if wine_actions {
                scroll_items = scroll_items
                    .push(ui::separator(palette).padding([3, 0]))
                    .push(context_menu_button(
                        "Run program...",
                        palette,
                        false,
                        Some(Message::OpenTool(
                            name.clone(),
                            crate::tools_page::Kind::Exec,
                        )),
                    ))
                    .push(context_menu_button(
                        "Winetricks...",
                        palette,
                        false,
                        Some(Message::OpenTool(
                            name.clone(),
                            crate::tools_page::Kind::Winetricks,
                        )),
                    ));
            }
            scroll_items = scroll_items
                .push(context_menu_button(
                    "Check for problems",
                    palette,
                    false,
                    Some(Message::Checkup(crate::checkup::Event::Open(
                        crate::checkup::Scope::Environment(name.clone()),
                    ))),
                ))
                .push(ui::separator(palette).padding([3, 0]));
            if transfer_actions {
                scroll_items = scroll_items.push(context_menu_button(
                    "Clone",
                    palette,
                    false,
                    (!self.management_busy() && removal_reason.is_none())
                        .then(|| Message::CloneEnvironment(name.clone())),
                ));
            }
            scroll_items = scroll_items.push(context_menu_button(
                "Remove...",
                palette,
                true,
                removal_reason
                    .is_none()
                    .then(|| Message::Remove(name.clone())),
            ));
            if let Some(reason) = removal_reason {
                scroll_items = scroll_items.push(
                    text(reason)
                        .size(12)
                        .width(Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .color(palette.muted),
                );
            }
            let menu_items = column![
                text(truncate_name(name, 224.0))
                    .size(14)
                    .font(Font {
                        weight: font::Weight::Semibold,
                        ..Font::DEFAULT
                    })
                    .color(palette.foreground)
                    .height(ACTION_MENU_HEADER_HEIGHT)
                    .align_y(Center)
                    .wrapping(iced::widget::text::Wrapping::None),
                scrollable(scroll_items)
                    .wineo_style(palette)
                    .height(action_menu_scroll_height(menu_height)),
                context_menu_button("Close menu", palette, false, Some(Message::CloseMenu)),
            ]
            .spacing(ACTION_MENU_SPACING)
            .height(Fill);
            let menu = container(menu_items)
                .padding(ACTION_MENU_PADDING)
                .width(ACTION_MENU_WIDTH)
                .height(menu_height)
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
            return page
                .push(
                    mouse_area(Space::new().width(Fill).height(Fill))
                        .on_press(Message::CloseMenu)
                        .on_right_press(Message::CloseMenu),
                )
                .push(
                    container(menu)
                        .padding(iced::Padding {
                            top: position.y,
                            left: position.x,
                            right: 0.0,
                            bottom: 0.0,
                        })
                        .width(Fill)
                        .height(Fill),
                )
                .into();
        }
        page.into()
    }

    fn page_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        if let Some((name, kind)) = &self.tool_page
            && let Some(page) = self.tool_pages.get(&(name.clone(), *kind))
        {
            let name = name.clone();
            let kind = *kind;
            let reason = if page.busy() {
                None
            } else {
                self.tool_start_reason_with(&self.facts, &name, kind)
            };
            let blocked = reason.is_some();
            let content: Element<'_, crate::tools_page::Event> = column![
                reason.map(|reason| text(reason).color(palette.warning)),
                page.view(palette, blocked),
            ]
            .into();
            return content.map(move |event| Message::Tool(name.clone(), kind, event));
        }
        if let Some(name) = &self.portable_page
            && let Some(page) = self.portable_pages.get(name)
        {
            let name = name.clone();
            return page
                .view(
                    palette,
                    self.management_busy() || self.remove_reason_with(&self.facts, &name).is_some(),
                )
                .map(move |event| Message::Portable(name.clone(), event));
        }
        if self.section == crate::sidebar::Section::Snapshots
            && let Some(page) = &self.snapshots_page
        {
            let blocked = self.management_busy()
                || page.selected_name().is_some_and(|name| {
                    self.prefix_change_reason_with(&self.facts, name).is_some()
                });
            return page.view(palette, blocked).map(Message::Snapshots);
        }
        if let Some(name) = &self.art_page
            && let Some(page) = self.art_pages.get(name)
        {
            let name = name.clone();
            let generation = page.generation;
            return page
                .view_with_key_availability(palette, self.steamgriddb_available)
                .map(move |event| Message::ArtPage(name.clone(), generation, event));
        }
        if let Some(page) = &self.settings {
            return page.view(&self.paths, palette);
        }
        let mut page = column![
            self.top_bar(),
            container(Space::new())
                .height(1)
                .style(move |_| container::Style::default().background(palette.border))
        ];
        if let Some(change) = &self.change {
            page = page.push(
                container(text(change.reason()).size(12).color(palette.muted)).padding([4, 16]),
            );
        }
        if self.section == crate::sidebar::Section::Downloads {
            return container(page.push(self.downloads_view()))
                .width(Fill)
                .height(Fill)
                .style(move |_| container::Style::default().background(palette.background))
                .into();
        }
        if self.section == crate::sidebar::Section::Trash {
            return container(page.push(self.trash_view()))
                .width(Fill)
                .height(Fill)
                .style(move |_| container::Style::default().background(palette.background))
                .into();
        }
        if self.section == crate::sidebar::Section::Checkup {
            return container(
                page.push(
                    container(
                        column![
                            crate::ui::page_title(self.section.label(), palette),
                            self.checkup_page(),
                        ]
                        .spacing(12),
                    )
                    .padding(24),
                ),
            )
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background))
            .into();
        }
        if let Some(banner) = self.library_banner() {
            page = page.push(banner);
        }
        if let Some(error) = &self.error {
            page = page.push(
                container(text(format!("Failed to load library: {error}")).color(palette.danger))
                    .padding(16),
            );
        }
        // Not `refreshing`, which flips every background refresh and made the
        // empty state blink.
        if self.cards.is_empty() && self.loaded {
            page = page.push(
                container(
                    column![
                        text("Your library is empty").size(22),
                        crate::ui::accent_control(
                            "Add something",
                            palette.primary,
                            !self.management_busy() && self.operation.is_none(),
                            Message::Add,
                        ),
                    ]
                    .spacing(12),
                )
                .padding(24),
            );
        }
        let visible = self.scroll_visible_until.is_some();
        let scroll = scrollable(self.grid())
            .id(iced::widget::Id::new(SCROLL_ID))
            .width(Fill)
            .height(Fill)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    .width(crate::ui::SCROLLBAR_WIDTH)
                    .scroller_width(crate::ui::SCROLLER_WIDTH),
            ))
            .on_scroll(Message::Scrolled)
            .style(move |theme, status| crate::ui::scroll_style(theme, status, palette, visible));
        let library = mouse_area(wheel::zoom(
            wheel::scale(scroll, WHEEL_STEP_SCALE),
            wheel::Zoom {
                on_zoom: self
                    .zoom_allowed()
                    .then_some(|delta| Message::Zoom(crate::zoom::Event::Wheel(delta))),
                on_input: self
                    .zooming
                    .anchoring()
                    .then_some(Message::Zoom(crate::zoom::Event::Interrupted)),
                modifiers: self.modifiers,
            },
        ))
        .on_exit(Message::Zoom(crate::zoom::Event::PointerLeft));
        let library = if self.panel_open {
            library.on_press(Message::ClosePanel)
        } else if self.collapse.faded() {
            library.on_press(Message::Rebuild)
        } else {
            library
        };
        let panel_layer: Element<'_, Message> = if self.panel_reserved() {
            let panel = self
                .selected
                .as_ref()
                .and_then(|name| self.cards.iter().find(|card| &card.name == name))
                .map(|card| self.panel(card))
                .unwrap_or_else(|| {
                    container(Space::new())
                        .width(crate::grid_motion::PANEL_WIDTH)
                        .height(Fill)
                        .into()
                });
            container(
                row![
                    iced::widget::opaque(
                        container(crate::panel_motion::presentation(
                            panel,
                            self.panel_motion.progress(),
                            self.panel_open && self.panel_motion.interactive(),
                            palette.surface,
                        ))
                        .width(crate::grid_motion::PANEL_WIDTH)
                        .height(Fill)
                        .style(move |_| container::Style::default().background(palette.surface)),
                    ),
                    Space::new().width(crate::grid_motion::RAIL_WIDTH),
                ]
                .width(crate::grid_motion::DOCK_WIDTH)
                .height(Fill),
            )
            .width(Fill)
            .height(Fill)
            .align_right(Fill)
            .into()
        } else {
            container(Space::new()).width(Fill).height(Fill).into()
        };
        // Always a layer, so the panel's place in the tree never moves.
        let content = stack![library, self.empty_state_view(), panel_layer];
        container(page.push(content))
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background))
            .into()
    }
    fn top_bar(&self) -> Element<'_, Message> {
        let palette = self.palette;
        if self.section != crate::sidebar::Section::Library {
            return container(
                row![text("wineo").size(18).color(palette.foreground)].align_y(Center),
            )
            .padding([8, 16])
            .width(Fill)
            .into();
        }
        let input = text_input("Search library...", &self.search)
            .id(iced::widget::Id::new(SEARCH_ID))
            .on_input(Message::SearchChanged)
            .width(Fill)
            .padding(iced::Padding::from([6, 10]).right(30))
            .style(move |_, status| text_input::Style {
                background: palette.surface.into(),
                border: border::rounded(crate::ui::CONTROL_RADIUS).width(1).color(
                    if matches!(status, text_input::Status::Focused { .. }) {
                        palette.primary
                    } else {
                        palette.border
                    },
                ),
                icon: palette.muted,
                placeholder: palette.muted,
                value: palette.foreground,
                selection: palette.primary.scale_alpha(0.4),
            });
        let input: Element<'_, Message> = if self.flow.is_some() {
            container(
                text(if self.search.is_empty() {
                    "Search library..."
                } else {
                    &self.search
                })
                .color(palette.muted),
            )
            .padding([6, 10])
            .width(Fill)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(
                        border::rounded(crate::ui::CONTROL_RADIUS)
                            .width(1)
                            .color(palette.border),
                    )
            })
            .into()
        } else {
            input.into()
        };
        // Keep the tree shape stable so clearing text does not drop input focus.
        let clear: Element<'_, Message> = if self.search.is_empty() {
            Space::new().into()
        } else {
            button(text("x").size(15).color(palette.muted))
                .on_press(Message::SearchChanged(String::new()))
                .padding(4)
                .style(|_, _| button::Style::default())
                .into()
        };
        let search = crate::fit_width::fit_width(
            stack![
                input,
                container(clear)
                    .width(Fill)
                    .height(Fill)
                    .align_right(Fill)
                    .center_y(Fill)
                    .padding([0, 6])
            ],
            SEARCH_MIN_WIDTH,
            SEARCH_WIDTH,
        );
        container(
            row![
                text("wineo")
                    .size(18)
                    .font(Font {
                        weight: font::Weight::Semibold,
                        ..Font::DEFAULT
                    })
                    .color(palette.foreground),
                {
                    let enabled = !self.management_busy() && self.operation.is_none();
                    crate::focus_button::control(
                        crate::ui::accent_control("Add", palette.primary, enabled, Message::Add)
                            .into(),
                        enabled,
                        ADD_ID,
                        Message::Add,
                    )
                },
                row![
                    search,
                    crate::sort_control::toolbar(self),
                    crate::filter_control::toolbar(self),
                    crate::zoom_control::toolbar(self),
                ]
                .spacing(8)
                .align_y(Center),
            ]
            .spacing(16)
            .align_y(Center),
        )
        .padding(iced::Padding {
            top: 8.0,
            bottom: crate::filter_tab::TOOLBAR_PADDING_BOTTOM,
            left: 16.0,
            right: 16.0,
        })
        .width(Fill)
        .into()
    }
}

/// A thin bar along the poster's bottom edge while Steam installs. It only
/// moves when the library reloads, so nothing animates while idle.
/// The GOG pill, or the badge that stands in for it on a poster too small
/// for its words.
enum Gog<'a> {
    Pill(Element<'a, Message>),
    Badge(Element<'a, Message>),
}

/// Whether the pill's longest word fits on one line in `room`, the width
/// the corner leaves it. A pill that fits wraps between its words at most.
fn pill_fits(label: &str, room: f32) -> bool {
    let longest = label
        .split_whitespace()
        .map(|word| {
            crate::tile_title::wrapped_size(word, f32::INFINITY, GOG_PILL_SIZE, Font::DEFAULT).width
        })
        .fold(0.0, f32::max);
    longest <= room - 2.0 * GOG_PILL_SIDE
}

/// The GOG pill as a round badge like the marks: its color, with a down
/// arrow in its ink. `color` carries the card's fade already.
fn gog_badge(color: Color, ink: Color, alpha: f32) -> Element<'static, Message> {
    let size = crate::library_actions::MARK_SIZE;
    // The renderer's tint ignores its alpha, so the arrow fades by opacity.
    container(
        crate::library_menu::icon(crate::sort_control::ARROW_DOWN, ink, GOG_BADGE_ICON)
            .opacity(alpha),
    )
    .width(size)
    .height(size)
    .center(size)
    .style(move |_| {
        container::Style::default()
            .background(color)
            .border(border::rounded(size / 2.0))
    })
    .into()
}

/// A card's content as one pointer target.
fn card_area<'a>(content: impl Into<Element<'a, Message>>, card: &Card) -> Element<'a, Message> {
    mouse_area(content)
        .on_press(Message::Select(card.name.clone()))
        .on_double_click(Message::DoubleClick(card.name.clone()))
        .on_right_press(Message::Context(card.name.clone()))
        .on_enter(Message::Hover(Some(card.name.clone())))
        .on_exit(Message::Hover(None))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// The poster-only title over the bottom of the art, on a gradient that's
/// dark in both themes so white text reads on any art. It ends above the
/// install bar's edge and clear of the Steam mark.
fn title_overlay(
    title: &str,
    layout: GridLayout,
    steam_mark: bool,
    ink: Color,
    alpha: f32,
) -> Element<'static, Message> {
    let font = Font {
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    };
    let right = if steam_mark {
        STEAM_MARK_ROOM
    } else {
        OVERLAY_PADDING
    };
    let width = (layout.art_frame().width - OVERLAY_PADDING - right).max(0.0);
    let lines = crate::tile_title::lines(title, width, CARD_TITLE_SIZE, font);
    let line = iced::widget::text::LineHeight::default()
        .to_absolute(CARD_TITLE_SIZE.into())
        .0;
    let height = OVERLAY_FADE + lines.len() as f32 * line + OVERLAY_PADDING;
    let shade = iced::gradient::Linear::new(iced::Radians::PI)
        .add_stop(0.0, Color::BLACK.scale_alpha(0.0))
        .add_stop(
            OVERLAY_FADE / height,
            Color::BLACK.scale_alpha(OVERLAY_SHADE),
        )
        .add_stop(1.0, Color::BLACK.scale_alpha(OVERLAY_SHADE_BOTTOM));
    let shade = iced::Background::Gradient(iced::Gradient::Linear(shade).scale_alpha(alpha));
    let label = container(
        text(lines.join("\n"))
            .size(CARD_TITLE_SIZE)
            .font(font)
            .color(ink)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .width(Fill)
    .height(height)
    .padding(iced::Padding {
        top: OVERLAY_FADE,
        right,
        bottom: OVERLAY_PADDING,
        left: OVERLAY_PADDING,
    })
    .style(move |_| {
        container::Style::default()
            .background(shade)
            .border(border::rounded(border::bottom(4)))
    });
    container(label)
        .width(Fill)
        .height(Fill)
        .align_bottom(Fill)
        .into()
}

fn install_bar(percent: u8, palette: Palette, alpha: f32) -> Element<'static, Message> {
    let done = u16::from(percent.min(100));
    let mut bar = row![].width(Fill).height(INSTALL_BAR_HEIGHT);
    if done > 0 {
        bar = bar.push(
            container(Space::new())
                .width(FillPortion(done))
                .height(Fill)
                .style(move |_| container::Style::default().background(palette.primary)),
        );
    }
    if done < 100 {
        bar = bar.push(Space::new().width(FillPortion(100 - done)));
    }
    container(bar)
        .width(Fill)
        .height(INSTALL_BAR_HEIGHT)
        .style(move |_| {
            container::Style::default().background(Color::BLACK.scale_alpha(0.55 * alpha))
        })
        .into()
}

fn text_width(content: &str) -> f32 {
    iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: Size::INFINITE,
        size: 14.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font: Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        },
        align_x: iced::advanced::text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::None,
    })
    .min_bounds()
    .width
}

pub(crate) fn menu_position(pointer: iced::Point, window: Size, menu_height: f32) -> iced::Point {
    iced::Point::new(
        (pointer.x + 4.0).clamp(
            ACTION_MENU_MARGIN,
            (window.width - ACTION_MENU_WIDTH - ACTION_MENU_MARGIN).max(ACTION_MENU_MARGIN),
        ),
        (pointer.y + 4.0).clamp(
            ACTION_MENU_MARGIN,
            (window.height - menu_height - ACTION_MENU_MARGIN).max(ACTION_MENU_MARGIN),
        ),
    )
}

fn action_menu_height(window_height: f32, body_content_height: f32) -> f32 {
    let chrome = ACTION_MENU_PADDING * 2.0
        + ACTION_MENU_HEADER_HEIGHT
        + ui::BUTTON_HEIGHT
        + ACTION_MENU_SPACING * 2.0;
    (chrome + body_content_height)
        .min(ACTION_MENU_MAX_HEIGHT)
        .min((window_height - ACTION_MENU_MARGIN * 2.0).max(0.0))
}

fn action_menu_scroll_height(menu_height: f32) -> f32 {
    (menu_height
        - ACTION_MENU_PADDING * 2.0
        - ACTION_MENU_HEADER_HEIGHT
        - ui::BUTTON_HEIGHT
        - ACTION_MENU_SPACING * 2.0)
        .max(0.0)
}

fn action_menu_body_height(buttons: usize, dividers: usize, reason: Option<&str>) -> f32 {
    let reason_height = reason.map_or(0.0, |reason| text_height(reason, ACTION_MENU_WIDTH - 24.0));
    let items = buttons + dividers + usize::from(reason.is_some());
    buttons as f32 * ui::BUTTON_HEIGHT
        + dividers as f32 * 7.0
        + reason_height
        + items.saturating_sub(1) as f32 * ACTION_MENU_SPACING
}

fn text_height(content: &str, width: f32) -> f32 {
    iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: Size::new(width, f32::INFINITY),
        size: 12.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font: Font::DEFAULT,
        align_x: iced::advanced::text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::WordOrGlyph,
    })
    .min_bounds()
    .height
}

fn context_menu_button(
    label: &'static str,
    palette: Palette,
    danger: bool,
    message: Option<Message>,
) -> iced::widget::Button<'static, Message> {
    button(text(label).size(14).height(Fill).align_y(Center))
        .width(Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press_maybe(message)
        .style(move |_, status| context_menu_style(palette, danger, status))
}

/// A quiet menu row, with danger for Remove.
pub(crate) fn context_menu_style(
    palette: Palette,
    danger: bool,
    status: button::Status,
) -> button::Style {
    let background = match status {
        button::Status::Active | button::Status::Disabled => Color::TRANSPARENT,
        button::Status::Hovered if danger => palette.danger.scale_alpha(0.12),
        button::Status::Pressed if danger => palette.danger.scale_alpha(0.2),
        button::Status::Hovered => palette.foreground.scale_alpha(0.06),
        button::Status::Pressed => palette.primary.scale_alpha(0.16),
    };
    iced::widget::button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            palette.muted
        } else if danger {
            palette.danger
        } else {
            palette.foreground
        },
        border: border::rounded(ui::CONTROL_RADIUS)
            .width(1)
            .color(Color::TRANSPARENT),
        ..iced::widget::button::Style::default()
    }
}

fn truncate_name(name: &str, max_width: f32) -> String {
    if text_width(name) <= max_width {
        return name.to_owned();
    }
    let chars: Vec<_> = name.chars().collect();
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = chars[..mid].iter().collect::<String>() + "...";
        if text_width(&candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    chars[..low]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_owned()
        + "..."
}

#[cfg(test)]
mod poster_tests;

#[cfg(test)]
mod star_tests;

#[cfg(test)]
mod fade_tests;

#[cfg(test)]
mod tests {

    #[test]
    fn every_poster_layer_fades_with_its_card() {
        use wineo_core::runtime::State;
        let mut app = crate::tests::app();
        let mut card = crate::tests::card("game", State::Inactive, 60);
        card.kind = Some(crate::library::CardKind::Proxied);
        card.steam = Some(crate::library::SteamDetails {
            install: wineo_core::steam::AppState::Installing(Some(40)).into(),
            proton: None,
            size: None,
        });
        // A pill, both marks, the Steam mark, the install bar, and the
        // selection border.
        card.gog = Some(crate::gog_library::GogCard {
            gog_id: Some("1".into()),
            unfinished: true,
            blocked_by: None,
            install: true,
            version: None,
        });
        card.favorite = true;
        card.hidden = true;
        app.selected = Some("game".into());
        assert!(card.install_progress().is_some());
        let size = Size::new(200.0, 400.0);
        let layout = GridLayout::default();
        let shown = crate::layout_probe::render(
            &mut app.card_drawn(&card, 1.0, layout, Details::Inline),
            size,
        );
        assert!(shown.chunks(4).any(|pixel| pixel[3] > 0));
        let gone = crate::layout_probe::render(
            &mut app.card_drawn(&card, 0.0, layout, Details::Inline),
            size,
        );
        assert!(gone.chunks(4).all(|pixel| pixel[3] == 0));
    }
    use super::*;

    #[test]
    fn card_height_counts_wrapped_state_and_playtime() {
        use crate::layout_probe::layout;
        use wineo_core::runtime::State;
        let app = crate::tests::app();
        let grid = GridLayout::default();
        let plain = crate::tests::card("game", State::Inactive, 60);
        let mut steam = crate::tests::card("game", State::SteamRunning, 60);
        steam.display_title = "Game".to_owned();
        let mut long_playtime = crate::tests::card("game", State::Stopped, 60);
        long_playtime.playtime = "1234 hours 56 minutes across many sessions".to_owned();
        let mut both = crate::tests::card("game", State::SteamRunning, 60);
        both.playtime = long_playtime.playtime.clone();
        for card in [&plain, &steam, &long_playtime, &both] {
            let mut view = app.card(card);
            let built = layout(&mut view, Size::new(400.0, 900.0)).bounds().height;
            assert_eq!(card_height(card, grid), built, "{:?}", card.state);
        }
        assert!(card_height(&steam, grid) > card_height(&plain, grid) + 15.0);
        assert!(card_height(&long_playtime, grid) > card_height(&plain, grid));
    }

    #[test]
    fn card_titles_wrap_to_two_balanced_lines_in_a_fixed_height() {
        use crate::layout_probe::{bounds_at, layout};
        let app = crate::tests::app();
        let font = Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        };
        let grid = GridLayout::default();
        let lines =
            |title| crate::tile_title::lines(title, grid.card_width(), CARD_TITLE_SIZE, font);
        assert_eq!(
            lines("Halo: The Master Chief Collection"),
            ["Halo: The Master", "Chief Collection"]
        );
        assert_eq!(lines("Portal 2"), ["Portal 2"]);
        let mut heights = Vec::new();
        for title in ["Portal 2", "Halo: The Master Chief Collection"] {
            let mut card = crate::tests::card("game", wineo_core::runtime::State::Inactive, 60);
            card.display_title = title.to_owned();
            let mut view = app.card(&card);
            let node = layout(&mut view, Size::new(400.0, 600.0));
            let title = bounds_at(&node, &[1]);
            assert_eq!(
                title.height,
                crate::tile_title::two_line_height(CARD_TITLE_SIZE)
            );
            // The identifier and the state and playtime row stay below.
            assert_eq!(iced::advanced::Layout::new(&node).children().count(), 4);
            heights.push(node.bounds().height);
        }
        assert_eq!(heights[0], heights[1]);
        // Art loading predicts row geometry with `card_height`.
        let mut untitled = crate::tests::card("game", wineo_core::runtime::State::Inactive, 60);
        let mut view = app.card(&untitled);
        assert_eq!(
            card_height(&untitled, grid),
            layout(&mut view, Size::new(400.0, 600.0)).bounds().height
        );
        drop(view);
        untitled.display_title = "Game".to_owned();
        let mut view = app.card(&untitled);
        assert_eq!(
            card_height(&untitled, grid),
            layout(&mut view, Size::new(400.0, 600.0)).bounds().height
        );
    }

    fn cards(count: usize) -> Vec<Card> {
        (0..count)
            .map(|index| {
                crate::tests::card(
                    &format!("game-{index:03}"),
                    wineo_core::runtime::State::Inactive,
                    60,
                )
            })
            .collect()
    }

    #[test]
    fn the_grid_lays_out_cards_at_the_zoomed_size() {
        use crate::layout_probe::{bounds_at, layout};
        let mut app = crate::tests::app();
        app.merge(cards(6));
        // At 100% the cards keep 0.36.0's sizes and spacing, and at 150%
        // they scale with the gap and padding unchanged. Both rows are
        // centered: 164 px left over at 100% and 20 px at 150%.
        for (zoom, columns, xs, width, poster) in [
            (100, 5, vec![98.0, 274.0, 450.0, 626.0, 802.0], 160.0, 240.0),
            (150, 4, vec![26.0, 282.0, 538.0, 794.0], 240.0, 360.0),
        ] {
            app.zoom = zoom;
            let grid = app.grid_layout();
            assert_eq!(grid.card_width(), width);
            let height = card_height(&app.cards[0], grid);
            let mut view = app.grid();
            let node = layout(&mut view, Size::new(1060.0, 2000.0));
            for index in 0..6 {
                let card = bounds_at(&node, &[index]);
                let (row, column) = (index / columns, index % columns);
                assert_eq!(card.x, xs[column], "{zoom}% card {index}");
                assert_eq!(card.y, 16.0 + row as f32 * (height + 16.0), "{zoom}%");
                assert_eq!(card.size(), Size::new(width, height), "{zoom}%");
                let frame = bounds_at(&node, &[index, 0]);
                assert_eq!(frame.size(), Size::new(width, poster), "{zoom}%");
                let art = bounds_at(&node, &[index, 0, 0]);
                assert_eq!(art.size(), grid.art_frame(), "{zoom}%");
                assert_eq!(
                    art.position(),
                    frame.position() + iced::Vector::new(2.0, 2.0)
                );
            }
        }
        // 0.36.0's card: a 160 x 240 poster, then the two-line title and
        // the state and playtime row.
        app.zoom = 100;
        let mut view = app.card(&app.cards[0]);
        assert_eq!(
            layout(&mut view, Size::new(400.0, 900.0)).bounds().height,
            card_height(&app.cards[0], GridLayout::default())
        );
    }

    #[test]
    fn card_heights_predict_the_layout_at_every_zoom() {
        use crate::layout_probe::layout;
        use wineo_core::runtime::State;
        let mut app = crate::tests::app();
        let mut titled =
            crate::tests::card("a-very-long-identifier-for-a-game", State::SteamRunning, 60);
        titled.display_title = "Halo: The Master Chief Collection".to_owned();
        titled.playtime = "1234 hours 56 minutes across many sessions".to_owned();
        let plain = crate::tests::card("game", State::Inactive, 60);
        for zoom in [60, 73, 100, 137, 150, 200] {
            app.zoom = zoom;
            for card in [&plain, &titled] {
                let mut view = app.card(card);
                let built = layout(&mut view, Size::new(800.0, 2000.0)).bounds();
                assert_eq!(
                    built.height,
                    card_height(card, app.grid_layout()),
                    "{zoom}%"
                );
                assert_eq!(built.width, app.grid_layout().card_width(), "{zoom}%");
            }
        }
    }

    #[test]
    fn art_less_tiles_scale_their_title_only() {
        use crate::layout_probe::{bounds_at, layout};
        let mut app = crate::tests::app();
        let card = crate::tests::card("game", wineo_core::runtime::State::Inactive, 60);
        let mut sizes = Vec::new();
        for zoom in [60, 100, 200] {
            app.zoom = zoom;
            let mut view = app.card(&card);
            let node = layout(&mut view, Size::new(800.0, 2000.0));
            // The tile's title, and the title under the poster.
            let tile = bounds_at(&node, &[0, 0, 0, 0, 0]);
            let below = bounds_at(&node, &[1]);
            sizes.push((tile.height, below.height));
        }
        let line = |size: f32| {
            iced::widget::text::LineHeight::default()
                .to_absolute(size.into())
                .0
        };
        assert_eq!(sizes[0].0, line(12.0));
        assert_eq!(sizes[1].0, line(14.0));
        assert_eq!(sizes[2].0, line(18.0));
        assert!(sizes.iter().all(|(_, below)| *below == sizes[1].1));
    }

    /// The library scrollable's viewport and the bounds of the cards with
    /// these ids, in order.
    struct Placed {
        ids: Vec<iced::widget::Id>,
        found: std::sync::Arc<std::sync::Mutex<(Option<iced::Rectangle>, Vec<iced::Rectangle>)>>,
    }

    impl iced::advanced::widget::Operation for Placed {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }

        fn container(&mut self, id: Option<&iced::widget::Id>, bounds: iced::Rectangle) {
            if id.is_some_and(|id| self.ids.contains(id)) {
                self.found.lock().unwrap().1.push(bounds);
            }
        }

        fn scrollable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: iced::Rectangle,
            _content_bounds: iced::Rectangle,
            _translation: iced::Vector,
            _state: &mut dyn iced::advanced::widget::operation::Scrollable,
        ) {
            if id == Some(&iced::widget::Id::new(SCROLL_ID)) {
                self.found.lock().unwrap().0 = Some(bounds);
            }
        }
    }

    /// The library viewport's width and how many cards the grid put in its
    /// first row, laid out the way the window draws them.
    fn drawn_columns(app: &App) -> (f32, usize) {
        let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
        let found = std::sync::Arc::default();
        laid.run(Box::new(Placed {
            ids: app
                .filtered()
                .iter()
                .map(|card| iced::widget::Id::from(format!("library-card:{}", card.name)))
                .collect(),
            found: std::sync::Arc::clone(&found),
        }));
        let (viewport, cards) = std::mem::take(&mut *found.lock().unwrap());
        let viewport = viewport.expect("the library scrollable");
        let first_row = cards.iter().filter(|card| card.y == cards[0].y).count();
        (viewport.width, first_row)
    }

    #[test]
    fn the_grid_and_the_app_agree_on_columns_with_a_scrollbar() {
        use std::time::{Duration, Instant};
        let mut app = crate::tests::app();
        // Enough rows that the scrollbar shows at every zoom here.
        app.merge(cards(120));
        // Viewports one pixel either side of a column boundary, closed and
        // docked: 896 fits five 160 px cards, and 896 more than the dock.
        let closed = [1035.0, 1036.0, 1037.0];
        let docked = [1465.0, 1466.0, 1467.0];
        for zoom in [60, 100, 150, 200] {
            app.zoom = zoom;
            for width in closed {
                let _ = app.window_resized(Size::new(width, 800.0));
                let (viewport, drawn) = drawn_columns(&app);
                assert_eq!(viewport, app.viewport_width, "{zoom}% at {width}");
                assert_eq!(drawn, app.columns(), "{zoom}% at {width}");
            }
        }
        app.zoom = 100;
        let _ = app.window_resized(Size::new(1035.0, 800.0));
        assert_eq!(drawn_columns(&app).1, 4);
        let _ = app.window_resized(Size::new(1036.0, 800.0));
        assert_eq!(drawn_columns(&app).1, 5);
        let _ = app.window_resized(Size::new(1466.0, 800.0));
        let _ = app.update(Message::Select(app.cards[0].name.clone()));
        let _ = app.update(Message::GridTick(
            app.grid_motion.generation(),
            Instant::now() + Duration::from_secs(1),
        ));
        assert!(app.panel_open && !app.grid_motion.active());
        for zoom in [60, 100, 150, 200] {
            app.zoom = zoom;
            for width in docked {
                let _ = app.window_resized(Size::new(width, 800.0));
                let (viewport, drawn) = drawn_columns(&app);
                assert_eq!(viewport, app.viewport_width, "{zoom}% docked at {width}");
                assert_eq!(drawn, app.columns(), "{zoom}% docked at {width}");
            }
        }
        app.zoom = 100;
        let _ = app.window_resized(Size::new(1465.0, 800.0));
        assert_eq!(drawn_columns(&app).1, 4);
        let _ = app.window_resized(Size::new(1466.0, 800.0));
        assert_eq!(drawn_columns(&app).1, 5);
    }

    #[test]
    fn the_window_draws_the_centered_cards_where_the_app_places_them() {
        let mut app = crate::tests::app();
        // Enough rows that the scrollbar shows, so a width it took from the
        // grid would put the cards off the app's places.
        app.merge(cards(120));
        let check = |app: &App, label: &str| {
            let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
            let found = std::sync::Arc::default();
            laid.run(Box::new(Placed {
                ids: app
                    .filtered()
                    .iter()
                    .map(|card| iced::widget::Id::from(format!("library-card:{}", card.name)))
                    .collect(),
                found: std::sync::Arc::clone(&found),
            }));
            let (viewport, cards) = std::mem::take(&mut *found.lock().unwrap());
            let viewport = viewport.expect("the library scrollable");
            assert_eq!(viewport.width, app.viewport_width, "{label}");
            let places = app.grid_places();
            assert!(app.grid_shift() > 0.0, "{label}");
            assert_eq!(cards.len(), places.slots.len(), "{label}");
            for (card, slot) in cards.iter().zip(&places.slots) {
                assert_eq!(card.x - viewport.x, slot.x, "{label}");
            }
        };
        for zoom in [60, 100, 150, 200] {
            app.zoom = zoom;
            for width in [1100.0, 1237.0, 1510.0] {
                let _ = app.window_resized(Size::new(width, 800.0));
                check(&app, &format!("{zoom}% at {width}"));
            }
        }
        let _ = app.update(Message::Select(app.cards[0].name.clone()));
        let _ = crate::tests::settle_grid(&mut app);
        assert!(app.panel_open && !app.grid_motion.active());
        for zoom in [60, 100, 150, 200] {
            app.zoom = zoom;
            for width in [1500.0, 1637.0, 1900.0] {
                let _ = app.window_resized(Size::new(width, 800.0));
                check(&app, &format!("{zoom}% docked at {width}"));
            }
        }
    }

    #[test]
    fn context_menu_follows_pointer_and_stays_inside_window() {
        let height = action_menu_height(800.0, 400.0);
        assert_eq!(
            menu_position(
                iced::Point::new(100.0, 200.0),
                Size::new(1200.0, 800.0),
                height,
            ),
            iced::Point::new(104.0, 204.0)
        );
        let position = menu_position(
            iced::Point::new(1199.0, 799.0),
            Size::new(1200.0, 800.0),
            height,
        );
        assert_eq!(position, iced::Point::new(944.0, 308.0));
        assert_eq!(
            menu_position(
                iced::Point::ORIGIN,
                Size::new(100.0, 100.0),
                action_menu_height(100.0, 0.0),
            ),
            iced::Point::new(8.0, 8.0)
        );
    }

    #[test]
    fn context_menu_keeps_close_reachable_for_long_reasons_in_small_windows() {
        let reason = format!("Busy: removing {}", "long-environment-name-".repeat(80));
        let content_height = action_menu_body_height(6, 1, Some(&reason));
        let window = Size::new(800.0, 600.0);
        let height = action_menu_height(window.height, content_height);
        let scroll_height = action_menu_scroll_height(height);
        let position = menu_position(iced::Point::new(799.0, 599.0), window, height);

        assert!(content_height > scroll_height);
        assert!(scroll_height > 0.0);
        assert!(height <= window.height - ACTION_MENU_MARGIN * 2.0);
        assert!(position.y + height <= window.height - ACTION_MENU_MARGIN);
        assert_eq!(height, window.height - ACTION_MENU_MARGIN * 2.0);
        // A tall window still caps it.
        assert_eq!(
            action_menu_height(2000.0, content_height),
            ACTION_MENU_MAX_HEIGHT
        );
    }

    #[test]
    fn context_menu_shrinks_to_its_content_for_native_environments() {
        // Favorite and Hide, Settings, Art, Copy, Check, Clone, Remove.
        let body_height = action_menu_body_height(8, 2, None);
        assert_eq!(action_menu_height(800.0, body_height), 390.0);
    }

    #[test]
    fn a_wine_prefix_menu_fits_without_scrolling_in_an_800_px_window() {
        // Favorite and Hide, Settings, Art, Snapshots, Copy, Run program,
        // Winetricks, Check, Clone, Remove, with three separators.
        let content_height = action_menu_body_height(11, 3, None);
        let height = action_menu_height(800.0, content_height);
        assert_eq!(height, 509.0);
        assert!(action_menu_scroll_height(height) >= content_height);
    }
}
