//! Draws coverflow's strip from `coverflow::geometry` and turns presses
//! and the wheel on it into messages. Covers are flat for now.

use iced::advanced::image::Renderer as _;
use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::svg::Renderer as _;
use iced::advanced::text::{self, Renderer as _};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget};
use iced::widget::image;
use iced::{
    Border, Color, Element, Event, Fill, Length, Pixels, Point, Rectangle, Renderer, Size, Theme,
    Vector, alignment, window,
};
use std::time::{Duration, Instant};

use crate::Message;
use crate::coverflow::{self, Event as StripEvent, StopId};
use crate::theme::Palette;

/// The strip's container id, which a test finds it on screen by.
pub(crate) const ID: &str = "library-strip";
/// Wheel travel that counts as one notch when the device reports pixels,
/// as a touchpad does.
const PIXELS_PER_NOTCH: f32 = 60.0;
/// How far each blank card behind a collapsed section's poster sits, as a
/// share of the poster's width.
const STACK_OFFSET: f32 = 0.06;
const TITLE_SIZE: f32 = 16.0;
const STATUS_SIZE: f32 = 13.0;
const LABEL_SIZE: f32 = 13.0;
const DOT: f32 = 8.0;

/// What one stop draws.
#[derive(Debug, Clone)]
pub(crate) struct Face {
    pub(crate) stop: usize,
    pub(crate) id: StopId,
    pub(crate) art: Option<image::Handle>,
    /// Its art warped toward the middle, for the side it's on, once warped.
    pub(crate) tilted: Option<image::Handle>,
    /// The title the name card shows when there's no art.
    pub(crate) title: String,
    /// Hidden or not installed, which the grid fades too.
    pub(crate) faded: bool,
    pub(crate) marked: bool,
    pub(crate) favorite: bool,
    /// A collapsed section's name and game count.
    pub(crate) section: Option<(String, usize)>,
}

/// The lines under the middle cover: a game's title and status, or a
/// collapsed section's name and count.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Caption {
    Game {
        title: String,
        state: &'static str,
        dot: Color,
        faded: bool,
    },
    /// A collapsed section's name and game count.
    Section { name: String, count: usize },
}

/// The captions under the middle cover: the one for the middle now, and
/// while the title crossfades, the one it's replacing and how far the new
/// one has come in.
#[derive(Debug, Clone, Default)]
pub(crate) struct Captions {
    pub(crate) now: Option<Caption>,
    pub(crate) before: Option<Caption>,
    pub(crate) progress: f32,
}

/// The strip of `count` stops at `position`, drawing `faces`, with the
/// cover at `middle` taking the clicks a grid card does. `open` keeps the
/// docked panel's room.
pub(crate) fn strip<'a>(
    faces: Vec<Face>,
    count: usize,
    position: f32,
    middle: usize,
    open: bool,
    captions: Captions,
    palette: Palette,
) -> Element<'a, Message> {
    Element::new(Strip {
        faces,
        count,
        position,
        middle,
        open,
        captions,
        palette,
    })
}

struct Strip {
    faces: Vec<Face>,
    count: usize,
    position: f32,
    middle: usize,
    open: bool,
    captions: Captions,
    palette: Palette,
}

/// A left press on the middle game: when, where, and on which copy.
struct Press {
    at: Instant,
    point: Point,
    slot: crate::slot::SlotId,
}

impl Press {
    /// Whether a press at `point` at `at` makes this one a double click,
    /// by iced's rule: within 300 ms and 6 px.
    fn doubled_by(&self, point: Point, at: Instant) -> bool {
        at > self.at
            && at - self.at <= DOUBLE_CLICK
            && self.point.distance(point) < DOUBLE_CLICK_DISTANCE
    }
}

const DOUBLE_CLICK: Duration = Duration::from_millis(300);
const DOUBLE_CLICK_DISTANCE: f32 = 6.0;

#[derive(Default)]
struct State {
    /// The size last sent to the app, which sizes the strip's art and its
    /// page step from it.
    measured: Size,
    /// The last left press on the middle game, so a double click counts
    /// only on the middle cover it started on.
    last_click: Option<Press>,
    /// Wheel travel not yet a whole notch.
    wheel: f32,
}

