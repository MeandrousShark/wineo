//! Coverflow: one strip of covers with the selection in the middle. This
//! holds the strip's place, its geometry, how the keys walk it, and the
//! messages its widget sends; `coverflow_widget` draws it.

use iced::widget::image;
use iced::{Point, Rectangle, Size, Task};

use crate::grid_motion::{DOCK_WIDTH, GridLayout, Places, Stop, ViewLayout};
use crate::keys::Direction;
use crate::slot::{SectionKey, SlotId};
use crate::{App, ArtKey, Message, artwork};

/// Covers drawn on each side of the middle one.
pub(crate) const NEIGHBORS: usize = 4;
/// The middle cover's height, as a share of the strip's.
const HEIGHT_SHARE: f32 = 0.55;
/// The middle cover's widest, as a share of the strip's width, so a tall
/// narrow window shrinks it rather than overflowing.
const WIDTH_SHARE: f32 = 0.4;
/// A cover's width over its height: a poster's 2:3.
const ASPECT: f32 = 2.0 / 3.0;
/// How much smaller each step from the middle draws, down to `MIN_SCALE`,
/// below which unmipmapped images shimmer.
const STEP_SCALE: f32 = 0.125;
const MIN_SCALE: f32 = 0.5;
/// How much darker each step from the middle draws.
const STEP_DIM: f32 = 0.15;
/// The first neighbor's center from the middle's, in middle widths, and
/// each later step's, so the side covers overlap.
const NEAR: f32 = 0.75;
const FAR: f32 = 0.3;
/// The room under the middle cover for its title and status line.
pub(crate) const CAPTION: f32 = 64.0;
/// The gap between the middle cover and its caption.
pub(crate) const CAPTION_GAP: f32 = 12.0;
/// The tallest `Cover` decode, and the step its height rounds up to.
const COVER_MAX: f32 = 1440.0;
const COVER_STEP: f32 = 240.0;

/// One cover as drawn: its stop, where it is in the strip, how large next
/// to the middle, and how bright.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Cover {
    pub(crate) stop: usize,
    pub(crate) bounds: Rectangle,
    pub(crate) scale: f32,
    pub(crate) opacity: f32,
}

/// The strip at one position: its covers back to front, and where the
/// middle cover's caption goes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Geometry {
    pub(crate) covers: Vec<Cover>,
    pub(crate) caption: Rectangle,
}

/// The middle cover's size in a strip of `size`: about 55% of its height,
/// never under the grid's usual poster, but at most 40% of its width, which
/// wins in a very narrow window, and never taller than leaves room for the
/// caption.
pub(crate) fn middle_size(size: Size) -> Size {
    let floor = GridLayout::default().poster_height();
    let room = (size.height - CAPTION).max(0.0);
    let height = (size.height * HEIGHT_SHARE)
        .max(floor)
        .min(room)
        .min(size.width * WIDTH_SHARE / ASPECT)
        .max(0.0)
        .round();
    Size::new((height * ASPECT).round(), height)
}

/// How large the cover `distance` stops from the middle draws, next to it.
fn scale(distance: f32) -> f32 {
    (1.0 - STEP_SCALE * distance.min(NEIGHBORS as f32)).max(MIN_SCALE)
}

/// How far right of the middle's center the cover `offset` stops away sits,
/// in middle widths, negative to the left.
fn reach(offset: f32) -> f32 {
    let distance = offset.abs();
    let units = NEAR * distance.min(1.0) + FAR * (distance - 1.0).max(0.0);
    units.copysign(offset)
}

