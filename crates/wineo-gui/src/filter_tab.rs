//! The tab that hangs from the Filters button while store, platform, or
//! tag filters are on: a chip with an x for each, and one x that clears
//! them all. It sits over the gap between the toolbar and the grid, so it
//! never moves the grid.

use iced::advanced::widget::{Id, Operation, operation::Focusable};
use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer, widget::tree,
};
use iced::widget::{Row, button, container, mouse_area, text, tooltip};
use iced::{Center, Color, Element, Fill, Font, Length, Rectangle, Size, Task, Vector, border};

use crate::filter_control::{Event, FILTER_BUTTON_ID};
use crate::library_menu::icon;
use crate::theme::Palette;
use crate::{App, Message, focus_button};

pub(crate) const CROSS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round"/></svg>"##;

const MORE_ID: &str = "library-filter-tab-more";
const CLEAR_ID: &str = "library-filter-tab-clear";

/// The room the tab leaves above and below itself, between the Filters
/// button and the first poster.
const TAB_GAP: f32 = 3.0;
const TAB_HEIGHT: f32 = 26.0;
/// The Library toolbar's bottom padding. With the 1 px rule under the
/// toolbar and the grid's padding, it makes room for the tab and a gap on
/// each side, so the tab never covers a poster at the top of the grid.
pub(crate) const TOOLBAR_PADDING_BOTTOM: f32 =
    2.0 * TAB_GAP + TAB_HEIGHT - 1.0 - crate::grid_motion::PADDING;
const TAB_RADIUS: f32 = 8.0;
const TAB_PADDING_LEFT: f32 = 4.0;
const TAB_PADDING_RIGHT: f32 = 1.0;
/// The tab's 1 px border on each side.
const TAB_BORDER: f32 = 1.0;
const SPACING: f32 = 4.0;
/// Room kept between the tab and the window's edge or the docked panel.
const MARGIN: f32 = 16.0;
const TEXT_SIZE: f32 = 13.0;
const CHIP_PADDING_LEFT: f32 = 8.0;
const CHIP_PADDING_RIGHT: f32 = 2.0;
const CHIP_GAP: f32 = 2.0;
/// A chip's x: a 12 px icon in a 16 px target.
const CHIP_X: f32 = 16.0;
const CHIP_ICON: f32 = 12.0;
const CHIP_RADIUS: f32 = 11.0;
const MORE_PADDING: f32 = 8.0;
const CLEAR_SIZE: f32 = 24.0;
const CLEAR_ICON: f32 = 14.0;

/// A checked filter the tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Chip {
    Store(&'static str),
    Platform(&'static str),
    Tag(String),
}

impl Chip {
    fn label(&self) -> &str {
        match self {
            Self::Store(label) | Self::Platform(label) => label,
            Self::Tag(tag) => tag,
        }
    }

    fn id(&self) -> String {
        match self {
            Self::Store(label) => format!("library-filter-tab-store-{}", label.to_lowercase()),
            Self::Platform(label) => {
                format!("library-filter-tab-platform-{}", label.to_lowercase())
            }
            Self::Tag(tag) => format!("library-filter-tab-tag-{tag}"),
        }
    }
}

/// What the tab shows: where it hangs, its chips, and how many of them fit
/// before the rest fold into +N.
#[derive(Debug)]
pub(crate) struct Tab {
    button: Rectangle,
    chips: Vec<Chip>,
    visible: usize,
}

impl Tab {
    fn hidden(&self) -> usize {
        self.chips.len() - self.visible
    }

    /// The tab's focusable controls, in order.
    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.chips[..self.visible].iter().map(Chip::id).collect();
        if self.hidden() > 0 {
            ids.push(MORE_ID.to_owned());
        }
        ids.push(CLEAR_ID.to_owned());
        ids
    }

    /// The control that stands for chip `index`: its own x, or +N when
    /// it's folded away.
    fn id_at(&self, index: usize) -> String {
        if index < self.visible {
            self.chips[index].id()
        } else {
            MORE_ID.to_owned()
        }
    }
}

