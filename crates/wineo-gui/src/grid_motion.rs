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
const PADDING: f32 = 16.0;

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

fn columns(width: f32) -> usize {
    ((width - 2.0 * PADDING + GAP) / (CARD_WIDTH + GAP))
        .floor()
        .max(1.0) as usize
}

pub(crate) fn columns_for_viewport(width: f32, open: bool) -> usize {
    columns(width - if open { DOCK_WIDTH } else { 0.0 })
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

fn slots(sizes: &[Size], columns: usize) -> (Vec<Point>, f32) {
    let mut result = Vec::with_capacity(sizes.len());
    let mut y = PADDING;
    for row in sizes.chunks(columns) {
        for (column, _) in row.iter().enumerate() {
            result.push(Point::new(PADDING + column as f32 * (CARD_WIDTH + GAP), y));
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
) -> Element<'a, Message> {
    Element::new(Grid {
        cards,
        names,
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
    positions: Vec<f32>,
    fallback: f32,
    scene: Option<Scene<'a, Message>>,
}

#[derive(Default)]
struct GridState {
    width: f32,
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
fn hit(look: Look, bounds: Rectangle, point: Point) -> bool {
    match look {
        Look::Hidden => false,
        Look::Normal(_) => bounds.contains(point),
        Look::Loose { rotation, .. } => {
            let poster = poster_bounds(bounds);
            let offset = point - poster.center();
            let (sin, cos) = rotation.sin_cos();
            let x = offset.x * cos + offset.y * sin;
            let y = -offset.x * sin + offset.y * cos;
            x.abs() <= poster.width / 2.0 && y.abs() <= poster.height / 2.0
        }
    }
}

/// The poster's area inside a card's node, within its selection frame.
fn poster_bounds(card: Rectangle) -> Rectangle {
    Rectangle::new(
        card.position() + Vector::new(2.0, 2.0),
        Size::new(CARD_WIDTH - 4.0, crate::grid::CARD_POSTER_HEIGHT - 4.0),
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
        if state.width != width || state.sizes != sizes {
            state.width = width;
            state.sizes = sizes.clone();
            state.closed = slots(&sizes, columns(width)).0;
            state.open = slots(&sizes, columns(width - DOCK_WIDTH)).0;
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
                let over = layout
                    .children()
                    .enumerate()
                    .any(|(index, layout)| hit(state.look(index), layout.bounds(), point));
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
                    let frame = poster_bounds(layout.bounds());
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
                    .enumerate()
                    .any(|(index, layout)| hit(state.look(index), layout.bounds(), point))
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
        let poster = poster_bounds(bounds);
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
        assert!(hit(upright, bounds, corner));
        assert!(!hit(upright, bounds, caption));
        assert!(!hit(turned, bounds, corner));
        assert!(hit(turned, bounds, poster.center()));
        assert!(hit(Look::Normal(Vector::ZERO), bounds, caption));
        assert!(!hit(Look::Hidden, bounds, poster.center()));
    }

    #[test]
    fn slots_use_tallest_card_in_each_row() {
        let sizes = [
            Size::new(160.0, 280.0),
            Size::new(160.0, 315.0),
            Size::new(160.0, 270.0),
        ];
        let (points, height) = slots(&sizes, 2);
        assert_eq!(points[2].y, PADDING + 315.0 + GAP);
        assert_eq!(height, PADDING + 315.0 + GAP + 270.0 + PADDING);
    }
}