impl Strip {
    fn face(&self, stop: usize) -> Option<&Face> {
        self.faces.iter().find(|face| face.stop == stop)
    }

    fn geometry(&self, bounds: Rectangle) -> (Rectangle, coverflow::Geometry) {
        let area = coverflow::area(bounds, self.open);
        let geometry = coverflow::geometry(self.position, self.count, area.size());
        (area, geometry)
    }

    /// The face drawn topmost under `point`.
    fn hit(&self, bounds: Rectangle, point: Point) -> Option<&Face> {
        let (area, geometry) = self.geometry(bounds);
        if !area.contains(point) {
            return None;
        }
        let origin = Vector::new(area.x, area.y);
        geometry
            .covers
            .iter()
            .rev()
            .find(|cover| (cover.bounds + origin).contains(point))
            .and_then(|cover| self.face(cover.stop))
    }
}

/// The part of `handle` that fills `frame` without stretching, as the
/// grid's posters crop.
fn cover_fit(handle: &image::Handle, frame: Rectangle) -> Rectangle {
    let size = match handle {
        image::Handle::Rgba { width, height, .. } if *width > 0 && *height > 0 => {
            let scale = (frame.width / *width as f32).max(frame.height / *height as f32);
            Size::new(*width as f32 * scale, *height as f32 * scale)
        }
        _ => frame.size(),
    };
    Rectangle::new(
        frame.center() - Vector::new(size.width / 2.0, size.height / 2.0),
        size,
    )
}

fn quad(renderer: &mut Renderer, bounds: Rectangle, color: Color, border: Border) {
    renderer.fill_quad(
        Quad {
            bounds,
            border,
            ..Quad::default()
        },
        color,
    );
}

/// One line of text centered on `x` with its top at `y`.
#[allow(clippy::too_many_arguments)]
fn line(
    renderer: &mut Renderer,
    content: String,
    x: f32,
    y: f32,
    width: f32,
    size: f32,
    font: iced::Font,
    color: Color,
    clip: Rectangle,
) {
    renderer.fill_text(
        text::Text {
            content,
            bounds: Size::new(width, f32::INFINITY),
            size: Pixels(size),
            line_height: text::LineHeight::default(),
            font,
            align_x: text::Alignment::Center,
            align_y: alignment::Vertical::Top,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        },
        Point::new(x, y),
        color,
        clip,
    );
}

/// "1 game", "2 games".
pub(crate) fn games(count: usize) -> String {
    if count == 1 {
        "1 game".to_owned()
    } else {
        format!("{count} games")
    }
}

impl Strip {
    /// A cover's outline filled in `color`: the tilt's trapezoid once it's
    /// more turned than not, a rectangle before that.
    fn fill_shape(&self, renderer: &mut Renderer, bounds: Rectangle, tilt: f32, color: Color) {
        if tilt.abs() < 0.5 {
            quad(renderer, bounds, color, Border::default());
            return;
        }
        let shape = if tilt < 0.0 {
            crate::tilt::LEFT_SHAPE
        } else {
            crate::tilt::RIGHT_SHAPE
        };
        // The renderer's tint ignores its alpha, so the shape fades by
        // opacity.
        renderer.draw_svg(
            iced::advanced::svg::Svg::new(iced::advanced::svg::Handle::from_memory(shape))
                .color(Color { a: 1.0, ..color })
                .opacity(color.a),
            bounds,
            bounds,
        );
    }

