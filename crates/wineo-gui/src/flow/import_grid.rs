//! The poster grid the Heroic, Steam, and GOG import pages share.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use iced::keyboard::{self, Key, key::Named};
use iced::widget::{Row, column, container, image, mouse_area, row, scrollable, stack, text};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Task, border, mouse};

use super::frame::{PAGE_MAX_WIDTH, PAGE_PADDING, SCROLL_GUTTER};
use super::posters;
use crate::ends::{MIN_LEFT, ends};
use crate::progress::Delayed;
use crate::theme::Palette;
use crate::ui::{ScrollStyle, accent_base, secondary_button};

/// One import page's grid: what is picked, where keyboard focus is, and how
/// far its posters have come. The page owns its listing and posters.
pub(super) struct ImportGrid<Message> {
    id: &'static str,
    /// The ids picked, kept while the person goes back and forth.
    pub chosen: BTreeSet<String>,
    pub focus: Option<usize>,
    /// Whether `focus` came from the keyboard. A click moves focus too, but
    /// like CSS :focus-visible only keyboard focus draws the ring.
    pub focus_visible: bool,
    /// The poster load replies must match to count.
    pub poster_generation: u64,
    pub progress: Delayed<Message>,
}

/// A grid key the page acts on itself.
pub(super) enum GridKey<Message> {
    /// An arrow moved focus; the task scrolls it into view.
    Moved(Task<Message>),
    /// Space, on the focused tile if there is one.
    Toggle(Option<usize>),
    SelectAll,
    Import,
}

impl<Message: Send + 'static> ImportGrid<Message> {
    /// `id` names the grid's scrollable, and `progress_due` is the page's
    /// message for the progress bar's delay.
    pub(super) fn new(
        id: &'static str,
        progress_due: fn(u64) -> Message,
        poster_generation: u64,
    ) -> Self {
        Self {
            id,
            chosen: BTreeSet::new(),
            focus: None,
            focus_visible: false,
            poster_generation,
            progress: Delayed::new(progress_due),
        }
    }

    /// Forgets the old listing's picks and focus, and starts a new poster
    /// load.
    pub(super) fn clear(&mut self) {
        self.chosen.clear();
        self.focus = None;
        self.focus_visible = false;
        self.restart_posters(self.poster_generation.wrapping_add(1));
    }

    /// Drops the poster load in flight, so its replies no longer count.
    pub(super) fn restart_posters(&mut self, generation: u64) {
        self.poster_generation = generation;
        self.progress.reset();
    }

    /// Decodes each poster in `jobs` on a worker, under the import decode
    /// permits, and replies with `message`.
    pub(super) fn decode_posters<K, T>(
        &mut self,
        cache: PathBuf,
        jobs: Vec<(K, PathBuf)>,
        decode: fn(&K, &Path, &Path) -> Result<T, String>,
        message: fn(u64, K, Result<T, String>) -> Message,
    ) -> Task<Message>
    where
        K: Clone + Send + 'static,
        T: Send + 'static,
    {
        let generation = self.poster_generation;
        let shown = self.progress.add(jobs.len(), Instant::now());
        Task::batch(
            jobs.into_iter()
                .map(|(key, path)| {
                    let cache = cache.clone();
                    let job = key.clone();
                    Task::perform(
                        async move {
                            posters::bounded(move || decode(&job, &path, &cache))
                                .await
                                .and_then(|result| result)
                        },
                        move |result| message(generation, key, result),
                    )
                })
                .chain([shown]),
        )
    }

    /// A click moves focus without drawing the ring.
    pub(super) fn click(&mut self, index: usize) {
        self.focus = Some(index);
        self.focus_visible = false;
    }

    pub(super) fn toggle(&mut self, id: &str) {
        if !self.chosen.remove(id) {
            self.chosen.insert(id.to_owned());
        }
    }

    pub(super) fn focus_ring(&self, index: usize) -> bool {
        self.focus_visible && self.focus == Some(index)
    }

    /// Arrows move focus over `count` tiles and scroll its row into view.
    /// None for any other key. The header sits inside the scrolled content
    /// with a height that depends on how its text wraps, so the first pass
    /// measures it and the second scrolls.
    pub(super) fn arrow(&mut self, key: &Key, width: f32, count: usize) -> Option<Task<Message>> {
        let Key::Named(
            named @ (Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown),
        ) = key.as_ref()
        else {
            return None;
        };
        let columns = grid_columns(width);
        self.focus = move_focus(self.focus, named, columns, count);
        self.focus_visible = true;
        let Some(index) = self.focus else {
            return Some(Task::none());
        };
        let columns = columns.max(1);
        Some(iced::advanced::widget::operate(measure(
            self.id,
            index / columns,
            count.div_ceil(columns),
        )))
    }

    /// The keys of a grid that picks games: arrows, Space, Ctrl+A, and
    /// Enter. None leaves the key to the flow's shared handling.
    pub(super) fn key(
        &mut self,
        key: &Key,
        modifiers: keyboard::Modifiers,
        width: f32,
        count: usize,
    ) -> Option<GridKey<Message>> {
        if let Some(task) = self.arrow(key, width, count) {
            return Some(GridKey::Moved(task));
        }
        match key.as_ref() {
            Key::Named(Named::Space) | Key::Character(" ") => Some(GridKey::Toggle(self.focus)),
            Key::Character("a" | "A") if modifiers.control() => Some(GridKey::SelectAll),
            Key::Named(Named::Enter) if !modifiers.control() => Some(GridKey::Import),
            _ => None,
        }
    }

    /// The tiles in rows as wide as the dialog allows, scrolling with
    /// `header` above them.
    pub(super) fn view<'a>(
        &self,
        header: Element<'a, Message>,
        tiles: impl IntoIterator<Item = (Tile<'a>, Message)>,
        width: f32,
        palette: Palette,
    ) -> Element<'a, Message>
    where
        Message: Clone + 'a,
    {
        let columns = grid_columns(width);
        let mut rows = column![].spacing(TILE_GAP);
        let mut tiles = tiles
            .into_iter()
            .map(|(item, on_press)| tile(item, palette, on_press))
            .peekable();
        while tiles.peek().is_some() {
            rows = rows.push(Row::with_children(tiles.by_ref().take(columns)).spacing(TILE_GAP));
        }
        grid_scroll(self.id, header, rows.into(), palette)
    }

    /// The bar for the posters still being prepared, once it's worth showing.
    pub(super) fn progress_bar<'a>(&self, palette: Palette) -> Option<Element<'a, Message>>
    where
        Message: 'a,
    {
        posters::progress_bar(&self.progress, palette)
    }

    /// The pinned row under a grid that picks games: Select all, Select none,
    /// the page's `extras`, the poster progress, and Import at the right. A
    /// None message disables its button.
    pub(super) fn footer<'a>(
        &self,
        select_all: Option<Message>,
        select_none: Option<Message>,
        extras: impl IntoIterator<Item = Element<'a, Message>>,
        import: Option<Message>,
        palette: Palette,
    ) -> Element<'a, Message>
    where
        Message: Clone + 'a,
    {
        let mut others = row![
            secondary_button("Select all", palette).on_press_maybe(select_all),
            secondary_button("Select none", palette).on_press_maybe(select_none),
        ]
        .spacing(12)
        .align_y(Center)
        .extend(extras);
        if let Some(bar) = self.progress_bar(palette) {
            others = others.push(bar);
        }
        let import =
            accent_base(import_label(self.chosen.len()), palette.primary).on_press_maybe(import);
        // A narrow dialog stacks the import button under the rest.
        ends(others.wrap().vertical_spacing(8), import, 12.0, MIN_LEFT)
    }
}

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
const TILE_GAP: f32 = 12.0;

