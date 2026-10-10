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
use iced::widget::{Id, Row, row, scrollable, text_editor as editor_widget};
use iced::{Background, Border, Color, Element, Font, Length, Task};

use crate::theme::Palette;
use crate::ui::secondary_button;
use crate::{WHEEL_STEP_SCALE, wheel};

pub(crate) struct LogView {
    content: Content,
    /// Whether the reader is at the bottom of the outer scrollable. It only
    /// flips on real scrolling (see `scrolled`), never on content growth.
    at_bottom: bool,
    last_offset: f32,
    /// Output arrived while paused and is not in `content` yet.
    stale: bool,
    copied: bool,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            content: Content::default(),
            at_bottom: true,
            last_offset: 0.0,
            stale: false,
            copied: false,
        }
    }
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

/// Prepares a completed hook tail for the error panel.
pub(crate) fn normalize_captured(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    let mut line_len = 0;
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    let _ = chars.next();
                    for code in chars.by_ref() {
                        if ('@'..='~').contains(&code) {
                            break;
                        }
                    }
                    continue;
                }
                Some(']') => {
                    let _ = chars.next();
                    while let Some(code) = chars.peek().copied() {
                        if code == '\n' || code == '\r' {
                            break;
                        }
                        let _ = chars.next();
                        if code == '\u{7}' {
                            break;
                        }
                        if code == '\u{1b}' && chars.peek() == Some(&'\\') {
                            let _ = chars.next();
                            break;
                        }
                    }
                    continue;
                }
                Some(code) if ('0'..='~').contains(code) => {
                    let _ = chars.next();
                    continue;
                }
                Some(code) if (' '..='/').contains(code) => {
                    let _ = chars.next();
                    while let Some(code) = chars.peek().copied() {
                        if (' '..='/').contains(&code) {
                            let _ = chars.next();
                        } else if ('0'..='~').contains(&code) {
                            let _ = chars.next();
                            break;
                        } else {
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                let _ = chars.next();
            }
            output.push('\n');
            line_len = 0;
            continue;
        }
        if ch == '\n' {
            output.push('\n');
            line_len = 0;
            continue;
        }
        let clean = if ch.is_control() && ch != '\t' {
            '\u{fffd}'
        } else {
            ch
        };
        if line_len < 200 {
            output.push(clean);
        } else if line_len == 200 {
            output.push('…');
        }
        if line_len <= 200 {
            line_len += 1;
        }
    }
    output
}

impl LogView {
    pub(crate) fn content(&self) -> &Content {
        &self.content
    }

