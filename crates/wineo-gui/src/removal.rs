use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::{Space, checkbox, column, container, mouse_area, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
#[cfg(test)]
use wineo_core::runtime::State;
use wineo_core::trash::{Entry, RemoveEvent};

use crate::prefix_facts::{Facts, Live};
use crate::ui::{accent_base, secondary_button};
use crate::{App, Message, focus_button, remove_work, spinner};

const PREFIX_ID: &str = "remove-prefix";
const CANCEL_ID: &str = "remove-cancel";
const REMOVE_ID: &str = "remove-confirm";
const TOAST_WRAPPING: iced::widget::text::Wrapping = iced::widget::text::Wrapping::WordOrGlyph;

pub(crate) struct Dialog {
    pub(crate) name: String,
    pub(crate) prefix: Option<PathBuf>,
    pub(crate) include_prefix: bool,
    pub(crate) size: Option<Result<u64, String>>,
    pub(crate) measure: Token,
    pub(crate) error: Option<String>,
}

pub(crate) enum Change {
    Remove {
        name: String,
        token: Token,
        phase: Option<RemoveEvent>,
    },
    Restore {
        id: String,
        name: String,
    },
    Trash(crate::trash_page::Change),
    Downloads(crate::downloads_page::Change),
    Checkup(crate::checkup::Change),
}

impl Change {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Remove { name, .. } => format!("Busy: removing {name}"),
            Self::Restore { name, .. } => format!("Busy: restoring {name}"),
            Self::Trash(change) => change.reason(),
            Self::Downloads(change) => change.reason(),
            Self::Checkup(change) => format!("Busy: fixing {}", change.scope.label()),
        }
    }
}

pub(crate) struct Toast {
    pub(crate) id: String,
    /// Tells this toast from a later one, for answers still in flight.
    serial: u64,
    pub(crate) name: String,
    pub(crate) text: String,
    pub(crate) remaining: Duration,
    pub(crate) hovered: bool,
    pub(crate) focused: bool,
    /// The expiry wakeup fired and the focus answer that decides it is still
    /// on its way, so there is nothing to arm a timer for.
    checking: bool,
    last: Instant,
}

impl Toast {
    fn new(entry: Entry) -> Self {
        let mut text = format!("Removed {} to the trash.", entry.name);
        if entry.prefix_left.is_some() && entry.prefix_present == Some(true) {
            text.push_str(" Prefix kept.");
        }
        if entry.desktop_removed > 0 {
            text.push_str(&format!(
                " {} desktop entr{} removed.",
                entry.desktop_removed,
                if entry.desktop_removed == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }
        if let Some(snapshots) = &entry.snapshots_left
            && snapshots.is_dir()
            && std::fs::read_dir(snapshots).is_ok_and(|mut entries| entries.next().is_some())
        {
            text.push_str(" Snapshots kept.");
        }
        static NEXT_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            id: entry.id,
            serial: NEXT_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name: entry.name,
            text,
            remaining: Duration::from_secs(10),
            hovered: false,
            focused: false,
            checking: false,
            last: Instant::now(),
        }
    }

    /// When the toast expires, if its clock is running. Hover and focus
    /// pause it, and an expired one has nothing left to wait for.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        (!self.hovered && !self.focused && !self.checking && !self.remaining.is_zero())
            .then(|| self.last + self.remaining)
    }

    pub(crate) fn advance(&mut self, now: Instant) {
        if !self.hovered && !self.focused {
            self.remaining = self
                .remaining
                .saturating_sub(now.saturating_duration_since(self.last));
        }
        self.last = now.max(self.last);
    }
}

pub(crate) fn key(
    app: &mut App,
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    status: iced::event::Status,
) -> Option<Task<Message>> {
    use iced::keyboard::{Key, Modifiers, key::Named};
    if app.settings.is_some() || app.art_page.is_some() {
        return None;
    }
    if status != iced::event::Status::Captured
        && app.toast.is_some()
        && modifiers == Modifiers::CTRL
        && matches!(key.as_ref(), Key::Character("z" | "Z"))
    {
        return Some(app.undo_remove());
    }
    if *key == Key::Named(Named::Tab) && app.toast.is_some() {
        return Some(
            iced::advanced::widget::operate(focus_button::cycle(
                &[focus_button::UNDO_ID],
                modifiers.shift(),
            ))
            .discard(),
        );
    }
    if *key == Key::Named(Named::Delete) && modifiers.is_empty() {
        return Some(
            iced::advanced::widget::operate(RemoveFocus::default())
                .map(move |(search, any)| Message::RemoveFocus(status, search, any)),
        );
    }
    None
}

#[derive(Default)]
struct RemoveFocus {
    search: bool,
    any: bool,
}

impl iced::advanced::widget::Operation<(bool, bool)> for RemoveFocus {
    fn traverse(
        &mut self,
        operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<(bool, bool)>),
    ) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: iced::Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if state.is_focused() {
            self.any = true;
            self.search |= id == Some(&iced::widget::Id::new(crate::SEARCH_ID));
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<(bool, bool)> {
        iced::advanced::widget::operation::Outcome::Some((self.search, self.any))
    }
}

pub(crate) fn delete_key(app: &mut App, search: bool, captured: bool) -> Task<Message> {
    if !search
        && !captured
        && !app.panel_log_visible()
        && !app.checkup_visible()
        && app.action_menu.is_none()
    {
        app.selected
            .clone()
            .map_or_else(Task::none, |name| app.open_remove(name))
    } else {
        Task::none()
    }
}

