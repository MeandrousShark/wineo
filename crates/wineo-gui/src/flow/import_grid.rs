//! The poster grid the Heroic, Steam, and GOG import pages share.

use iced::keyboard::key::Named;
use iced::widget::{column, container, image, mouse_area, scrollable, stack, text};
use iced::{Color, ContentFit, Element, Fill, Font, Task, border, mouse};

use super::heroic::{PAGE_MAX_WIDTH, PAGE_PADDING, SCROLL_GUTTER};
use crate::theme::Palette;
use crate::ui::ScrollStyle;

const TILE_WIDTH: f32 = 120.0;
/// With room for the focus ring on either side.
const TILE_OUTER_WIDTH: f32 = TILE_WIDTH + 2.0 * RING_INSET;
/// How far the focus ring sits outside the poster's selection border.
const RING_INSET: f32 = 4.0;
const RING_WIDTH: f32 = 2.0;
const SELECTION_RADIUS: f32 = 6.0;
const TILE_POSTER_HEIGHT: f32 = 180.0;
/// Room under the poster for two title lines and two lines of reason, fixed
/// so every row is the same height.
const TILE_HEIGHT: f32 = 262.0;
const TILE_TITLE_SIZE: f32 = 13.0;
const TILE_REASON_SIZE: f32 = 11.0;
pub(super) const TILE_GAP: f32 = 12.0;

/// How many tiles fit across a flow dialog `width` wide, after the page's
/// padding, max width, and the scrollbar.
pub(super) fn grid_columns(width: f32) -> usize {
    let content = (width - 2.0 * PAGE_PADDING).min(PAGE_MAX_WIDTH) - SCROLL_GUTTER;
    ((content + TILE_GAP) / (TILE_OUTER_WIDTH + TILE_GAP))
        .floor()
        .max(1.0) as usize
}

const ROWS_ID: &str = "import-grid-rows";

/// The import grid page's one scrolling region: the title, notes, and tile
/// rows scroll together, so a short dialog can still reach every game.
pub(super) fn grid_scroll<'a, Message: 'a>(
    id: &'static str,
    header: Element<'a, Message>,
    rows: Element<'a, Message>,
    palette: Palette,
) -> Element<'a, Message> {
    let rows = container(rows)
        .id(iced::widget::Id::new(ROWS_ID))
        .width(Fill);
    scrollable(
        container(column![header, rows].spacing(14).width(Fill))
            .padding(iced::Padding::ZERO.right(SCROLL_GUTTER)),
    )
    .wineo_style(palette)
    .id(iced::widget::Id::new(id))
    .width(Fill)
    .height(Fill)
    .into()
}

/// The smallest scroll offset that shows the row spanning `top..bottom` of
/// the content in a `viewport` tall window now scrolled to `current`. A row
/// taller than the window shows its top, and the first row shows the header
/// above it when both fit.
fn reveal_offset(current: f32, viewport: f32, top: f32, bottom: f32, first: bool) -> f32 {
    let lowest = bottom - viewport;
    if first && lowest <= 0.0 {
        return 0.0;
    }
    if lowest > top {
        return top;
    }
    current.clamp(lowest, top)
}

/// Scrolls the grid named `grid` so its row `row` of `rows` shows. The header
/// sits inside the scrolled content with a height that depends on how its text
/// wraps, so the first pass measures it and the second scrolls.
pub(super) fn reveal_row<T: Send + 'static>(
    grid: &'static str,
    row: usize,
    rows: usize,
) -> Task<T> {
    iced::advanced::widget::operate(measure(grid, row, rows))
}

pub(super) fn measure(grid: &'static str, row: usize, rows: usize) -> Measure {
    Measure {
        grid: iced::widget::Id::new(grid),
        row,
        rows,
        viewport: None,
        rows_top: None,
    }
}

pub(super) struct Measure {
    grid: iced::widget::Id,
    row: usize,
    rows: usize,
    /// The window's height, the content's top, and the current offset.
    viewport: Option<(f32, f32, f32)>,
    /// The tile rows' top and height.
    rows_top: Option<(f32, f32)>,
}

impl<T: Send + 'static> iced::advanced::widget::Operation<T> for Measure {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<T>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::widget::Id>, bounds: iced::Rectangle) {
        if id == Some(&iced::widget::Id::new(ROWS_ID)) {
            self.rows_top = Some((bounds.y, bounds.height));
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&self.grid) {
            self.viewport = Some((bounds.height, content_bounds.y, translation.y));
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<T> {
        use iced::advanced::widget::operation::Outcome;
        let (Some((viewport, content_top, current)), Some((rows_y, rows_height))) =
            (self.viewport, self.rows_top)
        else {
            return Outcome::None;
        };
        if self.rows == 0 {
            return Outcome::None;
        }
        // Rows are one height, so the pitch follows from the whole.
        let pitch = (rows_height + TILE_GAP) / self.rows as f32;
        let top = rows_y - content_top + self.row as f32 * pitch;
        let offset = reveal_offset(
            current,
            viewport,
            top,
            top + pitch - TILE_GAP,
            self.row == 0,
        );
        Outcome::Chain(Box::new(
            iced::advanced::widget::operation::scrollable::scroll_to(
                self.grid.clone(),
                iced::advanced::widget::operation::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(offset),
                },
            ),
        ))
    }
}

