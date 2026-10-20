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
}

#[derive(Debug, Default)]
pub(crate) struct GridMotion {
    springs: Vec<Spring>,
    target: f32,
    last_tick: Option<Instant>,
    deadline: Option<Instant>,
    generation: u64,
}

impl GridMotion {
    pub(crate) fn snap(&mut self, open: bool, count: usize) {
        self.target = if open { 1.0 } else { 0.0 };
        let now = Instant::now();
        self.springs = vec![
            Spring {
                position: self.target,
                velocity: 0.0,
                raw_position: self.target,
                raw_velocity: 0.0,
                delay_until: now,
            };
            count
        ];
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
                now + (STAGGER * (index % 8) as u32).min(MAX_STAGGER)
            };
        }
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation || !self.active() {
            return;
        }
        let previous = self.last_tick.unwrap_or(now);
        self.last_tick = Some(now);
        let deadline = self.deadline.expect("active grid motion has a deadline");
        for spring in &mut self.springs {
            let start = previous.max(spring.delay_until);
            if now > start {
                spring.advance(self.target, now.duration_since(start).as_secs_f32());
            }
            spring.present(self.target, deadline, now);
        }
        if now >= deadline {
            for spring in &mut self.springs {
                spring.position = self.target;
                spring.velocity = 0.0;
                spring.raw_position = self.target;
                spring.raw_velocity = 0.0;
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

pub(crate) fn grid<'a, Message: Clone + 'a>(
    cards: Vec<Element<'a, Message>>,
    names: Vec<String>,
    motion: &GridMotion,
    scene: Option<Scene<'a, Message>>,
    layout: GridLayout,
) -> Element<'a, Message> {
    Element::new(Grid {
        cards,
        names,
        layout,
        positions: (0..motion.springs.len())
            .map(|index| motion.progress(index))
            .collect(),
        fallback: motion.target,
        scene,
    })
}

struct Grid<'a, Message> {
    cards: Vec<Element<'a, Message>>,
    names: Vec<String>,
    layout: GridLayout,
    positions: Vec<f32>,
    fallback: f32,
    scene: Option<Scene<'a, Message>>,
}

#[derive(Default)]
struct GridState {
    width: f32,
    layout: GridLayout,
    sizes: Vec<Size>,
    closed: Vec<Point>,
    open: Vec<Point>,
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
        tree::State::new(GridState::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.cards.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
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
        let sizes: Vec<_> = nodes.iter().map(layout::Node::size).collect();
        let state = tree.state.downcast_mut::<GridState>();
        let grid = self.layout;
        if state.width != width || state.layout != grid || state.sizes != sizes {
            state.width = width;
            state.layout = grid;
            state.sizes = sizes.clone();
            state.closed = slots(&sizes, columns(width, grid), grid).0;
            state.open = slots(&sizes, columns(width - DOCK_WIDTH, grid), grid).0;
        }
        let mut height = 2.0 * PADDING;
        let collapse = self.scene.as_ref().map(|scene| scene.collapse);
        state.looks.clear();
        let children = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                let progress = self.positions.get(index).copied().unwrap_or(self.fallback);
                let position = Point::new(
                    bounded_axis(state.closed[index].x, state.open[index].x, progress),
                    bounded_axis(state.closed[index].y, state.open[index].y, progress),
                );
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
            self.cards
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
                let over = layout.children().enumerate().any(|(index, layout)| {
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
        for ((card, state), layout) in self
            .cards
            .iter_mut()
            .rev()
            .zip(tree.children.iter_mut().rev())
            .zip(layout.children().rev())
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
        for (index, ((card, state), layout)) in self
            .cards
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .enumerate()
        {
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
                layout.children().enumerate().any(|(index, layout)| {
                    hit(state.look(index), layout.bounds(), point, self.layout)
                })
            });
            return if over {
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::default()
            };
        }
        self.cards
            .iter()
            .rev()
            .zip(tree.children.iter().rev())
            .zip(layout.children().rev())
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
            &mut self.cards,
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
mod tests {
    use super::*;

    #[test]
    fn spring_reverses_continuously_and_settles() {
        let start = Instant::now();
        let mut motion = GridMotion::default();
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
        let mut motion = GridMotion::default();
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
        let mut motion = GridMotion::default();
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
}