fn text_width(content: &str) -> f32 {
    crate::tile_title::wrapped_size(content, f32::INFINITY, TEXT_SIZE, Font::DEFAULT)
        .width
        .ceil()
}

fn chip_width(label: &str) -> f32 {
    CHIP_PADDING_LEFT + text_width(label) + CHIP_GAP + CHIP_X + CHIP_PADDING_RIGHT
}

fn more_label(hidden: usize) -> String {
    format!("+{hidden}")
}

fn more_width(hidden: usize) -> f32 {
    2.0 * MORE_PADDING + text_width(&more_label(hidden))
}

/// How many of the chips, `widths` wide, fit in `room` along with a +N chip
/// for the rest when some don't, each followed by the spacing. When none
/// fit, only +N shows.
fn fit(widths: &[f32], more: impl Fn(usize) -> f32, room: f32) -> usize {
    let total = widths.len();
    let mut used = Vec::with_capacity(total + 1);
    let mut sum = 0.0;
    used.push(sum);
    for width in widths {
        sum += width + SPACING;
        used.push(sum);
    }
    if sum <= room {
        return total;
    }
    (0..total)
        .rev()
        .find(|&shown| used[shown] + more(total - shown) + SPACING <= room)
        .unwrap_or(0)
}

impl App {
    /// The checked stores, platforms, and tags, each group in the popup's order.
    pub(crate) fn filter_chips(&self) -> Vec<Chip> {
        let offered = self.offered();
        let mut chips: Vec<Chip> = offered
            .stores
            .into_iter()
            .filter(|label| self.filters.stores.contains(label))
            .map(Chip::Store)
            .collect();
        chips.extend(
            offered
                .platforms
                .into_iter()
                .filter(|label| self.filters.platforms.contains(label))
                .map(Chip::Platform),
        );
        chips.extend(
            offered
                .tags
                .into_iter()
                .filter(|tag| self.filters.tags.contains(tag))
                .map(Chip::Tag),
        );
        chips
    }

    /// The tab shows over the Library while a store, platform, or tag
    /// filter is on, once the Filters button has reported where it is. It
    /// stays away while a row under the toolbar shows, so it never covers a
    /// warning, and when even +N and its x don't fit.
    pub(crate) fn filter_tab(&self) -> Option<Tab> {
        if self.filters.active() == 0
            || self.section != crate::sidebar::Section::Library
            || self.panel_hidden()
            || self.change.is_some()
            || self.library_problem.is_some()
            || self.trash_banner_text().is_some()
            || self.error.is_some()
        {
            return None;
        }
        let button = self.filter_button?;
        let chips = self.filter_chips();
        let mut right = self.window_size.width - MARGIN;
        if self.panel_reserved() {
            right -= crate::grid_motion::DOCK_WIDTH;
        }
        let room =
            right - button.x - 2.0 * TAB_BORDER - TAB_PADDING_LEFT - TAB_PADDING_RIGHT - CLEAR_SIZE;
        let widths: Vec<f32> = chips.iter().map(|chip| chip_width(chip.label())).collect();
        let visible = fit(&widths, more_width, room);
        if visible == 0 && more_width(chips.len()) + SPACING > room {
            return None;
        }
        Some(Tab {
            button,
            chips,
            visible,
        })
    }

    /// A menu is open over the tab, which then only shows.
    fn filter_tab_blocked(&self) -> bool {
        self.library_menu_open() || self.action_menu.is_some()
    }

