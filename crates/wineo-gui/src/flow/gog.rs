//! Import from GOG: moving the GOG games Heroic installed into wineo, like
//! `wineo import gog`. Lists Heroic's games, reviews one, migrates it, then
//! adds GOG's playtime and art, which never fails the migration.

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, pick_list, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::art::{Kind, Source};
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Https, Provider};
use wineo_core::gog::account::Session;
use wineo_core::gog::extras::{self, Request};
use wineo_core::gog::folder::{Platform, Track};
use wineo_core::gog::migrate::{self, MigrateError, Migration, Outcome, Plan, StateChange, Target};
use wineo_core::heroic::{HeroicGogGame, HeroicSource};
use wineo_core::heroic_import::{self, Policy, RunnerSuggestion};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb;

use super::Action;
use super::heroic::{self, Prerequisite};
use crate::runner_label::Choice;
use crate::theme::Palette;
use crate::ui::{PickListStyle, accent_base, list_group, list_row, secondary_button};

pub(crate) const HEROIC_NOTE: &str =
    "Remove it from Heroic's library, without deleting its files, so both don't update it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Games,
    Review,
    Working,
    Finished,
}

#[derive(Debug, Clone)]
enum Standing {
    Ready(Box<Migration>),
    Already { environment: String },
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    source: HeroicSource,
    game: HeroicGogGame,
    standing: Standing,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Listing {
    games: Vec<Candidate>,
    /// Heroic records that couldn't be used, as sentences.
    issues: Vec<String>,
}

/// One game ready to review: its migration, with a runner planned for a new
/// Windows environment when one could be.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    source: HeroicSource,
    migration: Box<Migration>,
    runners: Vec<Choice>,
    runner: String,
    suggestion: Option<RunnerSuggestion>,
    missing_build: Option<(Component, Option<String>)>,
    /// Why it can't be migrated as it stands.
    blocking: Option<String>,
}

impl Prepared {
    fn ready(&self) -> bool {
        self.blocking.is_none()
            && self.missing_build.is_none()
            && self.migration.environment().is_some()
    }
}

/// What a migration did, for the finish page.
#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    lines: Vec<String>,
    warnings: Vec<String>,
    environment: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Discard,
    KeepEditing,
    Listed(u64, Result<Listing, String>),
    Pick(usize),
    Prepared(u64, Result<Box<Prepared>, String>),
    Runner(String),
    CopyRunner,
    DownloadRunner,
    DownloadBuild(Component, Option<String>),
    PrerequisiteFinished(u64, Result<String, String>),
    Migrate,
    Cancel,
    Force,
    Progress(u64, String),
    Finished(u64, Result<Report, String>),
    BackToLibrary,
}

