use std::collections::HashMap;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Space, button, column, container, pick_list, row, scrollable, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::build_lock::Kind;
use wineo_core::cancel::Token;
use wineo_core::download::{Provider, Release};

use crate::{App, Message, downloads_work, focus_button, removal, sidebar, spinner};

const CANCEL_ID: &str = "downloads-cancel";
const CONFIRM_ID: &str = "downloads-confirm";

#[derive(Debug, Clone)]
pub(crate) struct Build {
    pub(crate) kind: Kind,
    pub(crate) name: String,
    pub(crate) users: Vec<String>,
    pub(crate) error: Option<String>,
}

impl Build {
    pub(crate) fn new(kind: Kind, name: &str) -> Self {
        Self {
            kind,
            name: name.into(),
            users: Vec::new(),
            error: None,
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

    fn uses_label(&self) -> String {
        if self.users.is_empty() {
            "unused".into()
        } else {
            format!("used by {}", self.users.join(", "))
        }
    }
}

fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        let rounded = (value * 10.0).round() / 10.0;
        if rounded.fract() == 0.0 {
            format!("{rounded:.0} {}", UNITS[unit])
        } else {
            format!("{rounded:.1} {}", UNITS[unit])
        }
    }
}

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
                        .find(|build| build.id() == id && build.unused())
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
                        size_label(bytes)
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
        let mut content = column![text(format!("Delete {}?", build.name)).size(22)].spacing(16);
        content = content.push(text(match size {
            Some(bytes) => format!("This frees {}.", size_label(*bytes)),
            None => "Measuring size...".into(),
        }));
        content = content.push(
            row![
                Space::new().width(Fill),
                focus_button::control(
                    button("Cancel").on_press(message(Event::Dismiss)).into(),
                    true,
                    CANCEL_ID,
                    message(Event::Dismiss)
                ),
                focus_button::control(
                    button("Delete")
                        .on_press_maybe(size.is_some().then_some(message(Event::Confirm)))
                        .into(),
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
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            })
            .into()
    }

    pub(crate) fn downloads_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let page = &self.downloads_page;
        let enabled = self.downloads_action_allowed();
        let mut content = column![
            row![
                text("Downloads").size(22).width(Fill),
                button("Refresh").on_press_maybe(enabled.then_some(message(Event::Refresh))),
                button("Download...").on_press_maybe(enabled.then_some(message(Event::Choose))),
            ]
            .spacing(12)
            .align_y(Center)
        ]
        .spacing(16);
        if page.loading {
            content = content.push(text("Loading builds..."));
        }
        if let Some(error) = &page.error {
            content = content.push(text(error).color(palette.danger));
        }
        if let Some(notice) = &page.notice {
            content = content.push(text(notice));
        }
        if let Some(status) = &page.status {
            content = content.push(
                row![
                    spinner::spinner(palette, self.spinner_angle, 16.0),
                    text(status).width(Fill),
                    button("Cancel").on_press(message(Event::Cancel))
                ]
                .spacing(12)
                .align_y(Center),
            );
        }
        if page.choosing {
            let tags = page
                .releases
                .iter()
                .map(|release| release.tag.clone())
                .collect::<Vec<_>>();
            content = content.push(
                column![
                    row![
                        text("Provider"),
                        pick_list(Provider::ALL, page.provider, |provider| message(
                            Event::Provider(provider)
                        ))
                    ]
                    .spacing(12)
                    .align_y(Center),
                    row![
                        text("Release"),
                        pick_list(tags, page.selected.clone(), |tag| message(Event::Select(
                            tag
                        ))),
                        button("Load more").on_press_maybe(
                            (enabled && page.has_more && !page.listing)
                                .then_some(message(Event::More))
                        ),
                        button("Install").on_press_maybe(
                            (enabled && page.selected.is_some() && !page.listing)
                                .then_some(message(Event::Install))
                        ),
                        button("Back").on_press_maybe(enabled.then_some(message(Event::Dismiss))),
                    ]
                    .spacing(12)
                    .align_y(Center),
                ]
                .spacing(12),
            );
            if page.listing {
                content = content.push(text("Loading releases..."));
            }
        }
        match &page.snapshot {
            Some(Ok(snapshot)) => {
                let mut warnings = snapshot.warnings.clone();
                warnings.extend(page.warnings.clone());
                warnings.sort();
                warnings.dedup();
                for warning in warnings {
                    content = content.push(text(warning).color(palette.warning));
                }
                for kind in [Kind::Runner, Kind::Dxvk, Kind::Vkd3d] {
                    content = content.push(text(heading(kind)).size(18));
                    let builds = snapshot
                        .builds
                        .iter()
                        .filter(|build| build.kind == kind)
                        .collect::<Vec<_>>();
                    if builds.is_empty() {
                        content = content.push(text("None installed").color(palette.muted));
                    }
                    for build in builds {
                        let mut line = row![
                            text(&build.name),
                            text(build.uses_label()).color(palette.muted).width(Fill)
                        ]
                        .spacing(12)
                        .align_y(Center);
                        if build.unused() {
                            line = line.push(button("Delete").on_press_maybe(
                                enabled.then(|| message(Event::Review(build.id()))),
                            ));
                        }
                        content = content.push(line);
                        if let Some(error) =
                            page.row_errors.get(&build.id()).or(build.error.as_ref())
                        {
                            content = content.push(text(error).color(palette.danger));
                        }
                    }
                }
                if !snapshot.skipped.is_empty() {
                    content = content.push(text("Not usable").size(18));
                    for skipped in &snapshot.skipped {
                        content = content.push(text(skipped).color(palette.muted));
                    }
                }
            }
            Some(Err(error)) => {
                content = content
                    .push(text(error).color(palette.danger))
                    .push(button("Try again").on_press(message(Event::Refresh)));
            }
            None => {}
        }
        for (label, error) in page.feedback() {
            content = content.push(text(label).color(if error {
                palette.danger
            } else {
                palette.foreground
            }));
        }
        scrollable(container(content).padding(24).width(Fill))
            .height(Fill)
            .into()
    }
}

#[cfg(test)]
pub(crate) mod tests;
