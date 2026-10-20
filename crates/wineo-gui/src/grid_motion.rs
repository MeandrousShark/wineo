use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::advanced::image::Renderer as _;
use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::{Outcome, Scrollable};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget};
use iced::widget::image;
use iced::{
    Element, Event, Fill, Length, Point, Radians, Rectangle, Renderer, Size, Task, Theme, Vector,
};

use crate::collapse::{Collapse, Look};
use crate::{CARD_WIDTH, GAP, SCROLL_ID};
use wineo_core::library::{ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

const DURATION: Duration = Duration::from_millis(300);
/// Cards that arrive or leave fade over this long.
const FADE: Duration = Duration::from_millis(150);
const FREQUENCY: f32 = 28.0;
const DAMPING: f32 = 0.62;
const SETTLE: Duration = Duration::from_millis(70);
const MAX_OVERSHOOT: f32 = 8.0;
const STAGGER: Duration = Duration::from_millis(5);
const MAX_STAGGER: Duration = Duration::from_millis(35);
pub(crate) const PANEL_WIDTH: f32 = 420.0;
pub(crate) const RAIL_WIDTH: f32 = 10.0;
pub(crate) const DOCK_WIDTH: f32 = PANEL_WIDTH + RAIL_WIDTH;
pub(crate) const PADDING: f32 = 16.0;
/// The poster's height at 100%, a 2:3 cover under the base `CARD_WIDTH`.
const BASE_POSTER_HEIGHT: f32 = 240.0;
/// The selection border and the gap inside it, on each side of the art.
const FRAME_INSET: f32 = 4.0;
const TILE_TITLE_SIZE: f32 = 14.0;
const TILE_TITLE_MIN: f32 = 12.0;
const TILE_TITLE_MAX: f32 = 18.0;

/// The library grid's card geometry at one zoom. The gap between cards and
/// the grid's padding (`GAP`, `PADDING`) stay fixed at every zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GridLayout {
    zoom: f32,
    card_width: f32,
    poster_height: f32,
}

impl Default for GridLayout {
    fn default() -> Self {
        Self::new(ZOOM_DEFAULT)
    }
}

impl GridLayout {
    /// The geometry at `zoom` percent, clamped to the saved range. The card
    /// width rounds to whole pixels first, and the poster is then 1.5 times
    /// that rounded width, rounded, so every zoom keeps whole-pixel cards
    /// and the same width always gets the same poster.
    pub(crate) fn new(zoom: u16) -> Self {
        Self::at(f32::from(zoom))
    }

    /// The geometry at a zoom between whole percents, for a tween's frames.
    /// A whole `zoom` gives exactly what `new` does.
    pub(crate) fn at(zoom: f32) -> Self {
        let zoom = zoom.clamp(f32::from(ZOOM_MIN), f32::from(ZOOM_MAX));
        let card_width = (CARD_WIDTH * zoom / 100.0).round();
        let poster_height = (card_width * BASE_POSTER_HEIGHT / CARD_WIDTH).round();
        Self {
            zoom,
            card_width,
            poster_height,
        }
    }

    pub(crate) fn card_width(self) -> f32 {
        self.card_width
    }

    /// The poster's frame, selection border included.
    pub(crate) fn poster_height(self) -> f32 {
        self.poster_height
    }

    /// The art's size inside the poster's selection frame.
    pub(crate) fn art_frame(self) -> Size {
        Size::new(
            self.card_width - FRAME_INSET,
            self.poster_height - FRAME_INSET,
        )
    }

    /// The title on an art-less card's tile follows the poster's size, but
    /// stays readable when small and modest when large.
    pub(crate) fn tile_title_size(self) -> f32 {
        (TILE_TITLE_SIZE * self.zoom / 100.0).clamp(TILE_TITLE_MIN, TILE_TITLE_MAX)
    }
}

#[derive(Debug, Clone, Copy)]
struct Spring {
    position: f32,
    velocity: f32,
    raw_position: f32,
    raw_velocity: f32,
    delay_until: Instant,
}

impl Spring {
    fn at(position: f32, velocity: f32, delay_until: Instant) -> Self {
        Self {
            position,
            velocity,
            raw_position: position,
            raw_velocity: velocity,
            delay_until,
        }
    }

    fn advance(&mut self, target: f32, seconds: f32) {
        let decay = (-DAMPING * FREQUENCY * seconds).exp();
        let wave = FREQUENCY * (1.0 - DAMPING * DAMPING).sqrt();
        let error = self.raw_position - target;
        let a = error;
        let b = (self.raw_velocity + DAMPING * FREQUENCY * error) / wave;
        let angle = wave * seconds;
        let oscillation = a * angle.cos() + b * angle.sin();
        self.raw_position = target + decay * oscillation;
        self.raw_velocity = decay
            * (-DAMPING * FREQUENCY * oscillation + wave * (-a * angle.sin() + b * angle.cos()));
    }

    fn present(&mut self, target: f32, deadline: Instant, now: Instant) {
        let remaining = deadline.saturating_duration_since(now);
        let x = (remaining.as_secs_f32() / SETTLE.as_secs_f32()).clamp(0.0, 1.0);
        let envelope = x * x * (3.0 - 2.0 * x);
        let derivative = -6.0 * x * (1.0 - x) / SETTLE.as_secs_f32();
        let error = self.raw_position - target;
        self.position = target + error * envelope;
        self.velocity = self.raw_velocity * envelope + error * derivative;
    }

    /// One frame toward `target`, starting no earlier than its delay.
    fn step(&mut self, target: f32, previous: Instant, deadline: Instant, now: Instant) {
        let start = previous.max(self.delay_until);
        if now > start {
            self.advance(target, now.duration_since(start).as_secs_f32());
        }
        self.present(target, deadline, now);
    }

    fn rest(&mut self, target: f32) {
        self.position = target;
        self.velocity = 0.0;
        self.raw_position = target;
        self.raw_velocity = 0.0;
    }
}

fn stagger(index: usize) -> Duration {
    (STAGGER * (index % 8) as u32).min(MAX_STAGGER)
}

/// A staggered spring per card from 0 to 1 or back, for motion that only
/// goes between two fixed sets of places.
#[derive(Debug, Default)]
pub(crate) struct Sweep {
    springs: Vec<Spring>,
    target: f32,
    last_tick: Option<Instant>,
    deadline: Option<Instant>,
    generation: u64,
}