pub(crate) struct Page {
    paths: Paths,
    step: Step,
    generation: u64,
    listing: Option<Listing>,
    picked: Option<usize>,
    prepared: Option<Prepared>,
    report: Option<Report>,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    notes: Vec<String>,
    leave_prompt: bool,
    action: Option<Action>,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            step: Step::Games,
            generation: 0,
            listing: None,
            picked: None,
            prepared: None,
            report: None,
            token: None,
            status: String::new(),
            error: None,
            notes: Vec::new(),
            leave_prompt: false,
            action: None,
        };
        let task = page.list();
        (page, task)
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
    }

    /// A listing, preparation, or migration is running.
    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = "Force stop requested...".to_owned();
            } else {
                token.cancel();
                self.status = "Canceling...".to_owned();
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        if self.busy() || self.waiting() {
            return Task::none();
        }
        match self.step {
            Step::Review => self.start(),
            Step::Finished => self.update(Event::BackToLibrary),
            Step::Games | Step::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        // A reply to what this step started must not reopen it.
        self.next_generation();
        self.status.clear();
        self.error = None;
        match self.step {
            // A game was picked, so there is a draft to lose.
            Step::Games if self.picked.is_some() => self.leave_prompt = true,
            Step::Games => self.action = Some(Action::Choose),
            Step::Review => {
                self.notes.clear();
                self.step = Step::Games;
            }
            Step::Working => {}
            Step::Finished => self.exit(),
        }
        Task::none()
    }

    fn exit(&mut self) {
        self.action = Some(Action::Library {
            name: self
                .report
                .as_ref()
                .and_then(|report| report.environment.clone()),
            launch: false,
        });
    }

    fn list(&mut self) -> Task<Event> {
        self.step = Step::Games;
        self.listing = None;
        self.status = "Reading Heroic's GOG games...".to_owned();
        let generation = self.next_generation();
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load_listing(&paths))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |result| Event::Listed(generation, result),
        )
    }

    fn prepare(&mut self, runner: Option<String>) -> Task<Event> {
        let Some(candidate) = self
            .picked
            .and_then(|index| self.listing.as_ref()?.games.get(index))
        else {
            return Task::none();
        };
        let Standing::Ready(migration) = &candidate.standing else {
            return Task::none();
        };
        let source = candidate.source.clone();
        let migration = migration.clone();
        self.error = None;
        self.status = format!("Checking {}...", candidate.game.title);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    prepare(&paths, source, migration, runner).map(Box::new)
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Event::Prepared(generation, result),
        )
    }

    fn prerequisite(&mut self, operation: Prerequisite) -> Task<Event> {
        let token = Token::new();
        self.token = Some(token.clone());
        self.status = operation.label();
        self.error = None;
        self.step = Step::Working;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    heroic::install_prerequisite(&paths, operation, &token, &|line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    })
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Event::PrerequisiteFinished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn start(&mut self) -> Task<Event> {
        let Some(prepared) = self.prepared.clone() else {
            return Task::none();
        };
        if self.busy() || self.waiting() || !prepared.ready() {
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.error = None;
        self.status = format!("Migrating {}...", prepared.migration.game().title);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    run(
                        &paths,
                        &prepared,
                        &token,
                        |line| {
                            let _ = sender.unbounded_send(Event::Progress(generation, line));
                        },
                        import_extras,
                    )
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Back => self.back(),
            Event::Discard => {
                self.leave_prompt = false;
                self.action = Some(Action::Choose);
                Task::none()
            }
            Event::KeepEditing => {
                self.leave_prompt = false;
                Task::none()
            }
            Event::Listed(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(listing) => self.listing = Some(listing),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Pick(index) if self.step == Step::Games && !self.waiting() => {
                let ready = self
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.games.get(index))
                    .is_some_and(|game| matches!(game.standing, Standing::Ready(_)));
                if !ready {
                    return Task::none();
                }
                self.leave_prompt = false;
                self.picked = Some(index);
                self.prepared = None;
                self.notes.clear();
                self.prepare(None)
            }
            Event::Prepared(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(prepared) => {
                        self.prepared = Some(*prepared);
                        self.step = Step::Review;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Runner(name) if self.step == Step::Review && !self.waiting() => {
                self.prepare(Some(name))
            }
            Event::CopyRunner if self.step == Step::Review && !self.waiting() => {
                let Some(Prepared {
                    suggestion: Some(RunnerSuggestion::Copy(copy)),
                    ..
                }) = &self.prepared
                else {
                    return Task::none();
                };
                self.prerequisite(Prerequisite::CopyRunner(copy.clone()))
            }
            Event::DownloadRunner if self.step == Step::Review && !self.waiting() => {
                self.prerequisite(Prerequisite::Download(Provider::GeProton, None))
            }
            Event::DownloadBuild(component, version)
                if self.step == Step::Review && !self.waiting() =>
            {
                let provider = match component {
                    Component::Dxvk => Provider::Dxvk,
                    Component::Vkd3d => Provider::Vkd3d,
                };
                self.prerequisite(Prerequisite::Download(provider, version))
            }
            Event::PrerequisiteFinished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.step = Step::Review;
                match result {
                    Ok(name) => {
                        self.notes.push(format!(
                            "Installed {name}. It stays installed if this import is canceled."
                        ));
                        // A copied or first runner becomes the pick; a build
                        // leaves the runner as it was.
                        let runner = self.prepared.as_ref().and_then(|prepared| {
                            (prepared.runner.is_empty()
                                || matches!(prepared.suggestion, Some(RunnerSuggestion::Copy(_))))
                            .then(|| name.clone())
                            .or_else(|| {
                                (!prepared.runner.is_empty()).then(|| prepared.runner.clone())
                            })
                        });
                        self.prepare(runner)
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Migrate if self.step == Step::Review => self.start(),
            Event::Cancel if self.step == Step::Working => {
                self.cancel(false);
                Task::none()
            }
            Event::Force if self.step == Step::Working => {
                self.cancel(true);
                Task::none()
            }
            Event::Progress(generation, line)
                if generation == self.generation && self.step == Step::Working =>
            {
                if !self.token.as_ref().is_some_and(Token::is_canceled) {
                    self.status = line;
                }
                Task::none()
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                match result {
                    Ok(report) => {
                        self.report = Some(report);
                        self.step = Step::Finished;
                    }
                    Err(error) => {
                        // Nothing changed, so the review is still there to
                        // go back to.
                        self.error = Some(error);
                        self.step = Step::Review;
                    }
                }
                Task::none()
            }
            Event::BackToLibrary if self.step == Step::Finished => {
                self.exit();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub(crate) fn view(&self, palette: Palette, spin: f32) -> Element<'_, Event> {
        let title = match self.step {
            Step::Games => "Choose a GOG game",
            Step::Review => "Review GOG migration",
            Step::Working => "Migrating from GOG",
            Step::Finished => "GOG migration finished",
        };
        let mut body = column![crate::ui::page_title(title, palette)]
            .spacing(14)
            .width(Fill);
        if !self.status.is_empty() {
            body = body.push(
                row![
                    crate::spinner::spinner(palette, spin, 16.0),
                    text(&self.status).color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        for note in &self.notes {
            body = body.push(text(note).size(13).color(palette.muted));
        }
        match self.step {
            Step::Games => body = self.games_view(body, palette),
            Step::Review => body = self.review_view(body, palette),
            Step::Working => {}
            Step::Finished => {
                if let Some(report) = &self.report {
                    for line in &report.lines {
                        body = body.push(text(line));
                    }
                    for warning in &report.warnings {
                        body = body.push(text(warning).size(13).color(palette.warning));
                    }
                }
            }
        }
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let mut page = column![]
            .spacing(12)
            .max_width(heroic::PAGE_MAX_WIDTH)
            .height(Fill);
        if self.leave_prompt {
            page = page.push(text("Discard this GOG import draft?")).push(
                row![
                    secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                    secondary_button("Discard", palette).on_press(Event::Discard)
                ]
                .spacing(12),
            );
        }
        page = page
            .push(row![back].align_y(Center))
            .push(heroic::scroll_body(body, None, palette));
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Review => self.prepared.as_ref().map(|prepared| {
                accent_base("Migrate", palette.primary)
                    .on_press_maybe((prepared.ready() && !self.waiting()).then_some(Event::Migrate))
                    .into()
            }),
            Step::Working => self.token.as_ref().map(|token| {
                let (label, event) = if token.is_canceled() {
                    ("Force stop", Event::Force)
                } else {
                    ("Cancel", Event::Cancel)
                };
                secondary_button(label, palette).on_press(event).into()
            }),
            Step::Finished => Some(
                secondary_button("Back to library", palette)
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
            Step::Games => None,
        };
        if let Some(footer) = footer {
            page = page.push(container(footer).padding(12).width(Fill).style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            }));
        }
        heroic::page_frame(page)
    }

    fn games_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(listing) = &self.listing else {
            return body;
        };
        for issue in &listing.issues {
            body = body.push(text(issue).size(13).color(palette.warning));
        }
        if listing.games.is_empty() {
            return body.push(text("Heroic has no GOG games.").color(palette.muted));
        }
        let rows = listing.games.iter().enumerate().map(|(index, candidate)| {
            let (note, ready) = match &candidate.standing {
                Standing::Ready(_) => (None, true),
                Standing::Already { environment } => {
                    (Some(format!("Already migrated as {environment}.")), false)
                }
                Standing::Failed(reason) => (Some(reason.clone()), false),
            };
            let game = &candidate.game;
            let mut content = column![
                text(&game.title).color(if ready {
                    palette.foreground
                } else {
                    palette.muted
                }),
                text(format!("{} - {}", game.id, platform_label(game.platform)))
                    .size(12)
                    .color(palette.muted)
            ]
            .spacing(2);
            if let Some(note) = note {
                let color = if matches!(candidate.standing, Standing::Failed(_)) {
                    palette.warning
                } else {
                    palette.muted
                };
                content = content.push(text(note).size(12).color(color));
            }
            list_row(content, palette)
                .on_press_maybe((ready && !self.waiting()).then_some(Event::Pick(index)))
        });
        body.push(list_group(rows, palette))
    }

    fn review_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(prepared) = &self.prepared else {
            return body;
        };
        let mut summary = column![crate::ui::section_heading(
            &prepared.migration.game().title,
            palette
        )]
        .spacing(8);
        for (label, value) in review_rows(&prepared.migration) {
            summary = summary.push(
                row![
                    text(label).color(palette.muted).width(120),
                    text(value).width(Fill)
                ]
                .spacing(12),
            );
        }
        if matches!(prepared.migration.target(), Target::Windows { .. }) {
            summary = summary.push(
                row![
                    text("Runner").color(palette.muted).width(120),
                    pick_list(
                        prepared.runners.clone(),
                        (!prepared.runner.is_empty())
                            .then(|| Choice::find(&prepared.runners, &prepared.runner)),
                        |choice| Event::Runner(choice.name),
                    )
                    .placeholder("Choose a runner")
                    .wineo_style(palette),
                ]
                .spacing(12)
                .align_y(Center),
            );
        }
        body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(12).width(1).color(palette.border))
        }));
        match &prepared.suggestion {
            Some(RunnerSuggestion::Copy(copy)) => {
                body = body.push(
                    text(format!(
                        "Heroic has runner {} at {}",
                        copy.name,
                        copy.source.display()
                    ))
                    .color(palette.muted),
                );
                body = body.push(
                    secondary_button("Copy Heroic runner", palette)
                        .on_press_maybe((!self.waiting()).then_some(Event::CopyRunner)),
                );
            }
            Some(RunnerSuggestion::Choose {
                proton,
                copy_blocked,
            }) if prepared.runner.is_empty() => {
                body = body.push(
                    text(format!(
                        "Choose a compatible {} runner",
                        if *proton { "Proton" } else { "Wine" }
                    ))
                    .color(palette.muted),
                );
                if let Some(reason) = copy_blocked {
                    body = body.push(
                        text(format!("Heroic runner cannot be copied: {reason}"))
                            .color(palette.warning),
                    );
                }
            }
            _ => {}
        }
        if matches!(prepared.migration.target(), Target::Windows { .. })
            && prepared.runner.is_empty()
        {
            body = body.push(
                secondary_button("Download latest GE-Proton", palette)
                    .on_press_maybe((!self.waiting()).then_some(Event::DownloadRunner)),
            );
        }
        if let Some(plan) = windows_plan(&prepared.migration) {
            for warning in heroic::review_warnings(&plan.warnings()) {
                body = body.push(text(warning).size(13).color(palette.warning));
            }
        }
        if let Some((component, version)) = &prepared.missing_build {
            body = body.push(
                text(format!("{} isn't installed yet.", component.label()))
                    .size(13)
                    .color(palette.warning),
            );
            body = body.push(
                secondary_button(format!("Download {} build", component.label()), palette)
                    .on_press_maybe(
                        (!self.waiting())
                            .then(|| Event::DownloadBuild(*component, version.clone())),
                    ),
            );
        }
        if let Some(blocking) = &prepared.blocking {
            body = body.push(text(blocking).color(palette.danger));
        }
        body
    }
}

