//! Read-only, selectable log text built on iced's `text_editor`.
//!
//! The editor owns its scroll position and selection, and neither can be
//! restored after `Content::with_text`. So live output is pasted at the end
//! only while the reader is at the tail with nothing selected. Otherwise the
//! content is left untouched (no yanked scroll, no lost selection) and
//! rebuilt from the caller's source text once the reader is back at the tail.

use std::borrow::Cow;
use std::sync::Arc;

use iced::widget::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, Motion, Position,
};
use iced::widget::{Row, button, row, text_editor as editor_widget};
use iced::{Background, Border, Color, Element, Font, Length};

use crate::theme::Palette;

#[derive(Default)]
pub(crate) struct LogView {
    content: Content,
    /// Net lines scrolled above the tail. The editor does not expose its
    /// scroll offset, so this is tracked from the wheel actions it emits.
    away: i32,
    /// The caret was moved by keyboard, so the tail position is unknown and
    /// only an explicit jump to the end resumes following.
    lost: bool,
    /// Output arrived while paused and is not in `content` yet.
    stale: bool,
    copied: bool,
}

/// Carriage returns would split lines differently in the editor than in the
/// caller's source text, which breaks the front trimming below.
pub(crate) fn normalize(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace('\r', ""))
    } else {
        Cow::Borrowed(text)
    }
}

impl LogView {
    pub(crate) fn content(&self) -> &Content {
        &self.content
    }

    /// Whether live output is being held back. The selection check uses the
    /// cursor rather than `Content::selection`, which copies the whole
    /// selected text and would run on every frame.
    pub(crate) fn paused(&self) -> bool {
        self.lost || self.away > 0 || self.content.cursor().selection.is_some()
    }

    /// Whether there is held output the reader could jump to.
    pub(crate) fn can_follow(&self) -> bool {
        self.stale && self.paused()
    }

    pub(crate) fn copied(&self) -> bool {
        self.copied
    }

    pub(crate) fn mark_copied(&mut self) {
        self.copied = true;
    }

    /// Adds `added` at the end and removes `dropped` from the front, mirroring
    /// what the caller just did to its source text (already normalized).
    pub(crate) fn push(&mut self, added: &str, dropped: &str, source: impl FnOnce() -> String) {
        self.copied = false;
        if self.paused() {
            self.stale = true;
            return;
        }
        self.content.perform(Action::Move(Motion::DocumentEnd));
        self.content
            .perform(Action::Edit(Edit::Paste(Arc::new(added.to_owned()))));
        if !self.trim_front(dropped) {
            self.rebuild(&source());
        }
    }

    /// Replaces the content after the source changed in a way `push` cannot
    /// mirror. Only meaningful while following.
    pub(crate) fn resync(&mut self, source: &str) {
        self.copied = false;
        if self.paused() {
            self.stale = true;
        } else {
            self.rebuild(source);
        }
    }

    /// Applies a widget action. The log is read-only, so edits are dropped.
    pub(crate) fn perform(&mut self, action: Action, source: impl FnOnce() -> String) {
        if action.is_edit() {
            return;
        }
        self.copied = false;
        match &action {
            // The editor reports wheel-up as negative lines.
            Action::Scroll { lines } => self.away = (self.away - lines).max(0),
            Action::Move(Motion::DocumentEnd) => {
                self.away = 0;
                self.lost = false;
            }
            Action::Move(_) => self.lost = true,
            _ => {}
        }
        self.content.perform(action);
        if self.stale && !self.paused() {
            self.rebuild(&source());
        }
    }

    pub(crate) fn follow(&mut self, source: impl FnOnce() -> String) {
        self.perform(Action::Move(Motion::DocumentEnd), source);
    }

    fn rebuild(&mut self, source: &str) {
        self.content = Content::with_text(source);
        self.content.perform(Action::Move(Motion::DocumentEnd));
        self.away = 0;
        self.lost = false;
        self.stale = false;
    }

    /// Deletes the leading `dropped` text. Returns false if the content does
    /// not have that text shape, so the caller can rebuild instead.
    fn trim_front(&mut self, dropped: &str) -> bool {
        if dropped.is_empty() {
            return true;
        }
        let line = dropped.matches('\n').count();
        let column = dropped.len() - dropped.rfind('\n').map_or(0, |index| index + 1);
        let fits = self
            .content
            .line(line)
            .is_some_and(|text| text.text.len() >= column);
        if !fits {
            return false;
        }
        self.content.move_to(Cursor {
            position: Position { line: 0, column: 0 },
            selection: Some(Position { line, column }),
        });
        self.content.perform(Action::Edit(Edit::Delete));
        self.content.perform(Action::Move(Motion::DocumentEnd));
        true
    }
}

