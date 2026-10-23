//! The Heroic page's view and the widgets only it draws.

use iced::widget::{checkbox, column, pick_list, row, text, text_input};
use iced::{Center, Element, Fill};
use wineo_core::gamescope::{Filter, GamescopeSetting, Scaler, Window};
use wineo_core::heroic::{
    HeroicGameKind, HeroicListing, HeroicOrigin, HeroicSharedPrefix, SettingsReview,
};
use wineo_core::heroic_import::{Entry, Game, Resolution, RunnerSuggestion, SkipReason};

use crate::chips::Chips;
use crate::flow::frame::{PAGE_MAX_WIDTH, page_frame, scroll_body};
use crate::flow::import_grid::{Tile, import_label};
use crate::runner_label::{self, Choice};
use crate::theme::Palette;
use crate::ui::{
    ListRow, PickListStyle, accent_base, list_group, list_row, list_row_text, secondary_button,
};

use super::{
    BODY_ID, Draft, Event, Page, Review, SCOPE_NUMBER_LABELS, SOURCE_LIST, Step, hook_paths,
    quote_args, review_warnings, source_id,
};

impl Page {
    fn grid<'a>(
        &'a self,
        listing: &'a HeroicListing,
        palette: Palette,
        width: f32,
        header: Element<'a, Event>,
    ) -> Element<'a, Event> {
        let tiles = listing
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                (
                    Tile {
                        title: &candidate.title,
                        poster: self.poster(&candidate.id),
                        reason: self.ineligible(index),
                        note: None,
                        chosen: self.grid.chosen.contains(&candidate.id),
                        focus_ring: self.grid.focus_ring(index),
                        dimmed: false,
                    },
                    Event::Toggle(index),
                )
            });
        self.grid.view(header, tiles, width, palette)
    }

    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        width: f32,
        key_available: bool,
    ) -> Element<'_, Event> {
        let mut body = column![].spacing(14).width(Fill);
        // Pinned under the scrolling body so it stays reachable in a short dialog.
        let mut actions: Option<Element<'_, Event>> = None;
        let title = match self.step {
            Step::Sources => "Import from Heroic".to_owned(),
            Step::Games => "Choose Heroic games".to_owned(),
            Step::Details if self.draft.batch => import_label(self.draft.games.len()),
            Step::Details => "Import details".to_owned(),
            Step::Review => "Review Heroic import".to_owned(),
            Step::Working => "Importing from Heroic".to_owned(),
            Step::Finished => "Heroic import finished".to_owned(),
        };
        body = body.push(text(title).size(26).color(palette.foreground));
        if self.waiting() && !self.status_by_action() {
            body = body.push(self.working(palette));
        } else if !self.status.is_empty() && !self.status_by_action() {
            body = body.push(text(&self.status).color(palette.muted));
        }
        for note in &self.notes {
            body = body.push(text(note).size(13).color(palette.muted));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).size(14).color(palette.danger));
        }
        match self.step {
            Step::Sources => {
                if self.sources.is_empty() {
                    body = body.push(text("No Heroic configuration was found automatically. Choose its config folder.").color(palette.muted));
                }
                if !self.sources.is_empty() {
                    body = body.push(list_group(
                        SOURCE_LIST,
                        self.sources.iter().enumerate().map(|(index, source)| {
                            ListRow::new(
                                source_id(source),
                                list_row(
                                    list_row_text(format!(
                                        "{}: {}",
                                        origin_label(source.origin()),
                                        source.config_dir().display()
                                    )),
                                    palette,
                                ),
                                Event::Source(index),
                            )
                        }),
                        palette,
                    ));
                }
                body = body.push(
                    secondary_button("Browse for Heroic config folder...", palette)
                        .on_press(Event::BrowseSource),
                );
                body = body.push(field(
                    "Config folder path",
                    &self.source_path,
                    Event::SourcePath,
                ));
                body = body
                    .push(secondary_button("Open folder", palette).on_press(Event::OpenSourcePath));
            }
            Step::Games => {
                if let Some(listing) = &self.listing {
                    for issue in &listing.issues {
                        body = body.push(text(&issue.message).size(12).color(palette.warning));
                    }
                    let count = self.grid.chosen.len();
                    let idle = !self.busy();
                    actions = Some(self.grid.footer(
                        idle.then_some(Event::SelectAll),
                        (idle && count > 0).then_some(Event::SelectNone),
                        self.status_by_action().then(|| self.working(palette)),
                        (!self.waiting() && count > 0).then_some(Event::ImportSelected),
                        palette,
                    ));
                }
            }
            Step::Details => {
                if self.offer_shown() {
                    body = body.push(runner_label::offer_card(
                        palette,
                        !self.answering,
                        Event::Offer,
                    ));
                }
                if self.draft.batch {
                    body = body.push(
                        text("Windows games use the selected runner. Native games need no runner.")
                            .color(palette.muted),
                    );
                    body = body.push(field("Windows runner", &self.draft.runner, Event::Runner));
                    if let Some(options) = &self.batch_options {
                        body = body.push(
                            pick_list(
                                options.runners.clone(),
                                Some(Choice::find(&options.runners, &self.draft.runner)),
                                |choice| Event::Runner(choice.name),
                            )
                            .wineo_style(palette),
                        );
                    }
                    if self.draft.runner.is_empty() {
                        body = body.push(
                            secondary_button("Download latest GE-Proton", palette)
                                .on_press(Event::DownloadRunner),
                        );
                    }
                    body = body.push(
                        checkbox(self.draft.adopt)
                            .label("Use Heroic prefixes in place (otherwise copy)")
                            .on_toggle(Event::Adopt),
                    );
                    body = body.push(
                        checkbox(self.draft.desktop)
                            .label("Create desktop entries")
                            .on_toggle(Event::Desktop),
                    );
                    if let Some(options) = &self.batch_options {
                        if options.art_fetch_enabled && key_available {
                            body = body.push(
                                checkbox(self.draft.fetch_batch_art)
                                    .label("Fetch art using the top SteamGridDB match")
                                    .on_toggle(Event::FetchBatchArt),
                            );
                        } else if !options.art_fetch_enabled {
                            body = body.push(
                                text("Automatic artwork is disabled in Global Settings")
                                    .color(palette.muted),
                            );
                        } else {
                            body = body.push(
                                text("SteamGridDB needs a configured key. Manual artwork is available in single-game import.")
                                    .color(palette.muted),
                            );
                        }
                    }
                } else {
                    body = body.push(field("Environment name", &self.draft.name, Event::Name));
                    if self.paths.environment_config(&self.draft.name).exists() {
                        body = body.push(
                            text("An environment with this name already exists.")
                                .color(palette.warning),
                        );
                        body = body.push(
                            secondary_button("Open its settings", palette)
                                .on_press(Event::OpenExisting(self.draft.name.clone())),
                        );
                    }
                    body = body.push(field(
                        "Executable",
                        &self.draft.executable,
                        Event::Executable,
                    ));
                    body = body.push(
                        secondary_button("Browse executable...", palette)
                            .on_press(Event::BrowseExecutable),
                    );
                    body = body.push(text("Arguments").size(13));
                    if let Some(raw) = &self.draft.unparsed_arguments {
                        body = body.push(
                            text(format!("Heroic arguments need correction: {raw}"))
                                .color(palette.warning),
                        );
                    }
                    body = body.push(
                        Chips {
                            items: &self.draft.arguments,
                            input: &self.draft.argument_input,
                            placeholder: "One argument",
                            error: None,
                            on_input: Box::new(Event::ArgumentInput),
                            on_add: Event::ArgumentAdd,
                            on_remove: Box::new(Event::ArgumentRemove),
                            enabled: true,
                        }
                        .view(palette),
                    );
                    body = body.push(
                        secondary_button("No arguments", palette).on_press(Event::ArgumentsClear),
                    );
                    body = body.push(field(
                        "Working directory",
                        &self.draft.working_directory,
                        Event::WorkingDirectory,
                    ));
                    body = body.push(
                        secondary_button("Browse working directory...", palette)
                            .on_press(Event::BrowseWorkingDirectory),
                    );
                    if let Some(prepared) = &self.prepared {
                        for (label, path) in &prepared.hook_paths {
                            body = body.push(text(format!("{label}: {path}")));
                        }
                    }
                    let windows = self
                        .prepared
                        .as_ref()
                        .is_some_and(|prepared| prepared.candidate.kind == HeroicGameKind::Windows);
                    if windows {
                        body = body.push(field("Runner", &self.draft.runner, Event::Runner));
                        if let Some(prepared) = &self.prepared {
                            body = body.push(
                                pick_list(
                                    prepared.runners.clone(),
                                    Some(Choice::find(&prepared.runners, &self.draft.runner)),
                                    |choice| Event::Runner(choice.name),
                                )
                                .wineo_style(palette),
                            );
                            if let Some(RunnerSuggestion::Copy(copy)) = &prepared.suggested {
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
                                        .on_press(Event::CopyRunner),
                                );
                            } else if let Some(RunnerSuggestion::Choose {
                                proton,
                                copy_blocked,
                            }) = &prepared.suggested
                            {
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
                        }
                        if self.draft.runner.is_empty() {
                            body = body.push(
                                secondary_button("Download latest GE-Proton", palette)
                                    .on_press(Event::DownloadRunner),
                            );
                        }
                        body = body.push(
                            checkbox(self.draft.adopt)
                                .label("Use Heroic prefix in place (otherwise copy)")
                                .on_toggle(Event::Adopt),
                        );
                    }
                    body = body.push(
                        checkbox(self.draft.desktop)
                            .label("Create desktop entry")
                            .on_toggle(Event::Desktop),
                    );
                    body = body.push(launch_settings(&self.draft, windows, palette));
                    let art_generation = self.art_generation;
                    body = body.push(
                        self.art
                            .view(palette, key_available)
                            .map(move |event| Event::Art(art_generation, event)),
                    );
                }
                let mut next = row![
                    accent_base("Next: Review", palette.primary)
                        .on_press_maybe((!self.waiting()).then_some(Event::Plan))
                ]
                .spacing(12)
                .align_y(Center);
                if self.status_by_action() {
                    next = next.push(self.working(palette));
                }
                actions = Some(next.wrap().vertical_spacing(8).into());
            }
            Step::Review => {
                if let Some(review) = &self.review {
                    match review {
                        Review::Single {
                            game,
                            missing_build,
                            runner_line,
                        } => {
                            body = body.push(text(format!("{} -> {}", game.title(), game.name())));
                            match game.as_ref() {
                                Game::Windows(plan) => {
                                    body = body.push(runner_line.as_deref().map(text));
                                    body = body.push(text(format!(
                                        "Executable: {}",
                                        plan.resolved().exe.display()
                                    )));
                                    body = body.push(text(format!(
                                        "Arguments: {}",
                                        plan.resolved().args.join(" ")
                                    )));
                                    if let Some(cwd) = &plan.resolved().cwd {
                                        body = body.push(text(format!(
                                            "Working directory: {}",
                                            cwd.display()
                                        )));
                                    }
                                    body = body.push(text(format!(
                                        "Desktop entry: {}",
                                        if plan.desktop().enabled { "yes" } else { "no" }
                                    )));
                                    body = body.push(text(format!(
                                        "Prefix: {} -> {}",
                                        plan.inspection().source_root.display(),
                                        plan.destination().display()
                                    )));
                                    body = body.push(text(if plan.is_adopt() {
                                        "Use in place"
                                    } else {
                                        "Copy prefix"
                                    }));
                                    // Only what stops the import; the rest reads like
                                    // debug output here.
                                    for finding in plan.findings().iter().filter(|finding| {
                                        matches!(
                                            finding.resolution,
                                            Resolution::Blocking | Resolution::NeedsInput(_)
                                        )
                                    }) {
                                        body = body.push(
                                            text(&finding.issue.message)
                                                .size(13)
                                                .color(palette.warning),
                                        );
                                    }
                                    if let Some(note) =
                                        shared_prefix_note(plan.is_adopt(), plan.shared_prefix())
                                    {
                                        body =
                                            body.push(text(note).size(13).color(palette.warning));
                                    }
                                }
                                Game::Native(plan) => {
                                    body = body.push(text(format!(
                                        "Native executable: {}",
                                        plan.resolved().exe.display()
                                    )));
                                    body = body.push(text(format!(
                                        "Arguments: {}",
                                        plan.resolved().args.join(" ")
                                    )));
                                    if let Some(cwd) = plan.working_directory() {
                                        body = body.push(text(format!(
                                            "Working directory: {}",
                                            cwd.display()
                                        )));
                                    }
                                    body = body.push(text(format!(
                                        "Desktop entry: {}",
                                        if plan.desktop().enabled { "yes" } else { "no" }
                                    )));
                                }
                            }
                            let settings = match game.as_ref() {
                                Game::Windows(plan) => plan.settings(),
                                Game::Native(plan) => plan.settings(),
                            };
                            let hook_review = match game.as_ref() {
                                Game::Windows(plan) => &plan.heroic().settings_review,
                                Game::Native(plan) => &plan.heroic().settings_review,
                            };
                            for (label, path) in hook_paths(hook_review) {
                                body = body.push(text(format!("{label}: {path}")));
                            }
                            body = body.push(text(format!(
                                "GameMode: {}, MangoHud: {}",
                                toggle_label(settings.gamemode),
                                toggle_label(settings.mangohud)
                            )));
                            if matches!(game.as_ref(), Game::Windows(_)) {
                                body = body.push(text(format!(
                                    "Wine Wayland: {}",
                                    toggle_label(settings.wayland)
                                )));
                            }
                            body = body.push(text(format!(
                                "Gamescope: {}",
                                match &settings.gamescope {
                                    None => "Use global default".to_owned(),
                                    Some(GamescopeSetting::Off) => "Off".to_owned(),
                                    Some(GamescopeSetting::On(scope)) =>
                                        format!("On {}", quote_args(&scope.args(false))),
                                }
                            )));
                            for (key, value) in settings.env {
                                body = body.push(text(format!("Environment: {key}={value}")));
                            }
                            for warning in review_warnings(&game.warnings()) {
                                body = body.push(text(warning).size(13).color(palette.warning));
                            }
                            if let Some((component, version)) = missing_build {
                                body = body.push(
                                    secondary_button(
                                        format!("Download {} build", component.label()),
                                        palette,
                                    )
                                    .on_press(Event::DownloadBuild(*component, version.clone())),
                                );
                            }
                        }
                        Review::Batch(library) => {
                            body = body.push(text(format!(
                                "{} ready, {} already imported",
                                library.ready_count(),
                                library.already().len()
                            )));
                            for name in library.already() {
                                body = body.push(
                                    text(format!("Already imported: {name}"))
                                        .size(13)
                                        .color(palette.muted),
                                );
                            }
                            for entry in library.entries() {
                                let line = match entry {
                                    Entry::Ready(game) => {
                                        format!(
                                            "Ready: {} -> {} ({})",
                                            game.title(),
                                            game.name(),
                                            if matches!(game, Game::Native(_)) {
                                                "native"
                                            } else if self.draft.adopt {
                                                "use in place"
                                            } else {
                                                "copy"
                                            }
                                        )
                                    }
                                    Entry::Pending(game) => format!(
                                        "Waiting: {} - {}",
                                        game.plan().heroic().title,
                                        game.reason()
                                    ),
                                    Entry::Skipped(game) => {
                                        format!(
                                            "Skipped {}: {}",
                                            game.title,
                                            skip_reason(&game.reason)
                                        )
                                    }
                                };
                                body = body.push(text(line).size(13));
                                if let Entry::Ready(game) = entry {
                                    let hook_review = match game {
                                        Game::Windows(plan) => &plan.heroic().settings_review,
                                        Game::Native(plan) => &plan.heroic().settings_review,
                                    };
                                    if let Some(marker) = hook_marker(hook_review) {
                                        body = body.push(
                                            text(format!("  {marker}"))
                                                .size(12)
                                                .color(palette.muted),
                                        );
                                    }
                                    for warning in review_warnings(&game.warnings()) {
                                        body = body.push(
                                            text(format!("  {warning}"))
                                                .size(12)
                                                .color(palette.warning),
                                        );
                                    }
                                    if let Game::Windows(plan) = game
                                        && let Some(note) = shared_prefix_note(
                                            plan.is_adopt(),
                                            plan.shared_prefix(),
                                        )
                                    {
                                        body = body.push(
                                            text(format!("  {note}"))
                                                .size(12)
                                                .color(palette.warning),
                                        );
                                    }
                                }
                            }
                            for offer in library.pending_builds() {
                                let version =
                                    offer.versions.iter().find_map(|version| version.clone());
                                body = body.push(
                                    secondary_button(
                                        format!(
                                            "Download {} for {} game(s)",
                                            offer.component.label(),
                                            offer.games
                                        ),
                                        palette,
                                    )
                                    .on_press(Event::DownloadBuild(offer.component, version)),
                                );
                            }
                        }
                    }
                    let can_import = match review {
                        Review::Single { missing_build, .. } => missing_build.is_none(),
                        Review::Batch(library) => {
                            library.ready_count() > 0 || !library.backfills().is_empty()
                        }
                    };
                    let mut import =
                        row![accent_base("Import", palette.primary).on_press_maybe(
                            (can_import && !self.waiting()).then_some(Event::Import)
                        )]
                        .spacing(12)
                        .align_y(Center);
                    if self.status_by_action() {
                        import = import.push(self.working(palette));
                    }
                    actions = Some(import.wrap().vertical_spacing(8).into());
                }
            }
            Step::Working => {
                for (name, result) in &self.results {
                    let line = match result {
                        Ok(notes) => format!(
                            "Imported {name}{}",
                            if notes.is_empty() {
                                ""
                            } else {
                                " (with notes)"
                            }
                        ),
                        Err(error) => format!("Failed {name}: {error}"),
                    };
                    body = body.push(text(line).size(13));
                }
                if self.registered.is_some() && self.error.is_some() && self.token.is_none() {
                    actions = Some(
                        row![
                            secondary_button("Retry artwork", palette).on_press(Event::RetryArt),
                            secondary_button("Skip artwork", palette).on_press(Event::SkipArt),
                        ]
                        .spacing(12)
                        .wrap()
                        .vertical_spacing(8)
                        .into(),
                    );
                } else if let Some(token) = &self.token {
                    let (label, event) = if token.is_canceled() {
                        ("Force stop", Event::Force)
                    } else {
                        ("Cancel", Event::Cancel)
                    };
                    actions = Some(secondary_button(label, palette).on_press(event).into());
                }
            }
            Step::Finished => {
                for (name, result) in &self.results {
                    body = body.push(
                        text(match result {
                            Ok(_) => format!("Imported {name}"),
                            Err(error) => format!("Failed {name}: {error}"),
                        })
                        .size(13),
                    );
                    if let Ok(notes) = result {
                        for note in notes {
                            body = body
                                .push(text(format!("  {note}")).size(12).color(palette.warning));
                        }
                    }
                }
                let mut done = row![].spacing(12).align_y(Center);
                if self.registered.is_some() {
                    done = done.push(
                        secondary_button("Open settings", palette).on_press(Event::OpenSettings),
                    );
                }
                done = done.push(
                    secondary_button("Back to library", palette).on_press(Event::BackToLibrary),
                );
                actions = Some(done.wrap().vertical_spacing(8).into());
            }
        }
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let scrolling: Element<'_, Event> = match (&self.listing, self.step) {
            // The title and notes scroll with the grid, above fixed controls, so
            // a short dialog still shows games. Keyboard focus scrolls the
            // grid to a known row.
            (Some(listing), Step::Games) => self.grid(listing, palette, width, body.into()),
            _ => scroll_body(body, Some(BODY_ID), palette),
        };
        let content: Element<'_, Event> = match actions {
            Some(actions) => column![scrolling]
                .push(actions)
                .spacing(14)
                .height(Fill)
                .into(),
            None => scrolling,
        };
        let mut page = column![].spacing(12).max_width(PAGE_MAX_WIDTH).height(Fill);
        if self.leave_prompt {
            page = page.push(text("Discard this Heroic import draft?")).push(
                row![
                    secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                    secondary_button("Discard", palette).on_press(Event::Discard)
                ]
                .spacing(12),
            );
        }
        page_frame(page.push(row![back].align_y(Center)).push(content))
    }
}