fn windows_plan(migration: &Migration) -> Option<&heroic_import::GamePlan> {
    match migration.target() {
        Target::Windows { plan, .. } => plan.as_ref(),
        _ => None,
    }
}

fn platform_label(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "Linux",
        Platform::Windows => "Windows",
    }
}

/// The review's rows, the same ones `wineo import gog` prints.
fn review_rows(migration: &Migration) -> Vec<(&'static str, String)> {
    let committed = migration.committed();
    let kind = match migration.target() {
        Target::Existing { .. } => "existing",
        Target::Windows { .. } => "new Windows environment",
        Target::Native(_) => "new native environment",
    };
    let environment = migration.environment().unwrap_or("?");
    let version = committed
        .products
        .iter()
        .find(|product| product.id == committed.gog_id)
        .and_then(|product| match committed.track {
            Track::Installer => product.version.clone(),
            Track::Galaxy => product.build.clone().map(|build| format!("build {build}")),
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let mut rows = vec![
        ("Environment", format!("{environment} ({kind})")),
        ("Folder", migration.folder().display().to_string()),
        (
            "Track",
            match committed.track {
                Track::Installer => "installer",
                Track::Galaxy => "galaxy",
            }
            .to_owned(),
        ),
        ("Version", version),
        ("Language", committed.language.clone()),
    ];
    let dlc: Vec<String> = committed
        .products
        .iter()
        .filter(|product| product.id != committed.gog_id)
        .map(|product| product.id.to_string())
        .collect();
    if !dlc.is_empty() {
        rows.push(("DLC", format!("{} ({})", dlc.len(), dlc.join(", "))));
    }
    if let Some(plan) = windows_plan(migration) {
        rows.push((
            "Prefix",
            format!(
                "{} (Heroic's, kept where it is)",
                plan.destination().display()
            ),
        ));
    }
    if let StateChange::Reclaim { journal } = migration.state() {
        rows.push((
            "Folder state",
            if journal {
                "kept from before, with an interrupted update"
            } else {
                "kept from before"
            }
            .to_owned(),
        ));
    }
    rows
}

fn load_listing(paths: &Paths) -> Result<Listing, String> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.to_string());
    }
    let sources = heroic_import::sources(paths, None).map_err(|error| error.to_string())?;
    Ok(listing_from(paths, &sources))
}