    /// The tab as a layer over the page, or nothing when it isn't showing.
    pub(crate) fn filter_tab_view(&self) -> Element<'_, Message> {
        let Some(tab) = self.filter_tab() else {
            return iced::widget::Space::new().into();
        };
        let palette = self.palette;
        let live = !self.filter_tab_blocked();
        let hidden = tab.hidden();
        let mut items: Vec<Element<'_, Message>> = tab
            .chips
            .into_iter()
            .take(tab.visible)
            .map(|chip| chip_view(chip, palette, live))
            .collect();
        if hidden > 0 {
            items.push(more_view(hidden, palette, live));
        }
        items.push(clear_view(palette, live));
        let content = container(Row::with_children(items).spacing(SPACING).align_y(Center))
            .height(TAB_HEIGHT)
            .padding(iced::Padding {
                top: TAB_BORDER,
                bottom: TAB_BORDER,
                left: TAB_BORDER + TAB_PADDING_LEFT,
                right: TAB_BORDER + TAB_PADDING_RIGHT,
            })
            .center_y(TAB_HEIGHT)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(iced::Border {
                        color: palette.border,
                        width: TAB_BORDER,
                        radius: TAB_RADIUS.into(),
                    })
            });
        // Claims the pointer over the whole tab, so the grid underneath takes
        // no clicks or hover through it. That keeps the wheel from it too, so
        // the wheel is handed on.
        let shield = mouse_area(content)
            .interaction(mouse::Interaction::Idle)
            .on_scroll(|delta| Message::Filter(Event::Wheel(delta)));
        container(shield)
            .padding(iced::Padding {
                top: tab.button.y + tab.button.height + TAB_GAP,
                left: tab.button.x,
                right: 0.0,
                bottom: 0.0,
            })
            .width(Fill)
            .height(Fill)
            .into()
    }
}

fn tip<'a>(
    content: Element<'a, Message>,
    label: String,
    palette: Palette,
    live: bool,
) -> Element<'a, Message> {
    if !live {
        return content;
    }
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

/// A quiet round button: no fill until hovered or pressed, and a primary
/// border while pressed, like the toolbar's buttons.
fn quiet_style(palette: Palette, radius: f32, status: button::Status) -> button::Style {
    let (background, border_color) = match status {
        button::Status::Hovered => (
            crate::ui::blend(palette.surface, palette.foreground, 0.08),
            Color::TRANSPARENT,
        ),
        button::Status::Pressed => (
            crate::ui::blend(palette.surface, palette.primary, 0.18),
            palette.primary,
        ),
        button::Status::Active | button::Status::Disabled => {
            (Color::TRANSPARENT, Color::TRANSPARENT)
        }
    };
    button::Style {
        background: Some(background.into()),
        text_color: palette.foreground,
        border: border::rounded(radius).width(1).color(border_color),
        ..button::Style::default()
    }
}

fn cross(palette: Palette, size: f32, rest: Color, live: bool) -> Element<'static, Message> {
    icon(CROSS, rest, size)
        .style(move |_, status| iced::widget::svg::Style {
            color: Some(if live && status == iced::widget::svg::Status::Hovered {
                palette.foreground
            } else {
                rest
            }),
        })
        .into()
}

fn chip_view(chip: Chip, palette: Palette, live: bool) -> Element<'static, Message> {
    let width = chip_width(chip.label());
    let id = chip.id();
    let label = chip.label().to_owned();
    let x = button(container(cross(palette, CHIP_ICON, palette.muted, live)).center(CHIP_X))
        .width(CHIP_X)
        .height(CHIP_X)
        .padding(0)
        .on_press_maybe(live.then(|| Message::Filter(Event::Remove(chip.clone()))))
        .style(move |_, status| quiet_style(palette, CHIP_X / 2.0, status));
    let x = tip(x.into(), format!("Remove filter: {label}"), palette, live);
    let x = focus_button::keyboard_control(
        x,
        live,
        id,
        Message::Filter(Event::RemoveFromKeyboard(chip)),
    );
    container(
        iced::widget::row![
            text(label)
                .size(TEXT_SIZE)
                .wrapping(iced::widget::text::Wrapping::None),
            x,
        ]
        .spacing(CHIP_GAP)
        .align_y(Center),
    )
    .width(width)
    .padding(iced::Padding {
        top: 2.0,
        bottom: 2.0,
        left: CHIP_PADDING_LEFT,
        right: CHIP_PADDING_RIGHT,
    })
    .style(move |_| {
        container::Style::default()
            .border(border::rounded(CHIP_RADIUS).width(1).color(palette.border))
    })
    .into()
}

