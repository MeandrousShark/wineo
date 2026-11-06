use iced::advanced::widget::operation::Focusable;
use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer, widget::tree,
};
use iced::{Element, Event, Length, Padding, Rectangle, Size, Vector};

mod rows;

#[cfg(test)]
pub(crate) use rows::tests::{focusables, landed, press, press_key, run_task, walk_rows};
pub(crate) use rows::{ListRows, land, reveal, rows};

pub(crate) const UNDO_ID: &str = "trash-undo";

pub(crate) fn undo<'a>(
    content: Element<'a, crate::Message>,
    enabled: bool,
) -> Element<'a, crate::Message> {
    control(content, enabled, UNDO_ID, crate::Message::Undo)
}

pub(crate) fn control<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    enabled: bool,
    id: impl Into<iced::widget::Id>,
    on_press: M,
) -> Element<'a, M> {
    Element::new(FocusControl {
        content,
        enabled,
        id: id.into(),
        on_press,
        mouse_focus: true,
        leaves_submit: false,
        exclude: None,
        menu: None,
        ring: Padding::ZERO,
    })
}

/// A control that also takes X while focused, sending `on_exclude`, for a
/// row that can be excluded as well as chosen.
pub(crate) fn excludable<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    id: impl Into<iced::widget::Id>,
    on_press: M,
    on_exclude: M,
) -> Element<'a, M> {
    Element::new(FocusControl {
        content,
        enabled: true,
        id: id.into(),
        on_press,
        mouse_focus: true,
        leaves_submit: false,
        exclude: Some(on_exclude),
        menu: None,
        ring: Padding::ZERO,
    })
}

/// A control that also opens a menu: a right-click on it, or Shift+F10 or
/// the Menu key while it has focus, sends `on_menu`. The right-click works
/// only while the control is enabled, since a disabled one has no place to
/// open the menu from.
pub(crate) fn with_menu<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    enabled: bool,
    id: impl Into<iced::widget::Id>,
    on_press: M,
    on_menu: M,
) -> Element<'a, M> {
    Element::new(FocusControl {
        content,
        enabled,
        id: id.into(),
        on_press,
        mouse_focus: true,
        leaves_submit: false,
        exclude: None,
        menu: Some(on_menu),
        ring: Padding::ZERO,
    })
}

/// A row of a choice list, for `rows`. It leaves Ctrl+Enter alone, so the
/// flow's submit never also chooses the focused row.
pub(crate) fn row<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    enabled: bool,
    id: impl Into<iced::widget::Id>,
    on_press: M,
) -> Element<'a, M> {
    leaves_submit(content, enabled, id, on_press)
}

/// A button that answers a question inside a flow. Like a row it leaves
/// Ctrl+Enter alone, so submitting never also answers the question. This is
/// not the default for every control: an uncaptured Ctrl+Enter also reaches
/// the Library's Enter binding, which could start the selected game.
pub(crate) fn answer<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    enabled: bool,
    id: impl Into<iced::widget::Id>,
    on_press: M,
) -> Element<'a, M> {
    leaves_submit(content, enabled, id, on_press)
}

fn leaves_submit<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    enabled: bool,
    id: impl Into<iced::widget::Id>,
    on_press: M,
) -> Element<'a, M> {
    Element::new(FocusControl {
        content,
        enabled,
        id: id.into(),
        on_press,
        mouse_focus: true,
        leaves_submit: true,
        exclude: None,
        menu: None,
        ring: Padding::ZERO,
    })
}

/// A control that takes focus only from the keyboard, for a layer over the
/// page: its press never reaches the controls beneath, so taking focus on
/// one would leave theirs showing too.
pub(crate) fn keyboard_control<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    enabled: bool,
    id: impl Into<iced::widget::Id>,
    on_press: M,
) -> Element<'a, M> {
    Element::new(FocusControl {
        content,
        enabled,
        id: id.into(),
        on_press,
        mouse_focus: false,
        leaves_submit: false,
        exclude: None,
        menu: None,
        ring: Padding::ZERO,
    })
}

