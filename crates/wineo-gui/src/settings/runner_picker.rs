use std::fmt;

use iced::advanced::text::{self, Paragraph};
use iced::widget::{container, pick_list, text as label, tooltip};
use iced::{Element, Font, Size, alignment, border};

use super::Event;
use super::load::RunnerChoice;
use crate::Message;
use crate::theme::Palette;
use crate::ui::PickListStyle;

const WIDTH: f32 = 260.0;
const TEXT_SIZE: f32 = 16.0;
// Leave room for the padding, arrow, and a gap before the arrow.
const LABEL_WIDTH: f32 = WIDTH - 44.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OptionLabel {
    choice: RunnerChoice,
    label: String,
}

impl fmt::Display for OptionLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

pub(super) fn picker(
    runners: &[RunnerChoice],
    selected: &str,
    locked: bool,
    palette: Palette,
    on_pick: fn(RunnerChoice) -> Event,
) -> Element<'static, Message> {
    let options: Vec<_> = runners
        .iter()
        .map(|choice| OptionLabel {
            choice: choice.clone(),
            label: fit(&choice.name),
        })
        .collect();
    let selected_option = options
        .iter()
        .find(|option| option.choice.name == selected)
        .cloned();
    let shown = fit(selected);
    let control: Element<'static, Message> = if locked {
        container(
            label(shown.clone())
                .size(TEXT_SIZE)
                .font(Font::DEFAULT)
                .shaping(text::Shaping::Advanced)
                .wrapping(text::Wrapping::None)
                .color(palette.muted),
        )
        .width(WIDTH)
        .padding([5, 10])
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(4).width(1).color(palette.border))
        })
        .into()
    } else {
        pick_list(options, selected_option, move |option| {
            Message::Settings(on_pick(option.choice))
        })
        .on_open(Message::Settings(Event::MenuOpened))
        .on_close(Message::Settings(Event::MenuClosed))
        .width(WIDTH)
        .padding([5, 10])
        .text_size(TEXT_SIZE)
        .font(Font::DEFAULT)
        .text_shaping(text::Shaping::Advanced)
        .wineo_style(palette)
        .into()
    };
    if shown == selected {
        control
    } else {
        tooltip(
            control,
            container(
                label(selected.to_owned())
                    .size(14)
                    .color(palette.foreground),
            )
            .max_width(480),
            tooltip::Position::Top,
        )
        .padding(8)
        .gap(4)
        .delay(std::time::Duration::from_millis(350))
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(4).width(1).color(palette.border))
        })
        .into()
    }
}

fn measured_width(value: &str) -> f32 {
    type Paragraph = <iced::Renderer as text::Renderer>::Paragraph;
    Paragraph::with_text(text::Text {
        content: value,
        bounds: Size::INFINITE,
        size: TEXT_SIZE.into(),
        line_height: text::LineHeight::default(),
        font: Font::DEFAULT,
        align_x: text::Alignment::Default,
        align_y: alignment::Vertical::Center,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::None,
    })
    .min_width()
}

fn fit(value: &str) -> String {
    if measured_width(value) <= LABEL_WIDTH {
        return value.to_owned();
    }
    let chars: Vec<_> = value.chars().collect();
    let shortened = |keep: usize| {
        let left = keep.div_ceil(2);
        let right = keep / 2;
        chars[..left]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(chars[chars.len() - right..].iter())
            .collect::<String>()
    };
    let (mut low, mut high) = (0, chars.len());
    while low + 1 < high {
        let keep = (low + high) / 2;
        if measured_width(&shortened(keep)) <= LABEL_WIDTH {
            low = keep;
        } else {
            high = keep;
        }
    }
    shortened(low)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_fit_and_keep_both_ends_without_changing_the_runner() {
        for name in [
            "GE-Proton-Experimental-Extremely-Long-Runner-Build-2026-09-30-x86_64",
            "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW",
            "非常に長いランナー名とバージョン情報非常に長いランナー名とバージョン情報",
        ] {
            let shown = fit(name);
            assert!(shown.contains('…'));
            assert!(measured_width(&shown) <= LABEL_WIDTH);
            assert_eq!(shown.chars().next(), name.chars().next());
            assert_eq!(shown.chars().last(), name.chars().last());
            let option = OptionLabel {
                choice: RunnerChoice {
                    name: name.to_owned(),
                    proton: Some(true),
                },
                label: shown,
            };
            assert_eq!(option.choice.name, name);
        }
        assert_eq!(fit("GE-Proton10-10"), "GE-Proton10-10");
        assert_eq!(fit("wine"), "wine");
    }
}