/// The strip of `count` stops at `position`, a stop index that may sit
/// between two while it moves, in a strip `size` large. Covers more than
/// `NEIGHBORS` from it fade out over the next stop and then aren't drawn.
pub(crate) fn geometry(position: f32, count: usize, size: Size) -> Geometry {
    let middle = middle_size(size);
    let top = ((size.height - middle.height - CAPTION) / 2.0)
        .max(0.0)
        .round();
    let center = Point::new((size.width / 2.0).round(), top + middle.height / 2.0);
    let mut covers: Vec<Cover> = (0..count)
        .filter_map(|stop| {
            let offset = stop as f32 - position;
            let distance = offset.abs();
            let fade = (NEIGHBORS as f32 + 1.0 - distance).clamp(0.0, 1.0);
            if fade <= 0.0 {
                return None;
            }
            let scale = scale(distance);
            let drawn = Size::new(
                (middle.width * scale).round(),
                (middle.height * scale).round(),
            );
            let x = center.x + reach(offset) * middle.width;
            Some(Cover {
                stop,
                bounds: Rectangle::new(
                    Point::new(
                        (x - drawn.width / 2.0).round(),
                        (center.y - drawn.height / 2.0).round(),
                    ),
                    drawn,
                ),
                scale,
                opacity: (1.0 - STEP_DIM * distance.min(NEIGHBORS as f32)) * fade,
            })
        })
        .collect();
    // The farthest first, so nearer covers overlap them.
    covers.sort_by(|a, b| {
        let far = |cover: &Cover| (cover.stop as f32 - position).abs();
        far(b).total_cmp(&far(a))
    });
    let width = (middle.width * 2.0).min(size.width);
    Geometry {
        covers,
        caption: Rectangle::new(
            Point::new(
                (center.x - width / 2.0).round(),
                top + middle.height + CAPTION_GAP,
            ),
            Size::new(width, CAPTION - CAPTION_GAP),
        ),
    }
}

/// How many covers a strip `size` large shows whole or in part at rest
/// with stops to spare on both sides: PageUp and PageDown's step.
pub(crate) fn on_screen(size: Size) -> usize {
    let middle = middle_size(size);
    let side = (1..=NEIGHBORS)
        .take_while(|&step| {
            let distance = step as f32;
            let half = middle.width * scale(distance) / 2.0;
            reach(distance) * middle.width - half < size.width / 2.0
        })
        .count();
    1 + 2 * side
}

/// The strip's room in a widget `bounds` large: beside the docked panel's
/// space, kept while the panel is open even when a card hides it.
pub(crate) fn area(bounds: Rectangle, open: bool) -> Rectangle {
    let dock = if open { DOCK_WIDTH } else { 0.0 };
    Rectangle::new(
        bounds.position(),
        Size::new((bounds.width - dock).max(0.0), bounds.height),
    )
}

/// The `Cover` decode height for a middle cover `height` physical pixels
/// tall: rounded up a step so a small resize doesn't decode again, and
/// none at all while the grid's poster decode is as large.
pub(crate) fn cover_height(height: f32) -> Option<u16> {
    let grid = 480.0;
    if height <= grid {
        return None;
    }
    let stepped = ((height / COVER_STEP).ceil() * COVER_STEP).min(COVER_MAX);
    Some(stepped as u16)
}

/// What a stop is, kept apart from its index so a click that lands after
/// the order changed still finds it, or nothing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum StopId {
    Card(SlotId),
    Collapsed(SectionKey),
}

impl StopId {
    pub(crate) fn of(places: &Places, stop: Stop) -> Option<Self> {
        match stop {
            Stop::Card(index) => places.ids.get(index).cloned().map(Self::Card),
            Stop::Collapsed { header, .. } => places
                .headers
                .get(header)
                .map(|place| Self::Collapsed(place.header.key.clone())),
        }
    }
}

/// Where the strip is in `places`.
pub(crate) fn find(places: &Places, id: &StopId) -> Option<usize> {
    places
        .strip
        .iter()
        .position(|&stop| StopId::of(places, stop).as_ref() == Some(id))
}

/// The stop of the card slot `index`.
pub(crate) fn card_stop(places: &Places, index: usize) -> Option<usize> {
    places
        .strip
        .iter()
        .position(|stop| *stop == Stop::Card(index))
}

/// The stop of the collapsed section `key`'s card.
pub(crate) fn collapsed_stop(places: &Places, key: &SectionKey) -> Option<usize> {
    places.strip.iter().position(|stop| match stop {
        Stop::Collapsed { header, .. } => places
            .headers
            .get(*header)
            .is_some_and(|place| &place.header.key == key),
        Stop::Card(_) => false,
    })
}

