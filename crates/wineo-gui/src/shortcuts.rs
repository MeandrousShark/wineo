use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer, widget::tree,
};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Handles Ctrl+1/2/3/4/5, Ctrl+Comma, the active Undo shortcut, and the
/// zoom keys while they apply, before any child sees them.
/// A focused text_input inserts the digit and captures the key regardless of
/// Ctrl, and the app's key subscription skips captured events.
pub(crate) fn capture_with_undo<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_section: impl Fn(crate::sidebar::Section) -> Message + 'a,
    on_undo: Option<Message>,
    on_zoom: Option<fn(crate::zoom::Key) -> Message>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: AdvancedRenderer + 'a,
{
    Element::new(SidebarShortcuts {
        content: content.into(),
        on_section: Box::new(on_section),
        on_undo,
        on_zoom,
    })
}

struct SidebarShortcuts<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_section: Box<dyn Fn(crate::sidebar::Section) -> Message + 'a>,
    on_undo: Option<Message>,
    on_zoom: Option<fn(crate::zoom::Key) -> Message>,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for SidebarShortcuts<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: AdvancedRenderer,
{
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<tree::Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut tree::Tree) {
        self.content.as_widget().diff(tree);
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
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            && let Some(section) = crate::sidebar::shortcut(key, *modifiers)
        {
            shell.publish((self.on_section)(section));
            shell.capture_event();
            return;
        }

        if let Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            && *modifiers == iced::keyboard::Modifiers::CTRL
            && matches!(key.as_ref(), iced::keyboard::Key::Character("z" | "Z"))
            && let Some(message) = &self.on_undo
        {
            shell.publish(message.clone());
            shell.capture_event();
            return;
        }

        if let Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            && let Some(on_zoom) = self.on_zoom
            && let Some(zoom) = crate::zoom::key(key, *modifiers)
        {
            shell.publish(on_zoom(zoom));
            shell.capture_event();
            return;
        }
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut tree::Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{App, Message, sidebar::Section};
    use iced::event::Status;
    use iced::keyboard::{self, Key, Modifiers};
    use iced::widget::text_input;

    pub(crate) fn input_messages(
        value: &str,
        digit: &str,
        modifiers: Modifiers,
    ) -> (Vec<Message>, Status) {
        input_messages_with_undo(value, digit, modifiers, false)
    }

    fn input_messages_with_undo(
        value: &str,
        digit: &str,
        modifiers: Modifiers,
        undo: bool,
    ) -> (Vec<Message>, Status) {
        let input = text_input::TextInput::<Message, iced::Theme, ()>::new("Search", value)
            .on_input(Message::SearchChanged);
        let mut widget =
            capture_with_undo(input, Message::Section, undo.then_some(Message::Undo), None);
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(400.0, 40.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let state = tree.state.downcast_mut::<text_input::State<()>>();
        state.focus();
        state.move_cursor_to_end();
        let key = Key::Character(digit.into());
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: Some(digit.into()),
            repeat: false,
        });
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
        (messages, status)
    }

    pub(crate) fn press(app: &mut App, digit: &str, modifiers: Modifiers) {
        let (messages, status) = input_messages(&app.search, digit, modifiers);
        for message in messages {
            let _ = app.update(message);
        }
        let _ = app.update(Message::Key(
            Key::Character(digit.into()),
            modifiers,
            status,
        ));
    }

    fn fixture() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = wineo_core::paths::Paths::from_lookup(move |key| {
            (key == "HOME").then(|| home.as_os_str().to_owned())
        })
        .unwrap();
        (root, App::boot(paths).0)
    }

    #[test]
    fn control_z_reaches_active_undo_before_a_focused_input() {
        let (messages, status) = input_messages_with_undo("game", "z", Modifiers::CTRL, true);
        assert_eq!(status, Status::Captured);
        assert!(matches!(messages.as_slice(), [Message::Undo]));
        let (messages, _) = input_messages_with_undo("game", "z", Modifiers::CTRL, false);
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::Undo))
        );
    }

    #[test]
    fn control_digit_navigates_before_a_focused_search_can_insert_text() {
        let (_root, mut app) = fixture();
        app.search = "game".into();
        app.search_focused = true;
        let (messages, status) = input_messages(&app.search, "2", Modifiers::CTRL);
        assert_eq!(status, Status::Captured);
        assert!(matches!(
            messages.as_slice(),
            [Message::Section(Section::Trash)]
        ));
        press(&mut app, "2", Modifiers::CTRL);
        assert_eq!(app.section, Section::Trash);
        assert_eq!(app.search, "game");
    }

    #[test]
    fn control_comma_opens_settings_before_a_focused_input_but_extra_modifiers_type() {
        let (messages, status) = input_messages("game", ",", Modifiers::CTRL);
        assert_eq!(status, Status::Captured);
        assert!(matches!(
            messages.as_slice(),
            [Message::Section(Section::Settings)]
        ));
        for modifiers in [
            Modifiers::CTRL | Modifiers::SHIFT,
            Modifiers::CTRL | Modifiers::ALT,
        ] {
            let (messages, _) = input_messages("game", ",", modifiers);
            assert!(matches!(
                messages.as_slice(),
                [Message::SearchChanged(value)] if value == "game,"
            ));
        }
    }

    #[test]
    fn plain_digits_and_extra_modifiers_reach_the_focused_input() {
        for modifiers in [
            Modifiers::empty(),
            Modifiers::CTRL | Modifiers::SHIFT,
            Modifiers::CTRL | Modifiers::ALT,
            Modifiers::CTRL | Modifiers::LOGO,
        ] {
            let (messages, status) = input_messages("game", "2", modifiers);
            assert_eq!(status, Status::Captured);
            assert!(matches!(
                messages.as_slice(),
                [Message::SearchChanged(value)] if value == "game2"
            ));
            let (_root, mut app) = fixture();
            app.search = "game".into();
            press(&mut app, "2", modifiers);
            assert_eq!(app.section, Section::Library);
            assert_eq!(app.search, "game2");
        }
    }

    #[test]
    fn a_flow_swallows_control_digits_without_changing_the_input_or_page() {
        let (_root, mut app) = fixture();
        app.search = "game".into();
        app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
        for digit in ["1", "2", "3", "4"] {
            let (messages, status) = input_messages("dialog", digit, Modifiers::CTRL);
            assert_eq!(status, Status::Captured);
            assert!(matches!(messages.as_slice(), [Message::Section(_)]));
            press(&mut app, digit, Modifiers::CTRL);
            assert_eq!(app.section, Section::Library);
            assert_eq!(app.search, "game");
            assert!(matches!(app.flow, Some(crate::flow::Page::Choose(_))));
        }
    }
}