fn more_view(hidden: usize, palette: Palette, live: bool) -> Element<'static, Message> {
    let more = button(
        text(more_label(hidden))
            .size(TEXT_SIZE)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .width(more_width(hidden))
    .padding([2.0, MORE_PADDING])
    .on_press_maybe(live.then_some(Message::Filter(Event::ToggleMenu)))
    .style(move |_, status| {
        let mut style = quiet_style(palette, CHIP_RADIUS, status);
        if status != button::Status::Pressed {
            style.border.color = palette.border;
        }
        style
    });
    let label = if hidden == 1 {
        "1 more filter".to_owned()
    } else {
        format!("{hidden} more filters")
    };
    focus_button::keyboard_control(
        tip(more.into(), label, palette, live),
        live,
        MORE_ID,
        Message::Filter(Event::ToggleMenu),
    )
}

fn clear_view(palette: Palette, live: bool) -> Element<'static, Message> {
    let clear =
        button(container(cross(palette, CLEAR_ICON, palette.foreground, live)).center(Fill))
            .width(CLEAR_SIZE)
            .height(CLEAR_SIZE)
            .padding(0)
            .on_press_maybe(live.then_some(Message::Filter(Event::ClearChips)))
            .style(move |_, status| quiet_style(palette, CLEAR_SIZE / 2.0, status));
    focus_button::keyboard_control(
        tip(clear.into(), "Clear all filters".to_owned(), palette, live),
        live,
        CLEAR_ID,
        Message::Filter(Event::ClearChipsFromKeyboard),
    )
}

/// Removes one chip's filter. From the keyboard, focus moves to the next
/// chip's x, else the previous one's, else back to the Filters button.
pub(crate) fn remove(app: &mut App, chip: &Chip, keyboard: bool) -> Task<Message> {
    if app.filter_tab_blocked() {
        return Task::none();
    }
    let index = app.filter_chips().iter().position(|shown| shown == chip);
    let removed = match chip {
        Chip::Store(label) => app.filters.stores.remove(label),
        Chip::Platform(label) => app.filters.platforms.remove(label),
        Chip::Tag(tag) => app.filters.tags.remove(tag),
    };
    if !removed {
        return Task::none();
    }
    let resorted = app.resorted();
    let Some(index) = index.filter(|_| keyboard) else {
        return Task::batch([resorted, unfocus()]);
    };
    let target = match app.filter_tab() {
        Some(tab) if index < tab.chips.len() => tab.id_at(index),
        Some(tab) if index > 0 => tab.id_at(index - 1),
        _ => FILTER_BUTTON_ID.to_owned(),
    };
    Task::batch([resorted, iced::widget::operation::focus(Id::from(target))])
}

/// A click on the tab ends like a click on any control that then goes
/// away: nothing keeps focus, including a control beneath the tab that the
/// press never reached.
fn unfocus() -> Task<Message> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
}

/// Clears the store, platform, and tag filters, leaving the search alone
/// since the tab doesn't show it.
pub(crate) fn clear(app: &mut App, keyboard: bool) -> Task<Message> {
    if app.filter_tab_blocked() || app.filters.active() == 0 {
        return Task::none();
    }
    app.filters.stores.clear();
    app.filters.platforms.clear();
    app.filters.tags.clear();
    let resorted = app.resorted();
    if !keyboard {
        return Task::batch([resorted, unfocus()]);
    }
    Task::batch([
        resorted,
        iced::widget::operation::focus(Id::new(FILTER_BUTTON_ID)),
    ])
}

/// Scrolls the grid by a wheel turn over the tab, as the grid's own
/// scrollable would.
pub(crate) fn wheel(delta: mouse::ScrollDelta, shift: bool) -> Task<Message> {
    // iced's scrollable moves 60 px per line, and swaps a line turn's axes
    // while Shift is held.
    const LINE: f32 = 60.0;
    let y = match crate::wheel::scale_delta(delta, crate::WHEEL_STEP_SCALE) {
        mouse::ScrollDelta::Lines { x, .. } if shift => -x * LINE,
        mouse::ScrollDelta::Lines { y, .. } => -y * LINE,
        mouse::ScrollDelta::Pixels { y, .. } => -y,
    };
    iced::widget::operation::scroll_by(
        Id::new(crate::SCROLL_ID),
        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
    )
}