/// The section the stop `at` belongs to, by header index: the last header
/// starting at or before it.
fn section_of(places: &Places, at: usize) -> Option<usize> {
    places
        .headers
        .iter()
        .rposition(|place| place.slot.x as usize <= at)
}

/// The stop an arrow moves to from `current`, the first stop without one.
/// Left and Right move one stop and stop at the ends. Up and Down go to
/// the first stop of the section before or after, and stay at the first or
/// last section, or with no sections.
pub(crate) fn walk(places: &Places, current: Option<usize>, direction: Direction) -> Option<usize> {
    let count = places.strip.len();
    if count == 0 {
        return None;
    }
    let Some(at) = current.filter(|&at| at < count) else {
        return Some(0);
    };
    Some(match direction {
        Direction::Left => at.saturating_sub(1),
        Direction::Right => (at + 1).min(count - 1),
        Direction::Up | Direction::Down => {
            let Some(section) = section_of(places, at) else {
                return Some(at);
            };
            let next = match direction {
                Direction::Up => section.checked_sub(1),
                _ => Some(section + 1),
            };
            next.and_then(|next| places.headers.get(next))
                .map_or(at, |place| place.slot.x as usize)
        }
    })
}

/// The stop PageUp or PageDown moves to: `covers` stops left or right,
/// stopping at the ends. With nothing under the cursor it's the first.
pub(crate) fn page(
    places: &Places,
    current: Option<usize>,
    direction: Direction,
    covers: usize,
) -> Option<usize> {
    let count = places.strip.len();
    if count == 0 {
        return None;
    }
    let Some(at) = current.filter(|&at| at < count) else {
        return Some(0);
    };
    Some(match direction {
        Direction::Up | Direction::Left => at.saturating_sub(covers),
        Direction::Down | Direction::Right => (at + covers).min(count - 1),
    })
}

/// The strip's place. Moves snap for now, so it rests on the stop it was
/// sent to.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Strip {
    target: usize,
    position: f32,
    /// The strip widget's size as last laid out, before the panel's room.
    size: Size,
}

impl Strip {
    pub(crate) fn position(&self) -> f32 {
        self.position
    }

    pub(crate) fn target(&self) -> usize {
        self.target
    }

    pub(crate) fn center(&mut self, stop: usize) {
        self.target = stop;
        self.position = stop as f32;
    }

    /// Resting on its stop. Covers decode their large art only then, so a
    /// glide past them queues nothing.
    pub(crate) fn settled(&self) -> bool {
        self.position == self.target as f32
    }

    pub(crate) fn size(&self) -> Size {
        self.size
    }
}

/// The strip widget's input.
#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A press on a side cover, which brings it to the middle.
    Center(StopId),
    /// A press on the collapsed section card in the middle, which opens it.
    Open(SectionKey),
    /// Wheel notches: positive to the right.
    Step(i32),
    /// The widget's size, sent when its layout changes it.
    Measured(Size),
}

impl Event {
    pub(crate) fn is_input(&self) -> bool {
        !matches!(self, Self::Measured(_))
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Center(id) => {
            let places = app.grid_places();
            match find(&places, &id) {
                Some(stop) => app.step_strip(|_, _| Some(stop)),
                None => Task::none(),
            }
        }
        Event::Open(key) => app.open_collapsed(key),
        Event::Step(notches) => app.step_strip(|places, current| {
            let direction = if notches < 0 {
                Direction::Left
            } else {
                Direction::Right
            };
            page(places, current, direction, notches.unsigned_abs() as usize)
        }),
        Event::Measured(size) => {
            if app.strip.size == size {
                return Task::none();
            }
            app.strip.size = size;
            // The middle cover's art follows its size.
            app.load_visible_art()
        }
    }
}

impl App {
    /// Whether a collapsed section's card is in coverflow's middle, which
    /// hides the panel without closing it: Esc, closing the panel, and the
    /// view all read this one rule.
    pub(crate) fn panel_hidden_by_card(&self) -> bool {
        self.layout == ViewLayout::Coverflow && self.header_cursor().is_some()
    }