pub(super) fn move_focus(
    focus: Option<usize>,
    key: Named,
    columns: usize,
    count: usize,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let Some(index) = focus.filter(|index| *index < count) else {
        return Some(0);
    };
    let columns = columns.max(1);
    Some(match key {
        Named::ArrowLeft => index.saturating_sub(1),
        Named::ArrowRight => (index + 1).min(count - 1),
        Named::ArrowUp => index.checked_sub(columns).unwrap_or(index),
        Named::ArrowDown if index + columns < count => index + columns,
        // The last row can be short, so Down from above its end lands on its
        // last tile.
        Named::ArrowDown if (count - 1) / columns > index / columns => count - 1,
        _ => index,
    })
}

pub(super) fn import_label(count: usize) -> String {
    if count == 1 {
        "Import 1 game".to_owned()
    } else {
        format!("Import {count} games")
    }
}

pub(super) struct Tile<'a> {
    pub title: &'a str,
    pub poster: Option<&'a image::Handle>,
    pub reason: Option<String>,
    pub note: Option<String>,
    pub chosen: bool,
    /// Draws the keyboard focus ring. Pages set it only for focus that came
    /// from the keyboard, so clicking a tile doesn't outline it.
    pub focus_ring: bool,
    /// Dims the tile like an uninstalled game, while it stays selectable.
    pub dimmed: bool,
}

/// A ring outside the selection border, concentric with it and separated by
/// a gap, so focus reads differently from selection in both palettes.
fn focus_ring_style(palette: Palette, shown: bool) -> container::Style {
    container::Style::default().border(
        border::rounded(SELECTION_RADIUS + RING_INSET)
            .width(RING_WIDTH)
            .color(if shown {
                palette.primary
            } else {
                Color::TRANSPARENT
            }),
    )
}