/// Tab and Shift+Tab while the tab shows. Its controls come last in the
/// widget tree, since it's a layer over the page, so this moves through
/// them as if they came right after the Filters button.
pub(crate) fn step(app: &App, reverse: bool) -> Task<Message> {
    let tab = app
        .filter_tab()
        .map(|tab| tab.ids().into_iter().map(Id::from).collect())
        .unwrap_or_default();
    iced::advanced::widget::operate(Step {
        tab,
        ids: Vec::new(),
        focused: None,
        reverse,
    })
    .discard()
}

struct Step {
    tab: Vec<Id>,
    ids: Vec<Option<Id>>,
    focused: Option<usize>,
    reverse: bool,
}

impl Operation for Step {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() {
            self.focused = Some(self.ids.len());
        }
        self.ids.push(id.cloned());
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<()> {
        use iced::advanced::widget::operation::Outcome;
        let in_tab = |index: &usize| {
            self.ids[*index]
                .as_ref()
                .is_some_and(|id| self.tab.contains(id))
        };
        let (tab, mut order): (Vec<usize>, Vec<usize>) = (0..self.ids.len()).partition(in_tab);
        let filters = Id::new(FILTER_BUTTON_ID);
        let at = order
            .iter()
            .position(|&index| self.ids[index].as_ref() == Some(&filters))
            .map_or(order.len(), |at| at + 1);
        order.splice(at..at, tab);
        if order.is_empty() {
            return Outcome::None;
        }
        let current = self
            .focused
            .and_then(|focused| order.iter().position(|&index| index == focused));
        // Like `focus_next` and `focus_previous`, stepping past either end
        // leaves nothing focused rather than wrapping.
        let next = match (current, self.reverse) {
            (Some(at), true) => at.checked_sub(1),
            (Some(at), false) => Some(at + 1).filter(|&next| next < order.len()),
            (None, true) => Some(order.len() - 1),
            (None, false) => Some(0),
        };
        Outcome::Chain(Box::new(FocusIndex {
            target: next.map(|next| order[next]),
            current: 0,
        }))
    }
}

/// Focuses the focusable at `target` in tree order and unfocuses the rest.
struct FocusIndex {
    target: Option<usize>,
    current: usize,
}

impl Operation for FocusIndex {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if Some(self.current) == self.target {
            state.focus();
        } else {
            state.unfocus();
        }
        self.current += 1;
    }
}

/// Reports the Filters button's bounds when a redraw finds them changed,
/// so the tab can hang from it. A redraw follows every layout, so the tab
/// trails a move by at most a frame.
pub(crate) fn track<'a, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> Element<'a, Message, Theme, Renderer>
where
    Theme: 'a,
    Renderer: AdvancedRenderer + 'a,
{
    Element::new(Track {
        content: content.into(),
    })
}

struct Track<'a, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

#[derive(Default)]
struct Last(Option<Rectangle>);

impl<Theme, Renderer> Widget<Message, Theme, Renderer> for Track<'_, Theme, Renderer>
where
    Renderer: AdvancedRenderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Last>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Last::default())
    }

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
        renderer: &Renderer,
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
        renderer: &Renderer,
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
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let last = tree.state.downcast_mut::<Last>();
        let bounds = layout.bounds();
        if matches!(
            event,
            iced::Event::Window(iced::window::Event::RedrawRequested(_))
        ) && last.0 != Some(bounds)
        {
            last.0 = Some(bounds);
            shell.publish(Message::Filter(Event::ButtonBounds(bounds)));
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
        renderer: &mut Renderer,
        theme: &Theme,
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
        renderer: &Renderer,
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
pub(crate) mod tests;
