//! The toolbar's zoom slider and poster-only toggle. iced's slider can't
//! take focus and steps on the arrows only under the pointer, so the
//! slider is wineo's own.

use iced::advanced::widget::operation::Focusable;
use iced::advanced::{
    Clipboard, Layout, Renderer as _, Shell, Widget, layout, mouse, renderer, widget::tree,
};
use iced::keyboard::{self, key::Named};
use iced::widget::{button, container, text, tooltip};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, border};
use wineo_core::library::{ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

use crate::library_menu::icon;
use crate::theme::Palette;
use crate::zoom::Event as ZoomEvent;
use crate::{App, Message, focus_button, ui};

pub(crate) const SLIDER_ID: &str = "library-zoom";
pub(crate) const POSTER_ONLY_ID: &str = "library-poster-only";
pub(crate) const WIDTH: f32 = 96.0;
const THUMB: f32 = 14.0;
const TRACK: f32 = 4.0;
const TICK: Size = Size::new(2.0, 10.0);

/// A poster with a title line under it.
const TITLES: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="4" y="1.5" width="8" height="9.5" rx="1.5" fill="none" stroke="#000" stroke-width="1.6"/><path d="M4.5 14h7" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round"/></svg>"##;

/// The slider and the toggle, for the toolbar after Filters.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    let palette = app.palette;
    let zoom = app.zooming.shown().unwrap_or_else(|| f32::from(app.zoom));
    let slider = with_tooltip(
        Element::new(Slider {
            value: zoom,
            dragging: app.zooming.dragging(),
            modifiers: app.modifiers,
            palette,
        }),
        "Zoom",
        palette,
    );
    let on = app.poster_only;
    let toggle = button(
        container(icon(
            TITLES,
            if on {
                palette.primary
            } else {
                palette.foreground
            },
            16.0,
        ))
        .center(iced::Fill),
    )
    .width(ui::BUTTON_HEIGHT)
    .height(ui::BUTTON_HEIGHT)
    .padding(0)
    .on_press(Message::Zoom(ZoomEvent::TogglePosterOnly))
    .style(move |_, status| {
        let mut style = ui::secondary_style(palette, status);
        // On reads like a chosen tag chip: tinted with a primary border.
        if on {
            let tint = match status {
                button::Status::Hovered | button::Status::Pressed => 0.24,
                _ => 0.15,
            };
            style.background = Some(palette.primary.scale_alpha(tint).into());
            style.border = style.border.color(palette.primary);
        }
        style
    });
    let toggle = with_tooltip(toggle.into(), toggle_label(on), palette);
    iced::widget::row![
        slider,
        focus_button::control(
            toggle,
            true,
            POSTER_ONLY_ID,
            Message::Zoom(ZoomEvent::TogglePosterOnly),
        ),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .into()
}

/// What pressing the toggle does.
pub(crate) fn toggle_label(poster_only: bool) -> &'static str {
    if poster_only {
        "Show titles"
    } else {
        "Hide titles"
    }
}

fn with_tooltip<'a>(
    content: Element<'a, Message>,
    label: &'a str,
    palette: Palette,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(label).size(13)).padding([4, 8]),
        tooltip::Position::Bottom,
    )
    .style(move |_| {
        container::Style::default()
            .background(palette.surface)
            .border(border::rounded(8).width(1).color(palette.border))
    })
    .gap(6)
    .into()
}

struct Slider {
    value: f32,
    /// The app still follows a drag. A dialog that opens mid-drag takes
    /// the release, and the app ends the drag without it.
    dragging: bool,
    /// The keys the app knows are held, which the slider starts from and
    /// follows on each rebuild, like the grid's wheel.
    modifiers: keyboard::Modifiers,
    palette: Palette,
}

#[derive(Default)]
struct State {
    focused: bool,
    dragging: bool,
    modifiers: keyboard::Modifiers,
}

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

/// The thumb's centers at either end of the range.
fn travel(bounds: Rectangle) -> (f32, f32) {
    (
        bounds.x + THUMB / 2.0,
        bounds.x + bounds.width - THUMB / 2.0,
    )
}

/// The zoom the thumb shows with its center at `x`.
pub(crate) fn zoom_at(bounds: Rectangle, x: f32) -> f32 {
    let (start, end) = travel(bounds);
    let along = ((x - start) / (end - start)).clamp(0.0, 1.0);
    f32::from(ZOOM_MIN) + along * f32::from(ZOOM_MAX - ZOOM_MIN)
}

