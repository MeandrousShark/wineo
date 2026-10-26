//! The poster star's pop when a click on it favorites a card: the chip
//! springs up toward its target and settles while a ring ripples out once
//! and fades. Each pop runs to its end whatever the pointer does, and is
//! keyed by the card's name so it follows the card through a re-sort.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::advanced::layout;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Renderer as _, Widget};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, border, mouse};

use crate::zoom::ease_out;

pub(crate) const POP: Duration = Duration::from_millis(320);
/// The share of the pop the chip takes to spring up; it settles over the
/// rest.
const RISE: f32 = 0.3;

#[derive(Debug, Clone, Copy)]
struct Pop {
    /// Set by the first frame, so the pop sets out from what's on screen.
    start: Option<Instant>,
    t: f32,
}

#[derive(Debug, Default)]
pub(crate) struct StarPops {
    pops: HashMap<String, Pop>,
    generation: u64,
}

impl StarPops {
    /// Pops `name`'s star from the start, even if it's popping already.
    pub(crate) fn start(&mut self, name: &str) {
        if !self.active() {
            self.generation = self.generation.wrapping_add(1);
        }
        self.pops.insert(
            name.to_owned(),
            Pop {
                start: None,
                t: 0.0,
            },
        );
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation {
            return;
        }
        for pop in self.pops.values_mut() {
            let start = *pop.start.get_or_insert(now);
            pop.t = (now.saturating_duration_since(start).as_secs_f32() / POP.as_secs_f32())
                .clamp(0.0, 1.0);
        }
        self.pops.retain(|_, pop| pop.t < 1.0);
    }

    /// How far `name`'s pop has run, 0 to 1, while it runs.
    pub(crate) fn progress(&self, name: &str) -> Option<f32> {
        self.pops.get(name).map(|pop| pop.t)
    }

    /// Drops the pops `keep` turns down.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.pops.retain(|name, _| keep(name));
    }

    pub(crate) fn clear(&mut self) {
        self.pops.clear();
    }

    pub(crate) fn active(&self) -> bool {
        !self.pops.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Frames only while a pop runs. Like the hover fades, it isn't held
    /// back while the grid is covered, so a pop always finishes.
    pub(crate) fn subscription(&self) -> iced::Subscription<crate::Message> {
        if self.active() {
            iced::window::frames()
                .with(self.generation)
                .map(|(generation, now)| crate::Message::PopTick(generation, now))
        } else {
            iced::Subscription::none()
        }
    }
}

/// How far the chip has grown toward its target at `t`, 0 to 1: a quick
/// spring up, then an eased settle back to rest.
pub(crate) fn lift(t: f32) -> f32 {
    if t < RISE {
        ease_out(t / RISE)
    } else {
        let t = (t - RISE) / (1.0 - RISE);
        // Smoothstep, so it eases out of the top and into rest.
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// The ring's growth, 0 to 1, and its strength at `t`: it slows as it
/// spreads and is gone by the end.
pub(crate) fn ring(t: f32) -> (f32, f32) {
    (ease_out(t), (1.0 - t) * (1.0 - t))
}

/// A ring around `center`, clipped to a box of `size`. A container's clip
/// only narrows what its children are asked to draw, so the box clips the
/// ring in a layer of its own.
pub(crate) fn clipped_ring<'a, Message: 'a>(
    size: Size,
    center: Point,
    radius: f32,
    width: f32,
    color: Color,
) -> Element<'a, Message> {
    Element::new(Ring {
        size,
        center,
        radius,
        width,
        color,
    })
}

struct Ring {
    size: Size,
    center: Point,
    radius: f32,
    width: f32,
    color: Color,
}

impl<Message> Widget<Message, Theme, Renderer> for Ring {
    fn size(&self) -> Size<Length> {
        Size::new(
            Length::Fixed(self.size.width),
            Length::Fixed(self.size.height),
        )
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(self.size.width, self.size.height, Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let Some(clip) = layout.bounds().intersection(viewport) else {
            return;
        };
        let origin = layout.position();
        let bounds = Rectangle::new(
            Point::new(
                origin.x + self.center.x - self.radius,
                origin.y + self.center.y - self.radius,
            ),
            Size::new(2.0 * self.radius, 2.0 * self.radius),
        );
        renderer.with_layer(clip, |renderer| {
            renderer.fill_quad(
                Quad {
                    bounds,
                    border: border::rounded(self.radius)
                        .width(self.width)
                        .color(self.color),
                    ..Quad::default()
                },
                Color::TRANSPARENT,
            );
        });
    }
}
