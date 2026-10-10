use iced::advanced::text::Paragraph as _;
use iced::widget::{
    Space, button, column, container, image, mouse_area, row, scrollable, stack, svg, text,
    text_input,
};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Size, Theme, border, font, mouse};

use crate::library::Card;
use crate::theme::Palette;
use crate::ui;
use crate::wheel;
use crate::{App, CARD_WIDTH, Message, SCROLL_ID, SEARCH_ID, WHEEL_STEP_SCALE};

const ACTION_MENU_WIDTH: f32 = 248.0;
const ACTION_MENU_MAX_HEIGHT: f32 = 448.0;
const ACTION_MENU_MARGIN: f32 = 8.0;
const ACTION_MENU_PADDING: f32 = 12.0;
const ACTION_MENU_SPACING: f32 = 4.0;
const ACTION_MENU_HEADER_HEIGHT: f32 = 20.0;

impl App {
    pub(crate) fn grid(&self) -> Element<'_, Message> {
        let filtered = self.filtered();
        let names = filtered.iter().map(|card| card.name.clone()).collect();
        let cards = filtered.into_iter().map(|card| self.card(card)).collect();
        crate::grid_motion::grid(cards, names, &self.grid_motion)
    }
    fn card<'a>(&'a self, card: &'a Card) -> Element<'a, Message> {
        let palette = self.palette;
        let hovered = self.hovered.as_ref() == Some(&card.name);
        let selected = self.selected.as_ref() == Some(&card.name);
        let dimmed = card.not_installed();
        let art = card.art.as_ref().and_then(|art| {
            let key = (art.clone(), crate::artwork::Kind::Grid);
            if dimmed {
                self.dimmed_images.get(&key)
            } else {
                self.images.get(&key)
            }
        });
        let poster: Element<'_, Message> = match art {
            Some(handle) => image(handle.clone())
                .width(Fill)
                .height(Fill)
                .content_fit(ContentFit::Cover)
                .border_radius(4)
                .opacity(if dimmed {
                    0.45_f32
                } else if hovered {
                    0.85_f32
                } else {
                    1.0_f32
                })
                .into(),
            None => container(
                text(&card.display_title)
                    .size(14)
                    .color(palette.muted)
                    .center(),
            )
            .width(Fill)
            .height(Fill)
            .padding(12)
            .center(Fill)
            .style(move |_| {
                container::Style::default().background(palette.surface.scale_alpha(if dimmed {
                    0.45
                } else if hovered {
                    0.85
                } else {
                    1.0
                }))
            })
            .into(),
        };
        let mut layers = stack![poster];
        if card.kind == Some(crate::library::CardKind::Proxied) {
            let mark = svg(svg::Handle::from_memory(include_bytes!(
                "../assets/steam.svg"
            )))
            .width(20)
            .height(20)
            .opacity(if dimmed { 0.45_f32 } else { 1.0_f32 });
            layers = layers.push(
                container(mark)
                    .width(Fill)
                    .height(Fill)
                    .align_right(Fill)
                    .align_bottom(Fill)
                    .padding(8),
            );
        }
        let framed = container(container(layers).clip(true))
            .width(CARD_WIDTH)
            .height(240)
            .padding(2)
            .style(move |_| {
                container::Style::default().border(border::rounded(6).width(2).color(if selected {
                    palette.primary
                } else {
                    Color::TRANSPARENT
                }))
            });
        let label = truncate_name(&card.display_title, CARD_WIDTH);
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
        let mut content = column![
            framed,
            text(label)
                .size(14)
                .font(Font {
                    weight: font::Weight::Semibold,
                    ..Font::DEFAULT
                })
                .color(if dimmed {
                    palette.muted
                } else {
                    palette.foreground
                })
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(4)
        .width(CARD_WIDTH);
        if card.name != card.display_title {
            content = content.push(
                text(truncate_name(&card.name, CARD_WIDTH))
                    .size(12)
                    .color(palette.muted)
                    .wrapping(iced::widget::text::Wrapping::None),
            );
        }
        content = content.push(meta);
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
                && self.hook_stop_confirm.is_none()
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
        if let Some((message, _)) = &self.feature_notice {
            let palette = self.palette;
            page = page.push(
                container(
                    container(text(message).color(palette.foreground))
                        .padding(12)
                        .style(move |_| {
                            container::Style::default()
                                .background(palette.surface)
                                .border(border::rounded(8).width(1).color(palette.border))
                        }),
                )
                .padding(16)
                .align_bottom(Fill)
                .center_x(Fill),
            );
        }
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
        if self.hook_stop_confirm.is_some() {
            page = page.push(iced::widget::opaque(
                mouse_area(container(self.hook_stop_view()).center(Fill).style(|_| {
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
            // Fits all four Add choices; smaller windows scroll them.
            let height = if matches!(flow, crate::flow::Page::Choose(_)) {
                560.0
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
                if self.feature_finishing() {
                    "Finishing changes..."
                } else {
                    "Canceling and waiting for cleanup..."
                }
            } else {
                "An operation is still running. Cancel it and close wineo?"
            };
            let controls = if self.closing
                && (self.change.is_some() || self.art_busy() || self.filesystem_busy())
                && !self.checkup_force_available()
            {
                row![text("Waiting for the change to finish...")]
            } else if self.closing
                && (self
                    .tool_pages
                    .values()
                    .any(crate::tools_page::Page::force_available)
                    || self.operation.is_some()
                    || self.flow.as_ref().is_some_and(crate::flow::Page::busy))
            {
                row![button("Force stop").on_press(Message::ForceCloseWork)]
            } else if self.closing {
                row![text("Waiting for cleanup...")]
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
            let palette = self.palette;
            let wine_actions = self.wine_actions(name);
            let transfer_actions = self.transfer_actions(name);
            let removal_reason = self.remove_reason(name);
            let menu_buttons =
                4 + usize::from(wine_actions) * 3 + usize::from(transfer_actions) * 2;
            let menu_dividers = 1 + usize::from(wine_actions);
            let body_content_height =
                action_menu_body_height(menu_buttons, menu_dividers, removal_reason.as_deref());
            let menu_height = action_menu_height(self.window_size.height, body_content_height);
            let position = menu_position(self.action_menu_position, self.window_size, menu_height);
            let mut scroll_items = column![
                context_menu_button(
                    "Settings",
                    palette,
                    false,
                    Some(Message::ContextSettings(name.clone())),
                ),
                context_menu_button(
                    "Art...",
                    palette,
                    false,
                    Some(Message::ContextArt(name.clone())),
                ),
            ]
            .spacing(ACTION_MENU_SPACING);
            if wine_actions {
                scroll_items = scroll_items.push(context_menu_button(
                    "Snapshots...",
                    palette,
                    false,
                    Some(Message::SnapshotsEnvironment(name.clone())),
                ));
            }
            if transfer_actions {
                scroll_items = scroll_items.push(context_menu_button(
                    "Copy to portable folder...",
                    palette,
                    false,
                    Some(Message::CopyPortable(name.clone())),
                ));
            }
            if wine_actions {
                scroll_items = scroll_items
                    .push(ui::separator(palette).padding([3, 0]))
                    .push(context_menu_button(
                        "Run program...",
                        palette,
                        false,
                        Some(Message::OpenTool(
                            name.clone(),
                            crate::tools_page::Kind::Exec,
                        )),
                    ))
                    .push(context_menu_button(
                        "Winetricks...",
                        palette,
                        false,
                        Some(Message::OpenTool(
                            name.clone(),
                            crate::tools_page::Kind::Winetricks,
                        )),
                    ));
            }
            scroll_items = scroll_items
                .push(context_menu_button(
                    "Check for problems",
                    palette,
                    false,
                    Some(Message::Checkup(crate::checkup::Event::Open(
                        crate::checkup::Scope::Environment(name.clone()),
                    ))),
                ))
                .push(ui::separator(palette).padding([3, 0]));
            if transfer_actions {
                scroll_items = scroll_items.push(context_menu_button(
                    "Clone",
                    palette,
                    false,
                    (!self.management_busy() && removal_reason.is_none())
                        .then(|| Message::CloneEnvironment(name.clone())),
                ));
            }
            scroll_items = scroll_items.push(context_menu_button(
                "Remove...",
                palette,
                true,
                removal_reason
                    .is_none()
                    .then(|| Message::Remove(name.clone())),
            ));
            if let Some(reason) = removal_reason {
                scroll_items = scroll_items.push(
                    text(reason)
                        .size(12)
                        .width(Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .color(palette.muted),
                );
            }
            let menu_items = column![
                text(truncate_name(name, 224.0))
                    .size(14)
                    .font(Font {
                        weight: font::Weight::Semibold,
                        ..Font::DEFAULT
                    })
                    .color(palette.foreground)
                    .height(ACTION_MENU_HEADER_HEIGHT)
                    .align_y(Center)
                    .wrapping(iced::widget::text::Wrapping::None),
                scrollable(scroll_items).height(action_menu_scroll_height(menu_height)),
                context_menu_button("Close menu", palette, false, Some(Message::CloseMenu)),
            ]
            .spacing(ACTION_MENU_SPACING)
            .height(Fill);
            let menu = container(menu_items)
                .padding(ACTION_MENU_PADDING)
                .width(ACTION_MENU_WIDTH)
                .height(menu_height)
                .style(move |_| {
                    container::Style::default()
                        .background(palette.surface)
                        .border(
                            border::rounded(ui::SECTION_RADIUS)
                                .width(1)
                                .color(palette.border),
                        )
                        .shadow(iced::Shadow {
                            color: Color::BLACK.scale_alpha(0.2),
                            offset: iced::Vector::new(0.0, 4.0),
                            blur_radius: 14.0,
                        })
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
        if let Some((name, kind)) = &self.tool_page
            && let Some(page) = self.tool_pages.get(&(name.clone(), *kind))
        {
            let name = name.clone();
            let kind = *kind;
            let reason = if page.busy() {
                None
            } else {
                self.tool_start_reason(&name, kind)
            };
            let blocked = reason.is_some();
            let content: Element<'_, crate::tools_page::Event> = column![
                reason.map(|reason| text(reason).color(palette.warning)),
                page.view(palette, blocked),
            ]
            .into();
            return content.map(move |event| Message::Tool(name.clone(), kind, event));
        }
        if let Some(name) = &self.portable_page
            && let Some(page) = self.portable_pages.get(name)
        {
            let name = name.clone();
            return page
                .view(
                    palette,
                    self.management_busy() || self.remove_reason(&name).is_some(),
                )
                .map(move |event| Message::Portable(name.clone(), event));
        }
        if self.section == crate::sidebar::Section::Snapshots
            && let Some(page) = &self.snapshots_page
        {
            let blocked = self.management_busy()
                || page
                    .selected_name()
                    .is_some_and(|name| self.prefix_change_reason(name).is_some());
            return page.view(palette, blocked).map(Message::Snapshots);
        }
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
                        column![
                            crate::ui::page_title(self.section.label(), palette),
                            self.checkup_page(),
                        ]
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
                        crate::ui::accent_control(
                            "Add something",
                            palette.primary,
                            !self.management_busy() && self.operation.is_none(),
                            Message::Add,
                        ),
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
        let library = mouse_area(wheel::scale(scroll, WHEEL_STEP_SCALE));
        let library = if self.panel_open {
            library.on_press(Message::ClosePanel)
        } else {
            library
        };
        let panel_layer: Element<'_, Message> = if self.panel_reserved() {
            let panel = self
                .selected
                .as_ref()
                .and_then(|name| self.cards.iter().find(|card| &card.name == name))
                .map(|card| self.panel(card))
                .unwrap_or_else(|| {
                    container(Space::new())
                        .width(crate::grid_motion::PANEL_WIDTH)
                        .height(Fill)
                        .into()
                });
            container(
                row![
                    iced::widget::opaque(
                        container(crate::panel_motion::presentation(
                            panel,
                            self.panel_motion.progress(),
                            self.panel_open && self.panel_motion.interactive(),
                            palette.surface,
                        ))
                        .width(crate::grid_motion::PANEL_WIDTH)
                        .height(Fill)
                        .style(move |_| container::Style::default().background(palette.surface)),
                    ),
                    Space::new().width(crate::grid_motion::RAIL_WIDTH),
                ]
                .width(crate::grid_motion::DOCK_WIDTH)
                .height(Fill),
            )
            .width(Fill)
            .height(Fill)
            .align_right(Fill)
            .into()
        } else {
            container(Space::new()).width(Fill).height(Fill).into()
        };
        let content = stack![library, panel_layer];
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
                border: border::rounded(crate::ui::CONTROL_RADIUS).width(1).color(
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
                    .border(
                        border::rounded(crate::ui::CONTROL_RADIUS)
                            .width(1)
                            .color(palette.border),
                    )
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
                crate::ui::accent_control(
                    "Add",
                    palette.primary,
                    !self.management_busy() && self.operation.is_none(),
                    Message::Add,
                ),
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

fn menu_position(pointer: iced::Point, window: Size, menu_height: f32) -> iced::Point {
    iced::Point::new(
        (pointer.x + 4.0).clamp(
            ACTION_MENU_MARGIN,
            (window.width - ACTION_MENU_WIDTH - ACTION_MENU_MARGIN).max(ACTION_MENU_MARGIN),
        ),
        (pointer.y + 4.0).clamp(
            ACTION_MENU_MARGIN,
            (window.height - menu_height - ACTION_MENU_MARGIN).max(ACTION_MENU_MARGIN),
        ),
    )
}

fn action_menu_height(window_height: f32, body_content_height: f32) -> f32 {
    let chrome = ACTION_MENU_PADDING * 2.0
        + ACTION_MENU_HEADER_HEIGHT
        + ui::BUTTON_HEIGHT
        + ACTION_MENU_SPACING * 2.0;
    (chrome + body_content_height)
        .min(ACTION_MENU_MAX_HEIGHT)
        .min((window_height - ACTION_MENU_MARGIN * 2.0).max(0.0))
}

fn action_menu_scroll_height(menu_height: f32) -> f32 {
    (menu_height
        - ACTION_MENU_PADDING * 2.0
        - ACTION_MENU_HEADER_HEIGHT
        - ui::BUTTON_HEIGHT
        - ACTION_MENU_SPACING * 2.0)
        .max(0.0)
}

fn action_menu_body_height(buttons: usize, dividers: usize, reason: Option<&str>) -> f32 {
    let reason_height = reason.map_or(0.0, |reason| text_height(reason, ACTION_MENU_WIDTH - 24.0));
    let items = buttons + dividers + usize::from(reason.is_some());
    buttons as f32 * ui::BUTTON_HEIGHT
        + dividers as f32 * 7.0
        + reason_height
        + items.saturating_sub(1) as f32 * ACTION_MENU_SPACING
}

fn text_height(content: &str, width: f32) -> f32 {
    iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: Size::new(width, f32::INFINITY),
        size: 12.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font: Font::DEFAULT,
        align_x: iced::advanced::text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::WordOrGlyph,
    })
    .min_bounds()
    .height
}

fn context_menu_button(
    label: &'static str,
    palette: Palette,
    danger: bool,
    message: Option<Message>,
) -> iced::widget::Button<'static, Message> {
    button(text(label).size(14).height(Fill).align_y(Center))
        .width(Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press_maybe(message)
        .style(move |_, status| {
            let background = match status {
                button::Status::Active | button::Status::Disabled => Color::TRANSPARENT,
                button::Status::Hovered if danger => palette.danger.scale_alpha(0.12),
                button::Status::Pressed if danger => palette.danger.scale_alpha(0.2),
                button::Status::Hovered => palette.foreground.scale_alpha(0.06),
                button::Status::Pressed => palette.primary.scale_alpha(0.16),
            };
            iced::widget::button::Style {
                background: Some(background.into()),
                text_color: if status == button::Status::Disabled {
                    palette.muted
                } else if danger {
                    palette.danger
                } else {
                    palette.foreground
                },
                border: border::rounded(ui::CONTROL_RADIUS)
                    .width(1)
                    .color(Color::TRANSPARENT),
                ..iced::widget::button::Style::default()
            }
        })
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
        let height = action_menu_height(800.0, 400.0);
        assert_eq!(
            menu_position(
                iced::Point::new(100.0, 200.0),
                Size::new(1200.0, 800.0),
                height,
            ),
            iced::Point::new(104.0, 204.0)
        );
        let position = menu_position(
            iced::Point::new(1199.0, 799.0),
            Size::new(1200.0, 800.0),
            height,
        );
        assert_eq!(position, iced::Point::new(944.0, 344.0));
        assert_eq!(
            menu_position(
                iced::Point::ORIGIN,
                Size::new(100.0, 100.0),
                action_menu_height(100.0, 0.0),
            ),
            iced::Point::new(8.0, 8.0)
        );
    }

    #[test]
    fn context_menu_keeps_close_reachable_for_long_reasons_in_small_windows() {
        let reason = format!("Busy: removing {}", "long-environment-name-".repeat(80));
        let content_height = action_menu_body_height(6, 1, Some(&reason));
        let window = Size::new(800.0, 600.0);
        let height = action_menu_height(window.height, content_height);
        let scroll_height = action_menu_scroll_height(height);
        let position = menu_position(iced::Point::new(799.0, 599.0), window, height);

        assert!(content_height > scroll_height);
        assert!(scroll_height > 0.0);
        assert!(height <= window.height - ACTION_MENU_MARGIN * 2.0);
        assert!(position.y + height <= window.height - ACTION_MENU_MARGIN);
        assert_eq!(height, ACTION_MENU_MAX_HEIGHT);
    }

    #[test]
    fn context_menu_shrinks_to_its_content_for_native_environments() {
        let body_height = action_menu_body_height(6, 1, None);
        assert_eq!(action_menu_height(800.0, body_height), 307.0);
    }
}