pub(crate) fn dialog_key(
    app: &mut App,
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Task<Message> {
    use iced::keyboard::{Key, key::Named};
    match key {
        Key::Named(Named::Escape) => {
            app.cancel_remove_dialog();
            Task::none()
        }
        Key::Named(Named::Tab) => {
            let ids = app.remove_controls();
            iced::advanced::widget::operate(focus_button::cycle(&ids, modifiers.shift())).discard()
        }
        _ => Task::none(),
    }
}

pub(crate) fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::RemoveFocus(status, search, any) => {
            app.search_focused = search;
            if app.section == crate::sidebar::Section::Library
                && app.settings.is_none()
                && app.art_page.is_none()
                && app.flow.is_none()
                && app.removal.is_none()
                && app.hook_stop_confirm.is_none()
                && !app.close_prompt
            {
                delete_key(app, search, any || status == iced::event::Status::Captured)
            } else {
                Task::none()
            }
        }
        Message::Remove(name) => app.open_remove(name),
        Message::RemovePrefix(include) => {
            if app.change.is_none()
                && let Some(dialog) = &mut app.removal
            {
                dialog.include_prefix = include && dialog.prefix.is_some();
            }
            Task::none()
        }
        Message::RemoveSize(generation, size) => {
            if generation == app.remove_generation
                && app.change.is_none()
                && let Some(dialog) = &mut app.removal
            {
                dialog.size = Some(size);
            }
            Task::none()
        }
        Message::ConfirmRemove => app.confirm_remove(),
        Message::CancelRemove => {
            app.cancel_remove();
            Task::none()
        }
        Message::RemoveProgress(generation, event) => {
            if generation != app.change_generation {
                return Task::none();
            }
            if let Some(Change::Remove { phase, .. }) = &mut app.change {
                *phase = Some(event);
            }
            Task::none()
        }
        Message::RemoveFinished(generation, result) if generation == app.change_generation => {
            app.remove_finished(result)
        }
        Message::RemoveFinished(..) => Task::none(),
        Message::Undo => app.undo_remove(),
        Message::UndoFinished(generation, result) if generation == app.change_generation => {
            app.undo_finished(result)
        }
        Message::UndoFinished(..) => Task::none(),
        Message::ToastHover(hovered) => {
            if let Some(toast) = &mut app.toast {
                toast.advance(Instant::now());
                toast.hovered = hovered;
            }
            Task::none()
        }
        Message::ToastTick(generation) => {
            if app.toast_wake.fired(generation) {
                // Expiry is decided by the answer, not by the clock: Undo may
                // have gained focus since the last check, and that pauses it.
                if let Some(toast) = &mut app.toast {
                    toast.checking = true;
                }
                app.query_toast_focus(Instant::now())
            } else {
                Task::none()
            }
        }
        // An answer about a toast that has since been replaced is dropped.
        Message::ToastAnswer(serial, focused, now) => {
            if app
                .toast
                .as_ref()
                .is_some_and(|toast| toast.serial == serial)
            {
                update(app, Message::ToastFocus(focused, now))
            } else {
                Task::none()
            }
        }
        Message::ToastFocus(focused, now) => {
            if let Some(toast) = &mut app.toast {
                // Focus operations run before this message, so preserve the interval that gained focus.
                if focused {
                    toast.focused = true;
                }
                toast.advance(now);
                toast.focused = focused;
                toast.checking = false;
                if toast.remaining.is_zero() && app.change.is_none() {
                    app.toast = None;
                }
            }
            Task::none()
        }
        _ => Task::none(),
    }
}

impl App {
    /// Focus is only known by asking the widget tree, and nothing tells the
    /// toast when Undo gains or loses it, so it asks after the events that
    /// can move it: a click, a key coming up, and any change to the screen
    /// while Undo holds focus (it may have been disabled).
    pub(crate) fn toast_focus_may_have_changed(&self, message: &Message) -> bool {
        let Some(toast) = &self.toast else {
            return false;
        };
        match message {
            Message::Pressed | Message::FocusRecheck => true,
            Message::ToastTick(_)
            | Message::ToastHover(_)
            | Message::ToastFocus(..)
            | Message::ToastAnswer(..) => false,
            // Frame-rate and timer messages never move focus.
            Message::PanelTick(..)
            | Message::GridTick(..)
            | Message::CollapseTick(..)
            | Message::ScrollIdle(_) => false,
            _ => toast.focused,
        }
    }

    pub(crate) fn poll_toast_focus(&mut self) -> Task<Message> {
        let Some(toast) = &mut self.toast else {
            return Task::none();
        };
        let now = Instant::now();
        // Charge the time so far to the state the toast was last known to be
        // in, so a focus change found now only pauses from here on.
        if !toast.checking {
            toast.advance(now);
        }
        self.query_toast_focus(now)
    }

    fn query_toast_focus(&self, now: Instant) -> Task<Message> {
        let Some(toast) = &self.toast else {
            return Task::none();
        };
        let serial = toast.serial;
        iced::advanced::widget::operate(crate::focus_button::undo_focused())
            .map(move |focused| Message::ToastAnswer(serial, focused, now))
    }

    /// Drops a toast that ran out while a change blocked its removal, once
    /// the change is over.
    pub(crate) fn settle_toast(&mut self) {
        if self.change.is_none()
            && self
                .toast
                .as_ref()
                .is_some_and(|toast| toast.remaining.is_zero())
        {
            self.toast = None;
        }
    }

    /// The next time the toast or the feature notice needs the app to wake.
    pub(crate) fn next_toast_deadline(&self) -> Option<Instant> {
        let toast = self.toast.as_ref().and_then(Toast::deadline);
        let notice = self.feature_notice.as_ref().map(|(_, until)| *until);
        [toast, notice].into_iter().flatten().min()
    }

    pub(crate) fn remove_reason(&self, name: &str) -> Option<String> {
        self.remove_reason_with(&Live(&self.paths), name)
    }

    /// `view` passes the stored facts so a redraw never touches the disk.
    pub(crate) fn remove_reason_with(&self, facts: &dyn Facts, name: &str) -> Option<String> {
        if facts.restore_pending(name) {
            return Some("Roll back restore first.".into());
        }
        self.prefix_change_reason_with(facts, name)
    }

    pub(crate) fn prefix_change_reason(&self, name: &str) -> Option<String> {
        self.prefix_change_reason_with(&Live(&self.paths), name)
    }

