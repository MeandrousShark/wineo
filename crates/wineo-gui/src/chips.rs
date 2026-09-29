//! A list of values shown as removable chips, with a field to add one.

use iced::widget::{Row, button, column, container, row, text, text_input};
use iced::{Center, Element, border};

use crate::theme::Palette;

pub(crate) struct Chips<'a, Message> {
    pub(crate) items: &'a [String],
    pub(crate) input: &'a str,
    pub(crate) placeholder: &'a str,
    /// Why the typed value can't be added, shown under the field.
    pub(crate) error: Option<&'a str>,
    pub(crate) on_input: Box<dyn Fn(String) -> Message + 'a>,
    pub(crate) on_add: Message,
    pub(crate) on_remove: Box<dyn Fn(usize) -> Message + 'a>,
}

impl<'a, Message: Clone + 'a> Chips<'a, Message> {
    pub(crate) fn view(self, palette: Palette) -> Element<'a, Message> {
        let mut chips = Row::new().spacing(6).align_y(Center);
        for (index, item) in self.items.iter().enumerate() {
            let chip = container(
                row![
                    text(item.as_str()).size(13).color(palette.foreground),
                    button(text("x").size(12).color(palette.muted))
                        .on_press((self.on_remove)(index))
                        .padding([0, 4])
                        .style(|_, _| button::Style::default()),
                ]
                .spacing(4)
                .align_y(Center),
            )
            .padding([2, 8])
            .style(move |_| {
                iced::widget::container::Style::default()
                    .background(palette.background)
                    .border(border::rounded(12).width(1).color(palette.border))
            });
            chips = chips.push(chip);
        }
        let can_add = self.error.is_none() && !self.input.is_empty();
        let add = self.on_add.clone();
        let input = text_input(self.placeholder, self.input)
            .on_input(self.on_input)
            .on_submit_maybe(can_add.then_some(add))
            .width(180)
            .padding([4, 8])
            .style(move |_, status| input_style(palette, status));
        let plus = button(text("+").size(14))
            .on_press_maybe(can_add.then_some(self.on_add))
            .padding([4, 10])
            .style(move |_, status| secondary(palette, status));
        let mut content = column![chips.push(input).push(plus).wrap()].spacing(4);
        if let Some(error) = self.error {
            content = content.push(text(error).size(12).color(palette.danger));
        }
        content.into()
    }
}

pub(crate) fn secondary(palette: Palette, status: button::Status) -> button::Style {
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered) {
                palette.border
            } else {
                palette.surface
            }
            .into(),
        ),
        text_color: palette
            .foreground
            .scale_alpha(if disabled { 0.4 } else { 1.0 }),
        border: border::rounded(6).width(1).color(palette.border),
        ..button::Style::default()
    }
}

pub(crate) fn input_style(palette: Palette, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: palette.background.into(),
        border: border::rounded(6).width(1).color(
            if matches!(status, text_input::Status::Focused { .. }) {
                palette.primary
            } else {
                palette.border
            },
        ),
        icon: palette.muted,
        placeholder: palette.muted,
        value: palette.foreground,
        selection: palette.primary.scale_alpha(0.4),
    }
}
