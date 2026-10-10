use std::collections::HashMap;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Space, column, container, pick_list, row, scrollable, text};
use iced::{Center, Element, Fill, Task};
use wineo_core::build_lock::Kind;
use wineo_core::cancel::Token;
use wineo_core::download::{Provider, Release};
use wineo_core::external_runners::Source;

use crate::ui::{
    PickListStyle, ScrollStyle, accent_button, page_title, secondary_button, section,
    section_heading,
};
use crate::{App, Message, downloads_work, focus_button, removal, sidebar, spinner};

const CANCEL_ID: &str = "downloads-cancel";
const CONFIRM_ID: &str = "downloads-confirm";

#[derive(Debug, Clone)]
pub(crate) struct Build {
    pub(crate) kind: Kind,
    pub(crate) name: String,
    pub(crate) users: Vec<String>,
    pub(crate) error: Option<String>,
    /// Set for another launcher's build, which wineo never deletes.
    pub(crate) launcher: Option<Source>,
    /// Lower priority sources with a different build under the same name.
    pub(crate) different: Vec<Source>,
}

impl Build {
    pub(crate) fn new(kind: Kind, name: &str) -> Self {
        Self {
            kind,
            name: name.into(),
            users: Vec::new(),
            error: None,
            launcher: None,
            different: Vec::new(),
        }
    }

    fn id(&self) -> String {
        format!("{}/{}", heading(self.kind), self.name)
    }

    fn unused(&self) -> bool {
        self.users.is_empty()
            && self.error.is_none()
            && !(self.kind == Kind::Runner && self.name == "wine")
    }

    fn deletable(&self) -> bool {
        self.unused() && self.launcher.is_none()
    }

    fn uses_label(&self) -> String {
        if self.users.is_empty() {
            "unused".into()
        } else {
            format!("used by {}", self.users.join(", "))
        }
    }

