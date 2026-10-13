use std::time::{Duration, Instant};

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Renderer as _, Shell, Widget};
use iced::{Color, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

const DURATION: Duration = Duration::from_millis(160);
const SLIDE: f32 = 24.0;

#[derive(Debug, Clone, Copy)]
struct Transition {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

#[derive(Debug)]
pub(crate) struct PanelMotion {
    progress: f32,
    transition: Option<Transition>,
    generation: u64,
    /// The card and target a closing panel keeps drawing. It lives here so
    /// every way the exit ends, a tick or a snap, clears it.
    exit_target: Option<(String, String)>,
}

impl Default for PanelMotion {
    fn default() -> Self {
        Self {
            progress: 0.0,
            transition: None,
            generation: 0,
            exit_target: None,
        }
    }
}

impl PanelMotion {
    pub(crate) fn open(&mut self, now: Instant) {
        self.exit_target = None;
        self.move_to(1.0, now);
    }

    /// Starts the exit, drawing `exit_target` (card, target) until it ends.
    pub(crate) fn close(&mut self, now: Instant, exit_target: Option<(String, String)>) {
        self.move_to(0.0, now);
        self.exit_target = exit_target.filter(|_| self.reserved());
    }

    pub(crate) fn exit_target(&self) -> Option<&(String, String)> {
        self.exit_target.as_ref()
    }

    fn move_to(&mut self, to: f32, now: Instant) {
        if self
            .transition
            .map_or(self.progress, |transition| transition.to)
            == to
        {
            return;
        }
        let from = self.sample(now);
        self.progress = from;
        self.generation = self.generation.wrapping_add(1);
        let distance = (to - from).abs();
        self.transition = (distance > f32::EPSILON).then_some(Transition {
            from,
            to,
            started: now,
            duration: DURATION.mul_f32(distance),
        });
    }

    fn sample(&self, now: Instant) -> f32 {
        let Some(transition) = self.transition else {
            return self.progress;
        };
        let elapsed = now.saturating_duration_since(transition.started);
        let fraction = (elapsed.as_secs_f32() / transition.duration.as_secs_f32()).min(1.0);
        let eased = fraction * fraction * (3.0 - 2.0 * fraction);
        transition.from + (transition.to - transition.from) * eased
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation {
            return;
        }
        self.progress = self.sample(now);
        if self.transition.is_some_and(|transition| {
            now.saturating_duration_since(transition.started) >= transition.duration
        }) {
            self.transition = None;
        }
        if !self.reserved() {
            self.exit_target = None;
        }
    }

    pub(crate) fn snap(&mut self, open: bool) {
        self.progress = if open { 1.0 } else { 0.0 };
        self.transition = None;
        self.exit_target = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn active(&self) -> bool {
        self.transition.is_some()
    }

    pub(crate) fn reserved(&self) -> bool {
        self.active() || self.progress > 0.0
    }

    pub(crate) fn interactive(&self) -> bool {
        !self.active() && self.progress >= 1.0
    }

    pub(crate) fn progress(&self) -> f32 {
        self.progress
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

pub(crate) fn presentation<'a, Message: 'a>(
    content: Element<'a, Message>,
    progress: f32,
    interactive: bool,
    background: Color,
) -> Element<'a, Message> {
    Element::new(Presentation {
        content,
        progress,
        interactive,
        background,
    })
}

struct Presentation<'a, Message> {
    content: Element<'a, Message>,
    progress: f32,
    interactive: bool,
    background: Color,
}

impl<Message> Widget<Message, Theme, Renderer> for Presentation<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
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
        if self.progress >= 1.0 {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
            return;
        }
        let bounds = layout.bounds();
        renderer.with_layer(bounds, |renderer| {
            renderer.with_translation(
                Vector::new(SLIDE * (1.0 - self.progress), 0.0),
                |renderer| {
                    self.content
                        .as_widget()
                        .draw(tree, renderer, theme, style, layout, cursor, viewport);
                },
            );
            if self.progress < 1.0 {
                renderer.fill_quad(
                    Quad {
                        bounds,
                        ..Quad::default()
                    },
                    self.background.scale_alpha(1.0 - self.progress),
                );
            }
        });
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.interactive {
            self.content
                .as_widget_mut()
                .operate(tree, layout, renderer, operation);
        } else {
            let mut unfocus = iced::advanced::widget::operation::focusable::unfocus::<()>();
            self.content
                .as_widget_mut()
                .operate(tree, layout, renderer, &mut unfocus);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.interactive {
            self.content.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
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
        if self.interactive {
            self.content
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer)
        } else {
            mouse::Interaction::None
        }
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.interactive
            .then(|| {
                self.content
                    .as_widget_mut()
                    .overlay(tree, layout, renderer, viewport, translation)
            })
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_reverses_from_its_current_position() {
        let start = Instant::now();
        let mut motion = PanelMotion::default();
        motion.open(start);
        assert!(motion.reserved());
        assert!(!motion.interactive());
        motion.tick(motion.generation(), start + Duration::from_millis(80));
        let halfway = motion.progress();
        motion.close(start + Duration::from_millis(80), None);
        assert_eq!(motion.progress(), halfway);
        assert!(motion.reserved());
        motion.tick(motion.generation(), start + Duration::from_millis(160));
        assert!(!motion.reserved());
        assert!(!motion.active());
    }

    #[test]
    fn stale_ticks_cannot_restart_motion_after_navigation() {
        let start = Instant::now();
        let mut motion = PanelMotion::default();
        motion.open(start);
        let stale = motion.generation();
        motion.snap(true);
        motion.tick(stale, start + Duration::from_secs(1));
        assert_eq!(motion.progress(), 1.0);
        assert!(!motion.active());
        assert!(motion.interactive());
    }
}