/// One side of a segment: it takes focus only from the keyboard, since a
/// click already moves the segment's thumb, and its ring reaches past its
/// own edges by `ring` so it outlines the whole segment, not one side.
/// Focused, it also takes the hover tint, since that ring can't say which
/// side has focus.
pub(crate) fn segment_side<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    id: impl Into<iced::widget::Id>,
    on_press: M,
    ring: Padding,
) -> Element<'a, M> {
    Element::new(FocusControl {
        content,
        enabled: true,
        id: id.into(),
        on_press,
        mouse_focus: false,
        leaves_submit: false,
        exclude: None,
        menu: None,
        ring,
    })
}

/// Where the focus ring sits around a control at `bounds`.
fn ring_bounds(bounds: Rectangle, ring: Padding) -> Rectangle {
    bounds.expand(2.0).expand(ring)
}

/// Asks whether the Undo control has focus. Unlike `is_focused`, which says
/// nothing when the id is not in the tree, this always answers, with false
/// for an Undo that is absent or disabled (a disabled one does not register).
pub(crate) fn undo_focused() -> impl iced::advanced::widget::Operation<bool> {
    UndoFocused(false)
}

struct UndoFocused(bool);

impl iced::advanced::widget::Operation<bool> for UndoFocused {
    fn traverse(
        &mut self,
        operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<bool>),
    ) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if id == Some(&iced::widget::Id::new(UNDO_ID)) && state.is_focused() {
            self.0 = true;
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<bool> {
        iced::advanced::widget::operation::Outcome::Some(self.0)
    }
}

pub(crate) fn cycle<S: AsRef<str>>(
    ids: &[S],
    reverse: bool,
) -> impl iced::advanced::widget::Operation + use<S> {
    let cycle = Cycle {
        ids: ids
            .iter()
            .map(|id| iced::widget::Id::from(id.as_ref().to_owned()))
            .collect(),
        focused: None,
        reverse,
    };
    #[cfg(test)]
    CYCLES.with_borrow_mut(|cycles| cycles.push(cycle.clone()));
    cycle
}

