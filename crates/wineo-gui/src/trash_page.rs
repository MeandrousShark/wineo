use std::collections::HashMap;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Space, column, container, row, scrollable, text, text_input, tooltip};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::paths::Paths;
use wineo_core::trash::{Entry, PendingEntry, Phase, PruneReview};

use crate::ui::{
    ScrollStyle, accent_button, page_title, secondary_button, section, section_heading,
};
use crate::{App, Message, focus_button, removal, sidebar, spinner, trash_work};

const CANCEL_ID: &str = "trash-cancel";
const CONFIRM_ID: &str = "trash-confirm";

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) entries: Vec<Entry>,
    pub(crate) pending: Vec<PendingEntry>,
}

#[derive(Debug, Clone)]
pub(crate) enum Selection {
    Id(String),
    OlderThan(u32),
    All,
}

#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) result: Result<(), String>,
}

impl Outcome {
    fn label(&self) -> String {
        match &self.result {
            Ok(()) => format!("Deleted {} ({}) forever.", self.name, self.id),
            Err(error) => format!("Failed to delete {} ({}): {error}", self.name, self.id),
        }
    }
}

pub(crate) struct Page {
    pub(crate) snapshot: Option<Result<Snapshot, String>>,
    pub(crate) loading: bool,
    pub(crate) days: String,
    pub(crate) error: Option<String>,
    pub(crate) row_errors: HashMap<String, String>,
    pub(crate) recovery_errors: HashMap<String, String>,
    pub(crate) results: Vec<Outcome>,
    pub(crate) notice: Option<String>,
    /// What the last recovery kept in the trash instead of returning.
    pub(crate) notes: Vec<String>,
    pub(crate) review: Option<PruneReview>,
    pub(crate) reviewing: bool,
    pub(crate) review_generation: u64,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            snapshot: None,
            loading: false,
            days: "30".into(),
            error: None,
            row_errors: HashMap::new(),
            recovery_errors: HashMap::new(),
            results: Vec::new(),
            notice: None,
            notes: Vec::new(),
            review: None,
            reviewing: false,
            review_generation: 0,
        }
    }
}

impl Page {
    pub(crate) fn cancel_review(&mut self) {
        self.review = None;
        self.reviewing = false;
        self.review_generation = self.review_generation.wrapping_add(1);
    }

    fn clear_feedback(&mut self) {
        self.results.clear();
        self.notice = None;
        self.notes.clear();
        self.error = None;
        self.row_errors.clear();
        self.recovery_errors.clear();
    }

    fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()?.as_ref().ok()
    }

    fn feedback(&self) -> Vec<(String, bool)> {
        let mut feedback = Vec::new();
        if let Some(error) = &self.error {
            feedback.push((error.clone(), true));
        }
        if let Some(notice) = &self.notice {
            feedback.push((notice.clone(), false));
        }
        feedback.extend(self.notes.iter().map(|note| (note.clone(), false)));
        for (id, error) in &self.row_errors {
            if self
                .snapshot()
                .is_none_or(|snapshot| !snapshot.entries.iter().any(|entry| &entry.id == id))
            {
                feedback.push((format!("{id}: {error}"), true));
            }
        }
        for (id, error) in &self.recovery_errors {
            if self
                .snapshot()
                .is_none_or(|snapshot| !snapshot.pending.iter().any(|entry| &entry.id == id))
            {
                feedback.push((format!("{id}: {error}"), true));
            }
        }
        feedback.extend(
            self.results
                .iter()
                .map(|result| (result.label(), result.result.is_err())),
        );
        feedback
    }

    pub(crate) fn row_enabled(&self, id: &str) -> bool {
        self.snapshot().is_some_and(|snapshot| {
            snapshot
                .entries
                .iter()
                .any(|entry| entry.id == id && entry.phase == Phase::Trashed)
                && !snapshot.pending.iter().any(|entry| entry.id == id)
        })
    }
}

pub(crate) enum Change {
    Restore { id: String, name: String },
    Prune { ids: Vec<String> },
    Recover { id: String, name: String },
}