/// Every GOG game the Heroic configurations installed, planned so the page
/// can mark the ones already migrated. Planning only reads.
pub(crate) fn listing_from(paths: &Paths, sources: &[HeroicSource]) -> Listing {
    let mut listing = Listing::default();
    for source in sources {
        let found = match source.gog_installed() {
            Ok(found) => found,
            Err(error) => {
                listing
                    .issues
                    .push(format!("Couldn't read Heroic's GOG games: {error:#}"));
                continue;
            }
        };
        listing
            .issues
            .extend(found.issues.into_iter().map(|issue| match issue.field {
                Some(id) => format!("GOG game {id}: {}", issue.message),
                None => format!("Heroic: {}", issue.message),
            }));
        for game in found.games {
            let standing = match migrate::plan(paths, source, &game.id) {
                Ok(Plan::Migrate(migration)) => Standing::Ready(Box::new(migration)),
                Ok(Plan::AlreadyMigrated { environment, .. }) => Standing::Already { environment },
                Ok(Plan::NotInstalled { .. }) => continue,
                Err(error) => Standing::Failed(format!("{error:#}")),
            };
            listing.games.push(Candidate {
                source: source.clone(),
                game,
                standing,
            });
        }
    }
    listing
        .games
        .sort_by_cached_key(|candidate| candidate.game.title.to_lowercase());
    listing
}

/// Settles what the review shows: the runner for a new Windows environment,
/// as Heroic import picks it, and whatever would stop `apply`.
fn prepare(
    paths: &Paths,
    source: HeroicSource,
    mut migration: Box<Migration>,
    runner: Option<String>,
) -> Result<Prepared, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let mut prepared = Prepared {
        source,
        migration: Box::new(Migration::clone(&migration)),
        runners: Vec::new(),
        runner: String::new(),
        suggestion: None,
        missing_build: None,
        blocking: None,
    };
    if let Target::Windows { imported, .. } = migration.target() {
        let imported = imported.clone();
        let (runners, choices) = heroic::runner_choices(paths, &global, true)?;
        let (suggestion, suggested) =
            heroic::suggest(paths, &global, &runners, &choices, &imported)?;
        prepared.runners = choices;
        prepared.suggestion = Some(suggestion);
        prepared.runner = runner.unwrap_or(suggested);
        if !prepared.runner.is_empty()
            && let Err(error) = migration.plan_runner(paths, &global, &prepared.runner, None, None)
        {
            prepared.blocking = Some(format!("{error:#}"));
        }
    }
    // New environments get desktop entries like a Heroic import's. GOG games
    // only migrate into the host root, which is where entries are written.
    let desktop = paths.portable_root().is_none();
    if let Some(plan) = migration.native_plan_mut() {
        plan.set_desktop_enabled(desktop);
    }
    if let Some(plan) = migration.game_plan_mut() {
        plan.set_desktop_enabled(desktop);
        plan.resolve(Policy::Defaults);
        if prepared.blocking.is_none()
            && let Some(finding) = plan.blocking().next()
        {
            prepared.blocking = Some(finding.issue.message.clone());
        }
        match plan.missing_build(paths, &global) {
            Ok(missing) => {
                prepared.missing_build = missing.map(|missing| (missing.component, missing.version))
            }
            Err(error) => {
                prepared.blocking.get_or_insert_with(|| error.to_string());
            }
        }
    }
    if prepared.blocking.is_none() && migration.name_taken(paths) {
        let name = migration.environment().unwrap_or_default();
        prepared.blocking = Some(wineo_core::Error::EnvironmentExists(name.to_owned()).to_string());
    }
    prepared.migration = migration;
    Ok(prepared)
}