/// The log text box. The style keeps the panel look the old plain text had,
/// with a border so a focused log is visible.
pub(crate) fn editor<'a, Message: Clone + 'a>(
    view: &'a LogView,
    palette: Palette,
    height: impl Into<Length>,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    editor_widget(view.content())
        .on_action(on_action)
        .key_binding(key_binding)
        .font(Font::MONOSPACE)
        .size(12)
        .padding(8)
        .height(height)
        .style(move |theme, status| {
            let base = text_editor::default(theme, status);
            text_editor::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border {
                    color: if matches!(status, text_editor::Status::Focused { .. }) {
                        palette.primary
                    } else {
                        palette.border
                    },
                    ..base.border
                },
                value: palette.foreground,
                selection: Color {
                    a: 0.4,
                    ..palette.primary
                },
                ..base
            }
        })
        .into()
}

/// Copy, plus a jump to the end while live output is being held back.
pub(crate) fn controls<'a, Message: Clone + 'a>(
    view: &LogView,
    has_text: bool,
    copy: Message,
    follow: Message,
) -> Row<'a, Message> {
    let mut controls = row![
        button(if view.copied() { "Copied" } else { "Copy" })
            .on_press_maybe(has_text.then_some(copy))
            .padding([6, 12])
    ]
    .spacing(8);
    if view.can_follow() {
        controls = controls.push(button("Jump to end").on_press(follow).padding([6, 12]));
    }
    controls
}