fn field<'a>(
    label: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Event + 'a,
) -> Element<'a, Event> {
    column![
        text(label).size(13),
        text_input(label, value).on_input(on_input).padding(8)
    ]
    .spacing(4)
    .into()
}

fn launch_settings(draft: &Draft, windows: bool, palette: Palette) -> Element<'_, Event> {
    let mut body = column![text("Launch settings").size(19)].spacing(9);
    body = body.push(text("Environment variables (KEY=VALUE)").size(13));
    body = body.push(
        Chips {
            items: &draft.environment,
            input: &draft.environment_input,
            placeholder: "KEY=VALUE",
            error: None,
            on_input: Box::new(Event::EnvironmentInput),
            on_add: Event::EnvironmentAdd,
            on_remove: Box::new(Event::EnvironmentRemove),
            enabled: true,
        }
        .view(palette),
    );
    body = body.push(
        secondary_button("Clear environment variables", palette).on_press(Event::EnvironmentClear),
    );
    body = body.push(
        checkbox(draft.gamemode)
            .label("GameMode")
            .on_toggle(Event::Gamemode),
    );
    body = body.push(
        checkbox(draft.mangohud)
            .label("MangoHud")
            .on_toggle(Event::Mangohud),
    );
    if windows {
        body = body.push(
            checkbox(draft.wayland)
                .label("Wine Wayland")
                .on_toggle(Event::Wayland),
        );
        body = body.push(
            checkbox(draft.hdr)
                .label("HDR (requires Wine Wayland)")
                .on_toggle(Event::Hdr),
        );
    }
    let scope_on = matches!(draft.gamescope.setting, Some(GamescopeSetting::On(_)));
    body = body.push(
        checkbox(scope_on)
            .label("Gamescope")
            .on_toggle(Event::GamescopeEnabled),
    );
    if scope_on {
        let scope = draft.gamescope.scope();
        for (index, label) in SCOPE_NUMBER_LABELS.into_iter().enumerate() {
            body = body.push(field(
                label,
                &draft.gamescope.numbers[index],
                move |value| Event::GamescopeNumber(index, value),
            ));
        }
        for (index, (label, selected, choices)) in [
            (
                "Filter",
                scope.filter.map(Filter::as_str),
                Filter::ALL
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
            (
                "Scaler",
                scope.scaler.map(Scaler::as_str),
                Scaler::ALL
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
            (
                "Window",
                scope.window.map(Window::as_str),
                Window::ALL
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut options = vec!["Default".to_owned()];
            options.extend(choices);
            body = body.push(text(label).size(13));
            body = body.push(
                pick_list(
                    options,
                    Some(selected.unwrap_or("Default").to_owned()),
                    move |value| Event::GamescopeChoice(index, value),
                )
                .wineo_style(palette),
            );
        }
        body = body.push(
            checkbox(scope.grab_cursor)
                .label("Grab cursor")
                .on_toggle(Event::GamescopeGrab),
        );
        body = body.push(text("Extra gamescope arguments").size(13));
        body = body.push(
            Chips {
                items: &draft.gamescope.args,
                input: &draft.gamescope.args_input,
                placeholder: "One argument",
                error: None,
                on_input: Box::new(Event::GamescopeArgInput),
                on_add: Event::GamescopeArgAdd,
                on_remove: Box::new(Event::GamescopeArgRemove),
                enabled: true,
            }
            .view(palette),
        );
    }
    body.into()
}

fn toggle_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "On",
        Some(false) => "Off",
        None => "Use global default",
    }
}

