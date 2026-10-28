//! The global page's tabs: a bar under the title and the arrow keys that
//! move along it.

use iced::advanced::widget::operation::{Focusable, focusable};
use iced::advanced::widget::{Operation, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{Id, Space, button, container, row, stack, text};
use iced::{Bottom, Color, Element, Fill, Length, Rectangle, Size, Vector};

use super::{Event, Page};
use crate::theme::Palette;
use crate::ui::{BUTTON_HEIGHT, list_row_style};
use crate::{Message, focus_button};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    General,
    Appearance,
}

impl Tab {
    pub(crate) const ALL: [Self; 2] = [Self::General, Self::Appearance];

    fn index(self) -> usize {
        match self {
            Self::General => 0,
            Self::Appearance => 1,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::General => "settings-tab-general",
            Self::Appearance => "settings-tab-appearance",
        }
    }
}

impl Page {
    /// Remembers where the shown tab is scrolled to.
    pub(super) fn scrolled(&mut self, offset: f32) {
        if self.global {
            self.scroll[self.tab.index()] = offset;
        }
    }

    /// Switching only changes what shows, so the draft and its count stay.
    pub(super) fn select_tab(&mut self, tab: Tab) -> super::Step {
        if !self.global || self.leave_prompt || self.tab == tab {
            return super::Step::none();
        }
        self.tab = tab;
        // The other tab's sections may be gone from under an open menu, so
        // its close never arrives.
        self.menu_open = false;
        // The tabs share one scroll area, so each gets back where it was left.
        super::Step::task(iced::widget::operation::scroll_to(
            Id::new(super::view::SCROLL_ID),
            iced::widget::scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll[tab.index()],
            },
        ))
    }
}

/// The tab buttons, each with a 2 px underline along its bottom edge that is
/// primary on the selected tab and clear on the others, so nothing shifts
/// when it moves.
/// `active` is false while a prompt covers the page.
pub(super) fn bar(selected: Tab, active: bool, palette: Palette) -> Element<'static, Message> {
    let mut tabs = row![].spacing(4);
    let mut ids = Vec::new();
    for tab in Tab::ALL {
        let on = tab == selected;
        let press = Message::Settings(Event::Tab(tab));
        let label = button(text(tab.label()).size(14).center())
            .height(BUTTON_HEIGHT)
            .padding([0, 12])
            .on_press(press.clone())
            .style(move |_, status| {
                let mut style = list_row_style(palette, status);
                if !on && status == button::Status::Active {
                    style.text_color = palette.muted;
                }
                style
            });
        // Laid over the button's bottom edge, since a fill-width bar in a
        // shrink-wide column would stretch to the whole row.
        let underline = container(
            container(Space::new())
                .width(Fill)
                .height(2)
                .style(move |_| {
                    container::Style::default().background(if on {
                        palette.primary
                    } else {
                        Color::TRANSPARENT
                    })
                }),
        )
        .width(Fill)
        .height(Fill)
        .align_y(Bottom);
        tabs = tabs.push(focus_button::control(
            stack![label, underline].into(),
            true,
            tab.id(),
            press.clone(),
        ));
        ids.push((Id::new(tab.id()), press));
    }
    Element::new(Arrows {
        content: tabs.into(),
        active,
        tabs: ids,
    })
}

/// Left and Right on a focused tab select and focus its neighbor, stopping
/// at the ends like the choice lists, so the arrow never reaches the page.
struct Arrows<'a> {
    content: Element<'a, Message>,
    active: bool,
    /// The tabs in order: their ids and what selecting each sends.
    tabs: Vec<(Id, Message)>,
}

struct FocusedTab<'a> {
    tabs: &'a [(Id, Message)],
    found: Option<usize>,
}

impl Operation for FocusedTab<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused()
            && let Some(index) = self.tabs.iter().position(|(tab, _)| Some(tab) == id)
        {
            self.found = Some(index);
        }
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for Arrows<'_> {
    fn children(&self) -> Vec<tree::Tree> {
        vec![tree::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut tree::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.active
            && let iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(named @ (Named::ArrowLeft | Named::ArrowRight)),
                modifiers,
                ..
            }) = event
            && !(modifiers.control() || modifiers.alt() || modifiers.logo())
        {
            let mut focused = FocusedTab {
                tabs: &self.tabs,
                found: None,
            };
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut focused,
            );
            if let Some(index) = focused.found {
                let next = if *named == Named::ArrowLeft {
                    index.checked_sub(1)
                } else {
                    Some(index + 1).filter(|next| *next < self.tabs.len())
                };
                if let Some(next) = next {
                    let (id, select) = &self.tabs[next];
                    self.content.as_widget_mut().operate(
                        &mut tree.children[0],
                        layout,
                        renderer,
                        &mut focusable::focus::<()>(id.clone()),
                    );
                    shell.publish(select.clone());
                    shell.request_redraw();
                }
                shell.capture_event();
                return;
            }
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut tree::Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