impl Change {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Restore { name, .. } => format!("Busy: restoring {name}"),
            Self::Prune { .. } => "Busy: deleting trash entries".into(),
            Self::Recover { name, .. } => format!("Busy: recovering {name}"),
        }
    }

    fn affects(&self, id: &str) -> bool {
        match self {
            Self::Restore { id: active, .. } | Self::Recover { id: active, .. } => active == id,
            Self::Prune { ids } => ids.iter().any(|active| active == id),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Loaded(u64, Result<Snapshot, String>),
    CheckAgain,
    Days(String),
    Restore(String),
    Restored(u64, Result<(), String>),
    Review(Selection),
    ReviewOlder,
    Reviewed(u64, Result<PruneReview, String>),
    Dismiss,
    Confirm,
    Pruned(u64, Result<Vec<Outcome>, String>),
    Recover(String),
    Recovered(u64, Result<Vec<String>, String>),
}

fn message(event: Event) -> Message {
    Message::Trash(event)
}

pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> Task<Message> {
    match key {
        Key::Named(Named::Escape) => update(app, Event::Dismiss),
        Key::Named(Named::Tab) => iced::advanced::widget::operate(focus_button::cycle(
            &app.trash_confirm_controls(),
            modifiers.shift(),
        ))
        .discard(),
        _ => Task::none(),
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Loaded(generation, result) if generation == app.trash_generation => {
            app.trash_refreshing = false;
            app.trash_page.loading = false;
            app.trash_badge = match &result {
                Ok(snapshot) => sidebar::TrashBadge::from_entries(
                    Some(&snapshot.entries),
                    Some(&snapshot.pending),
                ),
                Err(_) => sidebar::TrashBadge::from_entries(None, None),
            };
            if result.is_err() {
                app.trash_page.cancel_review();
            }
            app.shown.show_badge(app.trash_badge.clone());
            app.shown.show_trash(result.clone(), jiff::Timestamp::now());
            app.trash_page.snapshot = Some(result);
            Task::none()
        }
        Event::Loaded(..) => Task::none(),
        Event::CheckAgain if !app.close_prompt && !app.trash_confirm_open() => {
            app.trash_page.clear_feedback();
            app.reload_trash_page()
        }
        Event::Days(days) if app.trash_action_allowed() => {
            app.trash_page.days = days;
            Task::none()
        }
        Event::Restore(id) if app.trash_action_allowed() && app.trash_page.row_enabled(&id) => {
            let name = app
                .trash_page
                .snapshot()
                .unwrap()
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .unwrap()
                .name
                .clone();
            app.trash_page.clear_feedback();
            app.begin_trash_change(Change::Restore {
                id: id.clone(),
                name,
            });
            trash_work::restore(app.change_generation, app.paths.clone(), id)
        }
        Event::Recover(id) if app.trash_action_allowed() => {
            let Some(entry) = app
                .trash_page
                .snapshot()
                .unwrap()
                .pending
                .iter()
                .find(|entry| entry.id == id)
            else {
                return Task::none();
            };
            let name = entry.name.clone();
            app.trash_page.clear_feedback();
            app.begin_trash_change(Change::Recover {
                id: id.clone(),
                name,
            });
            trash_work::recover(app.change_generation, app.paths.clone(), id)
        }
        Event::Restored(generation, result) if generation == app.change_generation => {
            if !matches!(
                app.change,
                Some(removal::Change::Trash(Change::Restore { .. }))
            ) {
                return Task::none();
            }
            let Some(removal::Change::Trash(Change::Restore { id, name })) = app.change.take()
            else {
                unreachable!()
            };
            match result {
                Ok(()) => {
                    app.trash_page.notice =
                        Some(format!("Restored environment {name} from trash {id}."))
                }
                Err(error) => {
                    app.trash_page.row_errors.insert(id, error);
                }
            }
            app.reload_after_uses_change()
        }
        Event::Recovered(generation, result) if generation == app.change_generation => {
            if !matches!(
                app.change,
                Some(removal::Change::Trash(Change::Recover { .. }))
            ) {
                return Task::none();
            }
            let Some(removal::Change::Trash(Change::Recover { id, name })) = app.change.take()
            else {
                unreachable!()
            };
            match result {
                Ok(notes) => {
                    app.trash_page.notice = Some(format!("Recovered {name}."));
                    app.trash_page.notes = notes;
                }
                Err(error) => {
                    app.trash_page.recovery_errors.insert(id, error);
                }
            }
            app.reload_after_uses_change()
        }
        Event::Review(selection) if app.trash_action_allowed() => {
            if let Selection::Id(id) = &selection
                && !app.trash_page.row_enabled(id)
            {
                return Task::none();
            }
            app.trash_page.clear_feedback();
            app.trash_page.reviewing = true;
            app.trash_page.review_generation = app.trash_page.review_generation.wrapping_add(1);
            trash_work::review(
                app.trash_page.review_generation,
                app.paths.clone(),
                selection,
            )
        }
        Event::ReviewOlder if app.trash_action_allowed() => {
            match parse_days(&app.trash_page.days) {
                Ok(days) => update(app, Event::Review(Selection::OlderThan(days))),
                Err(error) => {
                    app.trash_page.clear_feedback();
                    app.trash_page.error = Some(error.into());
                    Task::none()
                }
            }
        }
        Event::Reviewed(generation, result)
            if generation == app.trash_page.review_generation && app.trash_page.reviewing =>
        {
            app.trash_page.reviewing = false;
            match result {
                Ok(review) if review.entries.is_empty() => {
                    app.trash_page.notice = Some("No trash entries selected.".into());
                    app.trash_page.error = review_errors(&review);
                }
                Ok(review) => app.trash_page.review = Some(review),
                Err(error) => app.trash_page.error = Some(error),
            }
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
        }
        Event::Dismiss if !app.management_busy() => {
            app.trash_page.review = None;
            Task::none()
        }
        Event::Confirm
            if app.trash_confirm_open() && !app.management_busy() && !app.close_prompt =>
        {
            let review = app.trash_page.review.take().unwrap();
            let ids = review
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            app.trash_page.error = review_errors(&review);
            app.begin_trash_change(Change::Prune { ids });
            trash_work::prune(app.change_generation, app.paths.clone(), review)
        }
        Event::Pruned(generation, result) if generation == app.change_generation => {
            if !matches!(
                app.change,
                Some(removal::Change::Trash(Change::Prune { .. }))
            ) {
                return Task::none();
            }
            app.change = None;
            match result {
                Ok(results) => app.trash_page.results = results,
                Err(error) => app.trash_page.error = Some(error),
            }
            app.reload_after_change()
        }
        _ => Task::none(),
    }
}

fn review_errors(review: &PruneReview) -> Option<String> {
    (!review.errors.is_empty()).then(|| {
        review
            .errors
            .iter()
            .map(|(id, error)| format!("Could not review trash entry {id}: {error}"))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn parse_days(days: &str) -> Result<u32, &'static str> {
    let error = "Age must be positive whole days, such as 30.";
    if days.is_empty() || days.starts_with('0') || !days.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(error);
    }
    days.parse::<u32>()
        .ok()
        .filter(|days| *days > 0)
        .ok_or(error)
}

pub(crate) fn entry_label(entry: &Entry, now: jiff::Timestamp) -> String {
    let removed = entry.removed_at.parse::<jiff::Timestamp>().map_or_else(
        |_| entry.removed_at.clone(),
        |at| wineo_core::time::relative(now, at),
    );
    let mut label = format!("{}   removed {removed}", entry.name);
    if entry.include_prefix && entry.prefix_present == Some(true) {
        label.push_str("   prefix included");
    }
    label
}

fn hover_label(paths: &Paths, entry: &Entry) -> String {
    let mut label = entry.id.clone();
    if entry.prefix_present == Some(true)
        && let Some(prefix) = &entry.prefix_left
    {
        label.push_str(&format!("\nPrefix left in place: {}", prefix.display()));
    }
    if let Some(snapshots) = &entry.snapshots_left {
        label.push_str(&format!(
            "\nSnapshots left in place: {}",
            snapshots.display()
        ));
    }
    paths.contract_text(&label)
}

pub(crate) fn prefix_note(paths: &Paths, entry: &Entry) -> Option<String> {
    if entry.prefix_present != Some(true) {
        return None;
    }
    entry.prefix_left.as_ref().map(|prefix| {
        paths.contract_text(&format!(
            "{}'s prefix at {} stays where it is.",
            entry.name,
            prefix.display()
        ))
    })
}

impl App {
    pub(crate) fn reload_trash_page(&mut self) -> Task<Message> {
        self.trash_generation = self.trash_generation.wrapping_add(1);
        self.shown.reloading_trash();
        self.trash_refreshing = true;
        self.trash_page.loading = true;
        trash_work::load(self.trash_generation, self.paths.clone())
    }

    pub(crate) fn trash_confirm_open(&self) -> bool {
        self.section == sidebar::Section::Trash
            && self.settings.is_none()
            && self.trash_page.review.is_some()
    }

    fn trash_action_allowed(&self) -> bool {
        self.section == sidebar::Section::Trash
            && self.trash_page.snapshot().is_some()
            && !self.management_busy()
            && !self.trash_page.reviewing
            && !self.trash_confirm_open()
            && self.removal.is_none()
            && self.flow.is_none()
            && self.settings.is_none()
            && !self.close_prompt
    }

    fn begin_trash_change(&mut self, change: Change) {
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(removal::Change::Trash(change));
    }

    pub(crate) fn trash_confirm_controls(&self) -> Vec<&'static str> {
        if self.management_busy() {
            Vec::new()
        } else {
            vec![CANCEL_ID, CONFIRM_ID]
        }
    }

    pub(crate) fn trash_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let page = &self.trash_page;
        let mut content = column![page_title("Trash", palette)].spacing(16);
        match &page.snapshot {
            Some(Err(error)) => {
                content = self.trash_feedback(content);
                return scrollable(
                    container(section(
                        content.push(text(error).color(palette.danger)).push(
                            secondary_button("Check again", palette)
                                .on_press(message(Event::CheckAgain)),
                        ),
                        palette,
                    ))
                    .padding(24)
                    .width(Fill),
                )
                .wineo_style(palette)
                .height(Fill)
                .into();
            }
            None => {
                return container(content.push(section(
                    text("Loading trash...").color(palette.muted),
                    palette,
                )))
                .padding(24)
                .into();
            }
            Some(Ok(_)) => {}
        }
        let snapshot = page.snapshot().unwrap();
        let enabled = self.trash_action_allowed();
        let change = match &self.change {
            Some(removal::Change::Trash(change)) => Some(change),
            _ => None,
        };
        if !snapshot.pending.is_empty() {
            let mut recovery = column![
                section_heading("Interrupted operations", palette),
                text("These trash operations need recovery.").color(palette.warning),
            ]
            .spacing(8);
            for entry in &snapshot.pending {
                let mut line = row![
                    text(format!("{} ({})", entry.name, entry.id)).width(Fill),
                    accent_button(
                        "Recover",
                        palette.warning,
                        enabled,
                        message(Event::Recover(entry.id.clone())),
                    )
                ]
                .spacing(12)
                .align_y(Center);
                if change.is_some_and(|change| change.affects(&entry.id)) {
                    line = line.push(spinner::spinner(palette, 16.0));
                }
                recovery = recovery.push(line);
                if let Some(error) = page.recovery_errors.get(&entry.id) {
                    recovery = recovery.push(text(error).color(palette.danger));
                }
            }
            content = content.push(section(recovery, palette));
        }
        let mut entries = column![section_heading("Removed environments", palette)].spacing(8);
        if snapshot.entries.is_empty() {
            entries = entries.push(text("The trash is empty.").color(palette.muted));
        }
        let now = jiff::Timestamp::now();
        for (index, entry) in snapshot.entries.iter().enumerate() {
            if index > 0 {
                entries = entries.push(crate::ui::separator(palette));
            }
            let active = enabled && page.row_enabled(&entry.id);
            let mut label = row![text(entry_label(entry, now)).width(Fill)]
                .spacing(8)
                .align_y(Center);
            if change.is_some_and(|change| change.affects(&entry.id)) {
                label = label.push(spinner::spinner(palette, 16.0));
            }
            let mut entry_view = column![
                tooltip(
                    label,
                    text(hover_label(&self.paths, entry)),
                    tooltip::Position::FollowCursor
                )
                .style(move |_| container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border)))
                .gap(6),
                row![
                    secondary_button("Restore", palette)
                        .on_press_maybe(active.then(|| message(Event::Restore(entry.id.clone())))),
                    accent_button(
                        "Delete forever",
                        palette.danger,
                        active,
                        message(Event::Review(Selection::Id(entry.id.clone()))),
                    )
                ]
                .spacing(8)
            ]
            .spacing(8);
            if let Some(note) = prefix_note(&self.paths, entry) {
                entry_view = entry_view.push(
                    text(note)
                        .size(13)
                        .color(palette.muted)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                );
            }
            if let Some(error) = page.row_errors.get(&entry.id) {
                entry_view = entry_view.push(text(error).color(palette.danger));
            }
            entries = entries.push(entry_view);
        }
        content = content.push(section(entries, palette));
        let days = text_input("30", &page.days)
            .width(80)
            .on_input_maybe(enabled.then_some(|days| message(Event::Days(days))));
        let cleanup = column![
            section_heading("Cleanup", palette),
            text("Delete entries older than"),
            row![days, text("days")].spacing(8).align_y(Center),
            accent_button(
                "Delete",
                palette.danger,
                enabled,
                message(Event::ReviewOlder)
            ),
            accent_button(
                "Empty trash",
                palette.danger,
                enabled,
                message(Event::Review(Selection::All)),
            ),
        ]
        .spacing(12);
        content = content.push(section(cleanup, palette));
        if page.reviewing {
            content = content.push(section(
                row![
                    spinner::spinner(palette, 16.0),
                    text("Reviewing trash entries...").color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
                palette,
            ));
        }
        content = self.trash_feedback(content);
        scrollable(container(content).padding(24).width(Fill))
            .wineo_style(palette)
            .height(Fill)
            .into()
    }

    fn trash_feedback<'a>(
        &'a self,
        mut content: iced::widget::Column<'a, Message>,
    ) -> iced::widget::Column<'a, Message> {
        for (label, error) in self.trash_page.feedback() {
            content = content.push(text(label).color(if error {
                self.palette.danger
            } else {
                self.palette.foreground
            }));
        }
        content
    }

    pub(crate) fn trash_confirm_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let review = self.trash_page.review.as_ref().unwrap();
        let mut selection = column![
            page_title("Delete trash entries", palette),
            text("Permanently delete these trash entries:")
        ]
        .spacing(12);
        for entry in &review.entries {
            selection = selection.push(text(format!(
                "{} ({})   prefix: {}",
                entry.name,
                entry.id,
                if entry.include_prefix {
                    "included"
                } else {
                    "not included"
                }
            )));
            if let Some(note) = prefix_note(&self.paths, entry) {
                selection = selection.push(
                    text(note)
                        .size(13)
                        .color(palette.muted)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                );
            }
        }
        if let Some(errors) = review_errors(review) {
            selection = selection.push(text(errors).color(palette.danger));
        }
        let enabled = !self.management_busy();
        let controls = row![
            Space::new().width(Fill),
            focus_button::control(
                secondary_button("Cancel", palette)
                    .on_press_maybe(enabled.then(|| message(Event::Dismiss)))
                    .into(),
                enabled,
                CANCEL_ID,
                message(Event::Dismiss)
            ),
            focus_button::control(
                accent_button(
                    "Delete forever",
                    palette.danger,
                    enabled,
                    message(Event::Confirm),
                ),
                enabled,
                CONFIRM_ID,
                message(Event::Confirm)
            )
        ]
        .spacing(12);
        container(
            column![
                scrollable(selection)
                    .wineo_style(palette)
                    .height((self.window_size.height - 180.0).clamp(120.0, 480.0)),
                controls
            ]
            .spacing(16),
        )
        .padding(24)
        .width((self.window_size.width - 40.0).clamp(280.0, 700.0))
        .style(move |_| crate::ui::section_style(palette))
        .into()
    }
}

#[cfg(test)]
pub(crate) mod tests;
