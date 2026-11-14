//! Coverflow: one strip of covers with the selection in the middle. This
//! holds the strip's place, its geometry, how the keys walk it, and the
//! messages its widget sends; `coverflow_widget` draws it.

use iced::widget::image;
use iced::{Point, Rectangle, Size, Task};

use std::time::{Duration, Instant};

use crate::art_cache::Tilt;
use crate::coverflow_widget::Caption;
use crate::grid_motion::{DOCK_WIDTH, GridLayout, Places, Spring, Stop, ViewLayout};
use crate::keys::Direction;
use crate::slot::{SectionKey, SlotId};
use crate::{App, ArtKey, Message, artwork};

/// Covers drawn on each side of the middle one.
pub(crate) const NEIGHBORS: usize = 4;
/// Stops on each side of the strip's stop, and while it glides of where
/// it's drawn, whose posters and tilts are kept ready: those on screen and
/// two more, so a cover a glide brings in is already turned.
const PREWARM: usize = NEIGHBORS + 2;
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
/// each later step's, so the side covers overlap once tilted.
const NEAR: f32 = 0.68;
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
    /// How far it's turned toward the middle: 0 facing out, -1 fully as a
    /// cover on the left, 1 as one on the right. Its width narrows with it.
    pub(crate) tilt: f32,
}

/// A cover's width, as a share of its flat width, `tilt` of the way turned.
fn tilted_width(tilt: f32) -> f32 {
    1.0 - (1.0 - crate::tilt::WIDTH) * tilt.abs().min(1.0)
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
    middle_size_above(size, 0.0)
}