impl Sweep {
    pub(crate) fn snap(&mut self, open: bool, count: usize) {
        self.target = if open { 1.0 } else { 0.0 };
        self.springs = vec![Spring::at(self.target, 0.0, Instant::now()); count];
        self.last_tick = None;
        self.deadline = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn retarget(&mut self, open: bool, count: usize, now: Instant) {
        let target = if open { 1.0 } else { 0.0 };
        if self.springs.len() != count {
            self.snap(!open, count);
        }
        if self.target == target {
            return;
        }
        let was_active = self.active();
        self.target = target;
        self.last_tick = Some(now);
        self.deadline = Some(now + DURATION);
        self.generation = self.generation.wrapping_add(1);
        for (index, spring) in self.springs.iter_mut().enumerate() {
            spring.raw_position = spring.position;
            spring.raw_velocity = spring.velocity;
            spring.delay_until = if was_active {
                now
            } else {
                now + stagger(index)
            };
        }
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation || !self.active() {
            return;
        }
        let previous = self.last_tick.unwrap_or(now);
        self.last_tick = Some(now);
        let deadline = self.deadline.expect("an active sweep has a deadline");
        for spring in &mut self.springs {
            spring.step(self.target, previous, deadline, now);
        }
        if now >= deadline {
            for spring in &mut self.springs {
                spring.rest(self.target);
            }
            self.deadline = None;
            self.last_tick = None;
        }
    }

    pub(crate) fn progress(&self, index: usize) -> f32 {
        self.springs
            .get(index)
            .map_or(self.target, |spring| spring.position)
    }

    pub(crate) fn active(&self) -> bool {
        self.deadline.is_some()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Where the grid's cards belong, in shown order, measured the way the
/// grid lays them out.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Places {
    pub(crate) names: Vec<String>,
    pub(crate) slots: Vec<Rectangle>,
    pub(crate) columns: usize,
    /// The grid's content height with every card in its slot.
    pub(crate) height: f32,
}

impl Places {
    pub(crate) fn new(
        names: Vec<String>,
        heights: &[f32],
        columns: usize,
        layout: GridLayout,
    ) -> Self {
        let columns = columns.max(1);
        let sizes: Vec<Size> = heights
            .iter()
            .map(|height| Size::new(layout.card_width, *height))
            .collect();
        let (points, height) = slots(&sizes, columns, layout);
        Self {
            names,
            slots: points
                .into_iter()
                .zip(sizes)
                .map(|(point, size)| Rectangle::new(point, size))
                .collect(),
            columns,
            height,
        }
    }

    pub(crate) fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|shown| shown == name)
    }
}

/// A card on the grid: the slot it was last given, and how far from it and
/// how faded it's drawn.
#[derive(Debug, Clone, Copy)]
struct Body {
    slot: Rectangle,
    x: Spring,
    y: Spring,
    /// The offset each axis set out from, which bounds its overshoot.
    from: Vector,
    opacity: f32,
}

impl Body {
    fn offset(&self) -> Vector {
        Vector::new(
            bounded(self.x.position, self.from.x),
            bounded(self.y.position, self.from.y),
        )
    }

    fn moving(&self) -> bool {
        self.x.position != 0.0 || self.y.position != 0.0
    }
}

/// An offset springing back to zero from `from`, with any overshoot past
/// either end held to a few pixels, the way `bounded_axis` holds progress.
fn bounded(offset: f32, from: f32) -> f32 {
    if offset * from < 0.0 || (from == 0.0 && offset != 0.0) {
        MAX_OVERSHOOT * (offset / MAX_OVERSHOOT).tanh()
    } else if offset.abs() > from.abs() {
        from + MAX_OVERSHOOT * ((offset - from) / MAX_OVERSHOOT).tanh()
    } else {
        offset
    }
}

/// A card that left the grid, drawn where it was while it fades out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Leaving {
    pub(crate) name: String,
    pub(crate) bounds: Rectangle,
    pub(crate) opacity: f32,
}

/// The one thing that moves the grid's cards. Each card, by name, is drawn
/// at an offset from its slot that springs back to zero, so any change of
/// slots carries every card on from where it's drawn now. Cards that arrive
/// fade in at their slot, and cards that leave fade out where they were.
#[derive(Debug, Default)]
pub(crate) struct GridMotion {
    bodies: HashMap<String, Body>,
    leaving: Vec<Leaving>,
    columns: usize,
    height: f32,
    /// The content height kept while cards fade out, so the scroll range
    /// doesn't jump under them.
    hold: Option<f32>,
    /// A zoom's anchor, kept in place until the cards it set moving stop.
    pub(crate) anchor: Option<(String, f32)>,
    last_tick: Option<Instant>,
    deadline: Option<Instant>,
    generation: u64,
}