    fn draw_face(
        &self,
        renderer: &mut Renderer,
        face: &Face,
        bounds: Rectangle,
        tilt: f32,
        clip: Rectangle,
    ) {
        let palette = self.palette;
        let edge = Border::default().width(1).color(palette.border);
        if face.section.is_some() {
            // Two blank cards behind the first game's poster.
            for step in [2.0, 1.0] {
                let offset = bounds.width * STACK_OFFSET * step;
                quad(
                    renderer,
                    bounds + Vector::new(offset, -offset),
                    palette.surface,
                    edge,
                );
            }
        }
        // A faded poster shows the background through it, as in the grid,
        // and never the covers behind it.
        self.fill_shape(renderer, bounds, tilt, palette.background);
        let dim = if face.faded {
            crate::panel::DIMMED_OPACITY
        } else {
            1.0
        };
        // Turning, the flat art fades into its warp, which fills the bounds
        // exactly: it's the 2:3 crop the flat art shows.
        let turned = tilt.abs().min(1.0);
        let warp = face.tilted.as_ref().filter(|_| turned > 0.0);
        match &face.art {
            Some(handle) => {
                if let Some(frame) = bounds.intersection(&clip) {
                    let flat = if warp.is_some() { 1.0 - turned } else { 1.0 };
                    if flat > 0.0 {
                        renderer.draw_image(
                            iced::advanced::image::Image::new(handle.clone()).opacity(dim * flat),
                            cover_fit(handle, bounds),
                            frame,
                        );
                    }
                    if let Some(warp) = warp {
                        renderer.draw_image(
                            iced::advanced::image::Image::new(warp.clone()).opacity(dim * turned),
                            bounds,
                            frame,
                        );
                    }
                }
            }
            None => {
                quad(
                    renderer,
                    bounds,
                    palette.surface.scale_alpha(dim),
                    Border::default(),
                );
                let size = (bounds.height / 240.0 * 14.0).clamp(12.0, 28.0);
                let pad = bounds.width * 0.08;
                renderer.fill_text(
                    text::Text {
                        content: face.title.clone(),
                        bounds: Size::new((bounds.width - 2.0 * pad).max(0.0), bounds.height),
                        size: Pixels(size),
                        line_height: text::LineHeight::default(),
                        font: renderer.default_font(),
                        align_x: text::Alignment::Center,
                        align_y: alignment::Vertical::Center,
                        shaping: text::Shaping::Advanced,
                        wrapping: text::Wrapping::Word,
                    },
                    bounds.center(),
                    palette.muted,
                    bounds.intersection(&clip).unwrap_or(bounds),
                );
            }
        }
    }

    /// Whether `face` draws any badge over its art.
    fn badged(face: &Face) -> bool {
        face.marked || face.favorite
    }