/// `middle_size`, with the caption also kept above the bottom `bottom` of
/// the strip, where the marks and hint bars float.
fn middle_size_above(size: Size, bottom: f32) -> Size {
    let floor = GridLayout::default().poster_height();
    let room = (size.height - CAPTION - bottom).max(0.0);
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
#[cfg(test)]
pub(crate) fn geometry(position: f32, count: usize, size: Size) -> Geometry {
    geometry_above(position, count, size, 0.0)
}

/// `geometry`, with the middle cover's caption kept above the bottom
/// `bottom` of the strip, where the marks and hint bars float: centered as
/// usual where that's clear, lifted where it isn't, and smaller only when
/// lifting isn't enough.
pub(crate) fn geometry_above(position: f32, count: usize, size: Size, bottom: f32) -> Geometry {
    let middle = middle_size_above(size, bottom);
    let block = middle.height + CAPTION;
    let top = ((size.height - block) / 2.0)
        .min(size.height - bottom - block)
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
            let tilt = offset.clamp(-1.0, 1.0);
            let drawn = Size::new(
                (middle.width * scale * tilted_width(tilt)).round(),
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
                tilt,
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
            let half = middle.width * scale(distance) * tilted_width(1.0) / 2.0;
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

/// The app's card index of the game the stop shows: its own, or a
/// collapsed section's first.
fn stop_card(places: &Places, stop: usize) -> Option<usize> {
    match places.strip.get(stop)? {
        Stop::Card(index) => places.cards.get(*index).copied(),
        Stop::Collapsed { first, .. } => *first,
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

/// How long the middle's title crossfades when the middle changes, in full
/// and reduced motion alike.
pub(crate) const TITLE_FADE: Duration = Duration::from_millis(150);
/// How long a warp that arrives for a cover on screen takes to fade in
/// over its flat art, as long as the title's crossfade.
pub(crate) const WARP_FADE: Duration = TITLE_FADE;
/// How far past its stop the strip may swing as it lands, in stops.
const MAX_OVERSHOOT: f32 = 0.15;
/// How long the strip's size must hold before its large covers decode for
/// it, so a window being dragged bigger doesn't decode at every step.
pub(crate) const RESIZE_SETTLE: Duration = Duration::from_millis(250);

/// The strip's place: the stop it's heading to and where it's drawn on the
/// way, a spring like the grid's cards, stepped by the time between frames
/// so a dropped frame skips ahead rather than slowing the glide.
#[derive(Debug, Clone)]
pub(crate) struct Strip {
    target: usize,
    spring: Spring,
    /// Where the glide set out from, so an overshoot past the stop is
    /// bounded.
    from: f32,
    deadline: Option<Instant>,
    last_tick: Option<Instant>,
    generation: u64,
    /// The caption fading out and how far the new one has come in.
    title: Option<(Caption, f32)>,
    /// The strip widget's size as last laid out, before the panel's room.
    size: Size,
    /// The size its large covers decode for, which follows `size` once a
    /// resize settles.
    cover_size: Size,
    /// Counts resizes, so only the last one's wait settles it.
    resize_generation: u64,
    resize_pending: bool,
    /// When the last warp to arrive for a cover on screen is done fading
    /// in, which keeps frames coming until then.
    warp_fade: Option<Instant>,
}

impl Default for Strip {
    fn default() -> Self {
        Self {
            target: 0,
            spring: Spring::at(0.0, 0.0, Instant::now()),
            from: 0.0,
            deadline: None,
            last_tick: None,
            generation: 0,
            title: None,
            size: Size::ZERO,
            cover_size: Size::ZERO,
            resize_generation: 0,
            resize_pending: false,
            warp_fade: None,
        }
    }
}

impl Strip {
    /// Where the strip is drawn: a stop, or between two while it glides.
    pub(crate) fn position(&self) -> f32 {
        let target = self.target as f32;
        let error = self.spring.position() - target;
        let past = error * (self.from - target) < 0.0 || (self.from == target && error != 0.0);
        if past {
            target + MAX_OVERSHOOT * (error / MAX_OVERSHOOT).tanh()
        } else {
            self.spring.position()
        }
    }

    pub(crate) fn target(&self) -> usize {
        self.target
    }

    /// Heads for `stop`, from where it's drawn now. With `snap` it's there
    /// at once, as reduced motion and a strip nobody sees have it. Either
    /// way the title crossfades from `before`, the old middle's caption.
    pub(crate) fn go(&mut self, stop: usize, snap: bool, before: Option<Caption>, now: Instant) {
        if stop == self.target && (snap || self.deadline.is_some() || self.at_rest()) {
            if snap {
                self.snap();
            }
            return;
        }
        if let Some(before) = before {
            self.title = Some((before, 0.0));
            self.last_tick.get_or_insert(now);
        }
        self.target = stop;
        if snap {
            self.snap();
            return;
        }
        if self.deadline.is_none() {
            self.last_tick = Some(now);
        }
        self.from = self.position();
        self.deadline = Some(now + crate::grid_motion::DURATION);
    }

    /// Puts it on its stop at once, ending a glide but not a title's fade.
    pub(crate) fn snap(&mut self) {
        self.spring.rest(self.target as f32);
        self.from = self.target as f32;
        self.deadline = None;
        self.generation = self.generation.wrapping_add(1);
        if self.title.is_none() {
            self.last_tick = None;
        }
    }

    fn at_rest(&self) -> bool {
        self.deadline.is_none() && self.spring.position() == self.target as f32
    }

    /// One frame at `now`. Returns whether the strip came to rest on it.
    pub(crate) fn tick(&mut self, generation: u64, now: Instant) -> bool {
        if generation != self.generation || !self.active() {
            return false;
        }
        let previous = self.last_tick.unwrap_or(now);
        self.last_tick = Some(now);
        if let Some((_, progress)) = &mut self.title {
            *progress +=
                now.saturating_duration_since(previous).as_secs_f32() / TITLE_FADE.as_secs_f32();
            if *progress >= 1.0 {
                self.title = None;
            }
        }
        if self.warp_fade.is_some_and(|until| now >= until) {
            self.warp_fade = None;
        }
        let mut settled = false;
        if let Some(deadline) = self.deadline {
            let target = self.target as f32;
            if now >= deadline {
                self.spring.rest(target);
                self.deadline = None;
                settled = true;
            } else {
                self.spring.step(target, previous, deadline, now);
            }
        }
        if !self.active() {
            self.last_tick = None;
        }
        settled
    }

    /// Gliding, or the title or a warp still fading.
    pub(crate) fn active(&self) -> bool {
        self.deadline.is_some() || self.title.is_some() || self.warp_fade.is_some()
    }

    /// Asks for frames while a warp that just arrived at `now` fades in.
    pub(crate) fn fade_warp(&mut self, now: Instant) {
        self.warp_fade = Some(now + WARP_FADE);
        self.last_tick.get_or_insert(now);
    }

    /// Resting on its stop. The middle three decode their large art and
    /// warp it only then, so a glide past them queues none; grid posters
    /// and their warps go on while it glides.
    pub(crate) fn settled(&self) -> bool {
        self.deadline.is_none()
    }

    /// The caption fading out under the middle one, and how far along.
    pub(crate) fn title(&self) -> Option<&(Caption, f32)> {
        self.title.as_ref()
    }

    pub(crate) fn size(&self) -> Size {
        self.size
    }

    /// Takes the widget's new size. The first one sizes the large covers
    /// at once; a later one waits for the resize to settle, and gives the
    /// generation whose wait settles it. `None` when nothing changed.
    pub(crate) fn measured(&mut self, size: Size) -> Option<Option<u64>> {
        if size == self.size {
            return None;
        }
        self.size = size;
        if self.cover_size == Size::ZERO {
            self.cover_size = size;
            return Some(None);
        }
        self.resize_generation = self.resize_generation.wrapping_add(1);
        self.resize_pending = true;
        Some(Some(self.resize_generation))
    }

    /// The large covers take the size a resize settled on, when the wait
    /// that ended is the last resize's. Returns whether they did.
    pub(crate) fn resize_settled(&mut self, generation: u64) -> bool {
        if !self.resize_pending || generation != self.resize_generation {
            return false;
        }
        self.resize_pending = false;
        self.cover_size = self.size;
        true
    }

    #[cfg(test)]
    pub(crate) fn resize_generation(&self) -> u64 {
        self.resize_generation
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Frames only while it moves or fades, so a still strip asks for none.
    pub(crate) fn subscription(&self) -> iced::Subscription<Message> {
        if self.active() {
            iced::window::frames()
                .with(self.generation)
                .map(|(generation, now)| Message::Strip(Event::Tick(generation, now)))
        } else {
            iced::Subscription::none()
        }
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
    /// A frame of the glide or the title's fade.
    Tick(u64, Instant),
    /// The wait after the resize with this generation ended.
    ResizeSettled(u64),
}

impl Event {
    pub(crate) fn is_input(&self) -> bool {
        !matches!(
            self,
            Self::Measured(_) | Self::Tick(..) | Self::ResizeSettled(_)
        )
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
        Event::Tick(generation, now) => {
            if app.strip.tick(generation, now) {
                // Settled: the large covers and tilts can start.
                app.load_visible_art()
            } else {
                Task::none()
            }
        }
        Event::Measured(size) => {
            let Some(resize) = app.strip.measured(size) else {
                return Task::none();
            };
            // More or fewer covers fit now; the large ones wait for the
            // resize to settle, the last wait replacing any before it.
            let wait = match resize {
                Some(generation) => {
                    let (task, handle) = Task::perform(
                        async { tokio::time::sleep(RESIZE_SETTLE).await },
                        move |()| Message::Strip(Event::ResizeSettled(generation)),
                    )
                    .abortable();
                    app.resize_wait = Some(handle.abort_on_drop());
                    task
                }
                None => Task::none(),
            };
            Task::batch([app.load_visible_art(), wait])
        }
        Event::ResizeSettled(generation) => {
            if app.strip.resize_settled(generation) {
                app.load_visible_art()
            } else {
                Task::none()
            }
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
        let Some(stop) = self.cursor_stop(&places) else {
            return;
        };
        if stop == self.strip.target() && self.strip.settled() {
            return;
        }
        // Nobody sees a covered or unmeasured strip move, and reduced motion
        // snaps it; the title fades either way.
        let snap = self.motion.reduced() || self.panel_hidden() || self.strip.size() == Size::ZERO;
        let before = (stop != self.strip.target())
            .then(|| self.caption_at(&places, self.strip.target()))
            .flatten();
        self.strip.go(stop, snap, before, crate::clock::now());
    }

    /// The caption for the stop `at`: its game's title and status, or its
    /// section's name and count.
    pub(crate) fn caption_at(&self, places: &Places, at: usize) -> Option<Caption> {
        match places.strip.get(at)? {
            Stop::Card(index) => places
                .cards
                .get(*index)
                .and_then(|index| self.cards.get(*index))
                .map(|card| Caption::Game {
                    title: card.display_title.clone(),
                    state: card.state_word(),
                    dot: card.dot(self.palette),
                    faded: card.not_installed() || card.hidden,
                }),
            Stop::Collapsed { header, .. } => {
                places.headers.get(*header).map(|place| Caption::Section {
                    name: place.header.key.title(),
                    count: place.header.count,
                })
            }
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
    /// cursor on a collapsed section keeps its card with the section's
    /// first game selected, and on an open section goes to that game; a
    /// selection the strip doesn't show goes to its collapsed section's
    /// card, or with none, or nothing selected, to the first stop. Returns
    /// the art the new middle needs when it moved.
    pub(crate) fn sync_strip(&mut self) -> Task<Message> {
        if self.layout != ViewLayout::Coverflow {
            self.strip_art_due = false;
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
                    // The card stands for its section, whose first game can
                    // change under it with the sort or the cards. Every way
                    // of selecting a game leaves the header first.
                    self.selected = first
                        .and_then(|index| self.cards.get(index))
                        .map(|card| card.name.clone());
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
        let due = std::mem::take(&mut self.strip_art_due);
        if !due
            && (&self.selected, &self.cursor, self.strip.target) == (&before.0, &before.1, before.2)
        {
            Task::none()
        } else {
            Task::batch([self.load_art_now(), self.load_panel_art()])
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
        let size = match self.strip.cover_size {
            size if size.width > 0.0 && size.height > 0.0 => {
                area(Rectangle::with_size(size), self.panel_open).size()
            }
            _ => self.strip_size(),
        };
        let height = middle_size(size).height
            * crate::app_window::factor(self.interface_size)
            * self.display_scale;
        cover_height(height).map(artwork::Kind::Cover)
    }

    /// The art the strip draws, in the order it's needed: the grid poster
    /// of each stop `strip_near` holds, in its order, then, once the strip
    /// rests, the larger `Cover` of the middle and its two neighbors. A
    /// held `Cover` at least as large is kept rather than decoding another.
    /// Each key comes once, with whether it's drawn dimmed. This is all
    /// eviction protects for the strip, so large covers past the middle
    /// three age out of the cache's budget like any art.
    pub(crate) fn strip_art(&self) -> Vec<(ArtKey, bool)> {
        let places = self.grid_places();
        let near = self.strip_near(&places);
        let mut seen = std::collections::HashSet::new();
        let mut keys = Vec::new();
        let mut push = |key: ArtKey, dimmed: bool| {
            if seen.insert(key.clone()) {
                keys.push((key, dimmed));
            }
        };
        for stop in &near {
            if let Some(card) = stop_card(&places, *stop).and_then(|index| self.cards.get(index))
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
                let Some(card) = stop_card(&places, stop).and_then(|index| self.cards.get(index))
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

    /// The tilts the strip wants warped, each from the flat art it's made
    /// of, gray or not, and which way: each stop `strip_near` holds, turned
    /// the way it sits from where the strip heads, and while it glides,
    /// also the way it sits from where the strip is drawn, so a cover the
    /// middle passes on the way has both. At rest the middle's two
    /// neighbors warp from their large `Cover`, and from their grid poster
    /// too until that warp is here, so a blank card waits only on the
    /// cheap warp; otherwise every warp is from the grid's poster, cheap
    /// enough to make mid-glide.
    pub(crate) fn strip_tilts(&self) -> Vec<(ArtKey, bool, Tilt)> {
        let places = self.grid_places();
        let target = self.strip.target() as f32;
        let position = self.strip.position();
        let (low, high) = (position.min(target), position.max(target));
        let settled = self.strip.settled();
        let mut tilts = Vec::new();
        for stop in self.strip_near(&places) {
            let Some(card) = stop_card(&places, stop).and_then(|index| self.cards.get(index))
            else {
                continue;
            };
            let Some(art) = &card.art else {
                continue;
            };
            let grid = (art.clone(), artwork::Kind::Grid);
            let source = if settled {
                self.tilt_source(art, stop)
            } else {
                grid.clone()
            };
            let dimmed = card.not_installed();
            let at = stop as f32;
            for (tilt, wanted) in [(Tilt::Left, at < high), (Tilt::Right, at > low)] {
                if !wanted {
                    continue;
                }
                if source != grid && !self.images.contains_tilted(&source, dimmed, tilt) {
                    tilts.push((grid.clone(), dimmed, tilt));
                }
                tilts.push((source.clone(), dimmed, tilt));
            }
        }
        tilts
    }

    /// Whether a cover drawn now shows `art` turned `tilt`'s way.
    pub(crate) fn strip_shows(&self, art: &crate::library::ArtFile, tilt: Tilt) -> bool {
        if self.layout != ViewLayout::Coverflow {
            return false;
        }
        let places = self.grid_places();
        let position = self.strip.position();
        (0..places.strip.len())
            .filter(|&stop| {
                let offset = stop as f32 - position;
                offset != 0.0
                    && offset.abs() < NEIGHBORS as f32 + 1.0
                    && (offset < 0.0) == (tilt == Tilt::Left)
            })
            .filter_map(|stop| stop_card(&places, stop).and_then(|index| self.cards.get(index)))
            .any(|card| card.art.as_ref() == Some(art))
    }

    /// How far the warp of `art` turned `tilt`'s way has faded in: all the
    /// way unless it arrived a moment ago for a cover on screen.
    fn warp_shown(&self, art: &crate::library::ArtFile, tilt: Tilt) -> f32 {
        self.warp_arrived
            .get(&(art.clone(), tilt))
            .map_or(1.0, |at| {
                crate::clock::now()
                    .saturating_duration_since(*at)
                    .as_secs_f32()
                    / WARP_FADE.as_secs_f32()
            })
            .min(1.0)
    }

    /// The flat art the cover at `stop` warps from: its large `Cover` next
    /// to the middle, its grid poster elsewhere.
    fn tilt_source(&self, art: &crate::library::ArtFile, stop: usize) -> ArtKey {
        match self.cover_kind() {
            Some(kind) if stop.abs_diff(self.strip.target()) == 1 => self.held_cover(art, kind),
            _ => (art.clone(), artwork::Kind::Grid),
        }
    }

    /// The cover at `stop` turned `tilt`'s way, warped from the art it
    /// wants or, while that warps, from its grid poster.
    pub(crate) fn strip_tilted(
        &self,
        card: &crate::library::Card,
        stop: usize,
        tilt: Tilt,
    ) -> Option<image::Handle> {
        let art = card.art.as_ref()?;
        let dimmed = card.not_installed();
        self.images
            .shown_tilted(&self.tilt_source(art, stop), dimmed, tilt)
            .or_else(|| {
                self.images
                    .shown_tilted(&(art.clone(), artwork::Kind::Grid), dimmed, tilt)
            })
            .cloned()
    }

    /// The stops whose art the strip holds. At rest, those within
    /// `PREWARM` of its stop, more than are ever on screen, so a cover
    /// sliding in already has its poster and tilt. While it glides, every
    /// stop from `PREWARM` behind where it's drawn to `PREWARM` past the
    /// one it's heading to, so a cover it leaves or passes is turned too;
    /// a jump longer than that keeps only what's on screen and `PREWARM`
    /// ahead at each end, so Home on a huge library asks for no more than
    /// a short one. Nearest where it's drawn first, ahead before behind.
    fn strip_near(&self, places: &Places) -> Vec<usize> {
        let Some(last) = places.strip.len().checked_sub(1) else {
            return Vec::new();
        };
        let target = self.strip.target().min(last);
        let window = |at: usize| at.saturating_sub(PREWARM)..=(at + PREWARM).min(last);
        if self.strip.settled() {
            let mut near: Vec<usize> = window(target).collect();
            near.sort_by_key(|stop| stop.abs_diff(target));
            return near;
        }
        let position = self.strip.position().clamp(0.0, last as f32);
        let right = target as f32 >= position;
        let (floor, ceil) = (position.floor() as usize, position.ceil() as usize);
        let reach = NEIGHBORS + 1 + PREWARM;
        let start = if right {
            floor.saturating_sub(PREWARM)..=(ceil + reach).min(last)
        } else {
            floor.saturating_sub(reach)..=(ceil + PREWARM).min(last)
        };
        let (low, high) = (floor.min(target), ceil.max(target));
        let mut near: Vec<usize> = (low.saturating_sub(PREWARM)..=(high + PREWARM).min(last))
            .filter(|stop| start.contains(stop) || window(target).contains(stop))
            .collect();
        let behind = |stop: usize| ((stop as f32) < position) == right;
        near.sort_by(|a, b| {
            (*a as f32 - position)
                .abs()
                .total_cmp(&(*b as f32 - position).abs())
                .then_with(|| behind(*a).cmp(&behind(*b)))
        });
        near
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
        use crate::coverflow_widget::Face;
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
                // The side it's on as drawn now, which a glide can change.
                let side = if (stop as f32) < position {
                    Tilt::Left
                } else {
                    Tilt::Right
                };
                Some(Face {
                    stop,
                    id: StopId::of(&places, places.strip[stop])?,
                    art: card.and_then(|card| self.strip_handle(card, stop)),
                    tilted: card
                        .filter(|_| stop as f32 != position)
                        .and_then(|card| self.strip_tilted(card, stop, side)),
                    warp_shown: card
                        .and_then(|card| card.art.as_ref())
                        .map_or(1.0, |art| self.warp_shown(art, side)),
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
        let captions = crate::coverflow_widget::Captions {
            now: self.caption_at(&places, middle),
            before: self.strip.title().map(|(caption, _)| caption.clone()),
            progress: self.strip.title().map_or(1.0, |(_, progress)| *progress),
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
                self.bar_room(),
                captions,
                self.palette,
            ),
        ]
        .into()
    }
}

#[cfg(test)]
mod tests;
