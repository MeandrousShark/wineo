use iced::advanced::text::Paragraph as _;
use iced::widget::{
    Row, Space, button, column, container, image, mouse_area, row, scrollable, stack, text,
    text_input,
};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Size, Theme, border, font, mouse};

use crate::library::Card;
use crate::theme::Palette;
use crate::wheel;
use crate::{App, CARD_WIDTH, GAP, Message, SCROLL_ID, SEARCH_ID, WHEEL_STEP_SCALE};

impl App {
    pub(crate) fn grid(&self) -> Element<'_, Message> {
        let cards = self.filtered().into_iter().map(|card| self.card(card));
        container(
            Row::with_children(cards)
                .spacing(GAP)
                .wrap()
                .vertical_spacing(GAP),
        )
        .padding(16)
        .width(Fill)
        .into()
    }
    fn card<'a>(&'a self, card: &'a Card) -> Element<'a, Message> {
        let palette = self.palette;
        let hovered = self.hovered.as_ref() == Some(&card.name);
        let selected = self.selected.as_ref() == Some(&card.name);
        let art = card
            .art
            .as_ref()
            .and_then(|art| self.images.get(&(art.clone(), crate::artwork::Kind::Grid)));
        let poster: Element<'_, Message> = match art {
            Some(handle) => image(handle.clone())
                .width(Fill)
                .height(Fill)
                .content_fit(ContentFit::Cover)
                .border_radius(4)
                .opacity(if hovered { 0.85_f32 } else { 1.0_f32 })
                .into(),
            None => container(text(&card.name).size(14).color(palette.muted).center())
                .width(Fill)
                .height(Fill)
                .padding(12)
                .center(Fill)
                .style(move |_| {
                    container::Style::default().background(
                        palette
                            .surface
                            .scale_alpha(if hovered { 0.85 } else { 1.0 }),
                    )
                })
                .into(),
        };
        let framed = container(container(poster).clip(true))
            .width(CARD_WIDTH)
            .height(270)
            .padding(2)
            .style(move |_| {
                container::Style::default().border(border::rounded(6).width(2).color(if selected {
                    palette.primary
                } else {
                    Color::TRANSPARENT
                }))
            });
        let label = truncate_name(&card.name, CARD_WIDTH);
        let dot_color = card.dot(palette);
        let meta = row![
            container(Space::new())
                .width(8)
                .height(8)
                .style(move |_| container::Style::default()
                    .background(dot_color)
                    .border(border::rounded(4))),
            text(card.state_word()).size(12).color(palette.muted),
            text(card.playtime()).size(12).color(palette.muted),
        ]
        .spacing(8)
        .align_y(Center);
        let content = column![
            framed,
            text(label)
                .size(14)
                .font(Font {
                    weight: font::Weight::Semibold,
                    ..Font::DEFAULT
                })
                .color(palette.foreground)
                .wrapping(iced::widget::text::Wrapping::None),
            meta,
        ]
        .spacing(4)
        .width(CARD_WIDTH);
        mouse_area(content)
            .on_press(Message::Select(card.name.clone()))
            .on_double_click(Message::DoubleClick(card.name.clone()))
            .on_enter(Message::Hover(Some(card.name.clone())))
            .on_exit(Message::Hover(None))
            .interaction(mouse::Interaction::Pointer)
            .into()
    }
    pub(crate) fn view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        if let Some(page) = &self.settings {
            return page.view(palette, self.spinner_frame);
        }
        let mut page = column![
            self.top_bar(),
            container(Space::new())
                .height(1)
                .style(move |_| container::Style::default().background(palette.border))
        ];
        if let Some(error) = &self.error {
            page = page.push(
                container(text(format!("Failed to load library: {error}")).color(palette.danger))
                    .padding(16),
            );
        }
        let visible = self.scroll_visible_until.is_some();
        let scroll = scrollable(self.grid())
            .id(iced::widget::Id::new(SCROLL_ID))
            .width(Fill)
            .height(Fill)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new().width(6).scroller_width(4),
            ))
            .on_scroll(Message::Scrolled)
            .style(move |theme, status| scroll_style(theme, status, palette, visible));
        let mut content = row![wheel::scale(scroll, WHEEL_STEP_SCALE)]
            .width(Fill)
            .height(Fill);
        if self.panel_open
            && let Some(card) = self
                .selected
                .as_ref()
                .and_then(|name| self.cards.iter().find(|card| &card.name == name))
        {
            content = content.push(self.panel(card));
        }
        container(page.push(content))
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background))
            .into()
    }
    fn top_bar(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let input = text_input("Search library...", &self.search)
            .id(iced::widget::Id::new(SEARCH_ID))
            .on_input(Message::SearchChanged)
            .width(320)
            .padding(iced::Padding::from([6, 10]).right(30))
            .style(move |_, status| text_input::Style {
                background: palette.surface.into(),
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
            });
        // Keep the tree shape stable so clearing text does not drop input focus.
        let clear: Element<'_, Message> = if self.search.is_empty() {
            Space::new().into()
        } else {
            button(text("x").size(15).color(palette.muted))
                .on_press(Message::SearchChanged(String::new()))
                .padding(4)
                .style(|_, _| button::Style::default())
                .into()
        };
        let search = stack![
            input,
            container(clear)
                .width(Fill)
                .height(Fill)
                .align_right(Fill)
                .center_y(Fill)
                .padding([0, 6])
        ];
        container(
            row![
                text("wineo")
                    .size(18)
                    .font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                    .color(palette.foreground),
                search,
            ]
            .spacing(16)
            .align_y(Center),
        )
        .padding([8, 16])
        .width(Fill)
        .into()
    }
}

fn text_width(content: &str) -> f32 {
    iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: Size::INFINITE,
        size: 14.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font: Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        },
        align_x: iced::advanced::text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::None,
    })
    .min_bounds()
    .width
}

fn truncate_name(name: &str, max_width: f32) -> String {
    if text_width(name) <= max_width {
        return name.to_owned();
    }
    let chars: Vec<_> = name.chars().collect();
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = chars[..mid].iter().collect::<String>() + "...";
        if text_width(&candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    chars[..low]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_owned()
        + "..."
}

pub(super) fn scroll_style(
    theme: &Theme,
    status: scrollable::Status,
    palette: Palette,
    visible: bool,
) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    let show = visible || !matches!(status, scrollable::Status::Active { .. });
    for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
        rail.background = None;
        rail.border = border::rounded(3);
        rail.scroller.background = if show {
            palette.border.into()
        } else {
            Color::TRANSPARENT.into()
        };
        rail.scroller.border = border::rounded(3);
    }
    style
}