    /// The badges over a cover's art, as a grid poster draws them and
    /// scaled with the cover: the mark's check top left, the favorite's
    /// star top right. Nothing else shows on a cover.
    fn draw_badges(&self, renderer: &mut Renderer, face: &Face, bounds: Rectangle, tilt: f32) {
        use crate::library_actions::{MARK_BACKDROP, MARK_ICON, MARK_MARGIN, MARK_SIZE, STAR};
        let scale = bounds.height / crate::grid_motion::GridLayout::default().poster_height();
        let (size, icon, margin) = (MARK_SIZE * scale, MARK_ICON * scale, MARK_MARGIN * scale);
        let side = if tilt < 0.0 {
            crate::art_cache::Tilt::Left
        } else {
            crate::art_cache::Tilt::Right
        };
        let chip = |left: bool| {
            let x = if left {
                bounds.x + margin
            } else {
                bounds.x + bounds.width - margin - size
            };
            // A tilted cover's top slopes down to its far edge; the badge
            // keeps its margin below it.
            let across = (x + size / 2.0 - bounds.x) / bounds.width;
            let drop = crate::tilt::top_inset(side, across) * tilt.abs().min(1.0) * bounds.height;
            Rectangle::new(
                Point::new(x, bounds.y + drop + margin),
                Size::new(size, size),
            )
        };
        let glyph = |renderer: &mut Renderer, chip: Rectangle, bytes: &'static [u8], color| {
            let inset = (size - icon) / 2.0;
            renderer.draw_svg(
                iced::advanced::svg::Svg::new(iced::advanced::svg::Handle::from_memory(bytes))
                    .color(color),
                Rectangle::new(
                    chip.position() + Vector::new(inset, inset),
                    Size::new(icon, icon),
                ),
                chip,
            );
        };
        let round = Border::default().rounded(size / 2.0);
        if face.marked {
            let fill = self.palette.primary;
            let chip = chip(true);
            quad(renderer, chip, fill, round);
            glyph(
                renderer,
                chip,
                crate::library_menu::CHECK,
                crate::ui::accent_ink(fill),
            );
        }
        if face.favorite {
            let chip = chip(false);
            quad(
                renderer,
                chip,
                Color::BLACK.scale_alpha(MARK_BACKDROP),
                round,
            );
            glyph(renderer, chip, STAR, Color::WHITE);
        }
    }

    /// The middle's title and status line at `alpha`, as the title
    /// crossfades.
    fn draw_caption(
        &self,
        renderer: &mut Renderer,
        caption: &Caption,
        area: Rectangle,
        alpha: f32,
        clip: Rectangle,
    ) {
        let palette = self.palette.faded(alpha);
        let x = area.center_x();
        let title_font = crate::fonts::semibold();
        let body = renderer.default_font();
        let (title, faded) = match caption {
            Caption::Section { name, .. } => (name.clone(), false),
            Caption::Game { title, faded, .. } => (title.clone(), *faded),
        };
        let title = crate::tile_title::fit(&title, area.width, TITLE_SIZE, title_font);
        line(
            renderer,
            title,
            x,
            area.y,
            area.width,
            TITLE_SIZE,
            title_font,
            if faded {
                palette.muted
            } else {
                palette.foreground
            },
            clip,
        );
        let y = area.y + TITLE_SIZE * 1.3 + 6.0;
        match caption {
            Caption::Section { count, .. } => line(
                renderer,
                games(*count),
                x,
                y,
                area.width,
                STATUS_SIZE,
                body,
                palette.muted,
                clip,
            ),
            Caption::Game { state, dot, .. } => {
                let width = crate::tile_title::line_width(state, STATUS_SIZE, body);
                let gap = 8.0;
                let left = x - (DOT + gap + width) / 2.0;
                let height = STATUS_SIZE * 1.3;
                quad(
                    renderer,
                    Rectangle::new(
                        Point::new(left, y + (height - DOT) / 2.0),
                        Size::new(DOT, DOT),
                    ),
                    dot.scale_alpha(alpha),
                    Border::default().rounded(DOT / 2.0),
                );
                line(
                    renderer,
                    (*state).to_owned(),
                    left + DOT + gap + width / 2.0,
                    y,
                    width + 2.0,
                    STATUS_SIZE,
                    body,
                    palette.foreground,
                    clip,
                );
            }
        }
    }
}

