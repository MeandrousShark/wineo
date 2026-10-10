use std::time::{Duration, Instant};

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::{Outcome, Scrollable};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget};
use iced::{Element, Event, Fill, Length, Point, Rectangle, Renderer, Size, Task, Theme, Vector};

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

fn bounded_axis(from: f32, to: f32, progress: f32) -> f32 {
    let travel = to - from;
    if progress < 0.0 {
        from + MAX_OVERSHOOT * (travel * progress / MAX_OVERSHOOT).tanh()
    } else if progress > 1.0 {
        to + MAX_OVERSHOOT * (travel * (progress - 1.0) / MAX_OVERSHOOT).tanh()
    } else {
        from + travel * progress
    }
}

pub(crate) fn grid<'a, Message: 'a>(
    cards: Vec<Element<'a, Message>>,
    names: Vec<String>,
    motion: &GridMotion,
) -> Element<'a, Message> {
    Element::new(Grid {
        cards,
        names,
        positions: (0..motion.springs.len())
            .map(|index| motion.progress(index))
            .collect(),
        fallback: motion.target,
    })
}

struct Grid<'a, Message> {
    cards: Vec<Element<'a, Message>>,
    names: Vec<String>,
    positions: Vec<f32>,
    fallback: f32,
}

#[derive(Default)]
struct GridState {
    width: f32,
    sizes: Vec<Size>,
    closed: Vec<Point>,
    open: Vec<Point>,
}

impl<Message> Widget<Message, Theme, Renderer> for Grid<'_, Message> {
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
        let children = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                let progress = self.positions.get(index).copied().unwrap_or(self.fallback);
                let position = Point::new(
                    bounded_axis(state.closed[index].x, state.open[index].x, progress),
                    bounded_axis(state.closed[index].y, state.open[index].y, progress),
                );
                height = height.max(position.y + sizes[index].height + PADDING);
                node.move_to(position)
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
        for ((card, state), layout) in self
            .cards
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(viewport))
        {
            card.as_widget()
                .draw(state, renderer, theme, style, layout, cursor, viewport);
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
