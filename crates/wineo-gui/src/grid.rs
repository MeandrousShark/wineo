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

// Reserve enough height to keep the last action inside the window.
const ACTION_MENU_HEIGHT: f32 = 348.0;

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
            .on_right_press(Message::Context(card.name.clone()))
            .on_enter(Message::Hover(Some(card.name.clone())))
            .on_exit(Message::Hover(None))
            .interaction(mouse::Interaction::Pointer)
            .into()
    }
    /// How wide the Add, import, and clone dialog is in the current window.
    pub(crate) fn flow_width(&self) -> f32 {
        let widest = if matches!(self.flow, Some(crate::flow::Page::Choose(_))) {
            680.0
        } else {
            900.0
        };
        (self.window_size.width - 40.0).clamp(280.0, widest)
    }
    pub(crate) fn view(&self) -> Element<'_, Message> {
        crate::shortcuts::capture_with_undo(
            self.sidebar_view(),
            Message::Section,
            (self.toast.is_some()
                && self.removal.is_none()
                && !self.trash_confirm_open()
                && !self.downloads_confirm_open()
                && self.flow.is_none()
                && self.settings.is_none()
                && self.art_page.is_none()
                && !self.close_prompt)
                .then_some(Message::Undo),
        )
    }

    fn sidebar_view(&self) -> Element<'_, Message> {
        // Keep the page at a stable tree path so overlays preserve scroll and focus state.
        let mut page = stack![
            row![self.sidebar(), self.page_view()]
                .width(Fill)
                .height(Fill)
        ];
        if self.toast.is_some() {
            page = page.push(
                container(self.toast_view())
                    .padding(16)
                    .align_bottom(Fill)
                    .center_x(Fill),
            );
        }
        if self.removal.is_some() {
            page = page.push(iced::widget::opaque(
                mouse_area(container(self.remove_view()).center(Fill).style(|_| {
                    container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                }))
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.trash_confirm_open() {
            page = page.push(iced::widget::opaque(
                mouse_area(
                    container(self.trash_confirm_view())
                        .center(Fill)
                        .style(|_| {
                            container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                        }),
                )
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.downloads_confirm_open() {
            page = page.push(iced::widget::opaque(
                mouse_area(
                    container(self.downloads_confirm_view())
                        .center(Fill)
                        .style(|_| {
                            container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                        }),
                )
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if let Some(flow) = &self.flow {
            let palette = self.palette;
            let generation = self.flow_generation;
            let height = if matches!(flow, crate::flow::Page::Choose(_)) {
                430.0
            } else {
                720.0
            };
            let width = self.flow_width();
            let dialog = container(
                flow.view_with_key_availability(
                    palette,
                    width,
                    self.spinner_angle,
                    self.steamgriddb_available,
                )
                .map(move |event| Message::Flow(generation, event)),
            )
            .width(width)
            .height((self.window_size.height - 40.0).clamp(240.0, height))
            .clip(true)
            .style(move |_| {
                container::Style::default()
                    .background(palette.background)
                    .border(border::rounded(12).width(1).color(palette.border))
            });
            page = page.push(iced::widget::opaque(
                mouse_area(container(dialog).center(Fill).style(|_| {
                    container::Style::default().background(Color::BLACK.scale_alpha(0.55))
                }))
                .on_scroll(|_| Message::OverlayInput),
            ));
        }
        if self.close_prompt {
            let message = if self.closing {
                "Canceling and waiting for cleanup..."
            } else {
                "An operation is still running. Cancel it and close wineo?"
            };
            let controls = if self.closing
                && (self.change.is_some() || self.art_busy())
                && !self.checkup_force_available()
            {
                row![text("Waiting for the change to finish...")]
            } else if self.closing {
                row![button("Force stop").on_press(Message::ForceCloseWork)]
            } else {
                row![
                    button("Keep working").on_press(Message::KeepWorking),
                    button("Cancel and close").on_press(Message::CancelAndClose),
                ]
            }
            .spacing(12);
            let palette = self.palette;
            let prompt = container(column![text(message), controls].spacing(16))
                .padding(24)
                .style(move |_| {
                    container::Style::default()
                        .background(palette.surface)
                        .border(border::rounded(8).width(1).color(palette.border))
                });
            return page
                .push(iced::widget::opaque(
                    container(prompt)
                        .center(Fill)
                        .width(Fill)
                        .height(Fill)
                        .style(|_| {
                            container::Style::default().background(Color::BLACK.scale_alpha(0.6))
                        }),
                ))
                .into();
        }
        if let Some(name) = &self.action_menu {
            let mut position = menu_position(self.action_menu_position, self.window_size);
            position.y = position
                .y
                .min((self.window_size.height - ACTION_MENU_HEIGHT).max(8.0));
            let palette = self.palette;
            let menu = container(
                column![
                    text(truncate_name(name, 176.0))
                        .size(14)
                        .wrapping(iced::widget::text::Wrapping::None),
                    button("Settings").on_press(Message::ContextSettings(name.clone())),
                    button("Art...").on_press(Message::ContextArt(name.clone())),
                    button("Check for problems").on_press(Message::Checkup(
                        crate::checkup::Event::Open(crate::checkup::Scope::Environment(
                            name.clone()
                        ))
                    )),
                    button("Clone").on_press_maybe(
                        (!self.foreground_busy()).then(|| Message::CloneEnvironment(name.clone()))
                    ),
                    button("Remove...").on_press_maybe(
                        self.remove_reason(name)
                            .is_none()
                            .then(|| Message::Remove(name.clone()))
                    ),
                    text(self.remove_reason(name).unwrap_or_default())
                        .size(12)
                        .color(palette.muted),
                    button("Close menu").on_press(Message::CloseMenu),
                ]
                .spacing(8),
            )
            .padding(12)
            .width(220)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(6).width(1).color(palette.border))
            });
            return page
                .push(
                    mouse_area(Space::new().width(Fill).height(Fill))
                        .on_press(Message::CloseMenu)
                        .on_right_press(Message::CloseMenu),
                )
                .push(
                    container(menu)
                        .padding(iced::Padding {
                            top: position.y,
                            left: position.x,
                            right: 0.0,
                            bottom: 0.0,
                        })
                        .width(Fill)
                        .height(Fill),
                )
                .into();
        }
        page.into()
    }

    fn page_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        if let Some(name) = &self.art_page
            && let Some(page) = self.art_pages.get(name)
        {
            let name = name.clone();
            let generation = page.generation;
            return page
                .view_with_key_availability(palette, self.steamgriddb_available)
                .map(move |event| Message::ArtPage(name.clone(), generation, event));
        }
        if let Some(page) = &self.settings {
            return page.view(&self.paths, palette, self.spinner_angle);
        }
        let mut page = column![
            self.top_bar(),
            container(Space::new())
                .height(1)
                .style(move |_| container::Style::default().background(palette.border))
        ];
        if let Some(change) = &self.change {
            page = page.push(
                container(text(change.reason()).size(12).color(palette.muted)).padding([4, 16]),
            );
        }
        if self.section == crate::sidebar::Section::Downloads {
            return container(page.push(self.downloads_view()))
                .width(Fill)
                .height(Fill)
                .style(move |_| container::Style::default().background(palette.background))
                .into();
        }
        if self.section == crate::sidebar::Section::Trash {
            return container(page.push(self.trash_view()))
                .width(Fill)
                .height(Fill)
                .style(move |_| container::Style::default().background(palette.background))
                .into();
        }
        if self.section == crate::sidebar::Section::Checkup {
            return container(
                page.push(
                    container(
                        column![text(self.section.label()).size(22), self.checkup_page(),]
                            .spacing(12),
                    )
                    .padding(24),
                ),
            )
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background))
            .into();
        }
        if let Some(error) = &self.error {
            page = page.push(
                container(text(format!("Failed to load library: {error}")).color(palette.danger))
                    .padding(16),
            );
        }
        // Not `refreshing`, which flips every background refresh and made the
        // empty state blink.
        if self.cards.is_empty() && self.loaded {
            page = page.push(
                container(
                    column![
                        text("Your library is empty").size(22),
                        button("Add something")
                            .on_press_maybe((!self.foreground_busy()).then_some(Message::Add)),
                    ]
                    .spacing(12),
                )
                .padding(24),
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
        if self.section != crate::sidebar::Section::Library {
            return container(
                row![text("wineo").size(18).color(palette.foreground)].align_y(Center),
            )
            .padding([8, 16])
            .width(Fill)
            .into();
        }
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
        let input: Element<'_, Message> = if self.flow.is_some() {
            container(
                text(if self.search.is_empty() {
                    "Search library..."
                } else {
                    &self.search
                })
                .color(palette.muted),
            )
            .padding([6, 10])
            .width(320)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(6).width(1).color(palette.border))
            })
            .into()
        } else {
            input.into()
        };
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
                button("Add")
                    .on_press_maybe((!self.foreground_busy()).then_some(Message::Add))
                    .padding([6, 12])
                    .style(move |_, status| crate::chips::secondary(palette, status)),
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

fn menu_position(pointer: iced::Point, window: Size) -> iced::Point {
    iced::Point::new(
        (pointer.x + 4.0).clamp(8.0, (window.width - 228.0).max(8.0)),
        (pointer.y + 4.0).clamp(8.0, (window.height - 188.0).max(8.0)),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_menu_follows_pointer_and_stays_inside_window() {
        assert_eq!(
            menu_position(iced::Point::new(100.0, 200.0), Size::new(1200.0, 800.0)),
            iced::Point::new(104.0, 204.0)
        );
        let position = menu_position(iced::Point::new(1199.0, 799.0), Size::new(1200.0, 800.0));
        assert_eq!(position, iced::Point::new(972.0, 612.0));
        assert_eq!(
            menu_position(iced::Point::ORIGIN, Size::new(100.0, 100.0)),
            iced::Point::new(8.0, 8.0)
        );
    }
}