/// Where the thumb's center sits for `zoom`.
pub(crate) fn x_of(bounds: Rectangle, zoom: f32) -> f32 {
    let (start, end) = travel(bounds);
    let along = (zoom - f32::from(ZOOM_MIN)) / f32::from(ZOOM_MAX - ZOOM_MIN);
    start + along.clamp(0.0, 1.0) * (end - start)
}

impl Widget<Message, Theme, Renderer> for Slider {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            modifiers: self.modifiers,
            ..State::default()
        })
    }

    fn diff(&self, tree: &mut tree::Tree) {
        // A rebuild comes after the press's own message, so a drag the app
        // has let go of is over here too.
        let state = tree.state.downcast_mut::<State>();
        if !self.dragging {
            state.dragging = false;
        }
        state.modifiers = self.modifiers;
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(WIDTH), Length::Fixed(ui::BUTTON_HEIGHT))
    }

    fn layout(
        &mut self,
        _tree: &mut tree::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(WIDTH, ui::BUTTON_HEIGHT, Size::ZERO))
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        operation.focusable(
            Some(&iced::widget::Id::new(SLIDER_ID)),
            layout.bounds(),
            tree.state.downcast_mut::<State>(),
        );
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
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
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let over = cursor.is_over(bounds);
                // Like the toolbar's buttons, a click takes focus and a
                // click anywhere else lets it go.
                if state.focused != over {
                    state.focused = over;
                    shell.request_redraw();
                }
                if over && let Some(position) = cursor.position() {
                    state.dragging = true;
                    shell.publish(Message::Zoom(ZoomEvent::Drag(zoom_at(bounds, position.x))));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                shell.publish(Message::Zoom(ZoomEvent::Drag(zoom_at(bounds, position.x))));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                shell.publish(Message::Zoom(ZoomEvent::DragEnded));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta })
                if state.modifiers.control() && cursor.is_over(bounds) =>
            {
                shell.publish(Message::Zoom(ZoomEvent::Wheel(*delta)));
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Window(iced::window::Event::Unfocused) => {
                state.modifiers = keyboard::Modifiers::empty();
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(named @ (Named::ArrowLeft | Named::ArrowRight)),
                ..
            }) if state.focused => {
                shell.publish(Message::Zoom(ZoomEvent::Step(*named == Named::ArrowRight)));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let palette = self.palette;
        let bounds = layout.bounds();
        let (start, end) = travel(bounds);
        let middle = bounds.center_y();
        let thumb = x_of(bounds, self.value);
        let quad = |bounds: Rectangle, radius: f32| renderer::Quad {
            bounds,
            border: border::rounded(radius),
            ..renderer::Quad::default()
        };
        renderer.fill_quad(
            quad(
                Rectangle::new(
                    Point::new(start, middle - TRACK / 2.0),
                    Size::new(end - start, TRACK),
                ),
                TRACK / 2.0,
            ),
            palette.border,
        );
        renderer.fill_quad(
            quad(
                Rectangle::new(
                    Point::new(start, middle - TRACK / 2.0),
                    Size::new(thumb - start, TRACK),
                ),
                TRACK / 2.0,
            ),
            palette.primary,
        );
        let tick = x_of(bounds, f32::from(ZOOM_DEFAULT));
        renderer.fill_quad(
            quad(
                Rectangle::new(
                    Point::new(tick - TICK.width / 2.0, middle - TICK.height / 2.0),
                    TICK,
                ),
                TICK.width / 2.0,
            ),
            palette.muted,
        );
        let color = if state.dragging {
            ui::blend(palette.primary, iced::Color::BLACK, 0.18)
        } else if cursor.is_over(bounds) {
            ui::blend(palette.primary, iced::Color::WHITE, 0.10)
        } else {
            palette.primary
        };
        renderer.fill_quad(
            quad(
                Rectangle::new(
                    Point::new(thumb - THUMB / 2.0, middle - THUMB / 2.0),
                    Size::new(THUMB, THUMB),
                ),
                THUMB / 2.0,
            ),
            color,
        );
        if state.focused {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: bounds.expand(2.0),
                    border: border::rounded(ui::CONTROL_RADIUS + 2.0)
                        .width(2)
                        .color(palette.primary),
                    ..renderer::Quad::default()
                },
                iced::Color::TRANSPARENT,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if tree.state.downcast_ref::<State>().dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests;