/// Migrates, then imports GOG's playtime and art and writes desktop entries.
/// A migration that finished stays done even when canceled at the end; the
/// extras after it stop at the cancel and say what they skipped.
fn run(
    paths: &Paths,
    prepared: &Prepared,
    token: &Token,
    mut progress: impl FnMut(String),
    extras: impl FnOnce(&Paths, &GlobalConfig, &str, u64, &str, &Token) -> Extras,
) -> Result<Report, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let migration = &prepared.migration;
    let title = migration.game().title.clone();
    let outcome = migrate::apply(paths, &global, &prepared.source, migration, token, |_| {})
        .map_err(|error| {
            if token.is_canceled() {
                "Import canceled.".to_owned()
            } else {
                format!("{error:#}")
            }
        })?;
    let mut report = Report::default();
    let migrated = match outcome {
        Outcome::AlreadyMigrated {
            environment,
            folder,
        } => {
            report.lines.push(format!(
                "Already migrated. {environment} owns {}.",
                folder.display()
            ));
            report.environment = Some(environment);
            return Ok(report);
        }
        Outcome::Migrated(migrated) => migrated,
    };
    report.environment = Some(migrated.environment.clone());
    if let Some(registered) = &migrated.registered {
        report
            .warnings
            .extend(heroic::playtime_note(&registered.playtime));
    }
    let icon = if token.is_canceled() {
        report
            .warnings
            .push("Skipped GOG playtime and art: canceled.".to_owned());
        false
    } else {
        progress(format!("Adding GOG playtime and art for {title}..."));
        match migration.game().id.parse::<u64>() {
            Ok(gog_id) => {
                let extras = extras(paths, &global, &migrated.environment, gog_id, &title, token);
                report.lines.extend(extras.lines);
                report.warnings.extend(extras.notes);
                extras.icon
            }
            Err(_) => false,
        }
    };
    // Last, so a new icon is in the entries.
    match &migrated.registered {
        Some(registered) => match registered.finish(paths, &global) {
            Ok(changes) => report
                .lines
                .extend(changes.iter().map(crate::settings::desktop_line)),
            Err(error) => report.warnings.push(format!("Desktop entry: {error}")),
        },
        None if icon && wineo_core::desktop::supported(paths).is_ok() => {
            if let Err(error) = wineo_core::desktop::apply(paths, &migrated.environment, false) {
                report
                    .warnings
                    .push(format!("Couldn't update desktop entries: {error:#}"));
            }
        }
        None => {}
    }
    report
        .lines
        .insert(0, format!("Migrated {title} as {}.", migrated.environment));
    if matches!(migrated.state, StateChange::Reclaim { journal: true }) {
        report.lines.push(format!(
            "Its last update was interrupted. Use Finish update on {} in the library.",
            migrated.environment
        ));
    }
    report.lines.push(HEROIC_NOTE.to_owned());
    Ok(report)
}

/// What importing GOG's extras said.
#[derive(Debug, Default)]
struct Extras {
    lines: Vec<String>,
    notes: Vec<String>,
    icon: bool,
}

/// Imports GOG's playtime and art, like `import gog` does after migrating.
/// Every problem is a note; nothing here fails.
fn import_extras(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &str,
    gog_id: u64,
    title: &str,
    token: &Token,
) -> Extras {
    let mut out = Extras::default();
    let keyring = SecretServiceKeyring::for_spec(&wineo_core::gog::account::KEY_SPEC);
    // Playtime needs the login and art doesn't, so a login that can't be read
    // only costs the playtime.
    let mut session = match wineo_core::gog::client_from_env() {
        Ok(client) => match Session::open(paths, &keyring, client) {
            Ok(session) => session,
            Err(error) => {
                out.notes.push(format!("Skipped GOG playtime: {error}"));
                None
            }
        },
        Err(error) => {
            out.notes.push(format!("Skipped GOG playtime: {error}"));
            None
        }
    };
    let transport = match Https::new() {
        Ok(transport) => transport,
        Err(error) => {
            out.notes
                .push(format!("Skipped GOG playtime and art: {error:#}"));
            return out;
        }
    };
    let key = match steamgriddb::find_key(
        paths,
        |name| std::env::var_os(name),
        &SecretServiceKeyring::steamgriddb(),
    ) {
        Ok(key) => key.map(|key| key.value),
        Err(error) => {
            out.notes.push(format!("Skipped SteamGridDB art: {error}"));
            None
        }
    };
    let language = steamgriddb::language(|name| std::env::var_os(name));
    let kinds = global.art.fetch.kinds();
    let request = Request {
        name: environment,
        gog_id,
        title,
        kinds: &kinds,
        transport: &transport,
        steamgriddb_key: key.as_deref(),
        language: &language,
    };
    match extras::import(paths, session.as_mut(), &request, token) {
        Ok(report) => {
            out.lines = extras_lines(&report);
            out.notes.extend(report.notes.iter().cloned());
            out.icon = report.art.iter().any(|filled| filled.kind == Kind::Icon);
        }
        Err(error) => out
            .notes
            .push(format!("Couldn't import GOG playtime and art: {error:#}")),
    }
    out
}