    /// Whether live output is being held back. The selection check uses the
    /// cursor rather than `Content::selection`, which copies the whole
    /// selected text and would run on every frame.
    pub(crate) fn paused(&self) -> bool {
        !self.at_bottom || self.content.cursor().selection.is_some()
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
    /// Returns true when the content changed and the scrollable should snap
    /// to the end.
    #[must_use]
    pub(crate) fn push(
        &mut self,
        added: &str,
        dropped: &str,
        source: impl FnOnce() -> String,
    ) -> bool {
        self.copied = false;
        if self.paused() {
            self.stale = true;
            return false;
        }
        self.content.perform(Action::Move(Motion::DocumentEnd));
        self.content
            .perform(Action::Edit(Edit::Paste(Arc::new(added.to_owned()))));
        if !self.trim_front(dropped) {
            self.rebuild(&source());
        }
        true
    }

    /// Replaces the content after the source changed in a way `push` cannot
    /// mirror. Only takes effect while following.
    #[must_use]
    pub(crate) fn resync(&mut self, source: &str) -> bool {
        self.copied = false;
        if self.paused() {
            self.stale = true;
            false
        } else {
            self.rebuild(source);
            true
        }
    }

    /// Applies a widget action. The log is read-only, so edits are dropped,
    /// and wheel scrolling belongs to the outer scrollable. Returns true when
    /// the scrollable should snap to the end.
    #[must_use]
    pub(crate) fn perform(&mut self, action: Action, source: impl FnOnce() -> String) -> bool {
        if action.is_edit() || matches!(action, Action::Scroll { .. }) {
            return false;
        }
        self.copied = false;
        let jump = matches!(action, Action::Move(Motion::DocumentEnd));
        self.content.perform(action);
        if jump {
            self.at_bottom = true;
        }
        self.resume_if_ready(jump, source)
    }

    #[must_use]
    pub(crate) fn follow(&mut self, source: impl FnOnce() -> String) -> bool {
        self.perform(Action::Move(Motion::DocumentEnd), source)
    }

    /// Tracks the scrollable's viewport. Scrolling up leaves the tail; only
    /// reaching the bottom (or a jump) returns to it. Growth alone does not
    /// count: right after an append the content is taller than the offset
    /// until the scrollable is snapped, and that must not read as scrolling
    /// away. Returns true when the reader is back at the tail with held
    /// output.
    #[must_use]
    pub(crate) fn scrolled(
        &mut self,
        offset: f32,
        height: f32,
        content_height: f32,
        source: impl FnOnce() -> String,
    ) -> bool {
        if offset + height >= content_height - 1.0 {
            self.at_bottom = true;
        } else if offset < self.last_offset - 0.5 {
            self.at_bottom = false;
        }
        self.last_offset = offset;
        self.resume_if_ready(false, source)
    }

    fn resume_if_ready(&mut self, force: bool, source: impl FnOnce() -> String) -> bool {
        if self.stale && !self.paused() {
            self.rebuild(&source());
            true
        } else {
            force
        }
    }

    fn rebuild(&mut self, source: &str) {
        self.content = Content::with_text(source);
        self.content.perform(Action::Move(Motion::DocumentEnd));
        self.at_bottom = true;
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

/// The log text box. The editor lays out its full content and an outer
/// scrollable provides the viewport and scrollbar. That also lets the wheel
/// through: with an unbounded height the editor neither emits nor captures
/// scroll actions, so the scrollable handles it.
pub(crate) fn editor<'a, Message: Clone + 'a>(
    view: &'a LogView,
    palette: Palette,
    height: impl Into<Length>,
    id: &'static str,
    on_action: impl Fn(Action) -> Message + 'a,
    on_scroll: impl Fn(scrollable::Viewport) -> Message + 'a,
) -> Element<'a, Message> {
    let editor = editor_widget(view.content())
        .on_action(on_action)
        .key_binding(key_binding)
        .font(Font::MONOSPACE)
        .size(12)
        .padding(8)
        .style(move |theme, status| {
            let base = text_editor::default(theme, status);
            text_editor::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default(),
                value: palette.foreground,
                selection: Color {
                    a: 0.4,
                    ..palette.primary
                },
                ..base
            }
        });
    wheel::scale(
        scrollable(editor)
            .id(Id::new(id))
            .height(height)
            .on_scroll(on_scroll),
        WHEEL_STEP_SCALE,
    )
}

pub(crate) fn snap_to_end<Message: 'static>(id: &'static str) -> Task<Message> {
    iced::widget::operation::snap_to_end(Id::new(id))
}