#[cfg(test)]
// Tests inspect emitted focus operations without starting a GUI runtime.
thread_local! {
    static CYCLES: std::cell::RefCell<Vec<Cycle>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn take_cycles() -> Vec<impl iced::advanced::widget::Operation> {
    CYCLES.with_borrow_mut(std::mem::take)
}

#[cfg_attr(test, derive(Clone))]
struct Cycle {
    ids: Vec<iced::widget::Id>,
    focused: Option<usize>,
    reverse: bool,
}

impl iced::advanced::widget::Operation for Cycle {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if state.is_focused() {
            self.focused = self.ids.iter().position(|target| Some(target) == id);
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<()> {
        use iced::advanced::widget::operation::{Outcome, focusable};
        if self.ids.is_empty() {
            return Outcome::None;
        }
        // A lone control toggles, so Tab can leave the toast's Undo and let
        // its timer run again.
        if self.ids.len() == 1 && self.focused.is_some() {
            return Outcome::Chain(Box::new(focusable::unfocus()));
        }
        let next = match (self.focused, self.reverse) {
            (Some(index), true) => (index + self.ids.len() - 1) % self.ids.len(),
            (Some(index), false) => (index + 1) % self.ids.len(),
            (None, true) => self.ids.len() - 1,
            (None, false) => 0,
        };
        Outcome::Chain(Box::new(focusable::focus(self.ids[next].clone())))
    }
}

struct FocusControl<'a, M, R: AdvancedRenderer = iced::Renderer> {
    content: Element<'a, M, iced::Theme, R>,
    enabled: bool,
    id: iced::widget::Id,
    on_press: M,
    mouse_focus: bool,
    leaves_submit: bool,
    /// Sent when X is pressed while this control has focus. The control
    /// owns the key, since a popup's own key handler can't tell which row
    /// is focused.
    exclude: Option<M>,
    /// Sent for the keys and click that open a context menu.
    menu: Option<M>,
    /// How far the focus ring reaches past this control's own edges, on top
    /// of the usual 2 px, so one part of a larger control can ring all of it.
    ring: Padding,
}

#[derive(Default)]
struct State {
    focused: bool,
}

impl iced::advanced::widget::operation::Focusable for State {
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

impl<M, R> Widget<M, iced::Theme, R> for FocusControl<'_, M, R>
where
    M: Clone,
    R: AdvancedRenderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<tree::Tree> {
        vec![tree::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut tree::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
        if !self.enabled {
            tree.state.downcast_mut::<State>().unfocus();
        }
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
        renderer: &R,
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
        renderer: &R,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        if self.enabled {
            operation.focusable(
                Some(&self.id),
                layout.bounds(),
                tree.state.downcast_mut::<State>(),
            );
        } else {
            tree.state.downcast_mut::<State>().unfocus();
        }

        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(iced::touch::Event::FingerPressed { .. }) => {
                let focused = self.enabled && self.mouse_focus && cursor.is_over(layout.bounds());
                if state.focused != focused {
                    state.focused = focused;
                    shell.request_redraw();
                }
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key:
                    key @ iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::Enter | iced::keyboard::key::Named::Space,
                    ),
                modifiers,
                repeat: false,
                ..
            }) if self.enabled
                && state.focused
                && !(self.leaves_submit
                    && modifiers.control()
                    && *key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)) =>
            {
                shell.publish(self.on_press.clone());
                shell.capture_event();
                return;
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if self.enabled && cursor.is_over(layout.bounds()) =>
            {
                if let Some(menu) = &self.menu {
                    shell.publish(menu.clone());
                    shell.capture_event();
                    return;
                }
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(named),
                modifiers,
                repeat: false,
                ..
            }) if self.enabled
                && state.focused
                && !(modifiers.control() || modifiers.alt() || modifiers.logo())
                && match named {
                    iced::keyboard::key::Named::ContextMenu => true,
                    iced::keyboard::key::Named::F10 => modifiers.shift(),
                    _ => false,
                } =>
            {
                if let Some(menu) = &self.menu {
                    shell.publish(menu.clone());
                    shell.capture_event();
                    return;
                }
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Character(character),
                modifiers,
                repeat: false,
                ..
            }) if self.enabled
                && state.focused
                && character.eq_ignore_ascii_case("x")
                && !(modifiers.control() || modifiers.alt() || modifiers.logo()) =>
            {
                if let Some(exclude) = &self.exclude {
                    shell.publish(exclude.clone());
                    shell.capture_event();
                    return;
                }
            }
            _ => {}
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
        renderer: &mut R,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let focused = self.enabled && tree.state.downcast_ref::<State>().focused;
        // A ring around a whole segment can't say which side has focus, so
        // that side takes its hover tint as well. Under the pointer the
        // hover already draws it, and two would show darker.
        if focused && self.ring != Padding::ZERO && !cursor.is_over(layout.bounds()) {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    border: iced::border::rounded(crate::ui::CONTROL_RADIUS),
                    shadow: iced::Shadow::default(),
                    snap: false,
                },
                theme.palette().text.scale_alpha(0.08),
            );
        }
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        if focused {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: ring_bounds(layout.bounds(), self.ring),
                    border: iced::border::rounded(crate::ui::CONTROL_RADIUS + 2.0)
                        .width(2)
                        .color(theme.palette().primary),
                    shadow: iced::Shadow::default(),
                    snap: false,
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
        viewport: &Rectangle,
        renderer: &R,
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
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, M, iced::Theme, R>> {
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
mod tests {
    use super::*;
    use iced::keyboard::{self, Key, key::Named};

    /// Runs the real query the way the runtime does and returns every
    /// answer it gave.
    fn run_undo_focused(enabled: bool, focus: bool) -> Vec<bool> {
        let button = iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("Undo");
        let mut widget: Element<'_, crate::Message, iced::Theme, ()> = Element::new(FocusControl {
            content: button.into(),
            enabled,
            id: UNDO_ID.into(),
            on_press: crate::Message::Undo,
            mouse_focus: true,
            leaves_submit: false,
            exclude: None,
            menu: None,
            ring: Padding::ZERO,
        });
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(100.0, 32.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        if focus {
            tree.state.downcast_mut::<State>().focused = true;
        }
        let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = answers.clone();
        let mut operation = iced::advanced::widget::operation::map(undo_focused(), move |value| {
            sink.lock().unwrap().push(value);
        });
        widget
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &(), &mut operation);
        let _ = iced::advanced::widget::Operation::finish(&operation);
        answers.lock().unwrap().to_vec()
    }

    #[test]
    fn the_undo_focus_query_answers_for_focused_unfocused_and_disabled() {
        assert_eq!(run_undo_focused(true, true), [true]);
        assert_eq!(run_undo_focused(true, false), [false]);
        // A disabled control does not register, and still gets an answer.
        assert_eq!(run_undo_focused(false, true), [false]);
    }

    #[test]
    fn focus_operation_registers_the_undo_id_and_enter_activates_it() {
        let button = iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("Undo")
            .on_press(crate::Message::Undo);
        let mut widget: Element<'_, crate::Message, iced::Theme, ()> = Element::new(FocusControl {
            content: button.into(),
            enabled: true,
            id: UNDO_ID.into(),
            on_press: crate::Message::Undo,
            mouse_focus: true,
            leaves_submit: false,
            exclude: None,
            menu: None,
            ring: Padding::ZERO,
        });
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(100.0, 32.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let mut operation =
            iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(UNDO_ID));
        widget
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &(), &mut operation);

        let event = key_event(Named::Enter);
        let mut messages = Vec::new();
        let status = {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            shell.event_status()
        };

        assert!(matches!(messages.as_slice(), [crate::Message::Undo]));
        assert_eq!(status, iced::event::Status::Captured);
    }

    #[test]
    fn focused_space_activates_but_unfocused_space_does_not() {
        let button = iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("Undo")
            .on_press(crate::Message::Undo);
        let mut widget: Element<'_, crate::Message, iced::Theme, ()> = Element::new(FocusControl {
            content: button.into(),
            enabled: true,
            id: UNDO_ID.into(),
            on_press: crate::Message::Undo,
            mouse_focus: true,
            leaves_submit: false,
            exclude: None,
            menu: None,
            ring: Padding::ZERO,
        });
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(100.0, 32.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let mut messages = Vec::new();
        {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                &key_event(Named::Space),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
        }
        assert!(messages.is_empty());

        tree.state.downcast_mut::<State>().focus();
        {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                &key_event(Named::Space),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
        }

        assert!(matches!(messages.as_slice(), [crate::Message::Undo]));
    }

    #[test]
    fn tab_cycles_only_dialog_controls_and_can_reach_undo() {
        use iced::advanced::widget::{Operation, operation::Outcome};
        let ids = [
            crate::SEARCH_ID,
            "remove-prefix",
            "remove-cancel",
            "remove-confirm",
            UNDO_ID,
        ];
        let mut states = ids.map(|_| State::default());
        states[0].focus();
        for (reverse, expected) in [
            (false, 1),
            (false, 2),
            (false, 3),
            (false, 1),
            (true, 3),
            (true, 2),
            (true, 1),
        ] {
            let mut operation = cycle(
                &["remove-prefix", "remove-cancel", "remove-confirm"],
                reverse,
            );
            for (id, state) in ids.iter().zip(&mut states) {
                operation.focusable(
                    Some(&iced::widget::Id::new(id)),
                    Rectangle::default(),
                    state,
                );
            }
            let Outcome::Chain(mut focus) = operation.finish() else {
                panic!("missing focus operation")
            };
            for (id, state) in ids.iter().zip(&mut states) {
                focus.focusable(
                    Some(&iced::widget::Id::new(id)),
                    Rectangle::default(),
                    state,
                );
            }
            assert_eq!(
                states.iter().position(Focusable::is_focused),
                Some(expected)
            );
        }
        let operation = cycle(&[UNDO_ID], false);
        let Outcome::Chain(mut focus) = operation.finish() else {
            panic!("missing Undo focus")
        };
        for (id, state) in ids.iter().zip(&mut states) {
            focus.focusable(
                Some(&iced::widget::Id::new(id)),
                Rectangle::default(),
                state,
            );
        }
        assert_eq!(states.iter().position(Focusable::is_focused), Some(4));
        let mut operation = cycle(&[UNDO_ID], false);
        for (id, state) in ids.iter().zip(&mut states) {
            operation.focusable(
                Some(&iced::widget::Id::new(id)),
                Rectangle::default(),
                state,
            );
        }
        let Outcome::Chain(mut unfocus) = operation.finish() else {
            panic!("missing Undo unfocus")
        };
        for (id, state) in ids.iter().zip(&mut states) {
            unfocus.focusable(
                Some(&iced::widget::Id::new(id)),
                Rectangle::default(),
                state,
            );
        }
        assert_eq!(states.iter().position(Focusable::is_focused), None);
    }

    #[test]
    fn dialog_controls_activate_their_own_messages_with_enter_and_space() {
        for (id, on_press) in [
            ("remove-prefix", crate::Message::RemovePrefix(true)),
            ("remove-cancel", crate::Message::CancelRemove),
            ("remove-confirm", crate::Message::ConfirmRemove),
            (
                "trash-cancel",
                crate::Message::Trash(crate::trash_page::Event::Dismiss),
            ),
            (
                "trash-confirm",
                crate::Message::Trash(crate::trash_page::Event::Confirm),
            ),
        ] {
            for key in [Named::Enter, Named::Space] {
                let button =
                    iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("control");
                let mut widget: Element<'_, crate::Message, iced::Theme, ()> =
                    Element::new(FocusControl {
                        content: button.into(),
                        enabled: true,
                        id: id.into(),
                        on_press: on_press.clone(),
                        mouse_focus: true,
                        leaves_submit: false,
                        exclude: None,
                        menu: None,
                        ring: Padding::ZERO,
                    });
                let mut tree = tree::Tree::new(widget.as_widget());
                tree.state.downcast_mut::<State>().focus();
                let bounds = Size::new(100.0, 32.0);
                let node = widget.as_widget_mut().layout(
                    &mut tree,
                    &(),
                    &layout::Limits::new(Size::ZERO, bounds),
                );
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                widget.as_widget_mut().update(
                    &mut tree,
                    &key_event(key),
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &(),
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                assert_eq!(shell.event_status(), iced::event::Status::Captured);
                assert_eq!(format!("{messages:?}"), format!("[{on_press:?}]"));
            }
        }
    }

    fn key_event(key: Named) -> Event {
        let key = Key::Named(key);
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }

    #[test]
    fn either_side_of_a_segment_rings_the_whole_segment() {
        let list = Rectangle::new(iced::Point::new(10.0, 4.0), Size::new(32.0, 32.0));
        let grid = Rectangle::new(iced::Point::new(42.0, 4.0), Size::new(32.0, 32.0));
        let from_list = ring_bounds(
            list,
            Padding {
                right: 32.0,
                ..Padding::ZERO
            },
        );
        let from_grid = ring_bounds(
            grid,
            Padding {
                left: 32.0,
                ..Padding::ZERO
            },
        );
        assert_eq!(from_list, from_grid);
        assert_eq!(
            from_list,
            Rectangle::new(iced::Point::new(8.0, 2.0), Size::new(68.0, 36.0))
        );
        assert_eq!(ring_bounds(list, Padding::ZERO), list.expand(2.0));
    }

    #[test]
    fn a_click_presses_a_segment_side_but_never_focuses_it() {
        let button = iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("List")
            .on_press(crate::Message::Undo);
        let mut widget: Element<'_, crate::Message, iced::Theme, ()> = Element::new(FocusControl {
            content: button.into(),
            enabled: true,
            id: "segment-side".into(),
            on_press: crate::Message::Undo,
            mouse_focus: false,
            leaves_submit: false,
            exclude: None,
            menu: None,
            ring: Padding {
                right: 32.0,
                ..Padding::ZERO
            },
        });
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(32.0, 32.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let mut operation = iced::advanced::widget::operation::focusable::focus(
            iced::widget::Id::new("segment-side"),
        );
        widget
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &(), &mut operation);
        assert!(tree.state.downcast_ref::<State>().focused, "Tab focuses it");

        // Text has no size without a real renderer, so aim at the laid-out
        // control rather than a fixed point.
        let over = mouse::Cursor::Available(Layout::new(&node).bounds().center());
        let send = |widget: &mut Element<'_, crate::Message, iced::Theme, ()>,
                    tree: &mut tree::Tree,
                    event: Event| {
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                tree,
                &event,
                Layout::new(&node),
                over,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            messages
        };
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        for _ in 0..2 {
            let mut messages = send(&mut widget, &mut tree, press.clone());
            assert!(!tree.state.downcast_ref::<State>().focused);
            messages.extend(send(&mut widget, &mut tree, release.clone()));
            assert!(
                matches!(messages.as_slice(), [crate::Message::Undo]),
                "the click still reaches the side"
            );
        }

        // Tab back to it, and Enter presses it.
        widget
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &(), &mut operation);
        assert!(tree.state.downcast_ref::<State>().focused);
        let messages = send(&mut widget, &mut tree, key_event(Named::Enter));
        assert!(matches!(messages.as_slice(), [crate::Message::Undo]));
    }
}