/// What the extras report says happened, in `import gog`'s words.
fn extras_lines(report: &extras::Report) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(playtime) = report.playtime.filter(|playtime| playtime.raised()) {
        lines.push(format!(
            "Imported {} of GOG playtime.",
            duration(playtime.gog_seconds)
        ));
    }
    let gog: fn(&Source) -> bool = |source| matches!(source, Source::Gog(_));
    let grid: fn(&Source) -> bool = |source| matches!(source, Source::SteamGridDb(_));
    for (name, wanted) in [("GOG", gog), ("SteamGridDB", grid)] {
        let kinds: Vec<&str> = [Kind::Grid, Kind::Hero, Kind::Logo, Kind::Icon]
            .into_iter()
            .filter(|kind| {
                report
                    .art
                    .iter()
                    .any(|filled| filled.kind == *kind && wanted(&filled.source))
            })
            .map(|kind| match kind {
                Kind::Grid => "poster",
                other => other.as_str(),
            })
            .collect();
        if !kinds.is_empty() {
            lines.push(format!("Added {name} art: {}.", kinds.join(", ")));
        }
    }
    lines
}

/// Whole hours, or minutes for under an hour.
fn duration(seconds: i64) -> String {
    if seconds >= 3600 {
        format!("{} h", (seconds + 1800) / 3600)
    } else {
        format!("{} min", seconds / 60)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use wineo_core::gog::extras::{Filled, Playtime};

    use super::*;

    const RIMWORLD: &str = "1094900565";
    const STARDEW: &str = "1453375253";
    const DISCO: &str = "1771589310";

    struct Fixture {
        root: tempfile::TempDir,
        paths: Paths,
        source: HeroicSource,
        installed: Vec<serde_json::Value>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().to_owned();
            let paths =
                Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                    .unwrap();
            let heroic = root.path().join("heroic");
            for dir in ["GamesConfig", "gog_store", "store_cache"] {
                fs::create_dir_all(heroic.join(dir)).unwrap();
            }
            fs::write(
                heroic.join("config.json"),
                r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
            )
            .unwrap();
            fs::write(
                heroic.join("store_cache/gog_library.json"),
                format!(
                    r#"{{"games":[{{"runner":"gog","app_name":"{RIMWORLD}","title":"RimWorld"}},{{"runner":"gog","app_name":"{STARDEW}","title":"Stardew Valley"}},{{"runner":"gog","app_name":"{DISCO}","title":"Disco Elysium"}}],"__timestamp":1}}"#
                ),
            )
            .unwrap();
            let source = HeroicSource::from_config_dir_with_home(heroic, root.path().to_owned());
            let fixture = Self {
                root,
                paths,
                source,
                installed: Vec::new(),
            };
            fixture.write_installed();
            fixture
        }

        fn write_installed(&self) {
            fs::write(
                self.source.config_dir().join("gog_store/installed.json"),
                serde_json::json!({ "installed": self.installed }).to_string(),
            )
            .unwrap();
        }

        fn install(&mut self, entry: serde_json::Value) {
            self.installed.push(entry);
            self.write_installed();
        }

        /// A native game, its `gameinfo` naming `id`.
        fn native(&mut self, id: &str, title: &str, dlc: &[u64]) -> PathBuf {
            let dir = self.root.path().join("Games").join(title);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("gameinfo"),
                format!("{title}\n1.6.4871 rev588\n93132\nen-US\n{id}\n{id}\n59819264299049740\n"),
            )
            .unwrap();
            let start = dir.join("start.sh");
            fs::write(&start, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&start, fs::Permissions::from_mode(0o755)).unwrap();
            self.install(serde_json::json!({
                "platform": "linux", "executable": "", "install_path": dir,
                "install_size": "5.03 GiB", "is_dlc": false, "version": "1.6.4871 rev588",
                "appName": id, "installedDLCs": dlc,
                "language": "en", "versionEtag": "", "buildId": "", "pinnedVersion": false,
            }));
            dir
        }

        fn disco(&mut self) {
            let dir = self.root.path().join("Games/Disco Elysium");
            fs::create_dir_all(dir.join("bin")).unwrap();
            fs::write(dir.join("Disco Elysium.exe"), "game").unwrap();
            fs::write(
                dir.join(format!("goggame-{DISCO}.info")),
                serde_json::json!({
                    "gameId": DISCO, "rootGameId": DISCO, "buildId": "59781086834418085",
                    "playTasks": [{
                        "isPrimary": true, "type": "FileTask", "path": "Disco Elysium.exe",
                        "workingDir": "bin", "category": "game",
                    }],
                })
                .to_string(),
            )
            .unwrap();
            let prefix = self.root.path().join("Prefixes/Disco");
            fs::create_dir_all(prefix.join("drive_c")).unwrap();
            fs::write(
                prefix.join("system.reg"),
                "WINE REGISTRY Version 2\n#arch=win64\n",
            )
            .unwrap();
            fs::write(
                self.source
                    .config_dir()
                    .join(format!("GamesConfig/{DISCO}.json")),
                serde_json::json!({
                    DISCO: {
                        "winePrefix": prefix,
                        "wineVersion": {"type": "wine", "name": "System", "bin": "wine"},
                    }
                })
                .to_string(),
            )
            .unwrap();
            self.install(serde_json::json!({
                "platform": "windows", "executable": "", "install_path": dir,
                "install_size": "9.57 GiB", "is_dlc": false, "version": "",
                "appName": DISCO, "installedDLCs": [], "language": "en-US",
                "versionEtag": "", "buildId": "59781086834418085", "pinnedVersion": false,
            }));
        }

        fn listing(&self) -> Listing {
            listing_from(&self.paths, std::slice::from_ref(&self.source))
        }

        fn prepared(&self, id: &str) -> Prepared {
            let candidate = self
                .listing()
                .games
                .into_iter()
                .find(|candidate| candidate.game.id == id)
                .unwrap();
            let Standing::Ready(migration) = candidate.standing else {
                panic!("{id} isn't ready");
            };
            prepare(&self.paths, candidate.source, migration, None).unwrap()
        }

        fn page(&self) -> Page {
            let (mut page, _) = Page::new(self.paths.clone());
            let generation = page.generation;
            let _ = page.update(Event::Listed(generation, Ok(self.listing())));
            page
        }
    }

    fn no_extras(_: &Paths, _: &GlobalConfig, _: &str, _: u64, _: &str, _: &Token) -> Extras {
        Extras::default()
    }

    fn titles(listing: &Listing) -> Vec<&str> {
        listing
            .games
            .iter()
            .map(|candidate| candidate.game.title.as_str())
            .collect()
    }

    #[test]
    fn lists_heroic_games_with_migrated_ones_marked_and_bad_entries_as_issues() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[1233017772]);
        fx.native(STARDEW, "Stardew Valley", &[]);
        fx.install(serde_json::json!({"platform": "osx", "install_path": "/y", "appName": "5"}));
        let stardew = fx.prepared(STARDEW);
        let report = run(&fx.paths, &stardew, &Token::new(), |_| {}, no_extras).unwrap();
        assert_eq!(report.environment.as_deref(), Some("stardew-valley"));

        let listing = fx.listing();
        assert_eq!(titles(&listing), ["RimWorld", "Stardew Valley"]);
        assert!(matches!(listing.games[0].standing, Standing::Ready(_)));
        assert!(matches!(
            &listing.games[1].standing,
            Standing::Already { environment } if environment == "stardew-valley"
        ));
        assert_eq!(listing.issues, ["GOG game 5: osx games can't be migrated"]);

        let mut page = fx.page();
        // A migrated game can't be picked again.
        let _ = page.update(Event::Pick(1));
        assert_eq!(page.picked, None);
        let _ = page.update(Event::Pick(0));
        assert_eq!(page.picked, Some(0));
        assert_eq!(page.status, "Checking RimWorld...");
        crate::layout_probe::layout(
            &mut page.view(Palette::DARK, 0.0),
            iced::Size::new(680.0, 560.0),
        );
    }

    #[test]
    fn the_review_says_what_migrating_does() {
        let mut fx = Fixture::new();
        let folder = fx.native(RIMWORLD, "RimWorld", &[1233017772, 1827857764]);
        let prepared = fx.prepared(RIMWORLD);
        assert!(prepared.ready(), "{:?}", prepared.blocking);
        let rows = review_rows(&prepared.migration);
        let canonical = folder.canonicalize().unwrap();
        assert_eq!(
            rows,
            [
                (
                    "Environment",
                    "rimworld (new native environment)".to_owned()
                ),
                ("Folder", canonical.display().to_string()),
                ("Track", "installer".to_owned()),
                ("Version", "1.6.4871 rev588".to_owned()),
                ("Language", "en".to_owned()),
                ("DLC", "2 (1233017772, 1827857764)".to_owned()),
            ]
        );
    }

    #[test]
    fn a_windows_game_reviews_with_a_runner_choice() {
        let mut fx = Fixture::new();
        fx.disco();
        let prepared = fx.prepared(DISCO);
        assert!(matches!(
            prepared.migration.target(),
            Target::Windows { .. }
        ));
        assert!(prepared.suggestion.is_some());
        let rows = review_rows(&prepared.migration);
        assert_eq!(rows[2], ("Track", "galaxy".to_owned()));
        assert_eq!(rows[3], ("Version", "build 59781086834418085".to_owned()));
        // Without a runner the game can't be migrated yet.
        if prepared.runner.is_empty() {
            assert!(!prepared.ready());
        }
        let mut page = fx.page();
        page.picked = Some(0);
        page.prepared = Some(prepared);
        page.step = Step::Review;
        for (width, height) in [(280.0, 240.0), (680.0, 560.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view(Palette::DARK, 0.0);
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            let spilled = probe.outside(inset);
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        }
    }

    #[test]
    fn migrating_reports_like_the_cli_and_adds_the_extras() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let prepared = fx.prepared(RIMWORLD);
        let mut said = Vec::new();
        let report = run(
            &fx.paths,
            &prepared,
            &Token::new(),
            |line| said.push(line),
            |_, _, name, id, title, _| {
                assert_eq!((name, id, title), ("rimworld", 1094900565, "RimWorld"));
                Extras {
                    lines: vec!["Imported 41 h of GOG playtime.".to_owned()],
                    notes: vec!["Skipped SteamGridDB art: no key".to_owned()],
                    icon: false,
                }
            },
        )
        .unwrap();
        assert_eq!(said, ["Adding GOG playtime and art for RimWorld..."]);
        assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
        assert!(
            report
                .lines
                .contains(&"Imported 41 h of GOG playtime.".to_owned())
        );
        assert_eq!(report.lines.last().unwrap(), HEROIC_NOTE);
        assert_eq!(report.warnings, ["Skipped SteamGridDB art: no key"]);
        let config = fs::read_to_string(fx.paths.environment_config("rimworld")).unwrap();
        assert!(config.contains(RIMWORLD), "{config}");
    }

    /// A cancel that comes after the migration committed, while GOG's
    /// playtime and art are being fetched, reaches that work through the
    /// page's token: it stops, and the migration stays done.
    #[test]
    fn a_cancel_during_the_extras_stops_them_and_keeps_the_migration() {
        use std::sync::mpsc;
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let prepared = fx.prepared(RIMWORLD);
        let token = Token::new();
        let (started, extras_started) = mpsc::channel();
        let report = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                run(
                    &fx.paths,
                    &prepared,
                    &token,
                    |_| {},
                    |paths, _, name, _, _, extras_token| {
                        // The migration is committed before the extras start.
                        assert!(paths.environment_config(name).exists());
                        started.send(()).unwrap();
                        // Stands in for a slow GOG request that checks the
                        // token, as `extras::import` does between steps.
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(10);
                        while !extras_token.is_canceled() {
                            assert!(std::time::Instant::now() < deadline, "never canceled");
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Extras {
                            notes: vec!["Skipped GOG artwork: canceled.".to_owned()],
                            ..Extras::default()
                        }
                    },
                )
            });
            extras_started.recv().unwrap();
            token.cancel();
            worker.join().unwrap()
        })
        .unwrap();
        assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
        assert_eq!(report.warnings, ["Skipped GOG artwork: canceled."]);
        assert!(fx.paths.environment_config("rimworld").exists());
    }

    /// Core decides whether a cancel before the migration comes in time. A
    /// native game has nothing to copy, so it finishes and stays done, and
    /// the extras are skipped with a note.
    #[test]
    fn a_cancel_before_a_native_migration_keeps_it_and_skips_the_extras() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let prepared = fx.prepared(RIMWORLD);
        let token = Token::new();
        token.cancel();
        let report = run(
            &fx.paths,
            &prepared,
            &token,
            |_| {},
            |_, _, _, _, _, _| panic!("no extras after a cancel"),
        )
        .unwrap();
        assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
        assert_eq!(report.warnings, ["Skipped GOG playtime and art: canceled."]);
        assert!(fx.paths.environment_config("rimworld").exists());
    }

    #[test]
    fn working_cancels_and_a_failed_migration_returns_to_the_review() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let generation = page.generation;
        let _ = page.update(Event::Pick(0));
        let generation = page.generation.max(generation);
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        assert_eq!(page.step, Step::Review);
        let _ = page.update(Event::Migrate);
        assert_eq!(page.step, Step::Working);
        assert!(page.busy());
        let _ = page.update(Event::Cancel);
        assert!(page.token.as_ref().unwrap().is_canceled());
        assert_eq!(page.status, "Canceling...");
        // Back does nothing while it works.
        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Working);
        let generation = page.generation;
        let _ = page.update(Event::Finished(
            generation,
            Err("Import canceled.".to_owned()),
        ));
        assert!(!page.busy());
        assert_eq!(page.step, Step::Review);
        assert_eq!(page.error.as_deref(), Some("Import canceled."));
    }

    #[test]
    fn closing_the_window_while_migrating_asks_first_then_cancels() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let _ = page.update(Event::Pick(0));
        let generation = page.generation;
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        let _ = page.update(Event::Migrate);
        let mut app = crate::App::boot(fx.paths.clone()).0;
        app.flow = Some(super::super::Page::Gog(Box::new(page)));
        let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt);
        let _ = app.update(crate::Message::CancelAndClose);
        assert!(app.closing);
        let Some(super::super::Page::Gog(page)) = &app.flow else {
            panic!("the page stays until its work ends");
        };
        assert!(page.token.as_ref().unwrap().is_canceled());
    }

    #[test]
    fn leaving_after_picking_a_game_asks_first() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let _ = page.update(Event::Back);
        assert!(matches!(page.take_action(), Some(Action::Choose)));

        let mut page = fx.page();
        let _ = page.update(Event::Pick(0));
        let generation = page.generation;
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Games);
        assert!(page.take_action().is_none());
        let _ = page.update(Event::Back);
        assert!(page.leave_prompt);
        assert!(page.take_action().is_none());
        let _ = page.update(Event::KeepEditing);
        assert!(!page.leave_prompt);
        let _ = page.update(Event::Back);
        let _ = page.update(Event::Discard);
        assert!(matches!(page.take_action(), Some(Action::Choose)));
    }

    #[test]
    fn a_portable_session_says_where_gog_games_live() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(&home)),
            Some(&root.path().join("portable")),
        )
        .unwrap();
        let error = load_listing(&paths).unwrap_err();
        assert_eq!(
            error,
            "GOG games live in the host library. Run this without --portable."
        );
        let (mut page, _) = Page::new(paths);
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Err(error.clone())));
        assert_eq!(page.error.as_deref(), Some(error.as_str()));
        assert!(page.listing.is_none());
    }

    #[test]
    fn extras_read_like_import_gog() {
        let report = extras::Report {
            playtime: Some(Playtime {
                gog_seconds: 41 * 3600 + 600,
                before_seconds: 0,
                after_seconds: 41 * 3600 + 600,
            }),
            art: vec![
                Filled {
                    kind: Kind::Icon,
                    source: Source::Gog(1),
                },
                Filled {
                    kind: Kind::Grid,
                    source: Source::Gog(1),
                },
                Filled {
                    kind: Kind::Logo,
                    source: Source::SteamGridDb(9),
                },
            ],
            notes: Vec::new(),
        };
        assert_eq!(
            extras_lines(&report),
            [
                "Imported 41 h of GOG playtime.",
                "Added GOG art: poster, icon.",
                "Added SteamGridDB art: logo.",
            ]
        );
        assert_eq!(duration(25 * 60), "25 min");
    }
}
