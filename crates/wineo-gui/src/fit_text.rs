//! One line of text that ends in an ellipsis when the room it is laid out
//! in is too narrow for it. The room is only known at layout, which is why
//! this isn't a text widget with a string cut beforehand: a control that
//! folds under its label gets less than its usual width.

use iced::advanced::text::paragraph;
use iced::advanced::widget::{Operation, text, tree};
use iced::advanced::{Layout, Widget, layout, mouse, renderer};
use iced::{Element, Font, Length, Rectangle, Size};

pub(crate) fn fit_text<'a, Message: 'a>(
    content: impl Into<String>,
    font: Font,
    size: f32,
) -> Element<'a, Message> {
    Element::new(FitText {
        content: content.into(),
        font,
        size,
    })
}

struct FitText {
    content: String,
    font: Font,
    size: f32,
}

type Paragraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FitText {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<text::State<Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(paragraph::Plain::<Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let room = limits.max().width;
        let shown = if room.is_finite() {
            crate::tile_title::fit(&self.content, room, self.size, self.font)
        } else {
            self.content.clone()
        };
        text::layout(
            tree.state.downcast_mut::<text::State<Paragraph>>(),
            renderer,
            limits,
            &shown,
            text::Format {
                size: Some(self.size.into()),
                font: Some(self.font),
                wrapping: iced::widget::text::Wrapping::None,
                ..text::Format::default()
            },
        )
    }

    fn operate(
        &mut self,
        _tree: &mut tree::Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // The whole name, which is what a screen reader should read.
        operation.text(None, layout.bounds(), &self.content);
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        text::draw(
            renderer,
            style,
            layout.bounds(),
            tree.state.downcast_ref::<text::State<Paragraph>>().raw(),
            text::Style::default(),
            viewport,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_probe::render;
    use iced::widget::{container, text};

    const NAME: &str = "A family name much too long for the room it is given here";

    fn draw(element: Element<'static, ()>, room: f32) -> Vec<u8> {
        let mut element = container(element).width(room).into();
        render(&mut element, Size::new(300.0, 24.0))
    }

    #[test]
    fn a_name_too_wide_for_the_room_is_drawn_cut_with_an_ellipsis() {
        let room = 100.0;
        let cut = crate::tile_title::fit(NAME, room, 14.0, Font::DEFAULT);
        assert!(cut.ends_with("..."), "{cut}");
        let plain = text(cut)
            .size(14)
            .wrapping(iced::widget::text::Wrapping::None)
            .into();
        let fitted = draw(fit_text(NAME, Font::DEFAULT, 14.0), room);
        assert_eq!(fitted, draw(plain, room));
    }

    #[test]
    fn a_name_that_fits_is_drawn_whole() {
        let fitted = draw(fit_text("Short", Font::DEFAULT, 14.0), 100.0);
        let plain = text("Short")
            .size(14)
            .wrapping(iced::widget::text::Wrapping::None)
            .into();
        assert_eq!(fitted, draw(plain, 100.0));
    }
}