impl GridMotion {
    /// Puts every card in its slot at once, ending any motion.
    pub(crate) fn snap_to(&mut self, places: &Places) {
        let now = Instant::now();
        self.bodies = places
            .names
            .iter()
            .zip(&places.slots)
            .map(|(name, slot)| {
                (
                    name.clone(),
                    Body {
                        slot: *slot,
                        x: Spring::at(0.0, 0.0, now),
                        y: Spring::at(0.0, 0.0, now),
                        from: Vector::ZERO,
                        opacity: 1.0,
                    },
                )
            })
            .collect();
        self.leaving.clear();
        self.columns = places.columns;
        self.height = places.height;
        self.hold = None;
        self.anchor = None;
        self.last_tick = None;
        self.deadline = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Gives the cards new slots and springs each from where it's drawn.
    pub(crate) fn spring_to(&mut self, places: &Places, now: Instant) {
        let was_active = self.active();
        let previous_height = self.content_height_now();
        let mut old = std::mem::take(&mut self.bodies);
        let mut moving = false;
        for (index, (name, slot)) in places.names.iter().zip(&places.slots).enumerate() {
            let delay = if was_active {
                now
            } else {
                now + stagger(index)
            };
            let (drawn, velocity, opacity) = if let Some(body) = old.remove(name) {
                (
                    body.slot.position() + body.offset(),
                    Vector::new(body.x.velocity, body.y.velocity),
                    body.opacity,
                )
            } else if let Some(at) = self.leaving.iter().position(|card| &card.name == name) {
                let card = self.leaving.remove(at);
                (card.bounds.position(), Vector::ZERO, card.opacity)
            } else {
                (slot.position(), Vector::ZERO, 0.0)
            };
            let offset = drawn - slot.position();
            let body = Body {
                slot: *slot,
                x: Spring::at(offset.x, velocity.x, delay),
                y: Spring::at(offset.y, velocity.y, delay),
                from: offset,
                opacity,
            };
            moving |= body.moving() || velocity != Vector::ZERO;
            self.bodies.insert(name.clone(), body);
        }
        let mut left: Vec<_> = old.into_iter().collect();
        // Shown order, so the oldest cards draw first, beneath the rest.
        left.sort_by(|(_, a), (_, b)| {
            (a.slot.y, a.slot.x)
                .partial_cmp(&(b.slot.y, b.slot.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.leaving
            .extend(left.into_iter().map(|(name, body)| Leaving {
                name,
                bounds: Rectangle::new(body.slot.position() + body.offset(), body.slot.size()),
                opacity: body.opacity,
            }));
        self.hold = (!self.leaving.is_empty()).then_some(previous_height);
        self.columns = places.columns;
        self.height = places.height;
        self.deadline = moving.then_some(now + DURATION);
        self.last_tick = Some(now);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Gives the cards new slots without moving them from those slots: a
    /// zoom within one column count only scales the grid, and a spring in
    /// flight carries on toward the scaled slot.
    pub(crate) fn scale_to(&mut self, places: &Places) {
        for (name, slot) in places.names.iter().zip(&places.slots) {
            if let Some(body) = self.bodies.get_mut(name) {
                body.slot = *slot;
            }
        }
        self.columns = places.columns;
        self.height = places.height;
    }

    /// Moves to `places` the way a zoom does: springs when the column count
    /// or the cards changed, and only scales otherwise.
    pub(crate) fn zoom_to(&mut self, places: &Places, now: Instant) {
        let same = places.columns == self.columns
            && places.names.len() == self.bodies.len()
            && places
                .names
                .iter()
                .all(|name| self.bodies.contains_key(name));
        if same {
            self.scale_to(places);
        } else {
            self.spring_to(places, now);
        }
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation || !self.active() {
            return;
        }
        let previous = self.last_tick.unwrap_or(now);
        self.last_tick = Some(now);
        let fade = now.saturating_duration_since(previous).as_secs_f32() / FADE.as_secs_f32();
        let settled = self.deadline.is_some_and(|deadline| now >= deadline);
        for body in self.bodies.values_mut() {
            if let Some(deadline) = self.deadline {
                if settled {
                    body.x.rest(0.0);
                    body.y.rest(0.0);
                } else {
                    body.x.step(0.0, previous, deadline, now);
                    body.y.step(0.0, previous, deadline, now);
                }
            }
            body.opacity = (body.opacity + fade).min(1.0);
        }
        if settled {
            self.deadline = None;
        }
        for card in &mut self.leaving {
            card.opacity -= fade;
        }
        self.leaving.retain(|card| card.opacity > 0.0);
        if self.leaving.is_empty() {
            self.hold = None;
        }
        if !self.active() {
            self.last_tick = None;
        }
    }

    /// Springs are in flight or cards are fading.
    pub(crate) fn active(&self) -> bool {
        self.deadline.is_some()
            || !self.leaving.is_empty()
            || self.bodies.values().any(|body| body.opacity < 1.0)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Frames only while something moves, so an idle grid asks for none.
    pub(crate) fn subscription(&self) -> iced::Subscription<crate::Message> {
        if self.active() {
            iced::window::frames()
                .with(self.generation)
                .map(|(generation, now)| crate::Message::GridTick(generation, now))
        } else {
            iced::Subscription::none()
        }
    }

    /// How far from its slot `name` is drawn.
    pub(crate) fn offset(&self, name: &str) -> Vector {
        self.bodies.get(name).map_or(Vector::ZERO, Body::offset)
    }

    pub(crate) fn opacity(&self, name: &str) -> f32 {
        self.bodies.get(name).map_or(1.0, |body| body.opacity)
    }

    pub(crate) fn leaving(&self) -> &[Leaving] {
        &self.leaving
    }

    /// The content height kept for cards fading out.
    pub(crate) fn hold(&self) -> Option<f32> {
        self.hold
    }

    /// Where each card in `places` is drawn now.
    pub(crate) fn shown(&self, places: &Places) -> Vec<Rectangle> {
        places
            .names
            .iter()
            .zip(&places.slots)
            .map(|(name, slot)| *slot + self.offset(name))
            .collect()
    }

    /// The grid's drawn content height: its slots', or more while cards
    /// overshoot, fade out, or hold the height for those that did.
    pub(crate) fn content_height(&self, places: &Places, shown: &[Rectangle]) -> f32 {
        shown
            .iter()
            .chain(self.leaving.iter().map(|card| &card.bounds))
            .map(|bounds| bounds.y + bounds.height + PADDING)
            .fold(places.height, f32::max)
            .max(self.hold.unwrap_or(0.0))
    }

    /// The drawn height from the slots last given, for holding it.
    fn content_height_now(&self) -> f32 {
        self.bodies
            .values()
            .map(|body| body.slot.y + body.offset().y + body.slot.height + PADDING)
            .chain(
                self.leaving
                    .iter()
                    .map(|card| card.bounds.y + card.bounds.height + PADDING),
            )
            .fold(self.height, f32::max)
            .max(self.hold.unwrap_or(0.0))
    }
}

fn columns(width: f32, layout: GridLayout) -> usize {
    ((width - 2.0 * PADDING + GAP) / (layout.card_width + GAP))
        .floor()
        .max(1.0) as usize
}

pub(crate) fn columns_for_viewport(width: f32, open: bool, layout: GridLayout) -> usize {
    columns(width - if open { DOCK_WIDTH } else { 0.0 }, layout)
}

/// The cards whose rows overlap `top..bottom`, with rows stacked the way
/// `slots` stacks them: each as tall as its tallest card.
pub(crate) fn cards_between(
    heights: &[f32],
    columns: usize,
    top: f32,
    bottom: f32,
) -> std::ops::Range<usize> {
    let columns = columns.max(1);
    let (mut first, mut last) = (None, 0);
    let mut y = PADDING;
    for (row, cards) in heights.chunks(columns).enumerate() {
        let height = cards.iter().copied().fold(0.0, f32::max);
        if y > bottom {
            break;
        }
        if y + height >= top {
            first.get_or_insert(row * columns);
            last = row * columns + cards.len();
        }
        y += height + GAP;
    }
    first.map_or(0..0, |first| first..last)
}

/// Each card's top in the grid and the grid's height, with rows stacked the
/// way `slots` stacks them.
#[cfg(test)]
pub(crate) fn card_tops(heights: &[f32], columns: usize) -> (Vec<f32>, f32) {
    let mut tops = Vec::with_capacity(heights.len());
    let mut y = PADDING;
    for row in heights.chunks(columns.max(1)) {
        tops.extend(std::iter::repeat_n(y, row.len()));
        y += row.iter().copied().fold(0.0, f32::max) + GAP;
    }
    let height = if heights.is_empty() {
        2.0 * PADDING
    } else {
        y - GAP + PADDING
    };
    (tops, height)
}

fn slots(sizes: &[Size], columns: usize, layout: GridLayout) -> (Vec<Point>, f32) {
    let mut result = Vec::with_capacity(sizes.len());
    let mut y = PADDING;
    for row in sizes.chunks(columns) {
        for (column, _) in row.iter().enumerate() {
            result.push(Point::new(
                PADDING + column as f32 * (layout.card_width + GAP),
                y,
            ));
        }
        y += row.iter().map(|size| size.height).fold(0.0, f32::max) + GAP;
    }
    (
        result,
        if sizes.is_empty() {
            2.0 * PADDING
        } else {
            y - GAP + PADDING
        },
    )
}

pub(crate) fn bounded_axis(from: f32, to: f32, progress: f32) -> f32 {
    let travel = to - from;
    if progress < 0.0 {
        from + MAX_OVERSHOOT * (travel * progress / MAX_OVERSHOOT).tanh()
    } else if progress > 1.0 {
        to + MAX_OVERSHOOT * (travel * (progress - 1.0) / MAX_OVERSHOOT).tanh()
    } else {
        from + travel * progress
    }
}

/// What the grid needs while the cards are collapsed. Cards then ignore
/// their own input, and a click on one sends `on_press` instead.
pub(crate) struct Scene<'a, Message> {
    pub(crate) collapse: &'a Collapse,
    /// Each card's poster and its opacity, drawn on its own once loose.
    pub(crate) posters: Vec<Option<(image::Handle, f32)>>,
    pub(crate) on_press: Option<Message>,
}

/// A card leaving the grid: its element and where it's drawn.
pub(crate) struct Outgoing<'a, Message> {
    pub(crate) name: String,
    pub(crate) at: Point,
    pub(crate) card: Element<'a, Message>,
}

/// The grid of `cards`, each with its name, in shown order, then `leaving`
/// drawn beneath them. `open` picks the docked panel's column count.
pub(crate) fn grid<'a, Message: Clone + 'a>(
    cards: Vec<(String, Element<'a, Message>)>,
    leaving: Vec<Outgoing<'a, Message>>,
    motion: &GridMotion,
    open: bool,
    scene: Option<Scene<'a, Message>>,
    layout: GridLayout,
) -> Element<'a, Message> {
    let shown = cards.len();
    let offsets = cards.iter().map(|(name, _)| motion.offset(name)).collect();
    let mut names = Vec::with_capacity(shown + leaving.len());
    let mut elements = Vec::with_capacity(shown + leaving.len());
    for (name, card) in cards {
        names.push(name);
        elements.push(card);
    }
    let mut outgoing = Vec::with_capacity(leaving.len());
    for card in leaving {
        names.push(card.name);
        outgoing.push(card.at);
        elements.push(card.card);
    }
    Element::new(Grid {
        cards: elements,
        names,
        shown,
        offsets,
        outgoing,
        hold: motion.hold().unwrap_or(0.0),
        open,
        layout,
        scene,
    })
}

struct Grid<'a, Message> {
    /// The shown cards, then the leaving ones.
    cards: Vec<Element<'a, Message>>,
    names: Vec<String>,
    shown: usize,
    offsets: Vec<Vector>,
    outgoing: Vec<Point>,
    hold: f32,
    open: bool,
    layout: GridLayout,
    scene: Option<Scene<'a, Message>>,
}

#[derive(Default)]
struct GridState {
    /// Each child's card, so a card keeps its state when the order changes.
    names: Vec<String>,
    width: f32,
    open: bool,
    layout: GridLayout,
    sizes: Vec<Size>,
    slots: Vec<Point>,
    looks: Vec<Look>,
}

impl GridState {
    fn look(&self, index: usize) -> Look {
        self.looks
            .get(index)
            .copied()
            .unwrap_or(Look::Normal(Vector::ZERO))
    }
}

/// Whether `point` is over a card drawn with `look` at `bounds`. A loose
/// card only answers over its rotated poster.
fn hit(look: Look, bounds: Rectangle, point: Point, layout: GridLayout) -> bool {
    match look {
        Look::Hidden => false,
        Look::Normal(_) => bounds.contains(point),
        Look::Loose { rotation, .. } => {
            let poster = poster_bounds(bounds, layout);
            let offset = point - poster.center();
            let (sin, cos) = rotation.sin_cos();
            let x = offset.x * cos + offset.y * sin;
            let y = -offset.x * sin + offset.y * cos;
            x.abs() <= poster.width / 2.0 && y.abs() <= poster.height / 2.0
        }
    }
}

/// The poster's area inside a card's node, within its selection frame.
fn poster_bounds(card: Rectangle, layout: GridLayout) -> Rectangle {
    Rectangle::new(
        card.position() + Vector::new(FRAME_INSET / 2.0, FRAME_INSET / 2.0),
        layout.art_frame(),
    )
}

/// The poster's drawn size: its art fit inside the frame without cropping,
/// since a rotated image can't be clipped to an upright frame.
fn contain(handle: &image::Handle, frame: Rectangle) -> Rectangle {
    let size = match handle {
        image::Handle::Rgba { width, height, .. } if *width > 0 && *height > 0 => {
            let scale = (frame.width / *width as f32).min(frame.height / *height as f32);
            Size::new(*width as f32 * scale, *height as f32 * scale)
        }
        _ => frame.size(),
    };
    Rectangle::new(
        frame.center() - Vector::new(size.width / 2.0, size.height / 2.0),
        size,
    )
}

impl<Message: Clone> Widget<Message, Theme, Renderer> for Grid<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GridState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(GridState {
            names: self.names.clone(),
            ..GridState::default()
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.cards.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<GridState>();
        if state.names != self.names {
            // iced matches children by index; a card's hover and widget
            // state belong to the card, wherever it moved.
            if state.names.len() == tree.children.len() {
                let mut old: HashMap<String, Tree> = std::mem::take(&mut state.names)
                    .into_iter()
                    .zip(std::mem::take(&mut tree.children))
                    .collect();
                tree.children = self
                    .names
                    .iter()
                    .zip(&self.cards)
                    .map(|(name, card)| old.remove(name).unwrap_or_else(|| Tree::new(card)))
                    .collect();
            }
            state.names = self.names.clone();
        }
        tree.diff_children(&self.cards);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width;
        let card_limits = limits.loose();
        let nodes: Vec<_> = self
            .cards
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(card, state)| card.as_widget_mut().layout(state, renderer, &card_limits))
            .collect();
        let sizes: Vec<_> = nodes[..self.shown].iter().map(layout::Node::size).collect();
        let state = tree.state.downcast_mut::<GridState>();
        let grid = self.layout;
        if state.width != width
            || state.open != self.open
            || state.layout != grid
            || state.sizes != sizes
        {
            state.width = width;
            state.open = self.open;
            state.layout = grid;
            state.sizes = sizes.clone();
            let columns = columns(width - if self.open { DOCK_WIDTH } else { 0.0 }, grid);
            state.slots = slots(&sizes, columns, grid).0;
        }
        let mut height = (2.0 * PADDING).max(self.hold);
        let collapse = self.scene.as_ref().map(|scene| scene.collapse);
        state.looks.clear();
        let children = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                let Some(slot) = state.slots.get(index) else {
                    let position = self.outgoing[index - self.shown];
                    height = height.max(position.y + node.size().height + PADDING);
                    return node.move_to(position);
                };
                let offset = self.offsets.get(index).copied().unwrap_or(Vector::ZERO);
                let position = *slot + offset;
                // The content height follows the real slots only, so a
                // collapse never changes the scroll range.
                height = height.max(position.y + sizes[index].height + PADDING);
                let look = collapse.map_or(Look::Normal(Vector::ZERO), |collapse| {
                    collapse.look(index, position)
                });
                state.looks.push(look);
                node.move_to(match look {
                    Look::Normal(offset) => position + offset,
                    Look::Loose { position, .. } => position,
                    Look::Hidden => position,
                })
            })
            .collect();
        layout::Node::with_children(Size::new(width, height), children)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            // Leaving cards are no target for focus or a scroll to them.
            self.cards[..self.shown]
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .enumerate()
                .for_each(|(index, ((card, state), layout))| {
                    let id = Id::from(format!("library-card:{}", self.names[index]));
                    operation.container(Some(&id), layout.bounds());
                    card.as_widget_mut()
                        .operate(state, layout, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        mut cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Some(scene) = &self.scene {
            if let Event::Mouse(mouse::Event::ButtonPressed(button)) = event
                && let Some(point) = cursor.position()
                && viewport.contains(point)
            {
                let state = tree.state.downcast_ref::<GridState>();
                let over = layout
                    .children()
                    .take(self.shown)
                    .enumerate()
                    .any(|(index, layout)| {
                        hit(state.look(index), layout.bounds(), point, self.layout)
                    });
                if over {
                    if *button == mouse::Button::Left
                        && let Some(message) = &scene.on_press
                    {
                        shell.publish(message.clone());
                    }
                    shell.capture_event();
                }
            }
            return;
        }
        // Leaving cards take no input.
        for ((card, state), layout) in self.cards[..self.shown]
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .rev()
        {
            card.as_widget_mut().update(
                state, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
            if shell.is_event_captured() {
                return;
            }
            if cursor.is_over(layout.bounds()) {
                cursor = cursor.levitate();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let grid = tree.state.downcast_ref::<GridState>();
        let children: Vec<_> = self
            .cards
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .collect();
        let (shown, leaving) = children.split_at(self.shown.min(children.len()));
        for ((card, state), layout) in leaving {
            if layout.bounds().intersects(viewport) {
                card.as_widget()
                    .draw(state, renderer, theme, style, *layout, cursor, viewport);
            }
        }
        for (index, ((card, state), layout)) in shown.iter().enumerate() {
            let layout = *layout;
            match grid.look(index) {
                Look::Hidden => {}
                Look::Normal(_) => {
                    if layout.bounds().intersects(viewport) {
                        card.as_widget()
                            .draw(state, renderer, theme, style, layout, cursor, viewport);
                    }
                }
                Look::Loose { rotation, .. } => {
                    let Some(scene) = &self.scene else { continue };
                    let Some((handle, opacity)) = scene.posters.get(index).cloned().flatten()
                    else {
                        continue;
                    };
                    let frame = poster_bounds(layout.bounds(), self.layout);
                    // Clipped only to the view, so the rotated corners show.
                    renderer.draw_image(
                        iced::advanced::image::Image::new(handle.clone())
                            .rotation(Radians(rotation))
                            .opacity(opacity),
                        contain(&handle, frame),
                        *viewport,
                    );
                }
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.scene.is_some() {
            let state = tree.state.downcast_ref::<GridState>();
            let over = cursor.position().is_some_and(|point| {
                layout
                    .children()
                    .take(self.shown)
                    .enumerate()
                    .any(|(index, layout)| {
                        hit(state.look(index), layout.bounds(), point, self.layout)
                    })
            });
            return if over {
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::default()
            };
        }
        self.cards[..self.shown]
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .rev()
            .find_map(|((card, state), layout)| {
                if !cursor.is_over(layout.bounds()) {
                    return None;
                }
                let interaction = card
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer);
                (interaction != mouse::Interaction::None).then_some(interaction)
            })
            .unwrap_or_default()
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        if self.scene.is_some() {
            return None;
        }
        overlay::from_children(
            &mut self.cards[..self.shown],
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CardBounds {
    pub(crate) card: Rectangle,
    pub(crate) viewport: Rectangle,
    pub(crate) offset: Vector,
}

struct LocateCard {
    card_id: Id,
    scroll_id: Id,
    card: Option<Rectangle>,
    viewport: Option<Rectangle>,
    offset: Vector,
}

impl Operation<CardBounds> for LocateCard {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<CardBounds>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        if id == Some(&self.card_id) {
            self.card = Some(bounds);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.scroll_id) {
            self.viewport = Some(bounds);
            self.offset = translation;
        }
    }

    fn finish(&self) -> Outcome<CardBounds> {
        match (self.card, self.viewport) {
            (Some(card), Some(viewport)) => Outcome::Some(CardBounds {
                card,
                viewport,
                offset: self.offset,
            }),
            _ => Outcome::None,
        }
    }
}

pub(crate) fn locate_card(name: &str) -> Task<CardBounds> {
    locate(Id::from(format!("library-card:{name}")), Id::new(SCROLL_ID))
}

/// Finds a container and the scrollable it sits in, by their ids.
pub(crate) fn locate(card_id: Id, scroll_id: Id) -> Task<CardBounds> {
    iced::advanced::widget::operate(LocateCard {
        card_id,
        scroll_id,
        card: None,
        viewport: None,
        offset: Vector::ZERO,
    })
}

#[cfg(test)]
mod app_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_reverses_continuously_and_settles() {
        let start = Instant::now();
        let mut motion = Sweep::default();
        motion.snap(false, 4);
        motion.retarget(true, 4, start);
        motion.tick(motion.generation(), start + Duration::from_millis(90));
        let position = motion.progress(0);
        let velocity = motion.springs[0].velocity;
        assert!(position > 0.0 && position < 1.0);
        motion.retarget(false, 4, start + Duration::from_millis(90));
        assert_eq!(motion.progress(0), position);
        assert_eq!(motion.springs[0].velocity, velocity);
        motion.tick(motion.generation(), start + Duration::from_millis(390));
        assert!(!motion.active());
        assert_eq!(motion.progress(0), 0.0);
    }

    #[test]
    fn long_slot_travel_has_small_overshoot_and_smooth_deadline() {
        let start = Instant::now();
        let mut motion = Sweep::default();
        motion.snap(false, 1);
        motion.retarget(true, 1, start);
        let generation = motion.generation();
        motion.tick(generation, start + Duration::from_millis(140));
        let point = bounded_axis(0.0, 3000.0, motion.progress(0));
        assert!((3000.0..=3008.0).contains(&point));
        motion.tick(generation, start + Duration::from_millis(299));
        let point = bounded_axis(0.0, 3000.0, motion.progress(0));
        assert!((point - 3000.0).abs() < 0.05);
        assert!(motion.springs[0].velocity.abs() < 0.1);
        motion.tick(generation, start + DURATION);
        assert_eq!(motion.progress(0), 1.0);
        assert_eq!(motion.springs[0].velocity, 0.0);
    }

    #[test]
    fn stale_ticks_cannot_move_a_retargeted_grid() {
        let start = Instant::now();
        let mut motion = Sweep::default();
        motion.snap(false, 8);
        motion.retarget(true, 8, start);
        let stale = motion.generation();
        motion.tick(stale, start + Duration::from_millis(120));
        motion.retarget(false, 8, start + Duration::from_millis(120));
        let position = motion.progress(7);
        let velocity = motion.springs[7].velocity;
        motion.tick(stale, start + Duration::from_millis(300));
        assert_eq!(motion.progress(7), position);
        assert_eq!(motion.springs[7].velocity, velocity);
        motion.tick(motion.generation(), start + Duration::from_millis(220));
        assert!(motion.progress(7) < position);
    }

    #[test]
    fn loose_cards_answer_clicks_only_over_their_rotated_poster() {
        let bounds = Rectangle::new(Point::new(100.0, 100.0), Size::new(160.0, 300.0));
        let grid = GridLayout::default();
        let poster = poster_bounds(bounds, grid);
        let corner = poster.position() + Vector::new(4.0, 4.0);
        let caption = Point::new(180.0, 380.0);
        let upright = Look::Loose {
            position: bounds.position(),
            rotation: 0.0,
        };
        let turned = Look::Loose {
            position: bounds.position(),
            rotation: std::f32::consts::FRAC_PI_4,
        };
        assert!(hit(upright, bounds, corner, grid));
        assert!(!hit(upright, bounds, caption, grid));
        assert!(!hit(turned, bounds, corner, grid));
        assert!(hit(turned, bounds, poster.center(), grid));
        assert!(hit(Look::Normal(Vector::ZERO), bounds, caption, grid));
        assert!(!hit(Look::Hidden, bounds, poster.center(), grid));
    }

    #[test]
    fn layout_scales_cards_and_posters_with_the_zoom() {
        for (zoom, width, poster, art) in [
            (60, 96.0, 144.0, Size::new(92.0, 140.0)),
            (100, 160.0, 240.0, Size::new(156.0, 236.0)),
            (200, 320.0, 480.0, Size::new(316.0, 476.0)),
        ] {
            let layout = GridLayout::new(zoom);
            assert_eq!(layout.card_width(), width, "{zoom}");
            assert_eq!(layout.poster_height(), poster, "{zoom}");
            assert_eq!(layout.art_frame(), art, "{zoom}");
        }
        assert_eq!(GridLayout::default(), GridLayout::new(100));
        // The widest art frame still fits the 320 x 480 grid decode.
        let art = GridLayout::new(ZOOM_MAX).art_frame();
        assert!(art.width <= 320.0 && art.height <= 480.0);
    }

    #[test]
    fn layout_rounds_the_width_before_the_poster() {
        // 116.8 rounds to 117, and the poster is 1.5 times that: 175.5.
        let layout = GridLayout::new(73);
        assert_eq!(layout.card_width(), 117.0);
        assert_eq!(layout.poster_height(), 176.0);
        // 219.2 rounds to 219, then 328.5 rounds up.
        let layout = GridLayout::new(137);
        assert_eq!(layout.card_width(), 219.0);
        assert_eq!(layout.poster_height(), 329.0);
        for zoom in ZOOM_MIN..=ZOOM_MAX {
            let layout = GridLayout::new(zoom);
            assert_eq!(layout.card_width().fract(), 0.0, "{zoom}");
            assert_eq!(layout.poster_height().fract(), 0.0, "{zoom}");
            assert_eq!(
                layout.poster_height(),
                (layout.card_width() * 1.5).round(),
                "{zoom}"
            );
        }
    }

    #[test]
    fn layout_clamps_zooms_outside_the_range() {
        assert_eq!(GridLayout::new(0), GridLayout::new(ZOOM_MIN));
        assert_eq!(GridLayout::new(59), GridLayout::new(ZOOM_MIN));
        assert_eq!(GridLayout::new(201), GridLayout::new(ZOOM_MAX));
        assert_eq!(GridLayout::new(u16::MAX), GridLayout::new(ZOOM_MAX));
        assert_eq!(GridLayout::new(0).card_width(), 96.0);
        assert_eq!(GridLayout::new(u16::MAX).card_width(), 320.0);
    }

    #[test]
    fn tile_titles_scale_with_the_poster_inside_their_limits() {
        assert_eq!(GridLayout::new(100).tile_title_size(), 14.0);
        assert_eq!(GridLayout::new(60).tile_title_size(), 12.0);
        assert_eq!(GridLayout::new(85).tile_title_size(), 12.0);
        assert_eq!(GridLayout::new(120).tile_title_size(), 16.8);
        assert_eq!(GridLayout::new(150).tile_title_size(), 18.0);
        assert_eq!(GridLayout::new(200).tile_title_size(), 18.0);
    }

    #[test]
    fn columns_follow_the_zoom_and_the_docked_panel() {
        let at = |zoom| GridLayout::new(zoom);
        // A column fits once the width reaches 2 * padding + n * card + (n - 1) * gap.
        for (zoom, width, closed) in [
            (100, 895.0, 4),
            (100, 896.0, 5),
            (100, 1000.0, 5),
            (60, 575.0, 4),
            (60, 576.0, 5),
            (60, 1000.0, 8),
            (200, 687.0, 1),
            (200, 688.0, 2),
            (200, 1000.0, 2),
            (200, 100.0, 1),
        ] {
            assert_eq!(
                columns_for_viewport(width, false, at(zoom)),
                closed,
                "{zoom}% at {width}"
            );
        }
        for (zoom, width, open) in [
            (100, 1325.0, 4),
            (100, 1326.0, 5),
            (60, 1006.0, 5),
            (200, 1118.0, 2),
            (200, 1117.0, 1),
            (100, 400.0, 1),
        ] {
            assert_eq!(
                columns_for_viewport(width, true, at(zoom)),
                open,
                "{zoom}% at {width} docked"
            );
        }
    }

    #[test]
    fn slots_and_poster_bounds_follow_the_zoom() {
        for (zoom, step) in [(60, 96.0 + GAP), (200, 320.0 + GAP)] {
            let layout = GridLayout::new(zoom);
            let card = Size::new(layout.card_width(), layout.poster_height() + 60.0);
            let (points, height) = slots(&[card; 5], 2, layout);
            let xs: Vec<f32> = points.iter().map(|point| point.x).collect();
            assert_eq!(
                xs,
                [PADDING, PADDING + step, PADDING, PADDING + step, PADDING]
            );
            assert_eq!(points[2].y, PADDING + card.height + GAP);
            assert_eq!(height, PADDING + 3.0 * card.height + 2.0 * GAP + PADDING);

            let bounds = Rectangle::new(points[1], card);
            let poster = poster_bounds(bounds, layout);
            assert_eq!(poster.position(), points[1] + Vector::new(2.0, 2.0));
            assert_eq!(poster.size(), layout.art_frame());
            let look = Look::Loose {
                position: bounds.position(),
                rotation: 0.0,
            };
            let inside = poster.position() + Vector::new(poster.width - 1.0, poster.height - 1.0);
            assert!(hit(look, bounds, inside, layout), "{zoom}");
            let below = Point::new(poster.center().x, poster.y + poster.height + 4.0);
            assert!(!hit(look, bounds, below, layout), "{zoom}");
        }
        // A point on a 200% poster's lower corner is outside the 60% one.
        let corner = Point::new(PADDING + 300.0, PADDING + 400.0);
        let card = Rectangle::new(Point::new(PADDING, PADDING), Size::new(320.0, 540.0));
        let look = Look::Loose {
            position: card.position(),
            rotation: 0.0,
        };
        assert!(hit(look, card, corner, GridLayout::new(200)));
        assert!(!hit(look, card, corner, GridLayout::new(60)));
    }

    #[test]
    fn fractional_zooms_round_like_whole_ones() {
        for zoom in ZOOM_MIN..=ZOOM_MAX {
            assert_eq!(GridLayout::at(f32::from(zoom)), GridLayout::new(zoom));
        }
        let between = GridLayout::at(100.4);
        assert_eq!(between.card_width(), 161.0);
        assert_eq!(between.poster_height(), 242.0);
        assert_eq!(GridLayout::at(10.0), GridLayout::new(ZOOM_MIN));
    }

    #[test]
    fn card_tops_match_the_slots() {
        let heights = [280.0, 315.0, 270.0, 300.0, 290.0];
        let sizes: Vec<Size> = heights
            .iter()
            .map(|height| Size::new(160.0, *height))
            .collect();
        let (points, height) = slots(&sizes, 2, GridLayout::default());
        let (tops, total) = card_tops(&heights, 2);
        assert_eq!(tops, points.iter().map(|point| point.y).collect::<Vec<_>>());
        assert_eq!(total, height);
        assert_eq!(card_tops(&[], 3), (Vec::new(), 2.0 * PADDING));
    }

    #[test]
    fn slots_use_tallest_card_in_each_row() {
        let sizes = [
            Size::new(160.0, 280.0),
            Size::new(160.0, 315.0),
            Size::new(160.0, 270.0),
        ];
        let (points, height) = slots(&sizes, 2, GridLayout::default());
        assert_eq!(points[2].y, PADDING + 315.0 + GAP);
        assert_eq!(height, PADDING + 315.0 + GAP + 270.0 + PADDING);
    }

    fn places(names: &[&str], columns: usize, layout: GridLayout) -> Places {
        let heights = vec![layout.poster_height() + 60.0; names.len()];
        Places::new(
            names.iter().map(|name| (*name).to_owned()).collect(),
            &heights,
            columns,
            layout,
        )
    }

    fn drawn(motion: &GridMotion, places: &Places, name: &str) -> Point {
        motion.shown(places)[places.index(name).unwrap()].position()
    }

    fn frames(motion: &GridMotion) -> usize {
        iced::advanced::subscription::into_recipes(motion.subscription()).len()
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01
    }

    #[test]
    fn a_new_order_settles_every_card_at_its_slot() {
        let layout = GridLayout::default();
        let start = Instant::now();
        let mut motion = GridMotion::default();
        let before = places(&["a", "b", "c", "d", "e"], 2, layout);
        motion.snap_to(&before);
        assert!(!motion.active());
        assert_eq!(frames(&motion), 0);
        let after = places(&["e", "d", "c", "b", "a"], 2, layout);
        motion.spring_to(&after, start);
        assert!(motion.active());
        assert_eq!(frames(&motion), 1);
        // Each card starts where it was drawn.
        for name in ["a", "b", "c", "d", "e"] {
            assert_eq!(
                drawn(&motion, &after, name),
                before.slots[before.index(name).unwrap()].position()
            );
        }
        let generation = motion.generation();
        motion.tick(generation, start + Duration::from_millis(140));
        assert!(motion.active());
        motion.tick(generation, start + DURATION);
        assert!(!motion.active());
        assert_eq!(frames(&motion), 0);
        for (name, slot) in after.names.iter().zip(&after.slots) {
            assert_eq!(drawn(&motion, &after, name), slot.position());
        }
    }

    #[test]
    fn a_retarget_mid_flight_starts_from_the_drawn_place() {
        let layout = GridLayout::default();
        let start = Instant::now();
        let mut motion = GridMotion::default();
        // The dock: three columns become two.
        motion.snap_to(&places(&["a", "b", "c", "d"], 3, layout));
        let docked = places(&["a", "b", "c", "d"], 2, layout);
        motion.spring_to(&docked, start);
        let mid = start + Duration::from_millis(90);
        motion.tick(motion.generation(), mid);
        let at = drawn(&motion, &docked, "c");
        let velocity = motion.bodies["c"].y.velocity;
        assert!(at != docked.slots[2].position());
        // A re-sort lands halfway through.
        let sorted = places(&["d", "c", "b", "a"], 2, layout);
        motion.spring_to(&sorted, mid);
        assert!(close(drawn(&motion, &sorted, "c"), at));
        assert_eq!(motion.bodies["c"].y.velocity, velocity);
        let generation = motion.generation();
        motion.tick(generation, mid + Duration::from_millis(16));
        // It carries on smoothly instead of jumping to either slot.
        let next = drawn(&motion, &sorted, "c");
        assert!((next.y - at.y).abs() < 40.0, "{at:?} to {next:?}");
        motion.tick(generation, mid + DURATION);
        assert!(!motion.active());
        assert_eq!(drawn(&motion, &sorted, "c"), sorted.slots[1].position());
    }

    #[test]
    fn a_zoom_scales_within_a_column_count_and_springs_across_one() {
        let start = Instant::now();
        let mut motion = GridMotion::default();
        let names = ["a", "b", "c", "d", "e"];
        motion.snap_to(&places(&names, 4, GridLayout::new(100)));
        let generation = motion.generation();
        // Same columns: the slots scale and nothing springs.
        let scaled = places(&names, 4, GridLayout::new(105));
        motion.zoom_to(&scaled, start);
        assert!(!motion.active());
        assert_eq!(motion.generation(), generation);
        assert_eq!(drawn(&motion, &scaled, "e"), scaled.slots[4].position());
        // Halfway through a tween the column count drops, and each card
        // springs from where the last frame drew it.
        let before = drawn(&motion, &scaled, "e");
        let fewer = places(&names, 3, GridLayout::new(110));
        motion.zoom_to(&fewer, start);
        assert!(motion.active());
        assert_ne!(motion.generation(), generation);
        assert!(close(drawn(&motion, &fewer, "e"), before));
        // Later frames scale the slots while the springs carry on.
        let later = places(&names, 3, GridLayout::new(115));
        let generation = motion.generation();
        motion.zoom_to(&later, start + Duration::from_millis(16));
        assert_eq!(motion.generation(), generation);
        assert!(motion.active());
        motion.tick(generation, start + DURATION);
        assert_eq!(drawn(&motion, &later, "e"), later.slots[4].position());
    }

    #[test]
    fn leaving_cards_fade_where_they_were_and_come_back_mid_fade() {
        let layout = GridLayout::default();
        let start = Instant::now();
        let mut motion = GridMotion::default();
        let all = places(&["a", "b", "c", "d", "e", "f"], 3, layout);
        motion.snap_to(&all);
        let b = all.slots[1];
        let fewer = places(&["a", "c"], 3, layout);
        motion.spring_to(&fewer, start);
        let leaving: Vec<_> = motion.leaving().iter().map(|card| &card.name).collect();
        assert_eq!(leaving, ["b", "d", "e", "f"]);
        assert_eq!(motion.leaving()[0].bounds, b);
        assert_eq!(motion.leaving()[0].opacity, 1.0);
        // The scroll height holds for the cards fading out.
        assert_eq!(motion.hold(), Some(all.height));
        let shown = motion.shown(&fewer);
        assert_eq!(motion.content_height(&fewer, &shown), all.height);
        assert!(fewer.height < all.height);

        let mid = start + FADE / 2;
        motion.tick(motion.generation(), mid);
        let faded = motion.leaving()[0].opacity;
        assert!((faded - 0.5).abs() < 0.01, "{faded}");
        assert_eq!(motion.leaving()[0].bounds, b);
        // "b" comes back halfway out: it fades in from there, springing
        // from where it was drawn to its new slot.
        let back = places(&["a", "b", "c"], 3, layout);
        motion.spring_to(&back, mid);
        assert_eq!(motion.opacity("b"), faded);
        assert!(close(drawn(&motion, &back, "b"), b.position()));
        assert!(motion.leaving().iter().all(|card| card.name != "b"));
        let generation = motion.generation();
        motion.tick(generation, mid + FADE / 4);
        assert!(motion.opacity("b") > faded);
        // The other leaving cards fade on, and the height lets go with the
        // last of them.
        motion.tick(generation, mid + FADE);
        assert!(motion.leaving().is_empty());
        assert_eq!(motion.hold(), None);
        assert_eq!(motion.opacity("b"), 1.0);
        motion.tick(generation, mid + DURATION);
        assert!(!motion.active());
        assert_eq!(frames(&motion), 0);
    }

    #[test]
    fn arriving_cards_fade_in_where_they_land() {
        let layout = GridLayout::default();
        let start = Instant::now();
        let mut motion = GridMotion::default();
        motion.snap_to(&places(&["a", "c"], 3, layout));
        let more = places(&["a", "b", "c"], 3, layout);
        motion.spring_to(&more, start);
        assert_eq!(motion.opacity("b"), 0.0);
        assert_eq!(drawn(&motion, &more, "b"), more.slots[1].position());
        let generation = motion.generation();
        motion.tick(generation, start + FADE / 3);
        assert!(motion.opacity("b") > 0.0 && motion.opacity("b") < 1.0);
        assert_eq!(drawn(&motion, &more, "b"), more.slots[1].position());
        motion.tick(generation, start + DURATION);
        assert!(!motion.active());
        assert_eq!(motion.opacity("b"), 1.0);
    }

    #[test]
    fn a_snap_settles_everything_at_once() {
        let layout = GridLayout::default();
        let start = Instant::now();
        let mut motion = GridMotion::default();
        motion.snap_to(&places(&["a", "b", "c"], 3, layout));
        let fewer = places(&["c", "a"], 3, layout);
        motion.spring_to(&fewer, start);
        motion.anchor = Some(("a".into(), 10.0));
        let stale = motion.generation();
        motion.snap_to(&fewer);
        assert!(!motion.active());
        assert!(motion.leaving().is_empty());
        assert_eq!(motion.hold(), None);
        assert_eq!(motion.anchor, None);
        assert_eq!(motion.offset("c"), Vector::ZERO);
        motion.tick(stale, start + Duration::from_millis(50));
        assert!(!motion.active());
        assert_eq!(frames(&motion), 0);
    }

    #[test]
    fn overshoot_past_either_end_stays_within_a_few_pixels() {
        assert_eq!(bounded(-100.0, -300.0), -100.0);
        let past = bounded(50.0, -300.0);
        assert!(past > 0.0 && past <= MAX_OVERSHOOT);
        let behind = bounded(-400.0, -300.0);
        assert!((-300.0 - MAX_OVERSHOOT..-300.0).contains(&behind));
        assert!(bounded(30.0, 0.0) <= MAX_OVERSHOOT);
        // The same limits the dock's progress spring had.
        let (from, to) = (0.0, 3000.0);
        for progress in [-0.2, 0.3, 1.0, 1.1] {
            let offset = (from - to) * (1.0 - progress);
            assert!(
                (to + bounded(offset, from - to) - bounded_axis(from, to, progress)).abs() < 0.01,
                "{progress}"
            );
        }
    }
}