/// How many tiles fit across a flow dialog `width` wide, after the page's
/// padding, max width, and the scrollbar.
fn grid_columns(width: f32) -> usize {
    let content = (width - 2.0 * PAGE_PADDING).min(PAGE_MAX_WIDTH) - SCROLL_GUTTER;
    ((content + TILE_GAP) / (TILE_OUTER_WIDTH + TILE_GAP))
        .floor()
        .max(1.0) as usize
}

const ROWS_ID: &str = "import-grid-rows";

/// The import grid page's one scrolling region: the title, notes, and tile
/// rows scroll together, so a short dialog can still reach every game.
fn grid_scroll<'a, Message: 'a>(
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

fn move_focus(focus: Option<usize>, key: Named, columns: usize, count: usize) -> Option<usize> {
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

fn tile<'a, Message: Clone + 'a>(
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

    #[test]
    fn arrows_move_focus_across_rows() {
        assert_eq!(move_focus(None, Named::ArrowRight, 3, 0), None);
        assert_eq!(move_focus(None, Named::ArrowDown, 3, 5), Some(0));
        assert_eq!(move_focus(Some(0), Named::ArrowLeft, 3, 5), Some(0));
        assert_eq!(move_focus(Some(0), Named::ArrowRight, 3, 5), Some(1));
        assert_eq!(move_focus(Some(4), Named::ArrowRight, 3, 5), Some(4));
        assert_eq!(move_focus(Some(1), Named::ArrowDown, 3, 5), Some(4));
        // The last row has two tiles, so Down from its right lands on its end.
        assert_eq!(move_focus(Some(2), Named::ArrowDown, 3, 5), Some(4));
        assert_eq!(move_focus(Some(4), Named::ArrowDown, 3, 5), Some(4));
        assert_eq!(move_focus(Some(4), Named::ArrowUp, 3, 5), Some(1));
        assert_eq!(move_focus(Some(1), Named::ArrowUp, 3, 5), Some(1));
        assert!(grid_columns(900.0) >= 4);
        assert_eq!(grid_columns(280.0), 1);
    }
}
