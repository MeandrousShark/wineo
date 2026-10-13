use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Instant;

use iced::advanced::svg::{self, Svg};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, renderer, widget::tree};
use iced::{Color, Element, Event, Length, Point, Radians, Rectangle, Size, window};

use crate::theme::Palette;

/// Shared by every spinner so the ones on screen turn in step.
static START: OnceLock<Instant> = OnceLock::new();

fn start() -> Instant {
    *START.get_or_init(Instant::now)
}

/// A faint track with a rotating three-quarter arc. It animates itself on
/// redraws rather than through app messages, so only a spinner that is
/// actually drawn keeps the window redrawing, and the view isn't rebuilt
/// every frame.
pub(crate) fn spinner<'a, Message: 'a, Theme: 'a, Renderer>(
    palette: Palette,
    size: f32,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: svg::Renderer + 'a,
{
    Element::new(Spinner {
        // Same bytes for the same palette, so iced reuses the rasterized image
        // and each frame only changes the rotation.
        handle: svg::Handle::from_memory(source(palette).into_bytes()),
        size,
        angle: angle(start(), Instant::now()),
    })
}

struct Spinner {
    handle: svg::Handle,
    size: f32,
    angle: f32,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Spinner
where
    Renderer: svg::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(
        &mut self,
        _tree: &mut tree::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(self.size, self.size, Size::new(self.size, self.size)))
    }

    fn update(
        &mut self,
        _tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            self.angle = angle(start(), *now);
            // Scrollables forward redraws to children outside their viewport.
            // Scrolling one back in redraws the window, which restarts it.
            if layout.bounds().intersects(viewport) {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        _tree: &tree::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        // Fit the square image like ContentFit::Contain would.
        let side = bounds.width.min(bounds.height);
        let square = Rectangle::new(
            Point::new(
                bounds.center_x() - side / 2.0,
                bounds.center_y() - side / 2.0,
            ),
            Size::new(side, side),
        );
        renderer.draw_svg(
            Svg::new(self.handle.clone()).rotation(Radians(self.angle)),
            square,
            bounds,
        );
    }
}

/// One full turn per second since `start`.
pub(crate) fn angle(start: Instant, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() % 1.0) * TAU
}

fn source(palette: Palette) -> String {
    // A 3/4 arc from the top, clockwise to the left, on a 16 unit circle of radius 6.5.
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.5" fill="none" stroke="{}" stroke-width="2"/><path d="M8 1.5 A6.5 6.5 0 1 1 1.5 8" fill="none" stroke="{}" stroke-width="2" stroke-linecap="round"/></svg>"##,
        hex(palette.border),
        hex(palette.muted),
    )
}

fn hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn angle_turns_once_per_second() {
        let start = Instant::now();
        assert_eq!(angle(start, start), 0.0);
        let quarter = angle(start, start + Duration::from_millis(250));
        assert!((quarter - TAU / 4.0).abs() < 1e-4);
        let wrapped = angle(start, start + Duration::from_millis(1250));
        assert!((wrapped - TAU / 4.0).abs() < 1e-3);
    }

    fn redraw(viewport: Rectangle) -> (Spinner, window::RedrawRequest, Instant) {
        let mut spinner = Spinner {
            handle: svg::Handle::from_memory(source(Palette::DARK).into_bytes()),
            size: 16.0,
            angle: 0.0,
        };
        let mut tree = tree::Tree::empty();
        let node = layout::Node::new(Size::new(16.0, 16.0));
        let now = start() + Duration::from_millis(250);
        let mut messages: Vec<()> = Vec::new();
        let mut shell = Shell::new(&mut messages);
        Widget::<(), iced::Theme, ()>::update(
            &mut spinner,
            &mut tree,
            &Event::Window(window::Event::RedrawRequested(now)),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &(),
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &viewport,
        );
        assert!(shell.is_empty());
        let request = shell.redraw_request();
        (spinner, request, now)
    }

    #[test]
    fn redraws_turn_the_spinner_and_ask_for_the_next_frame() {
        let (spinner, request, now) = redraw(Rectangle::with_size(Size::new(16.0, 16.0)));
        assert_eq!(request, window::RedrawRequest::NextFrame);
        assert_eq!(spinner.angle, angle(start(), now));
    }

    #[test]
    fn a_spinner_outside_the_viewport_stops_asking_for_frames() {
        let below = Rectangle::new(Point::new(0.0, 100.0), Size::new(16.0, 16.0));
        let (_, request, _) = redraw(below);
        assert_eq!(request, window::RedrawRequest::Wait);
    }

    #[test]
    fn source_uses_palette_colors_and_is_stable() {
        let dark = source(Palette::DARK);
        assert!(dark.contains(&hex(Palette::DARK.border)));
        assert!(dark.contains(&hex(Palette::DARK.muted)));
        assert_eq!(dark, source(Palette::DARK));
    }
}