pub(super) fn tile<'a, Message: Clone + 'a>(
    tile: Tile<'a>,
    palette: Palette,
    on_press: Message,
) -> Element<'a, Message> {
    let refused = tile.reason.is_some();
    let dimmed = refused || tile.dimmed;
    let art: Element<'_, Message> = match tile.poster {
        Some(handle) => image(handle.clone())
            .width(Fill)
            .height(Fill)
            .content_fit(ContentFit::Cover)
            .border_radius(4)
            .opacity(if refused {
                0.35_f32
            } else if dimmed {
                crate::panel::DIMMED_OPACITY
            } else {
                1.0_f32
            })
            .into(),
        None => container(
            text(tile.title)
                .size(13)
                .color(if dimmed {
                    palette.muted.scale_alpha(0.6)
                } else {
                    palette.muted
                })
                .center(),
        )
        .width(Fill)
        .height(Fill)
        .padding(8)
        .center(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.surface.scale_alpha(if dimmed { 0.5 } else { 1.0 }))
                .border(border::rounded(4))
        })
        .into(),
    };
    let chosen = tile.chosen;
    let mut layers = stack![
        container(container(art).clip(true))
            .width(TILE_WIDTH)
            .height(TILE_POSTER_HEIGHT)
            .padding(3)
            .style(move |_| {
                container::Style::default().border(
                    border::rounded(SELECTION_RADIUS).width(3).color(if chosen {
                        palette.primary
                    } else {
                        Color::TRANSPARENT
                    }),
                )
            })
    ];
    if chosen {
        layers = layers.push(
            container(
                container(text("✓").size(14).color(palette.background))
                    .width(22)
                    .height(22)
                    .center(22)
                    .style(move |_| {
                        container::Style::default()
                            .background(palette.primary)
                            .border(border::rounded(11))
                    }),
            )
            .width(TILE_WIDTH)
            .padding(8)
            .align_right(TILE_WIDTH),
        );
    }
    let focus_ring = tile.focus_ring;
    let ring = container(layers)
        .padding(RING_INSET)
        .style(move |_| focus_ring_style(palette, focus_ring));
    let mut details = column![
        text(
            crate::tile_title::lines(tile.title, TILE_WIDTH, TILE_TITLE_SIZE, Font::DEFAULT)
                .join("\n")
        )
        .size(TILE_TITLE_SIZE)
        .height(crate::tile_title::two_line_height(TILE_TITLE_SIZE))
        .color(if dimmed {
            palette.muted
        } else {
            palette.foreground
        })
        .wrapping(iced::widget::text::Wrapping::None),
    ]
    .spacing(4)
    .padding([0.0, RING_INSET])
    .width(TILE_OUTER_WIDTH);
    if let Some(reason) = tile.reason {
        // Balanced and cut like the title, since the tile is clipped and a
        // freely wrapping reason would lose its end.
        details = details.push(
            text(
                crate::tile_title::lines(&reason, TILE_WIDTH, TILE_REASON_SIZE, Font::DEFAULT)
                    .join("\n"),
            )
            .size(TILE_REASON_SIZE)
            .height(crate::tile_title::two_line_height(TILE_REASON_SIZE))
            .color(palette.warning)
            .wrapping(iced::widget::text::Wrapping::None),
        );
    }
    if let Some(note) = tile.note {
        details = details.push(text(note).size(11).color(palette.muted));
    }
    let framed = container(column![ring, details])
        .width(TILE_OUTER_WIDTH)
        .height(TILE_HEIGHT)
        .clip(true);
    let area = mouse_area(framed);
    if refused {
        area.into()
    } else {
        area.on_press(on_press)
            .interaction(mouse::Interaction::Pointer)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tile(title: &str) -> Tile<'_> {
        Tile {
            title,
            poster: None,
            reason: None,
            note: None,
            chosen: false,
            focus_ring: false,
            dimmed: false,
        }
    }

    #[test]
    fn tiles_reserve_two_title_lines_whatever_the_title() {
        use crate::layout_probe::{bounds_at, layout};
        let size = iced::Size::new(400.0, 400.0);
        let mut heights = Vec::new();
        for title in ["Portal", "Grand Theft Auto IV: The Complete Edition"] {
            let mut view = tile(sample_tile(title), Palette::DARK, ());
            let node = layout(&mut view, size);
            assert_eq!(node.bounds().height, TILE_HEIGHT);
            let title = bounds_at(&node, &[0, 1, 0]);
            assert_eq!(
                title.height,
                crate::tile_title::two_line_height(TILE_TITLE_SIZE)
            );
            heights.push(title.y);
        }
        assert_eq!(heights[0], heights[1]);
    }

    #[test]
    fn a_long_reason_stays_inside_the_tile_in_two_lines() {
        use crate::layout_probe::{bounds_at, layout};
        let size = iced::Size::new(400.0, 400.0);
        for reason in [
            "Already migrated as disco-elysium-the-final-cut.",
            "Already migrated as a",
        ] {
            let mut view = tile(
                Tile {
                    reason: Some(reason.to_owned()),
                    ..sample_tile("Disco Elysium - The Final Cut")
                },
                Palette::DARK,
                (),
            );
            let node = layout(&mut view, size);
            let reason = bounds_at(&node, &[0, 1, 1]);
            assert_eq!(
                reason.height,
                crate::tile_title::two_line_height(TILE_REASON_SIZE)
            );
            assert!(reason.y + reason.height <= node.bounds().y + TILE_HEIGHT);
        }
    }

    #[test]
    fn the_focus_ring_wraps_the_poster_without_moving_the_tile() {
        use crate::layout_probe::{bounds_at, layout, within};
        let size = iced::Size::new(400.0, 400.0);
        let mut layouts = Vec::new();
        for focus_ring in [false, true] {
            let mut view = tile(
                Tile {
                    focus_ring,
                    chosen: true,
                    ..sample_tile("Portal")
                },
                Palette::DARK,
                (),
            );
            let node = layout(&mut view, size);
            let ring = bounds_at(&node, &[0, 0]);
            let poster = bounds_at(&node, &[0, 0, 0, 0]);
            assert_eq!(
                poster.size(),
                iced::Size::new(TILE_WIDTH, TILE_POSTER_HEIGHT)
            );
            assert_eq!(ring, poster.expand(RING_INSET));
            assert!(within(ring, node.bounds()));
            layouts.push((node.bounds(), ring, poster));
        }
        assert_eq!(layouts[0], layouts[1]);
    }

    #[test]
    fn the_focus_ring_is_primary_and_distinct_from_selection() {
        for palette in [
            Palette::from_mode(iced::theme::Mode::Dark),
            Palette::from_mode(iced::theme::Mode::Light),
        ] {
            let shown = focus_ring_style(palette, true).border;
            assert_eq!(shown.color, palette.primary);
            assert_eq!(shown.width, RING_WIDTH);
            // Concentric with the 3 px selection border, with a gap between.
            assert_eq!(shown.radius, (SELECTION_RADIUS + RING_INSET).into());
            const { assert!(RING_WIDTH < RING_INSET && RING_WIDTH != 3.0) };
            assert_eq!(
                focus_ring_style(palette, false).border.color,
                Color::TRANSPARENT
            );
        }
    }

    #[test]
    fn reveal_offset_moves_only_as_far_as_needed() {
        // A 100 tall window over rows 0..40 and 52..92 (the header is above).
        assert_eq!(reveal_offset(0.0, 100.0, 52.0, 92.0, false), 0.0);
        assert_eq!(reveal_offset(0.0, 100.0, 152.0, 192.0, false), 92.0);
        assert_eq!(reveal_offset(300.0, 100.0, 152.0, 192.0, false), 152.0);
        assert_eq!(reveal_offset(140.0, 100.0, 152.0, 192.0, false), 140.0);
        // Taller than the window shows the top.
        assert_eq!(reveal_offset(0.0, 30.0, 52.0, 92.0, false), 52.0);
        // The first row shows the header when both fit.
        assert_eq!(reveal_offset(80.0, 100.0, 30.0, 70.0, true), 0.0);
        assert_eq!(reveal_offset(80.0, 40.0, 30.0, 70.0, true), 30.0);
    }
}