    /// The stop the cursor is on in `places`: its collapsed section's card
    /// or the slot of its game.
    pub(crate) fn cursor_stop(&self, places: &Places) -> Option<usize> {
        match self.header_cursor() {
            Some(key) => collapsed_stop(places, key),
            None => self
                .selected_index(places)
                .and_then(|index| card_stop(places, index)),
        }
    }

    /// The strip's room: the widget's size once laid out, and before that
    /// what the window leaves it.
    pub(crate) fn strip_size(&self) -> Size {
        let size = self.strip.size();
        let size = if size.width > 0.0 && size.height > 0.0 {
            size
        } else {
            Size::new(self.viewport_width, self.viewport_height)
        };
        area(Rectangle::with_size(size), self.panel_open).size()
    }

    /// Puts the cursor on `stop`: its game's slot, or a collapsed section's
    /// card with that section's first game selected.
    fn set_stop(&mut self, places: &Places, stop: usize) {
        match places.strip.get(stop) {
            Some(Stop::Card(index)) => self.set_cursor(places.ids[*index].clone()),
            Some(Stop::Collapsed { header, first }) => {
                self.selected = first
                    .and_then(|index| self.cards.get(index))
                    .map(|card| card.name.clone());
                self.cursor = Some(crate::app_selection::Cursor::Header(
                    places.headers[*header].header.key.clone(),
                ));
            }
            None => {}
        }
    }

    /// Moves the cursor to the stop `pick` chooses from where it is now,
    /// and centers the strip on it.
    pub(crate) fn step_strip(
        &mut self,
        pick: impl FnOnce(&Places, Option<usize>) -> Option<usize>,
    ) -> Task<Message> {
        let old = self.selected.clone();
        let places = self.grid_places();
        if places.strip.is_empty() {
            return Task::none();
        }
        if let Some(next) = pick(&places, self.cursor_stop(&places)) {
            self.set_stop(&places, next);
        }
        if self.selected != old {
            self.selection_changed();
        }
        self.center_strip();
        Task::batch([self.load_visible_art(), self.load_panel_art()])
    }

    /// Centers the strip on the cursor's stop.
    pub(crate) fn center_strip(&mut self) {
        let places = self.grid_places();
        if let Some(stop) = self.cursor_stop(&places) {
            self.strip.center(stop);
        }
    }

    /// Opens the collapsed section `key` from its card, landing on its
    /// first game.
    pub(crate) fn open_collapsed(&mut self, key: SectionKey) -> Task<Message> {
        if !self.collapsed.contains(&key) {
            return Task::none();
        }
        self.toggle_section(key)
    }

    /// Keeps the middle cover what Enter acts on, after anything that may
    /// have moved the selection, the order, or the sections: a header
    /// cursor on an open section goes to its first game; a selection the
    /// strip doesn't show goes to its collapsed section's card, or with
    /// none, or nothing selected, to the first stop. Returns the art the
    /// new middle needs when it moved.
    pub(crate) fn sync_strip(&mut self) -> Task<Message> {
        if self.layout != ViewLayout::Coverflow {
            return Task::none();
        }
        let before = (
            self.selected.clone(),
            self.cursor.clone(),
            self.strip.target,
        );
        let places = self.grid_places();
        if let Some(key) = self.header_cursor().cloned() {
            match places
                .headers
                .iter()
                .position(|place| place.header.key == key)
            {
                Some(header) if places.headers[header].header.collapsed => {
                    let first = places.strip.iter().find_map(|stop| match stop {
                        Stop::Collapsed { header: at, first } if *at == header => *first,
                        _ => None,
                    });
                    let first = first
                        .and_then(|index| self.cards.get(index))
                        .map(|card| card.name.clone());
                    if self.selected.is_none() || self.selected == first {
                        self.selected = first;
                    } else {
                        // The selection moved on without the cursor.
                        self.cursor = None;
                    }
                }
                Some(header) => {
                    let at = places.headers[header].header.at;
                    match places.ids.get(at) {
                        Some(slot) => self.set_cursor(slot.clone()),
                        None => self.cursor = None,
                    }
                }
                None => self.cursor = None,
            }
        }
        if self.header_cursor().is_none() && !self.selected_shown() {
            let stop = self
                .selected
                .as_deref()
                .and_then(|name| self.collapsed_holding(&places, name))
                .or_else(|| (!places.strip.is_empty()).then_some(0));
            if let Some(stop) = stop {
                self.set_stop(&places, stop);
            }
        }
        if self.selected != before.0 {
            self.selection_changed();
        }
        self.center_strip();
        if (&self.selected, &self.cursor, self.strip.target) == (&before.0, &before.1, before.2) {
            Task::none()
        } else {
            Task::batch([self.load_visible_art(), self.load_panel_art()])
        }
    }