    pub(crate) fn prefix_change_reason_with(
        &self,
        facts: &dyn Facts,
        name: &str,
    ) -> Option<String> {
        if self.filesystem_busy() {
            return Some("Busy: changing files.".into());
        }
        if let Some(reason) = self.tool_reason_with(facts, name) {
            return Some(reason);
        }
        if let Some(change) = &self.change {
            return Some(change.reason());
        }
        if let Some(other) = self.cards.iter().find(|card| {
            card.name != name
                && self.same_prefix_with(facts, name, &card.name)
                && (card.lost || card.unresolved || card.active() || card.busy)
        }) {
            return Some(if other.lost || other.unresolved {
                "Stop it first. Use Clean up in the panel.".into()
            } else {
                "Stop it first".into()
            });
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|operation| self.same_prefix_with(facts, name, &operation.name))
        {
            return Some("Stop it first".into());
        }
        let card = self.cards.iter().find(|card| card.name == name)?;
        if card.lost || card.unresolved {
            Some("Stop it first. Use Clean up in the panel.".into())
        } else if card.active()
            || card.busy
            || self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.name == name)
        {
            Some("Stop it first".into())
        } else {
            None
        }
    }

    pub(crate) fn open_remove(&mut self, name: String) -> Task<Message> {
        self.refresh_facts();
        if self.removal.is_some()
            || self.flow.is_some()
            || self.settings.is_some()
            || self.art_page.is_some()
            || self.close_prompt
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
            || self.management_busy()
            || self.remove_reason(&name).is_some()
        {
            return Task::none();
        }
        let Some(card) = self.cards.iter().find(|card| card.name == name) else {
            return Task::none();
        };
        let prefix = card.prefix.clone();
        self.action_menu = None;
        self.remove_generation = self.remove_generation.wrapping_add(1);
        let measure = Token::new();
        let task = prefix.clone().map_or_else(Task::none, |path| {
            remove_work::measure(self.remove_generation, path, measure.clone())
        });
        self.removal = Some(Dialog {
            name,
            prefix,
            include_prefix: false,
            size: None,
            measure,
            error: None,
        });
        Task::batch([
            task,
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
        ])
    }

    pub(crate) fn confirm_remove(&mut self) -> Task<Message> {
        let Some(dialog) = &self.removal else {
            return Task::none();
        };
        if self.management_busy() || self.remove_reason(&dialog.name).is_some() {
            return Task::none();
        }
        dialog.measure.cancel();
        let token = Token::new();
        let name = dialog.name.clone();
        let include_prefix = dialog.include_prefix && dialog.prefix.is_some();
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(Change::Remove {
            name: name.clone(),
            token: token.clone(),
            phase: None,
        });
        self.removal.as_mut().unwrap().error = None;
        remove_work::remove(
            self.change_generation,
            self.paths.clone(),
            name,
            include_prefix,
            token,
        )
    }

    pub(crate) fn cancel_management(&self) {
        self.cancel_checkup(false);
        if let Some(Change::Downloads(crate::downloads_page::Change::Download { token, .. })) =
            &self.change
        {
            token.cancel();
        }
        if let Some(Change::Remove {
            token,
            phase: None | Some(RemoveEvent::Waiting),
            ..
        }) = &self.change
        {
            token.cancel();
        }
    }

    pub(crate) fn cancel_remove_dialog(&mut self) {
        if self.change.is_none()
            && let Some(dialog) = self.removal.take()
        {
            dialog.measure.cancel();
        }
    }

    pub(crate) fn cancel_remove(&mut self) {
        if self.change.is_some() {
            self.cancel_management();
        } else {
            self.cancel_remove_dialog();
        }
    }

    pub(crate) fn reload_after_change(&mut self) -> Task<Message> {
        self.refreshing = true;
        self.trash_refreshing = true;
        // Always queue fresh reads, even if a periodic read was already in flight.
        Task::batch([self.reload_library(), self.reload_trash_page()])
    }

    pub(crate) fn reload_after_uses_change(&mut self) -> Task<Message> {
        Task::batch([self.reload_after_change(), self.reload_downloads_page()])
    }

    pub(crate) fn remove_finished(
        &mut self,
        result: Result<Entry, remove_work::RemoveError>,
    ) -> Task<Message> {
        if !matches!(self.change, Some(Change::Remove { .. })) {
            return Task::none();
        }
        self.change = None;
        match result {
            Ok(entry) => {
                self.cancel_remove_dialog();
                self.checkups
                    .remove(&crate::checkup::Scope::Environment(entry.name.clone()));
                if self.selected.as_deref() == Some(&entry.name) {
                    self.close_panel();
                    self.selected = None;
                }
                self.toast = Some(Toast::new(entry));
            }
            Err(remove_work::RemoveError::Canceled) => self.cancel_remove_dialog(),
            Err(remove_work::RemoveError::Failed(error)) => {
                if let Some(dialog) = &mut self.removal {
                    dialog.error = Some(error);
                    // Confirm canceled the measurement, so measure again for
                    // the dialog that stays open.
                    if dialog.size.is_none()
                        && let Some(path) = dialog.prefix.clone()
                    {
                        self.remove_generation = self.remove_generation.wrapping_add(1);
                        dialog.measure = Token::new();
                        let measure = remove_work::measure(
                            self.remove_generation,
                            path,
                            dialog.measure.clone(),
                        );
                        return Task::batch([measure, self.reload_after_uses_change()]);
                    }
                }
            }
        }
        self.reload_after_uses_change()
    }

    pub(crate) fn undo_remove(&mut self) -> Task<Message> {
        if self.management_busy()
            || self.removal.is_some()
            || self.flow.is_some()
            || self.settings.is_some()
            || self.art_page.is_some()
            || self.close_prompt
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
        {
            return Task::none();
        }
        let Some(toast) = &self.toast else {
            return Task::none();
        };
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(Change::Restore {
            id: toast.id.clone(),
            name: toast.name.clone(),
        });
        remove_work::restore(self.change_generation, self.paths.clone(), toast.id.clone())
    }

    pub(crate) fn undo_finished(&mut self, result: Result<(), String>) -> Task<Message> {
        if !matches!(self.change, Some(Change::Restore { .. })) {
            return Task::none();
        }
        let Some(Change::Restore { id, .. }) = self.change.take() else {
            return Task::none();
        };
        if let Some(toast) = &mut self.toast
            && toast.id == id
        {
            match result {
                Ok(()) => self.toast = None,
                Err(error) => {
                    toast.text = error;
                    toast.remaining = Duration::from_secs(10);
                    toast.last = Instant::now();
                }
            }
        }
        self.reload_after_uses_change()
    }

    fn remove_controls(&self) -> Vec<&'static str> {
        let mut ids = Vec::new();
        let Some(dialog) = &self.removal else {
            return ids;
        };
        if self.change.is_none() && dialog.prefix.is_some() {
            ids.push(PREFIX_ID);
        }
        if !matches!(
            self.change,
            Some(Change::Remove {
                phase: Some(RemoveEvent::Moving),
                ..
            })
        ) {
            ids.push(CANCEL_ID);
        }
        if self.change.is_none() && self.remove_reason(&dialog.name).is_none() {
            ids.push(REMOVE_ID);
        }
        ids
    }

    pub(crate) fn remove_view(&self) -> Element<'_, Message> {
        let dialog = self.removal.as_ref().unwrap();
        let palette = self.palette;
        let mut content = column![
            text(format!("Remove {}?", dialog.name)).size(22),
            text(
                "Its config, art, and playtime go to the trash.\nYou can restore them from Trash."
            )
        ]
        .spacing(16);
        if let Some(prefix) = &dialog.prefix {
            let size = match &dialog.size {
                None => "measuring...".into(),
                Some(Ok(bytes)) => crate::size::format_bytes(*bytes),
                Some(Err(error)) => format!("Size unavailable: {error}"),
            };
            content = content
                .push(focus_button::control(
                    checkbox(dialog.include_prefix)
                        .label("Also trash the prefix and snapshots")
                        .on_toggle_maybe(self.change.is_none().then_some(Message::RemovePrefix))
                        .into(),
                    self.change.is_none(),
                    PREFIX_ID,
                    Message::RemovePrefix(!dialog.include_prefix),
                ))
                .push(
                    text(format!(
                        "{} ({size})",
                        self.paths.shown(&prefix.to_string_lossy())
                    ))
                    .size(13)
                    .color(palette.muted),
                );
        }
        if let Some(error) = &dialog.error {
            content = content.push(text(error).color(palette.danger));
        }
        let moving = matches!(
            self.change,
            Some(Change::Remove {
                phase: Some(RemoveEvent::Moving),
                ..
            })
        );
        let running = self.change.is_some();
        if running {
            content = content.push(
                row![
                    spinner::spinner(palette, 16.0),
                    text(match &self.change {
                        Some(Change::Remove {
                            phase: Some(RemoveEvent::Moving),
                            ..
                        }) => "Moving to the trash...",
                        Some(Change::Remove {
                            phase: Some(RemoveEvent::Waiting),
                            ..
                        }) => "Waiting for wine to exit",
                        _ => "Removing...",
                    })
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        let reason = self.remove_reason_with(&self.facts, &dialog.name);
        if !running && let Some(reason) = &reason {
            content = content.push(text(reason.clone()).size(13).color(palette.muted));
        }
        let mut controls = row![
            Space::new().width(Fill),
            focus_button::control(
                secondary_button("Cancel", palette)
                    .on_press_maybe((!moving).then_some(Message::CancelRemove))
                    .into(),
                !moving,
                CANCEL_ID,
                Message::CancelRemove,
            )
        ]
        .spacing(12);
        if !running {
            controls = controls.push(focus_button::control(
                accent_base("Remove", palette.danger)
                    .on_press_maybe(reason.is_none().then_some(Message::ConfirmRemove))
                    .into(),
                reason.is_none(),
                REMOVE_ID,
                Message::ConfirmRemove,
            ));
        }
        container(content.push(controls))
            .padding(24)
            .width((self.window_size.width - 40.0).clamp(280.0, 600.0))
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            })
            .into()
    }

    pub(crate) fn toast_view(&self) -> Element<'_, Message> {
        let toast = self.toast.as_ref().unwrap();
        let palette = self.palette;
        let enabled = !self.management_busy()
            && !self.trash_confirm_open()
            && !self.downloads_confirm_open()
            && self.hook_stop_confirm.is_none()
            && self.flow.is_none()
            && self.removal.is_none()
            && self.settings.is_none()
            && self.art_page.is_none()
            && !self.close_prompt;
        let undo = crate::focus_button::undo(
            secondary_button("Undo", palette)
                .on_press_maybe(enabled.then_some(Message::Undo))
                .into(),
            enabled,
        );
        let mut content = row![text(&toast.text).width(Fill).wrapping(TOAST_WRAPPING), undo]
            .spacing(12)
            .align_y(Center);
        if let Some(change) = &self.change {
            content = content.push(text(change.reason()).size(13));
        }
        mouse_area(container(content).padding(12).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(6).width(1).color(palette.border))
        }))
        .on_enter(Message::ToastHover(true))
        .on_exit(Message::ToastHover(false))
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operation, OperationKind, sidebar};
    use iced::event::Status;
    use iced::keyboard::{Key, Modifiers, key::Named};
    use wineo_core::paths::Paths;
    use wineo_core::trash;

    fn fixture(wine: bool) -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("game");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let content = if wine {
            let prefix = paths.default_prefix_root().join("game");
            std::fs::create_dir_all(&prefix).unwrap();
            format!(
                "prefix = '{}'\nrunner = 'missing-test-runner'\narch = 'win64'\n",
                prefix.display()
            )
        } else {
            "kind = 'native'\n".into()
        };
        std::fs::write(config, content).unwrap();
        let mut app = App::boot(paths).0;
        app.cards = crate::library::load_library(&app.paths).unwrap();
        app.refreshing = false;
        app.trash_refreshing = false;
        (root, app)
    }

    fn open(app: &mut App) {
        let _ = app.update(Message::Remove("game".into()));
        assert!(app.removal.is_some());
    }

    #[test]
    fn canceled_remove_closes_quietly_but_real_errors_stay() {
        let (_root, mut app) = fixture(false);
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        let _ = app.update(Message::RemoveProgress(
            app.change_generation,
            RemoveEvent::Waiting,
        ));
        let _ = app.update(Message::CancelRemove);
        let _ = app.update(Message::RemoveFinished(
            app.change_generation,
            Err(remove_work::RemoveError::Canceled),
        ));
        assert!(app.removal.is_none() && app.change.is_none() && app.toast.is_none());
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        let _ = app.update(Message::RemoveFinished(
            app.change_generation,
            Err("real failure".into()),
        ));
        assert_eq!(
            app.removal.as_ref().unwrap().error.as_deref(),
            Some("real failure")
        );
    }

    #[test]
    fn a_failed_remove_measures_the_prefix_again() {
        let (_root, mut app) = fixture(true);
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        assert!(app.removal.as_ref().unwrap().measure.is_canceled());
        let before = app.remove_generation;
        let _ = app.update(Message::RemoveFinished(
            app.change_generation,
            Err("real failure".into()),
        ));
        let dialog = app.removal.as_ref().unwrap();
        assert!(!dialog.measure.is_canceled());
        assert_ne!(app.remove_generation, before);
        let _ = app.update(Message::RemoveSize(app.remove_generation, Ok(42)));
        assert_eq!(app.removal.as_ref().unwrap().size, Some(Ok(42)));
    }

    #[test]
    fn removing_another_card_keeps_selection_and_panel() {
        let (_root, mut app) = fixture(false);
        std::fs::write(app.paths.environment_config("other"), "kind = 'native'\n").unwrap();
        app.cards = crate::library::load_library(&app.paths).unwrap();
        app.selected = Some("other".into());
        app.panel_open = true;
        removed(&mut app);
        assert_eq!(app.selected.as_deref(), Some("other"));
        assert!(app.panel_open);
    }

    #[test]
    fn confirming_cancels_measurement_and_ignores_its_result() {
        let (_root, mut app) = fixture(true);
        open(&mut app);
        let token = app.removal.as_ref().unwrap().measure.clone();
        let _ = app.update(Message::ConfirmRemove);
        assert!(token.is_canceled());
        let _ = app.update(Message::RemoveSize(
            app.remove_generation,
            Err("late measurement".into()),
        ));
        assert!(app.removal.as_ref().unwrap().size.is_none());
    }

    #[test]
    fn settings_keep_control_z_and_block_undo() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
        assert!(app.settings.is_some());
        let _ = app.update(Message::Key(
            Key::Character("z".into()),
            Modifiers::CTRL,
            Status::Ignored,
        ));
        assert!(app.change.is_none());
        let _ = app.update(Message::Undo);
        assert!(app.change.is_none());
        assert!(
            key(
                &mut app,
                &Key::Character("z".into()),
                Modifiers::CTRL,
                Status::Ignored
            )
            .is_none()
        );
    }

    #[test]
    fn library_tab_is_unchanged_until_undo_is_available() {
        let (_root, mut app) = fixture(false);
        for modifiers in [Modifiers::empty(), Modifiers::SHIFT] {
            assert!(
                key(
                    &mut app,
                    &Key::Named(Named::Tab),
                    modifiers,
                    Status::Ignored
                )
                .is_none()
            );
            assert_eq!(
                crate::keys::action(
                    &Key::Named(Named::Tab),
                    modifiers,
                    false,
                    false,
                    false,
                    true
                ),
                crate::keys::Action::None
            );
        }
        removed(&mut app);
        assert!(
            key(
                &mut app,
                &Key::Named(Named::Tab),
                Modifiers::empty(),
                Status::Ignored
            )
            .is_some()
        );
    }

    #[test]
    fn dialog_focus_controls_match_each_phase() {
        let (_root, mut app) = fixture(true);
        open(&mut app);
        assert_eq!(app.remove_controls(), [PREFIX_ID, CANCEL_ID, REMOVE_ID]);
        let _ = app.update(Message::ConfirmRemove);
        assert_eq!(app.remove_controls(), [CANCEL_ID]);
        let _ = app.update(Message::RemoveProgress(
            app.change_generation,
            RemoveEvent::Moving,
        ));
        assert!(app.remove_controls().is_empty());
        let (_root, mut app) = fixture(false);
        open(&mut app);
        assert_eq!(app.remove_controls(), [CANCEL_ID, REMOVE_ID]);
    }

    #[test]
    fn delete_focus_distinguishes_search_from_undo() {
        use iced::advanced::widget::{Operation, operation::Focusable};
        #[derive(Default)]
        struct Focus(bool);
        impl Focusable for Focus {
            fn is_focused(&self) -> bool {
                self.0
            }
            fn focus(&mut self) {
                self.0 = true;
            }
            fn unfocus(&mut self) {
                self.0 = false;
            }
        }
        for search in [false, true] {
            let mut query = RemoveFocus::default();
            query.focusable(
                Some(&iced::widget::Id::new(crate::SEARCH_ID)),
                iced::Rectangle::default(),
                &mut Focus(search),
            );
            query.focusable(
                Some(&iced::widget::Id::new(focus_button::UNDO_ID)),
                iced::Rectangle::default(),
                &mut Focus(!search),
            );
            let (_root, mut app) = fixture(false);
            app.selected = Some("game".into());
            app.search_focused = !search;
            let iced::advanced::widget::operation::Outcome::Some((actual_search, any)) =
                query.finish()
            else {
                panic!("missing focus result")
            };
            let _ = app.update(Message::RemoveFocus(Status::Ignored, actual_search, any));
            assert_eq!(app.search_focused, search);
            assert!(app.removal.is_none());
        }
    }

    #[test]
    fn toast_pluralizes_desktop_entries_and_mentions_retained_snapshots() {
        let (_root, mut app) = fixture(true);
        let mut entry = removed(&mut app);
        let snapshots = app.paths.default_prefix_root().join("kept-snapshots");
        std::fs::create_dir_all(&snapshots).unwrap();
        entry.snapshots_left = Some(snapshots.clone());
        assert!(!Toast::new(entry.clone()).text.contains("Snapshots kept."));
        std::fs::write(snapshots.join("snapshot"), "snapshot").unwrap();
        for count in [1, 2] {
            entry.desktop_removed = count;
            let toast = Toast::new(entry.clone());
            assert!(toast.text.contains(if count == 1 {
                "1 desktop entry removed."
            } else {
                "2 desktop entries removed."
            }));
            assert!(toast.text.contains("Prefix kept.") && toast.text.contains("Snapshots kept."));
            assert!(!toast.text.contains('\n'));
        }
    }

    #[test]
    fn long_undo_errors_wrap_even_with_an_unbroken_path() {
        use iced::advanced::text::{self as advanced_text, Paragraph as _};
        type Paragraph = <iced::Renderer as advanced_text::Renderer>::Paragraph;
        let error = format!("Restore failed at /{}", "long-directory-name".repeat(32));
        let paragraph = Paragraph::with_text(advanced_text::Text {
            content: error.as_str(),
            bounds: iced::Size::new(240.0, f32::INFINITY),
            size: 16.0.into(),
            line_height: advanced_text::LineHeight::default(),
            font: iced::Font::DEFAULT,
            align_x: advanced_text::Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: advanced_text::Shaping::default(),
            wrapping: TOAST_WRAPPING,
        });
        assert!(paragraph.min_width() <= 240.0);
        assert!(paragraph.min_height() > 32.0);
    }

    #[test]
    fn expiry_during_restore_keeps_toast_and_double_undo_is_blocked() {
        let (_root, mut app) = fixture(false);
        let entry = removed(&mut app);
        let start = app.toast.as_ref().unwrap().last;
        let _ = app.update(Message::Undo);
        let generation = app.change_generation;
        let _ = app.update(Message::Undo);
        assert_eq!(app.change_generation, generation);
        assert!(matches!(&app.change, Some(Change::Restore { id, .. }) if id == &entry.id));
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(11)));
        assert!(app.toast.as_ref().unwrap().remaining.is_zero());
        let _ = app.update(Message::UndoFinished(
            generation,
            Err("restore failed".into()),
        ));
        assert_eq!(
            app.toast.as_ref().unwrap().remaining,
            Duration::from_secs(10)
        );
    }

    #[test]
    fn prefix_defaults_off_and_native_has_no_checkbox() {
        for wine in [false, true] {
            let (_root, mut app) = fixture(wine);
            open(&mut app);
            assert!(!app.removal.as_ref().unwrap().include_prefix);
            assert_eq!(app.removal.as_ref().unwrap().prefix.is_some(), wine);
            let _ = app.update(Message::RemovePrefix(true));
            assert_eq!(app.removal.as_ref().unwrap().include_prefix, wine);
            let _ = app.update(Message::ConfirmRemove);
            assert!(matches!(app.change, Some(Change::Remove { .. })));
        }
    }

    #[test]
    fn remove_refuses_running_busy_lost_unresolved_and_in_flight() {
        for state in 0..5 {
            let (_root, mut app) = fixture(false);
            match state {
                0 => app.cards[0].state = State::Running,
                1 => app.cards[0].busy = true,
                2 => app.cards[0].lost = true,
                3 => app.cards[0].unresolved = true,
                _ => {
                    app.operation = Some(Operation {
                        name: "game".into(),
                        kind: OperationKind::Stop,
                        token: None,
                        cancelled: false,
                        status: String::new(),
                        install_done: None,
                        installing: false,
                        after_hook: false,
                    })
                }
            }
            assert!(
                app.remove_reason("game")
                    .unwrap()
                    .starts_with("Stop it first")
            );
            let _ = app.update(Message::Remove("game".into()));
            assert!(app.removal.is_none());
        }
    }

    #[test]
    fn cancel_wait_and_moving_and_one_change_at_a_time() {
        for moving in [false, true] {
            let (_root, mut app) = fixture(false);
            open(&mut app);
            let _ = app.update(Message::ConfirmRemove);
            let token = match &app.change {
                Some(Change::Remove { token, .. }) => token.clone(),
                _ => unreachable!(),
            };
            let _ = app.update(Message::RemoveProgress(
                app.change_generation,
                if moving {
                    RemoveEvent::Moving
                } else {
                    RemoveEvent::Waiting
                },
            ));
            let _ = app.update(Message::ConfirmRemove);
            let _ = app.update(Message::Undo);
            let _ = app.update(Message::Section(sidebar::Section::Trash));
            assert_eq!(app.section, sidebar::Section::Library);
            let _ = app.update(Message::CancelRemove);
            assert_eq!(token.is_canceled(), !moving);
            assert!(app.removal.is_some());
            assert_eq!(app.change.as_ref().unwrap().reason(), "Busy: removing game");
            let _ = app.update(Message::RemoveFinished(
                app.change_generation,
                Err("failed".into()),
            ));
            assert!(app.change.is_none());
            assert_eq!(
                app.removal.as_ref().unwrap().error.as_deref(),
                Some("failed")
            );
            assert!(app.refreshing && app.trash_refreshing);
        }
    }

    fn removed(app: &mut App) -> Entry {
        open(app);
        let _ = app.update(Message::ConfirmRemove);
        let entry = trash::remove(&app.paths, "game", false).unwrap();
        let _ = app.update(Message::RemoveFinished(
            app.change_generation,
            Ok(entry.clone()),
        ));
        entry
    }

    #[test]
    fn success_closes_panel_deselects_and_reloads_with_undo_id() {
        let (_root, mut app) = fixture(true);
        app.selected = Some("game".into());
        app.panel_open = true;
        let entry = removed(&mut app);
        assert!(app.removal.is_none() && !app.panel_open && app.selected.is_none());
        assert!(app.refreshing && app.trash_refreshing);
        let toast = app.toast.as_ref().unwrap();
        assert_eq!(toast.id, entry.id);
        assert!(toast.text.starts_with("Removed game to the trash."));
        assert!(toast.text.contains("Prefix kept."));
    }

    #[test]
    fn successful_remove_clears_only_that_environments_checkup() {
        let (_root, mut app) = fixture(false);
        let scope = crate::checkup::Scope::Environment("game".into());
        let other = crate::checkup::Scope::Environment("other".into());
        let _ = app.update(Message::Checkup(crate::checkup::Event::Open(
            crate::checkup::Scope::Environment("game".into()),
        )));
        app.checkups.insert(other.clone(), Default::default());
        assert!(app.checkups.contains_key(&scope));
        let entry = removed(&mut app);
        assert!(!app.checkups.contains_key(&scope));
        assert!(app.checkups.contains_key(&other));
        let _ = app.update(Message::Undo);
        trash::restore(&app.paths, &entry.id).unwrap();
        let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
        let _ = app.update(Message::Loaded(Ok(crate::library::load_library(
            &app.paths,
        )
        .unwrap())));
        let _ = app.update(Message::Select("game".into()));
        assert!(app.panel_open && !app.checkup_visible());
        assert!(!app.checkups.contains_key(&scope));
    }

    #[test]
    fn undo_uses_entry_id_refusal_stays_and_success_dismisses() {
        let (_root, mut app) = fixture(false);
        let entry = removed(&mut app);
        let _ = app.update(Message::Undo);
        assert!(matches!(&app.change, Some(Change::Restore { id, .. }) if id == &entry.id));
        let _ = app.update(Message::Remove("game".into()));
        assert!(app.removal.is_none());
        let config = app.paths.environment_config("game");
        std::fs::write(&config, "kind = 'native'\n").unwrap();
        let error = trash::restore(&app.paths, &entry.id)
            .unwrap_err()
            .to_string();
        app.refreshing = false;
        app.trash_refreshing = false;
        let _ = app.update(Message::UndoFinished(
            app.change_generation,
            Err(error.clone()),
        ));
        assert_eq!(app.toast.as_ref().unwrap().text, error);
        assert!(app.change.is_none() && app.refreshing && app.trash_refreshing);
        std::fs::remove_file(config).unwrap();
        let _ = app.update(Message::Key(
            Key::Character("z".into()),
            Modifiers::CTRL,
            Status::Ignored,
        ));
        assert!(matches!(app.change, Some(Change::Restore { .. })));
        trash::restore(&app.paths, &entry.id).unwrap();
        let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
        assert!(app.toast.is_none());
    }

    #[test]
    fn toast_expiry_pauses_on_hover_and_focus() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        let start = app.toast.as_ref().unwrap().last;
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(4)));
        assert_eq!(
            app.toast.as_ref().unwrap().remaining,
            Duration::from_secs(6)
        );
        let _ = app.update(Message::ToastHover(true));
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(24)));
        assert_eq!(
            app.toast.as_ref().unwrap().remaining,
            Duration::from_secs(6)
        );
        let _ = app.update(Message::ToastHover(false));
        let _ = app.update(Message::ToastFocus(true, start + Duration::from_secs(44)));
        assert_eq!(
            app.toast.as_ref().unwrap().remaining,
            Duration::from_secs(6)
        );
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(64)));
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(70)));
        assert!(app.toast.is_none());
    }

    #[test]
    fn toast_has_one_wakeup_at_expiry_and_none_while_paused() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        let toast = app.toast.as_ref().unwrap();
        let expiry = toast.last + toast.remaining;
        assert_eq!(app.toast_wake.due(), Some(expiry));

        let _ = app.update(Message::ToastHover(true));
        assert!(app.toast_wake.due().is_none(), "hover leaves no timer");
        let paused = app.toast.as_ref().unwrap().remaining;

        let _ = app.update(Message::ToastHover(false));
        let toast = app.toast.as_ref().unwrap();
        assert_eq!(
            toast.remaining, paused,
            "unpausing keeps the remaining time"
        );
        assert_eq!(app.toast_wake.due(), Some(toast.last + toast.remaining));

        let start = toast.last;
        let _ = app.update(Message::ToastFocus(true, start + Duration::from_secs(1)));
        assert!(app.toast_wake.due().is_none(), "focus leaves no timer");
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(2)));
        let toast = app.toast.as_ref().unwrap();
        assert_eq!(app.toast_wake.due(), Some(toast.last + toast.remaining));
    }

    #[test]
    fn a_stale_toast_wakeup_changes_nothing_and_the_live_one_checks_focus() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        let live = app.toast_wake.generation();
        let remaining = app.toast.as_ref().unwrap().remaining;

        let _ = app.update(Message::ToastTick(live.wrapping_sub(1)));
        assert!(app.toast_wake.due().is_some());
        assert_eq!(app.toast.as_ref().unwrap().remaining, remaining);

        let _ = app.update(Message::ToastTick(live));
        // The wakeup was consumed and nothing re-arms until the focus answer
        // comes back.
        assert!(app.toast_wake.due().is_none());
        let _ = app.update(Message::ToastFocus(false, Instant::now()));
        assert!(app.toast_wake.due().is_some());
    }

    #[test]
    fn an_expired_toast_waits_for_the_change_and_then_goes() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        let _ = app.update(Message::Undo);
        let start = app.toast.as_ref().unwrap().last;
        let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(11)));
        assert!(app.toast.as_ref().unwrap().remaining.is_zero());
        assert!(app.toast_wake.due().is_none(), "nothing left to wait for");

        app.change = None;
        let _ = app.update(Message::ToastHover(false));
        assert!(app.toast.is_none());
    }

    #[test]
    fn the_feature_notice_gets_its_own_wakeup() {
        let (_root, mut app) = fixture(false);
        let until = Instant::now() + Duration::from_secs(8);
        app.feature_notice = Some(("Done".into(), until));
        let _ = app.update(Message::ToastHover(false));
        assert_eq!(app.toast_wake.due(), Some(until));

        removed(&mut app);
        let toast = app.toast.as_ref().unwrap();
        let expiry = toast.last + toast.remaining;
        assert_eq!(app.toast_wake.due(), Some(expiry.min(until)));

        let live = app.toast_wake.generation();
        app.feature_notice = Some(("Done".into(), Instant::now() - Duration::from_millis(1)));
        let _ = app.update(Message::ToastTick(live));
        assert!(app.feature_notice.is_none());
    }

    #[test]
    fn the_expiry_wakeup_asks_about_focus_before_it_expires_the_toast() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        // Undo gained focus (a held Tab) after the last check, just before
        // the deadline passed.
        let toast = app.toast.as_mut().unwrap();
        toast.last = Instant::now() - Duration::from_secs(11);
        let live = app.toast_wake.generation();

        let _ = app.update(Message::ToastTick(live));
        assert!(app.toast.is_some(), "expired before the answer arrived");
        assert!(!app.toast.as_ref().unwrap().remaining.is_zero());
        assert!(app.toast_wake.due().is_none(), "no timer while it waits");

        let _ = app.update(Message::ToastFocus(true, Instant::now()));
        let toast = app.toast.as_ref().unwrap();
        assert!(toast.focused && !toast.remaining.is_zero());
        assert!(app.toast_wake.due().is_none());

        // Focus left again: the first answer ends the pause, the next one
        // finds the clock run out.
        let later = Instant::now() + Duration::from_secs(11);
        let _ = app.update(Message::ToastFocus(false, Instant::now()));
        let _ = app.update(Message::ToastFocus(false, later));
        assert!(app.toast.is_none());
    }

    #[test]
    fn the_expiry_wakeup_expires_an_unfocused_toast_on_the_answer() {
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        app.toast.as_mut().unwrap().last = Instant::now() - Duration::from_secs(11);
        let live = app.toast_wake.generation();
        let _ = app.update(Message::ToastTick(live));
        let _ = app.update(Message::ToastFocus(false, Instant::now()));
        assert!(app.toast.is_none());
    }

    #[test]
    fn a_focus_answer_for_a_replaced_toast_is_ignored() {
        let (_root, mut app) = fixture(false);
        let entry = removed(&mut app);
        let old = app.toast.as_ref().unwrap().serial;
        app.toast = Some(Toast::new(entry));
        let toast = app.toast.as_ref().unwrap();
        assert_ne!(toast.serial, old);
        let (remaining, last) = (toast.remaining, toast.last);

        let _ = app.update(Message::ToastAnswer(
            old,
            true,
            last + Duration::from_secs(30),
        ));
        let toast = app.toast.as_ref().unwrap();
        assert!(!toast.focused && toast.remaining == remaining && toast.last == last);

        let live = toast.serial;
        let _ = app.update(Message::ToastAnswer(live, true, last));
        assert!(app.toast.as_ref().unwrap().focused);
    }

    #[test]
    fn focus_is_rechecked_after_a_click_a_key_release_or_any_change_while_focused() {
        let (_root, mut app) = fixture(false);
        assert!(!app.toast_focus_may_have_changed(&Message::Pressed));
        removed(&mut app);
        assert!(app.toast_focus_may_have_changed(&Message::Pressed));
        assert!(app.toast_focus_may_have_changed(&Message::FocusRecheck));
        assert!(!app.toast_focus_may_have_changed(&Message::Rebuild));
        app.toast.as_mut().unwrap().focused = true;
        assert!(app.toast_focus_may_have_changed(&Message::Rebuild));
        assert!(!app.toast_focus_may_have_changed(&Message::ToastFocus(true, Instant::now())));
        assert!(!app.toast_focus_may_have_changed(&Message::ScrollIdle(0)));
    }

    #[test]
    fn delete_and_escape_respect_focus_section_modal_and_running_state() {
        for blocked in 0..6 {
            let (_root, mut app) = fixture(false);
            app.selected = Some("game".into());
            if blocked == 1 {
                app.section = sidebar::Section::Trash;
            }
            if blocked == 2 {
                app.log_view = true;
            }
            if blocked == 3 {
                app.close_prompt = true;
            }
            let _ = app.update(Message::KeyWithFocus(
                Key::Named(Named::Delete),
                Modifiers::empty(),
                if blocked == 4 {
                    Status::Captured
                } else {
                    Status::Ignored
                },
                blocked == 5,
            ));
            assert_eq!(app.removal.is_some(), blocked == 0);
            if blocked == 0 {
                let measure = app.removal.as_ref().unwrap().measure.clone();
                let _ = app.update(Message::Key(
                    Key::Named(Named::Escape),
                    Modifiers::empty(),
                    Status::Ignored,
                ));
                assert!(app.removal.is_none() && measure.is_canceled());
                open(&mut app);
                let _ = app.update(Message::ConfirmRemove);
                let _ = app.update(Message::Key(
                    Key::Named(Named::Escape),
                    Modifiers::empty(),
                    Status::Ignored,
                ));
                assert!(app.removal.is_some());
            }
        }
    }

    #[test]
    fn mouse_back_dismisses_the_remove_dialog_like_escape() {
        let (_root, mut app) = fixture(false);
        app.selected = Some("game".into());
        open(&mut app);
        let measure = app.removal.as_ref().unwrap().measure.clone();
        crate::tests::press_back(&mut app, Status::Captured);
        assert!(app.removal.is_none() && measure.is_canceled());
    }

    #[test]
    fn closing_prompts_cancels_wait_and_waits_for_moving_or_restore() {
        for moving in [false, true] {
            let (_root, mut app) = fixture(false);
            open(&mut app);
            let _ = app.update(Message::ConfirmRemove);
            if moving {
                let _ = app.update(Message::RemoveProgress(
                    app.change_generation,
                    RemoveEvent::Moving,
                ));
            }
            let token = match &app.change {
                Some(Change::Remove { token, .. }) => token.clone(),
                _ => unreachable!(),
            };
            let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
            assert!(app.close_prompt && !app.closing);
            let _ = app.update(Message::KeepWorking);
            assert!(!app.close_prompt);
            let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
            let _ = app.update(Message::CancelAndClose);
            assert!(app.closing && app.foreground_busy());
            assert_eq!(token.is_canceled(), !moving);
            let _ = app.update(Message::RemoveFinished(
                app.change_generation,
                Err("failed".into()),
            ));
            assert!(!app.foreground_busy());
        }
        let (_root, mut app) = fixture(false);
        removed(&mut app);
        let _ = app.update(Message::Undo);
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        let _ = app.update(Message::CancelAndClose);
        assert!(app.closing && app.foreground_busy());
        let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
        assert!(!app.foreground_busy());
    }

    #[test]
    fn stale_reads_and_progress_cannot_replace_a_new_change() {
        let (_root, mut app) = fixture(false);
        let old_library = app.library_generation;
        let old_badge = app.trash_generation;
        let stale_cards = app.cards.clone();
        let entry = removed(&mut app);
        let old_change = app.change_generation;
        let _ = app.update(Message::LibraryReloaded(
            app.library_generation,
            Ok(Vec::new()),
        ));
        let fresh_badge = sidebar::TrashBadge {
            count: Some(1),
            warning: false,
        };
        let _ = app.update(Message::TrashReloaded(
            app.trash_generation,
            fresh_badge.clone(),
        ));
        let _ = app.update(Message::LibraryReloaded(old_library, Ok(stale_cards)));
        let _ = app.update(Message::TrashReloaded(
            old_badge,
            sidebar::TrashBadge::default(),
        ));
        assert!(app.cards.is_empty());
        assert_eq!(app.trash_badge, fresh_badge);
        let _ = app.update(Message::Undo);
        let _ = app.update(Message::RemoveProgress(old_change, RemoveEvent::Moving));
        let _ = app.update(Message::RemoveFinished(old_change, Err("late".into())));
        assert!(matches!(&app.change, Some(Change::Restore { id, .. }) if id == &entry.id));
        trash::restore(&app.paths, &entry.id).unwrap();
        let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
        app.cards = crate::library::load_library(&app.paths).unwrap();
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        let _ = app.update(Message::RemoveProgress(old_change, RemoveEvent::Moving));
        assert!(matches!(
            app.change,
            Some(Change::Remove { phase: None, .. })
        ));
        let _ = app.update(Message::UndoFinished(old_change + 1, Ok(())));
        assert!(matches!(app.change, Some(Change::Remove { .. })));
    }

    #[test]
    fn another_games_launch_does_not_block_removing_an_idle_card() {
        let (_root, mut app) = fixture(false);
        app.operation = Some(Operation {
            name: "other".into(),
            kind: OperationKind::Start,
            token: Some(Token::new()),
            cancelled: false,
            status: String::new(),
            install_done: None,
            installing: false,
            after_hook: false,
        });
        assert!(app.remove_reason("game").is_none());
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        assert!(matches!(app.change, Some(Change::Remove { .. })));
    }

    #[test]
    fn closing_a_dialog_cancels_measurement_and_ignores_old_size() {
        let (_root, mut app) = fixture(true);
        open(&mut app);
        let old = app.remove_generation;
        let token = app.removal.as_ref().unwrap().measure.clone();
        let _ = app.update(Message::CancelRemove);
        assert!(token.is_canceled());
        open(&mut app);
        let _ = app.update(Message::RemoveSize(old, Ok(42)));
        assert!(app.removal.as_ref().unwrap().size.is_none());
        let _ = app.update(Message::RemoveSize(app.remove_generation, Ok(24)));
        assert_eq!(app.removal.as_ref().unwrap().size, Some(Ok(24)));
        let token = app.removal.as_ref().unwrap().measure.clone();
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        assert!(app.removal.is_none() && token.is_canceled());
    }
}