impl Widget<Message, Theme, Renderer> for Strip {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Fill, Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Fill, Fill, Size::ZERO))
    }

    fn operate(
        &mut self,
        _tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(Some(&Id::new(ID)), layout.bounds());
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        match event {
            Event::Window(window::Event::RedrawRequested(_)) => {
                if state.measured != bounds.size() {
                    state.measured = bounds.size();
                    shell.publish(Message::Strip(StripEvent::Measured(bounds.size())));
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let Some(point) = cursor.position_over(bounds) else {
                    return;
                };
                let at = crate::clock::now();
                let previous = state.last_click.take();
                // A double click stays with the middle cover it started on
                // while that's still the middle, though the first click
                // opened the panel and moved the strip from under it.
                if *button == mouse::Button::Left
                    && let Some(press) = &previous
                    && self.face(self.middle).map(|face| &face.id)
                        == Some(&StopId::Card(press.slot.clone()))
                    && press.doubled_by(point, at)
                {
                    shell.publish(Message::Press(press.slot.clone()));
                    shell.publish(Message::DoubleClick(press.slot.name().to_owned()));
                    shell.capture_event();
                    return;
                }
                let Some(face) = self.hit(bounds, point) else {
                    return;
                };
                let id = face.id.clone();
                let middle = face.stop == self.middle;
                match (button, &id) {
                    // Only a press that starts on the middle cover can be
                    // the first of a double click, so one that centered a
                    // cover never launches it.
                    (mouse::Button::Left, _) if !middle => {
                        shell.publish(Message::Strip(StripEvent::Center(id)));
                    }
                    (mouse::Button::Left, StopId::Collapsed(key)) => {
                        shell.publish(Message::Strip(StripEvent::Open(key.clone())));
                    }
                    (mouse::Button::Left, StopId::Card(slot)) => {
                        shell.publish(Message::Press(slot.clone()));
                        state.last_click = Some(Press {
                            at,
                            point,
                            slot: slot.clone(),
                        });
                    }
                    (mouse::Button::Right, StopId::Card(slot)) if middle => {
                        shell.publish(Message::Context(slot.name().to_owned()));
                    }
                    _ => return,
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if !cursor.is_over(bounds) {
                    return;
                }
                let travel = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => {
                        if x.abs() > y.abs() {
                            x
                        } else {
                            y
                        }
                    }
                    mouse::ScrollDelta::Pixels { x, y } => {
                        (if x.abs() > y.abs() { x } else { y }) / PIXELS_PER_NOTCH
                    }
                };
                // Down and right reveal what's later in the strip.
                state.wheel -= travel;
                let notches = state.wheel.trunc();
                if notches != 0.0 {
                    state.wheel -= notches;
                    shell.publish(Message::Strip(StripEvent::Step(notches as i32)));
                }
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        match cursor.position_over(layout.bounds()) {
            Some(point) if self.hit(layout.bounds(), point).is_some() => {
                mouse::Interaction::Pointer
            }
            _ => mouse::Interaction::default(),
        }
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
        let (area, geometry) = self.geometry(layout.bounds());
        let Some(clip) = area.intersection(viewport) else {
            return;
        };
        let origin = Vector::new(area.x, area.y);
        let shade = self.palette.background;
        for cover in &geometry.covers {
            let Some(face) = self.face(cover.stop) else {
                continue;
            };
            let bounds = cover.bounds + origin;
            // Each cover is a layer of its own, since a layer draws its
            // quads before its images, whatever order they came in.
            renderer.with_layer(clip, |renderer| {
                self.draw_face(renderer, face, bounds, cover.tilt, clip);
            });
            // The badges' icons draw after their layer's quads, so the
            // shade over them is a layer of its own.
            if Self::badged(face) {
                renderer.with_layer(clip, |renderer| {
                    self.draw_badges(renderer, face, bounds, cover.tilt);
                });
            }
            if cover.opacity < 1.0 {
                renderer.with_layer(clip, |renderer| {
                    self.fill_shape(
                        renderer,
                        bounds,
                        cover.tilt,
                        shade.scale_alpha(1.0 - cover.opacity),
                    );
                });
            }
        }
        // A collapsed section's name sits under its card, over whatever
        // covers reach below it; the middle one's is its caption.
        renderer.with_layer(clip, |renderer| {
            for cover in &geometry.covers {
                let Some(face) = self.face(cover.stop) else {
                    continue;
                };
                let Some((name, count)) = &face.section else {
                    continue;
                };
                if cover.stop == self.middle {
                    continue;
                }
                let bounds = cover.bounds + origin;
                let width = bounds.width * 1.2;
                let label = format!("{name} ({count})");
                let body = renderer.default_font();
                line(
                    renderer,
                    crate::tile_title::fit(&label, width, LABEL_SIZE, body),
                    bounds.center_x(),
                    bounds.y + bounds.height + 6.0,
                    width,
                    LABEL_SIZE,
                    body,
                    self.palette.muted.scale_alpha(cover.opacity),
                    clip,
                );
            }
            let area = geometry.caption + origin;
            let captions = &self.captions;
            if let Some(before) = &captions.before {
                self.draw_caption(renderer, before, area, 1.0 - captions.progress, clip);
            }
            if let Some(now) = &captions.now {
                let alpha = if captions.before.is_some() {
                    captions.progress
                } else {
                    1.0
                };
                self.draw_caption(renderer, now, area, alpha, clip);
            }
        });
    }
}