    /// The card of the first collapsed section holding the game `name`.
    fn collapsed_holding(&self, places: &Places, name: &str) -> Option<usize> {
        let sections = self.order.sections(self.order_inputs());
        let section = sections.iter().find(|section| {
            section.collapsed
                && section
                    .cards
                    .iter()
                    .any(|&index| self.cards.get(index).is_some_and(|card| card.name == name))
        })?;
        collapsed_stop(places, &section.key)
    }

    /// PageUp and PageDown's step in coverflow: the covers on screen.
    pub(crate) fn covers_per_screen(&self) -> usize {
        on_screen(self.strip_size())
    }

    /// The `Cover` kind the middle cover decodes at now, if its grid
    /// poster isn't already large enough.
    pub(crate) fn cover_kind(&self) -> Option<artwork::Kind> {
        let height = middle_size(self.strip_size()).height
            * crate::app_window::factor(self.interface_size)
            * self.display_scale;
        cover_height(height).map(artwork::Kind::Cover)
    }

    /// The art the strip draws, in the order it's needed: the grid
    /// poster of each cover on screen and of the next one past each end,
    /// from the middle out, then, once the strip rests, the larger `Cover`
    /// of the middle and its two neighbors. A held `Cover` at least as
    /// large is kept rather than decoding another. Each key comes once,
    /// with whether it's drawn dimmed. This is all eviction protects for
    /// the strip, so large covers past the middle three age out of the
    /// cache's budget like any art.
    pub(crate) fn strip_art(&self) -> Vec<(ArtKey, bool)> {
        let places = self.grid_places();
        let position = self.strip.position();
        let card_of = |stop: &Stop| match stop {
            Stop::Card(index) => places.cards.get(*index).copied(),
            Stop::Collapsed { first, .. } => *first,
        };
        // The covers on screen and the next one past each end, so a step
        // finds its poster ready, and nothing more is held for the strip.
        let size = self.strip_size();
        let shown = geometry(position, places.strip.len(), size)
            .covers
            .into_iter()
            .filter(|cover| {
                cover.bounds.x < size.width && cover.bounds.x + cover.bounds.width > 0.0
            })
            .map(|cover| cover.stop);
        let (first, last) = shown.fold((usize::MAX, 0), |(first, last), stop| {
            (first.min(stop), last.max(stop))
        });
        let mut near: Vec<usize> = if first > last {
            Vec::new()
        } else {
            (first.saturating_sub(1)..=(last + 1).min(places.strip.len() - 1)).collect()
        };
        near.sort_by(|a, b| {
            (*a as f32 - position)
                .abs()
                .total_cmp(&(*b as f32 - position).abs())
        });
        let mut seen = std::collections::HashSet::new();
        let mut keys = Vec::new();
        let mut push = |key: ArtKey, dimmed: bool| {
            if seen.insert(key.clone()) {
                keys.push((key, dimmed));
            }
        };
        for stop in &near {
            if let Some(card) =
                card_of(&places.strip[*stop]).and_then(|index| self.cards.get(index))
                && let Some(art) = &card.art
            {
                push((art.clone(), artwork::Kind::Grid), card.not_installed());
            }
        }
        if self.strip.settled()
            && let Some(kind) = self.cover_kind()
        {
            let target = self.strip.target();
            let large = [Some(target), target.checked_sub(1), Some(target + 1)];
            for stop in large.into_iter().flatten() {
                let Some(card) = places
                    .strip
                    .get(stop)
                    .and_then(card_of)
                    .and_then(|index| self.cards.get(index))
                else {
                    continue;
                };
                if let Some(art) = &card.art {
                    push(self.held_cover(art, kind), card.not_installed());
                }
            }
        }
        keys
    }