    /// The muted line under the name: where it's from when that isn't
    /// wineo, what uses it, and any different build its name hides.
    fn detail(&self) -> String {
        let mut parts = Vec::new();
        if let Some(launcher) = self.launcher {
            parts.push(launcher.to_string());
        }
        parts.push(self.uses_label());
        match self.different.as_slice() {
            [] => {}
            [source] => parts.push(format!("{source} has a different build")),
            sources => parts.push(format!(
                "{} have different builds",
                sources
                    .iter()
                    .map(|source| source.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
        parts.join(", ")
    }
}

const EXTERNAL_HEADING: &str = "From Steam, Heroic, and Lutris";

fn heading(kind: Kind) -> &'static str {
    match kind {
        Kind::Runner => "Runners",
        Kind::Dxvk => "DXVK",
        Kind::Vkd3d => "vkd3d-proton",
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) builds: Vec<Build>,
    pub(crate) skipped: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReleasePage {
    pub(crate) releases: Vec<Release>,
    pub(crate) has_more: bool,
}

#[derive(Default)]
pub(crate) struct Page {
    pub(crate) snapshot: Option<Result<Snapshot, String>>,
    pub(crate) loading: bool,
    generation: u64,
    review_generation: u64,
    review: Option<(Build, Option<u64>)>,
    row_errors: HashMap<String, String>,
    warnings: Vec<String>,
    notice: Option<String>,
    error: Option<String>,
    choosing: bool,
    provider: Option<Provider>,
    releases: Vec<Release>,
    selected: Option<String>,
    release_generation: u64,
    listing: bool,
    page: u32,
    has_more: bool,
    status: Option<String>,
}

impl Page {
    fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()?.as_ref().ok()
    }

    fn feedback(&self) -> Vec<(String, bool)> {
        self.row_errors
            .iter()
            .filter(|(id, _)| {
                self.snapshot()
                    .is_none_or(|snapshot| !snapshot.builds.iter().any(|build| &build.id() == *id))
            })
            .map(|(id, error)| (format!("{id}: {error}"), true))
            .collect()
    }

    pub(crate) fn cancel_review(&mut self) {
        self.review = None;
        self.review_generation = self.review_generation.wrapping_add(1);
    }
}

pub(crate) enum Change {
    Delete(Build),
    Download { name: String, token: Token },
}

impl Change {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Delete(build) => format!("Busy: deleting {}", build.name),
            Self::Download { name, .. } => format!("Busy: downloading {name}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Loaded(u64, Result<Snapshot, String>),
    Refresh,
    Review(String),
    Measured(u64, Result<u64, String>),
    Dismiss,
    Confirm,
    Deleted(u64, Result<(u64, Vec<String>), String>),
    Choose,
    Provider(Provider),
    Listed(u64, u32, Result<ReleasePage, String>),
    More,
    Select(String),
    Install,
    Progress(u64, String),
    Cancel,
    Downloaded(u64, Result<Option<String>, String>),
}

fn message(event: Event) -> Message {
    Message::Downloads(event)
}

pub(crate) fn key(app: &mut App, key: &Key, modifiers: Modifiers, captured: bool) -> Task<Message> {
    if app.downloads_confirm_open() {
        return match key {
            Key::Named(Named::Escape) => update(app, Event::Dismiss),
            Key::Named(Named::Tab) => iced::advanced::widget::operate(focus_button::cycle(
                &app.downloads_confirm_controls(),
                modifiers.shift(),
            ))
            .discard(),
            _ => Task::none(),
        };
    }
    if captured {
        return Task::none();
    }
    match key {
        Key::Named(Named::Escape) => update(app, Event::Dismiss),
        Key::Named(Named::Tab) if modifiers.shift() => iced::widget::operation::focus_previous(),
        Key::Named(Named::Tab) => iced::widget::operation::focus_next(),
        _ => Task::none(),
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Loaded(generation, result) if generation == app.downloads_page.generation => {
            app.downloads_page.loading = false;
            app.downloads_page.snapshot = Some(result);
            Task::none()
        }
        Event::Refresh
            if app.section == sidebar::Section::Downloads
                && !app.management_busy()
                && !app.close_prompt
                && !app.downloads_confirm_open()
                && app.flow.is_none()
                && app.settings.is_none()
                && app.removal.is_none() =>
        {
            app.reload_downloads_page()
        }
        Event::Review(id) if app.downloads_action_allowed() => {
            let Some(build) = app
                .downloads_page
                .snapshot()
                .and_then(|snapshot| {
                    snapshot
                        .builds
                        .iter()
                        .find(|build| build.id() == id && build.deletable())
                })
                .cloned()
            else {
                return Task::none();
            };
            let page = &mut app.downloads_page;
            page.review_generation = page.review_generation.wrapping_add(1);
            page.row_errors.remove(&id);
            page.review = Some((build.clone(), None));
            Task::batch([
                downloads_work::measure(page.review_generation, app.paths.clone(), build),
                iced::advanced::widget::operate(
                    iced::advanced::widget::operation::focusable::unfocus(),
                ),
            ])
        }
        Event::Measured(generation, result)
            if generation == app.downloads_page.review_generation =>
        {
            if let Some((build, size)) = &mut app.downloads_page.review {
                match result {
                    Ok(bytes) => *size = Some(bytes),
                    Err(error) => {
                        app.downloads_page.row_errors.insert(build.id(), error);
                        app.downloads_page.cancel_review();
                    }
                }
            }
            Task::none()
        }
        Event::Dismiss if !app.management_busy() => {
            if app.downloads_confirm_open() {
                app.downloads_page.cancel_review();
            } else {
                app.downloads_page.choosing = false;
                app.downloads_page.release_generation =
                    app.downloads_page.release_generation.wrapping_add(1);
                app.downloads_page.listing = false;
            }
            Task::none()
        }
        Event::Confirm
            if app.downloads_confirm_open() && !app.management_busy() && !app.close_prompt =>
        {
            let Some((build, Some(_))) = app.downloads_page.review.clone() else {
                return Task::none();
            };
            app.downloads_page.cancel_review();
            app.begin_downloads_change(Change::Delete(build.clone()));
            downloads_work::delete(app.change_generation, app.paths.clone(), build)
        }
        Event::Deleted(generation, result)
            if generation == app.change_generation
                && matches!(
                    app.change,
                    Some(removal::Change::Downloads(Change::Delete(_)))
                ) =>
        {
            let Some(removal::Change::Downloads(Change::Delete(build))) = app.change.take() else {
                unreachable!()
            };
            match result {
                Ok((bytes, warnings)) => {
                    app.downloads_page.notice = Some(format!(
                        "Deleted {}. {} freed.",
                        build.name,
                        crate::size::format_bytes(bytes)
                    ));
                    app.downloads_page.warnings = warnings;
                }
                Err(error) => {
                    app.downloads_page.row_errors.insert(build.id(), error);
                }
            }
            Task::batch([app.reload_after_change(), app.reload_downloads_page()])
        }
        Event::Choose if app.downloads_action_allowed() => {
            app.downloads_page.choosing = true;
            app.downloads_page.error = None;
            Task::none()
        }
        Event::Provider(provider)
            if app.downloads_action_allowed() && app.downloads_page.choosing =>
        {
            let page = &mut app.downloads_page;
            page.provider = Some(provider);
            page.releases.clear();
            page.selected = None;
            page.page = 0;
            page.has_more = false;
            page.error = None;
            page.release_generation = page.release_generation.wrapping_add(1);
            page.listing = true;
            downloads_work::releases(page.release_generation, provider, 1)
        }
        Event::Listed(generation, number, result)
            if generation == app.downloads_page.release_generation
                && app.downloads_page.listing
                && number == app.downloads_page.page + 1 =>
        {
            let page = &mut app.downloads_page;
            page.listing = false;
            match result {
                Ok(catalog) => {
                    page.page = number;
                    page.has_more = catalog.has_more;
                    page.releases.extend(catalog.releases);
                    if page.selected.is_none() {
                        page.selected = page.releases.first().map(|release| release.tag.clone());
                    }
                    // Match core's latest selection when upstream's first page has no supported builds.
                    if page.releases.is_empty() && page.has_more && number < 1000 {
                        page.listing = true;
                        return downloads_work::releases(
                            page.release_generation,
                            page.provider.unwrap(),
                            number + 1,
                        );
                    }
                    if page.releases.is_empty() && !page.has_more {
                        page.error = Some("No supported stable releases found.".into());
                    }
                }
                Err(error) => page.error = Some(error),
            }
            Task::none()
        }
        Event::More
            if app.downloads_action_allowed()
                && app.downloads_page.choosing
                && app.downloads_page.has_more
                && !app.downloads_page.listing =>
        {
            let page = &mut app.downloads_page;
            page.listing = true;
            page.error = None;
            downloads_work::releases(
                page.release_generation,
                page.provider.unwrap(),
                page.page + 1,
            )
        }
        Event::Select(tag) if app.downloads_action_allowed() && app.downloads_page.choosing => {
            if app
                .downloads_page
                .releases
                .iter()
                .any(|release| release.tag == tag)
            {
                app.downloads_page.selected = Some(tag);
            }
            Task::none()
        }
        Event::Install
            if app.downloads_action_allowed()
                && app.downloads_page.choosing
                && !app.downloads_page.listing =>
        {
            let Some(release) = app
                .downloads_page
                .releases
                .iter()
                .find(|release| Some(&release.tag) == app.downloads_page.selected.as_ref())
                .cloned()
            else {
                return Task::none();
            };
            let token = Token::new();
            app.downloads_page.error = None;
            app.downloads_page.notice = None;
            app.downloads_page.status = Some("Starting download...".into());
            app.begin_downloads_change(Change::Download {
                name: release.installed_name.clone(),
                token: token.clone(),
            });
            downloads_work::download(app.change_generation, app.paths.clone(), release, token)
        }
        Event::Progress(generation, status)
            if generation == app.change_generation
                && matches!(
                    app.change,
                    Some(removal::Change::Downloads(Change::Download { .. }))
                ) =>
        {
            app.downloads_page.status = Some(app.paths.contract_text(&status));
            Task::none()
        }
        Event::Cancel => {
            if let Some(removal::Change::Downloads(Change::Download { token, .. })) = &app.change {
                token.cancel();
                app.downloads_page.status = Some("Canceling download...".into());
            }
            Task::none()
        }
        Event::Downloaded(generation, result)
            if generation == app.change_generation
                && matches!(
                    app.change,
                    Some(removal::Change::Downloads(Change::Download { .. }))
                ) =>
        {
            app.change = None;
            app.downloads_page.status = None;
            match result {
                Ok(notice) => {
                    app.downloads_page.notice =
                        Some(notice.unwrap_or_else(|| "Download canceled.".into()))
                }
                Err(error) => app.downloads_page.error = Some(error),
            }
            Task::batch([app.reload_after_change(), app.reload_downloads_page()])
        }
        _ => Task::none(),
    }
}

impl App {
    pub(crate) fn reload_downloads_page(&mut self) -> Task<Message> {
        let page = &mut self.downloads_page;
        page.generation = page.generation.wrapping_add(1);
        page.loading = true;
        downloads_work::load(page.generation, self.paths.clone())
    }

    pub(crate) fn downloads_confirm_open(&self) -> bool {
        self.section == sidebar::Section::Downloads
            && self.settings.is_none()
            && self.downloads_page.review.is_some()
    }

    fn downloads_action_allowed(&self) -> bool {
        self.section == sidebar::Section::Downloads
            && self.downloads_page.snapshot().is_some()
            && !self.downloads_page.loading
            && !self.management_busy()
            && !self.downloads_confirm_open()
            && self.removal.is_none()
            && self.flow.is_none()
            && self.settings.is_none()
            && !self.close_prompt
    }

    fn begin_downloads_change(&mut self, change: Change) {
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(removal::Change::Downloads(change));
    }

    fn downloads_confirm_controls(&self) -> Vec<&'static str> {
        let mut controls = vec![CANCEL_ID];
        if matches!(self.downloads_page.review, Some((_, Some(_)))) && !self.management_busy() {
            controls.push(CONFIRM_ID);
        }
        controls
    }

    pub(crate) fn downloads_confirm_view(&self) -> Element<'_, Message> {
        let (build, size) = self.downloads_page.review.as_ref().unwrap();
        let palette = self.palette;
        let mut content = column![page_title("Delete download", palette)].spacing(16);
        content = content.push(text(format!("Delete {}?", build.name)));
        content = content.push(text(match size {
            Some(bytes) => format!("This frees {}.", crate::size::format_bytes(*bytes)),
            None => "Measuring size...".into(),
        }));
        content = content.push(
            row![
                Space::new().width(Fill),
                focus_button::control(
                    secondary_button("Cancel", palette)
                        .on_press(message(Event::Dismiss))
                        .into(),
                    true,
                    CANCEL_ID,
                    message(Event::Dismiss)
                ),
                focus_button::control(
                    accent_button(
                        "Delete",
                        palette.danger,
                        size.is_some(),
                        message(Event::Confirm),
                    ),
                    size.is_some(),
                    CONFIRM_ID,
                    message(Event::Confirm)
                ),
            ]
            .spacing(12),
        );
        container(content)
            .padding(24)
            .width((self.window_size.width - 40.0).clamp(280.0, 600.0))
            .style(move |_| crate::ui::section_style(palette))
            .into()
    }

    pub(crate) fn downloads_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let page = &self.downloads_page;
        let enabled = self.downloads_action_allowed();
        let mut content = column![
            row![
                page_title("Downloads", palette).width(Fill),
                secondary_button("Refresh", palette)
                    .on_press_maybe(enabled.then_some(message(Event::Refresh))),
                accent_button(
                    "Download...",
                    palette.primary,
                    enabled,
                    message(Event::Choose)
                ),
            ]
            .spacing(12)
            .align_y(Center)
        ]
        .spacing(16);
        let mut feedback = column![].spacing(8);
        if page.loading {
            feedback = feedback.push(text("Loading builds...").color(palette.muted));
        }
        if let Some(error) = &page.error {
            feedback = feedback.push(text(error).color(palette.danger));
        }
        if let Some(notice) = &page.notice {
            feedback = feedback.push(text(notice).color(palette.foreground));
        }
        if let Some(status) = &page.status {
            feedback = feedback.push(
                row![
                    spinner::spinner(palette, self.spinner_angle, 16.0),
                    text(status).width(Fill),
                    secondary_button("Cancel", palette).on_press(message(Event::Cancel))
                ]
                .spacing(12)
                .align_y(Center),
            );
        }
        if page.loading || page.error.is_some() || page.notice.is_some() || page.status.is_some() {
            content = content.push(section(feedback, palette));
        }
        if page.choosing {
            let tags = page
                .releases
                .iter()
                .map(|release| release.tag.clone())
                .collect::<Vec<_>>();
            content = content.push(section(
                column![
                    section_heading("Choose a download", palette),
                    row![
                        text("Provider"),
                        pick_list(Provider::ALL, page.provider, |provider| message(
                            Event::Provider(provider)
                        ))
                        .wineo_style(palette)
                    ]
                    .spacing(12)
                    .align_y(Center),
                    row![
                        text("Release"),
                        pick_list(tags, page.selected.clone(), |tag| message(Event::Select(
                            tag
                        )))
                        .wineo_style(palette)
                    ]
                    .spacing(12)
                    .align_y(Center),
                    row![
                        secondary_button("Load more", palette).on_press_maybe(
                            (enabled && page.has_more && !page.listing)
                                .then_some(message(Event::More)),
                        ),
                        accent_button(
                            "Install",
                            palette.primary,
                            enabled && page.selected.is_some() && !page.listing,
                            message(Event::Install),
                        ),
                    ]
                    .spacing(12)
                    .align_y(Center),
                    secondary_button("Back", palette)
                        .on_press_maybe(enabled.then_some(message(Event::Dismiss))),
                ]
                .spacing(12),
                palette,
            ));
            if page.listing {
                content = content.push(section(
                    text("Loading releases...").color(palette.muted),
                    palette,
                ));
            }
        }
        match &page.snapshot {
            Some(Ok(snapshot)) => {
                let mut warnings = snapshot.warnings.clone();
                warnings.extend(page.warnings.clone());
                warnings.sort();
                warnings.dedup();
                if !warnings.is_empty() {
                    let notices = warnings.into_iter().fold(
                        column![section_heading("Warnings", palette)].spacing(8),
                        |rows, warning| rows.push(text(warning).color(palette.warning)),
                    );
                    content = content.push(section(notices, palette));
                }
                let external = snapshot
                    .builds
                    .iter()
                    .filter(|build| build.launcher.is_some())
                    .collect::<Vec<_>>();
                for kind in [Kind::Runner, Kind::Dxvk, Kind::Vkd3d] {
                    let builds = snapshot
                        .builds
                        .iter()
                        .filter(|build| build.kind == kind && build.launcher.is_none())
                        .collect::<Vec<_>>();
                    content =
                        content.push(build_section(heading(kind), builds, page, enabled, palette));
                    if kind == Kind::Runner && !external.is_empty() {
                        content = content.push(build_section(
                            EXTERNAL_HEADING,
                            external.clone(),
                            page,
                            enabled,
                            palette,
                        ));
                    }
                }
                if !snapshot.skipped.is_empty() {
                    let skipped = snapshot.skipped.iter().fold(
                        column![section_heading("Not usable", palette)].spacing(8),
                        |rows, item| rows.push(text(item).color(palette.muted)),
                    );
                    content = content.push(section(skipped, palette));
                }
            }
            Some(Err(error)) => {
                content = content.push(section(
                    column![
                        text(error).color(palette.danger),
                        secondary_button("Try again", palette).on_press(message(Event::Refresh))
                    ]
                    .spacing(8),
                    palette,
                ));
            }
            None => {}
        }
        let notices = page.feedback();
        if !notices.is_empty() {
            let notices = notices
                .into_iter()
                .fold(column![].spacing(8), |rows, (label, error)| {
                    rows.push(text(label).color(if error {
                        palette.danger
                    } else {
                        palette.foreground
                    }))
                });
            content = content.push(section(notices, palette));
        }
        scrollable(container(content).padding(24).width(Fill))
            .wineo_style(palette)
            .height(Fill)
            .into()
    }
}

fn build_section<'a>(
    title: &'a str,
    builds: Vec<&'a Build>,
    page: &'a Page,
    enabled: bool,
    palette: crate::theme::Palette,
) -> Element<'a, Message> {
    let mut rows = column![section_heading(title, palette)].spacing(8);
    if builds.is_empty() {
        rows = rows.push(text("None installed").color(palette.muted));
    }
    for (index, build) in builds.into_iter().enumerate() {
        let mut line = row![
            column![
                text(&build.name).color(palette.foreground),
                text(build.detail()).size(13).color(palette.muted)
            ]
            .width(Fill),
        ]
        .align_y(Center);
        if build.deletable() {
            line = line.push(
                secondary_button("Delete", palette)
                    .on_press_maybe(enabled.then(|| message(Event::Review(build.id())))),
            );
        }
        if index > 0 {
            rows = rows.push(crate::ui::separator(palette));
        }
        rows = rows.push(line);
        if let Some(error) = page.row_errors.get(&build.id()).or(build.error.as_ref()) {
            rows = rows.push(text(error).color(palette.danger));
        }
    }
    section(rows, palette).into()
}

#[cfg(test)]
pub(crate) mod tests;