/// Why a batch left a game out, as the end of "Skipped Title: ...".
pub(super) fn skip_reason(reason: &SkipReason) -> String {
    match reason {
        SkipReason::LauncherArgs => "Heroic's launch arguments couldn't be parsed".to_owned(),
        SkipReason::NeedsRunner { proton: true } => "needs a Proton runner".to_owned(),
        SkipReason::NeedsRunner { proton: false } => "needs a Wine runner".to_owned(),
        SkipReason::NameTaken(name) => format!("the name {name} is taken"),
        SkipReason::Blocked(message)
        | SkipReason::Unreadable(message)
        | SkipReason::Unavailable(message) => message.clone(),
    }
}

/// Only matters when the prefix is used in place, since then the other games
/// keep changing it; a copy is wineo's own.
pub(super) fn shared_prefix_note(adopt: bool, shared: &[HeroicSharedPrefix]) -> Option<String> {
    if !adopt || shared.is_empty() {
        return None;
    }
    let titles: Vec<&str> = shared.iter().map(|game| game.title.as_str()).collect();
    Some(format!("Shares prefix with: {}", titles.join(", ")))
}

/// How a Heroic config was found, as the Sources page names it.
fn origin_label(origin: HeroicOrigin) -> &'static str {
    match origin {
        HeroicOrigin::Native => "Native",
        HeroicOrigin::Flatpak => "Flatpak",
        HeroicOrigin::Explicit => "Folder",
    }
}

pub(super) fn hook_marker(review: &SettingsReview) -> Option<&'static str> {
    (!review.hooks().is_empty()).then_some("runs launch scripts")
}