    /// The `Cover` key for `art` at `kind`, or a larger one already held.
    fn held_cover(&self, art: &crate::library::ArtFile, kind: artwork::Kind) -> ArtKey {
        let artwork::Kind::Cover(wanted) = kind else {
            return (art.clone(), kind);
        };
        self.images
            .keys()
            .filter(|(file, _)| file == art)
            .filter_map(|(_, held)| match held {
                artwork::Kind::Cover(height) if *height >= wanted => Some(*height),
                _ => None,
            })
            .min()
            .map_or((art.clone(), kind), |height| {
                (art.clone(), artwork::Kind::Cover(height))
            })
    }

    /// The handle to draw for the stop's card art: its `Cover` when it's
    /// one of the three that get one and it's here, else its grid poster.
    pub(crate) fn strip_handle(
        &self,
        card: &crate::library::Card,
        stop: usize,
    ) -> Option<image::Handle> {
        let art = card.art.as_ref()?;
        let dimmed = card.not_installed();
        let near = stop.abs_diff(self.strip.target()) <= 1;
        let cover = near
            .then(|| self.cover_kind())
            .flatten()
            .map(|kind| self.held_cover(art, kind))
            .and_then(|key| self.images.shown(&key, dimmed));
        cover
            .or_else(|| {
                self.images
                    .shown(&(art.clone(), artwork::Kind::Grid), dimmed)
            })
            .cloned()
    }
}

impl App {
    /// The strip with the section's label above it.
    pub(crate) fn strip_view(&self) -> iced::Element<'_, Message> {
        use crate::coverflow_widget::{Caption, Face};
        let places = self.grid_places();
        let position = self.strip.position();
        let middle = self.strip.target();
        let faces = (0..places.strip.len())
            .filter(|&stop| (stop as f32 - position).abs() < NEIGHBORS as f32 + 1.0)
            .filter_map(|stop| {
                let (card, section) = match places.strip[stop] {
                    Stop::Card(index) => (places.cards.get(index).copied(), None),
                    Stop::Collapsed { header, first } => {
                        let header = &places.headers.get(header)?.header;
                        (first, Some((header.key.title(), header.count)))
                    }
                };
                let card = card.and_then(|index| self.cards.get(index));
                Some(Face {
                    stop,
                    id: StopId::of(&places, places.strip[stop])?,
                    art: card.and_then(|card| self.strip_handle(card, stop)),
                    title: card
                        .map(|card| card.display_title.clone())
                        .unwrap_or_default(),
                    faded: card.is_some_and(|card| card.not_installed() || card.hidden),
                    marked: section.is_none()
                        && card.is_some_and(|card| self.marked.contains(&card.name)),
                    favorite: section.is_none() && card.is_some_and(|card| card.favorite),
                    section,
                })
            })
            .collect();
        let caption = match places.strip.get(middle) {
            Some(Stop::Card(index)) => places
                .cards
                .get(*index)
                .and_then(|index| self.cards.get(*index))
                .map(|card| Caption {
                    title: card.display_title.clone(),
                    state: card.state_word(),
                    dot: card.dot(self.palette),
                    faded: card.not_installed() || card.hidden,
                }),
            _ => None,
        };
        // The middle stop's section, collapsed or open, which the label
        // collapses or opens.
        let label: iced::Element<'_, Message> = match places
            .strip
            .get(middle)
            .and_then(|_| section_of(&places, middle))
            .and_then(|header| places.headers.get(header))
        {
            Some(place) => iced::widget::container(self.section_header(&place.header, 1.0, true))
                .padding(iced::Padding {
                    right: crate::grid_motion::PADDING
                        + if self.panel_open { DOCK_WIDTH } else { 0.0 },
                    ..iced::Padding::from([0.0, crate::grid_motion::PADDING])
                })
                .into(),
            None => iced::widget::Space::new().into(),
        };
        iced::widget::column![
            label,
            crate::coverflow_widget::strip(
                faces,
                places.strip.len(),
                position,
                middle,
                self.panel_open,
                caption,
                self.palette,
            ),
        ]
        .into()
    }
}

#[cfg(test)]
mod tests;