/// Copy, plus a jump to the end while live output is being held back.
pub(crate) fn controls<'a, Message: Clone + 'a>(
    view: &LogView,
    has_text: bool,
    copy: Message,
    follow: Message,
    palette: Palette,
) -> Row<'a, Message> {
    let mut controls = row![
        secondary_button(if view.copied() { "Copied" } else { "Copy" }, palette)
            .on_press_maybe(has_text.then_some(copy))
    ]
    .spacing(8);
    if view.can_follow() {
        controls = controls.push(secondary_button("Jump to end", palette).on_press(follow));
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
            assert!(view.push(chunk, "", || source.clone()));
            assert_eq!(text(&view), source);
        }
        assert!(!view.paused());
    }

    #[test]
    fn selection_pauses_updates_and_keeps_the_selection() {
        let mut view = LogView::default();
        assert!(view.push("one\ntwo\n", "", || unreachable!()));
        let _ = view.perform(Action::SelectAll, || unreachable!());
        assert!(view.paused());
        assert_eq!(view.content.selection().as_deref(), Some("one\ntwo\n"));

        assert!(!view.push("three\n", "", || unreachable!()));
        assert_eq!(text(&view), "one\ntwo\n");
        assert_eq!(view.content.selection().as_deref(), Some("one\ntwo\n"));
        assert!(view.can_follow());
    }

    #[test]
    fn jumping_to_the_end_resumes_with_everything() {
        let mut view = LogView::default();
        let mut source = "one\n".to_owned();
        assert!(view.push(&source, "", || unreachable!()));
        let _ = view.perform(Action::SelectAll, || unreachable!());
        source.push_str("two\nthree");
        assert!(!view.push("two\nthree", "", || unreachable!()));
        assert!(view.follow(|| source.clone()));
        assert_eq!(text(&view), source);
        assert!(!view.paused());
        assert!(!view.can_follow());
    }

    #[test]
    fn scrolling_up_pauses_and_reaching_the_bottom_resumes() {
        let mut view = LogView::default();
        let mut source = numbered(50);
        assert!(view.push(&source, "", || unreachable!()));
        // Viewport of 200 over 1000: first at the bottom, then scrolled up.
        assert!(!view.scrolled(800.0, 200.0, 1000.0, || unreachable!()));
        assert!(!view.scrolled(500.0, 200.0, 1000.0, || unreachable!()));
        assert!(view.paused());

        source.push_str("late\n");
        assert!(!view.push("late\n", "", || unreachable!()));
        assert!(!text(&view).contains("late"));
        assert!(view.can_follow());

        // Scrolling down without reaching the bottom stays paused.
        assert!(!view.scrolled(700.0, 200.0, 1000.0, || unreachable!()));
        assert!(view.paused());
        assert!(view.scrolled(800.0, 200.0, 1000.0, || source.clone()));
        assert!(!view.paused());
        assert_eq!(text(&view), source);
    }

    #[test]
    fn growth_after_an_append_does_not_read_as_scrolling_away() {
        let mut view = LogView::default();
        assert!(!view.scrolled(800.0, 200.0, 1000.0, || unreachable!()));
        // Same offset, taller content: the scrollable has not snapped yet.
        assert!(!view.scrolled(800.0, 200.0, 1040.0, || unreachable!()));
        assert!(!view.paused());
        assert!(view.push("more\n", "", || unreachable!()));
    }

    #[test]
    fn content_clamped_by_a_trim_stays_at_the_bottom() {
        let mut view = LogView::default();
        assert!(!view.scrolled(800.0, 200.0, 1000.0, || unreachable!()));
        // The offset shrinks only because the content did.
        assert!(!view.scrolled(400.0, 200.0, 600.0, || unreachable!()));
        assert!(!view.paused());
    }

    #[test]
    fn keyboard_motion_keeps_following_but_the_end_key_clears_selection() {
        let mut view = LogView::default();
        let mut source = numbered(5);
        assert!(view.push(&source, "", || unreachable!()));
        // Caret motion does not scroll the outer scrollable, so it is not a
        // reason to pause.
        assert!(!view.perform(Action::Move(Motion::Up), || unreachable!()));
        assert!(!view.paused());
        let _ = view.perform(Action::Select(Motion::Up), || unreachable!());
        assert!(view.paused());
        source.push_str("more\n");
        assert!(!view.push("more\n", "", || unreachable!()));
        assert!(view.perform(Action::Move(Motion::DocumentEnd), || source.clone()));
        assert_eq!(text(&view), source);
    }

    #[test]
    fn wheel_scroll_actions_are_left_to_the_scrollable() {
        let mut view = LogView::default();
        assert!(view.push(&numbered(5), "", || unreachable!()));
        assert!(!view.perform(Action::Scroll { lines: -4 }, || unreachable!()));
        assert!(!view.paused());
    }

    #[test]
    fn edits_are_rejected() {
        let mut view = LogView::default();
        assert!(view.push("keep\n", "", || unreachable!()));
        for edit in [
            Edit::Insert('x'),
            Edit::Paste(Arc::new("pasted".to_owned())),
            Edit::Enter,
            Edit::Backspace,
            Edit::Delete,
        ] {
            assert!(!view.perform(Action::Edit(edit), || unreachable!()));
        }
        assert_eq!(text(&view), "keep\n");
        assert!(!view.paused());
    }

    #[test]
    fn front_trimming_mirrors_the_source() {
        let mut view = LogView::default();
        let mut source = numbered(20);
        assert!(view.push(&source, "", || unreachable!()));
        // Whole lines, then a cut in the middle of a line.
        for cut in [8, 17] {
            let dropped: String = source.drain(..cut).collect();
            assert!(view.push("", &dropped, || unreachable!()));
            assert_eq!(text(&view), source);
        }
        assert!(!view.paused());
    }

    #[test]
    fn mismatched_trim_rebuilds_from_the_source() {
        let mut view = LogView::default();
        let _ = view.push("a\n", "", || unreachable!());
        let _ = view.push("b\n", "x\ny\nz\nw\n", || "rebuilt".to_owned());
        assert_eq!(text(&view), "rebuilt");
    }

    #[test]
    fn carriage_returns_are_removed() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nbc");
        assert!(matches!(normalize("plain\n"), Cow::Borrowed(_)));
    }

    #[test]
    fn captured_output_strips_terminal_sequences_and_bounds_lines() {
        let text = "\u{1b}[31mred\u{1b}[0m\u{1b}(B\u{1b}7\u{1b}]0;title\u{7}\0\u{7}\rnext\r\nend\u{1b}]2;other\u{1b}\\\u{1b}]unterminated\nvisible";
        assert_eq!(normalize_captured(text), "red��\nnext\nend\nvisible");
        assert_eq!(
            normalize_captured(&"x".repeat(205)),
            format!("{}…", "x".repeat(200))
        );
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