/// Key bindings for a read-only log: motion, selection, and copy work, while
/// every editing key is swallowed. Swallowing matters because an ignored
/// character would reach the app shortcuts (for example "/" focuses search).
pub(crate) fn key_binding<Message>(press: KeyPress) -> Option<Binding<Message>> {
    let command = press.modifiers.command();
    let binding = Binding::from_key_press(press)?;
    Some(match binding {
        // Control shortcuts the app owns (Ctrl+F, Ctrl+N) must pass through.
        Binding::Insert(_) if command => return None,
        Binding::Cut => Binding::Copy,
        Binding::Paste
        | Binding::Insert(_)
        | Binding::Enter
        | Binding::Backspace
        | Binding::Delete => Binding::Sequence(Vec::new()),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{self, Modifiers, key};
    use iced::widget::text_editor::Status;

    use super::*;

    fn text(view: &LogView) -> String {
        view.content.text()
    }

    fn numbered(count: usize) -> String {
        (0..count).map(|index| format!("line {index}\n")).collect()
    }

    fn press(key: keyboard::Key, modifiers: Modifiers, text: Option<&str>) -> KeyPress {
        KeyPress {
            modified_key: key.clone(),
            key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            modifiers,
            text: text.map(Into::into),
            status: Status::Focused { is_hovered: true },
        }
    }

    #[test]
    fn appends_follow_the_source_while_at_the_tail() {
        let mut view = LogView::default();
        let mut source = String::new();
        for chunk in ["one\ntw", "o\nthree\n", "four"] {
            source.push_str(chunk);
            view.push(chunk, "", || source.clone());
            assert_eq!(text(&view), source);
        }
        assert!(!view.paused());
    }

    #[test]
    fn selection_pauses_updates_and_keeps_the_selection() {
        let mut view = LogView::default();
        let mut source = "one\ntwo\n".to_owned();
        view.push(&source, "", || unreachable!());
        view.perform(Action::SelectAll, || unreachable!());
        assert!(view.paused());
        assert_eq!(view.content.selection().as_deref(), Some("one\ntwo\n"));

        source.push_str("three\n");
        view.push("three\n", "", || unreachable!());
        assert_eq!(text(&view), "one\ntwo\n");
        assert_eq!(view.content.selection().as_deref(), Some("one\ntwo\n"));
        assert!(view.can_follow());
    }

    #[test]
    fn jumping_to_the_end_resumes_with_everything() {
        let mut view = LogView::default();
        let mut source = "one\n".to_owned();
        view.push(&source, "", || unreachable!());
        view.perform(Action::SelectAll, || unreachable!());
        source.push_str("two\nthree");
        view.push("two\nthree", "", || unreachable!());
        view.follow(|| source.clone());
        assert_eq!(text(&view), source);
        assert!(!view.paused());
        assert!(!view.can_follow());
    }

    #[test]
    fn scrolling_up_pauses_until_scrolled_back_down() {
        let mut view = LogView::default();
        let mut source = numbered(50);
        view.push(&source, "", || unreachable!());
        view.perform(Action::Scroll { lines: -8 }, || unreachable!());
        assert!(view.paused());
        source.push_str("late\n");
        view.push("late\n", "", || unreachable!());
        assert!(!text(&view).contains("late"));

        view.perform(Action::Scroll { lines: 5 }, || source.clone());
        assert!(view.paused());
        view.perform(Action::Scroll { lines: 5 }, || source.clone());
        assert!(!view.paused());
        assert_eq!(text(&view), source);
    }

    #[test]
    fn keyboard_motion_pauses_until_the_end_key() {
        let mut view = LogView::default();
        let mut source = numbered(5);
        view.push(&source, "", || unreachable!());
        view.perform(Action::Move(Motion::Up), || unreachable!());
        assert!(view.paused());
        source.push_str("more\n");
        view.push("more\n", "", || unreachable!());
        assert!(!text(&view).contains("more"));
        view.perform(Action::Move(Motion::DocumentEnd), || source.clone());
        assert_eq!(text(&view), source);
    }

    #[test]
    fn edits_are_rejected() {
        let mut view = LogView::default();
        view.push("keep\n", "", || unreachable!());
        for edit in [
            Edit::Insert('x'),
            Edit::Paste(Arc::new("pasted".to_owned())),
            Edit::Enter,
            Edit::Backspace,
            Edit::Delete,
        ] {
            view.perform(Action::Edit(edit), || unreachable!());
        }
        assert_eq!(text(&view), "keep\n");
        assert!(!view.paused());
    }

    #[test]
    fn front_trimming_mirrors_the_source() {
        let mut view = LogView::default();
        let mut source = numbered(20);
        view.push(&source, "", || unreachable!());
        // Whole lines, then a cut in the middle of a line.
        for cut in [8, 17] {
            let dropped: String = source.drain(..cut).collect();
            view.push("", &dropped, || unreachable!());
            assert_eq!(text(&view), source);
        }
        assert!(!view.paused());
    }

    #[test]
    fn mismatched_trim_rebuilds_from_the_source() {
        let mut view = LogView::default();
        view.push("a\n", "", || unreachable!());
        view.push("b\n", "x\ny\nz\nw\n", || "rebuilt".to_owned());
        assert_eq!(text(&view), "rebuilt");
    }

    #[test]
    fn carriage_returns_are_removed() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nbc");
        assert!(matches!(normalize("plain\n"), Cow::Borrowed(_)));
    }

    #[test]
    fn editing_keys_are_swallowed_and_copy_kept() {
        let binding = |key, modifiers, text| key_binding::<()>(press(key, modifiers, text));
        let character = |value: &str| keyboard::Key::Character(value.into());

        assert_eq!(
            binding(character("c"), Modifiers::COMMAND, Some("c")),
            Some(Binding::Copy)
        );
        assert_eq!(
            binding(character("a"), Modifiers::COMMAND, Some("a")),
            Some(Binding::SelectAll)
        );
        assert_eq!(
            binding(character("x"), Modifiers::COMMAND, Some("x")),
            Some(Binding::Copy)
        );
        for (key, text) in [
            (character("/"), Some("/")),
            (keyboard::Key::Named(key::Named::Enter), None),
            (keyboard::Key::Named(key::Named::Backspace), None),
            (keyboard::Key::Named(key::Named::Delete), None),
        ] {
            assert_eq!(
                binding(key, Modifiers::empty(), text),
                Some(Binding::Sequence(Vec::new()))
            );
        }
        assert_eq!(
            binding(character("v"), Modifiers::COMMAND, Some("v")),
            Some(Binding::Sequence(Vec::new()))
        );
        assert_eq!(binding(character("f"), Modifiers::COMMAND, Some("f")), None);
        assert_eq!(
            binding(
                keyboard::Key::Named(key::Named::Escape),
                Modifiers::empty(),
                None
            ),
            Some(Binding::Unfocus)
        );
    }
}
