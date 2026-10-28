use std::sync::Arc;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{button, checkbox, column, row, scrollable, text};
use iced::{Center, Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::validate::{Area, Fix, Level, Problem, Report};
use wineo_core::winetricks::WinetricksEvent;

use crate::prefix_facts::{Facts, Live};
use crate::removal::Change as RunningChange;
use crate::theme::Palette;
use crate::ui::{ScrollStyle, secondary_button, section, section_heading};
use crate::{App, Message, checkup_work, focus_button, spinner};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    Global,
    Environment(String),
}

impl Scope {
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Global => "problems",
            Self::Environment(name) => name,
        }
    }

    fn id(&self, control: &str) -> String {
        match self {
            Self::Global => format!("checkup-global-{control}"),
            Self::Environment(name) => format!("checkup-environment-{name}-{control}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Group {
    Global,
    /// `library.toml` as a whole, not one environment's entry.
    Library,
    Template(String),
    Environment(String),
    Leftover(String),
    Panel,
}

impl Group {
    fn of(problem: &Problem) -> Self {
        if let Some(name) = &problem.template {
            Self::Template(name.clone())
        } else if let Some(name) = &problem.environment {
            Self::Environment(name.clone())
        } else if let Some(name) = &problem.stray {
            Self::Leftover(name.clone())
        } else if problem.area == Area::Library {
            Self::Library
        } else {
            Self::Global
        }
    }

    fn environment(&self) -> Option<&str> {
        match self {
            Self::Environment(name) => Some(name),
            _ => None,
        }
    }

    fn heading(&self, paths: &wineo_core::paths::Paths) -> String {
        let shown = |path: std::path::PathBuf| {
            paths
                .contract(&path)
                .unwrap_or_else(|_| path.display().to_string())
        };
        match self {
            Self::Global => shown(paths.global_config()),
            Self::Library => shown(paths.library_file()),
            Self::Template(name) => shown(paths.templates_dir().join(format!("{name}.toml"))),
            Self::Environment(name) | Self::Leftover(name) => name.clone(),
            Self::Panel => String::new(),
        }
    }
}

fn groups<'a>(report: &'a Report, scope: &Scope) -> Vec<(Group, Vec<(usize, &'a Problem)>)> {
    let mut groups: Vec<(Group, Vec<(usize, &Problem)>)> = Vec::new();
    for (index, problem) in report.problems.iter().enumerate() {
        let group = if *scope == Scope::Global {
            Group::of(problem)
        } else {
            Group::Panel
        };
        if let Some((_, problems)) = groups.iter_mut().find(|(key, _)| *key == group) {
            problems.push((index, problem));
        } else {
            groups.push((group, vec![(index, problem)]));
        }
    }
    // Right after the global config, or first without it.
    if let Some(index) = groups
        .iter()
        .position(|(group, _)| *group == Group::Library)
    {
        let library = groups.remove(index);
        let at = groups
            .iter()
            .position(|(group, _)| *group == Group::Global)
            .map_or(0, |global| global + 1);
        groups.insert(at, library);
    }
    groups
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Outcome {
    pub(crate) applied: Vec<Fix>,
    pub(crate) failed: Vec<(Fix, String)>,
    /// What a fix reported doing, like a library repair, one entry per report.
    pub(crate) notes: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) canceled: bool,
    pub(crate) skipped: usize,
}

impl Outcome {
    fn lines(&self, fixed: usize) -> Vec<(String, Option<Level>)> {
        let done = if fixed == 0 {
            "Nothing was fixed.".into()
        } else {
            format!("Fixed {}.", plural(fixed, "problem", "problems"))
        };
        let mut lines = vec![(done, None)];
        lines.extend(
            self.notes
                .iter()
                .flat_map(|note| note.lines())
                .map(|line| (line.to_owned(), None)),
        );
        lines.extend(self.failed.iter().map(|(fix, error)| {
            (
                format!("error: {} failed: {error}", fix.label()),
                Some(Level::Error),
            )
        }));
        lines.extend(
            self.warnings
                .iter()
                .map(|warning| (format!("warning: {warning}"), Some(Level::Warning))),
        );
        if self.canceled {
            lines.push((
                if self.skipped == 0 {
                    "Canceled.".into()
                } else {
                    format!(
                        "Canceled. {} skipped.",
                        plural(self.skipped, "fix", "fixes")
                    )
                },
                None,
            ));
        }
        lines
    }
}

#[derive(Default)]
pub(crate) struct Results {
    generation: u64,
    pub(crate) checking: bool,
    showing: bool,
    report: Option<Report>,
    checked: Vec<Fix>,
    error: Option<String>,
    outcome: Option<Outcome>,
    clear_outcome_on_accept: bool,
    fixed: usize,
    recovery_confirm: Option<Fix>,
}

impl Results {
    fn accept(&mut self, report: Result<Report, String>) {
        self.checking = false;
        if self.clear_outcome_on_accept {
            self.outcome = None;
        }
        match report {
            Ok(report) => {
                self.checked = report.fixes();
                self.report = Some(report);
                self.error = None;
            }
            Err(error) => {
                self.report = None;
                self.checked.clear();
                self.error = Some(error);
            }
        }
    }

    fn selected(&self) -> Vec<Fix> {
        self.report.as_ref().map_or_else(Vec::new, |report| {
            report
                .fixes()
                .into_iter()
                .filter(|fix| self.checked.contains(fix))
                .collect()
        })
    }
}

pub(crate) struct Change {
    pub(crate) scope: Scope,
    token: Token,
    status: String,
    install: Option<(String, Token)>,
    problem_fixes: Vec<Fix>,
    affects_downloads: bool,
    noncancellable: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Open(Scope),
    Environment(String),
    OpenInstallLog(Scope),
    Check(Scope),
    Checked(Scope, u64, Result<Report, String>),
    Toggle(Scope, Fix, bool),
    Fix(Scope),
    ReviewRecovery(Scope, Fix),
    ConfirmRecovery(Scope),
    DismissRecovery(Scope),
    Back(Scope),
    Cancel,
    Progress(u64, String),
    Winetricks(u64, Arc<WinetricksEvent>),
    InstallStarted(u64, String, Token),
    InstallEnded(u64),
    Finished(u64, Result<Outcome, String>),
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Open(Scope::Global) => {
            if app.section != crate::sidebar::Section::Checkup {
                return Task::none();
            }
            if app
                .checkup_change()
                .is_some_and(|change| change.scope == Scope::Global)
            {
                return Task::none();
            }
            app.check(Scope::Global, false)
        }
        Event::Environment(name) => {
            if !app.cards.iter().any(|card| card.name == name) {
                return Task::none();
            }
            let navigation = app.navigate_section(crate::sidebar::Section::Library);
            if app.settings.is_some() {
                if app.pending_section == Some(crate::sidebar::Section::Library) {
                    app.pending_checkup_environment = Some(name);
                }
                return navigation;
            }
            if app.section != crate::sidebar::Section::Library {
                return navigation;
            }
            app.close_panel();
            Task::batch([navigation, app.update(Message::Select(name))])
        }
        Event::OpenInstallLog(scope) => {
            let Some((name, source)) = app.checkup_install_log(&scope) else {
                return Task::none();
            };
            let task = app.open_log_for(name, source);
            app.checkup_log_scope = Some(scope);
            task
        }
        Event::Open(Scope::Environment(name)) => {
            if app.flow.is_some()
                || app.removal.is_some()
                || app.trash_confirm_open()
                || app.downloads_confirm_open()
                || app.settings.is_some()
                || app.close_prompt
                || !app.cards.iter().any(|card| card.name == name)
            {
                return Task::none();
            }
            app.close_log();
            app.action_menu = None;
            if app.selected.as_deref() != Some(&name) {
                app.close_checkup();
            }
            app.selected = Some(name.clone());
            app.panel_open = true;
            app.target = app
                .cards
                .iter()
                .find(|card| card.name == name)
                .and_then(|card| card.default_target.clone());
            app.panel_error = None;
            app.panel_note = None;
            let scope = Scope::Environment(name);
            app.checkups.entry(scope.clone()).or_default().showing = true;
            let check = app.check(scope, true);
            Task::batch([check, app.load_panel_art(), unfocus()])
        }
        Event::Check(scope) => app.check(scope, true),
        Event::Checked(scope, generation, report) => {
            if let Some(results) = app.checkups.get_mut(&scope)
                && generation == results.generation
                && results.checking
            {
                results.accept(report);
            }
            Task::none()
        }
        Event::Toggle(scope, fix, checked) => {
            if !app
                .checkup_change()
                .is_some_and(|change| change.scope == scope)
                && let Some(results) = app.checkups.get_mut(&scope)
                && !results.checking
                && results
                    .report
                    .as_ref()
                    .is_some_and(|report| report.fixes().contains(&fix))
            {
                results.checked.retain(|chosen| chosen != &fix);
                if checked {
                    results.checked.push(fix);
                }
            }
            Task::none()
        }
        Event::Fix(scope) => {
            if app.management_busy() || app.close_prompt {
                return Task::none();
            }
            if app
                .checkups
                .get(&scope)
                .is_some_and(|results| app.checkup_tool_conflict(results))
            {
                return Task::none();
            }
            let Some(results) = app.checkups.get_mut(&scope) else {
                return Task::none();
            };
            let fixes = results.selected();
            if results.checking || fixes.is_empty() {
                return Task::none();
            }
            app.check_generation = app.check_generation.wrapping_add(1);
            results.generation = app.check_generation;
            results.outcome = None;
            let problem_fixes = results
                .report
                .as_ref()
                .unwrap()
                .problems
                .iter()
                .filter_map(|problem| problem.fix.clone())
                .collect();
            let token = Token::new();
            app.change_generation = app.change_generation.wrapping_add(1);
            app.change = Some(RunningChange::Checkup(Change {
                scope,
                token: token.clone(),
                status: "Fixing...".into(),
                install: None,
                problem_fixes,
                affects_downloads: fixes.iter().any(|fix| {
                    matches!(
                        fix,
                        Fix::Download { .. }
                            | Fix::CleanUpLostRun { .. }
                            | Fix::CleanUpInstaller { .. }
                            | Fix::RemoveStateDir { .. }
                            | Fix::RecoverTrash { .. }
                    )
                }),
                noncancellable: false,
            }));
            checkup_work::apply(app.change_generation, app.paths.clone(), fixes, token)
        }
        Event::ReviewRecovery(scope, fix) => {
            if app.management_busy()
                || app.close_prompt
                || !matches!(
                    fix,
                    Fix::RecoverSnapshot { .. } | Fix::DeleteTrashEntry { .. }
                )
            {
                return Task::none();
            }
            if let Fix::RecoverSnapshot { environment } = &fix
                && app.tool_prefix_busy(environment)
            {
                return Task::none();
            }
            if let Some(results) = app.checkups.get_mut(&scope)
                && !results.checking
                && results.report.as_ref().is_some_and(|report| {
                    report
                        .problems
                        .iter()
                        .any(|problem| problem.fix.as_ref() == Some(&fix))
                })
            {
                results.recovery_confirm = Some(fix);
            }
            Task::none()
        }
        Event::DismissRecovery(scope) => {
            if let Some(results) = app.checkups.get_mut(&scope) {
                results.recovery_confirm = None;
            }
            Task::none()
        }
        Event::ConfirmRecovery(scope) => {
            if app.management_busy() || app.close_prompt {
                return Task::none();
            }
            if app
                .checkups
                .get(&scope)
                .and_then(|results| results.recovery_confirm.as_ref())
                .is_some_and(|fix| match fix {
                    Fix::RecoverSnapshot { environment } => app.tool_prefix_busy(environment),
                    Fix::DeleteTrashEntry { .. } => false,
                    _ => true,
                })
            {
                return Task::none();
            }
            let Some(results) = app.checkups.get_mut(&scope) else {
                return Task::none();
            };
            let Some(fix @ (Fix::RecoverSnapshot { .. } | Fix::DeleteTrashEntry { .. })) =
                results.recovery_confirm.take()
            else {
                return Task::none();
            };
            if results.checking
                || !results.report.as_ref().is_some_and(|report| {
                    report
                        .problems
                        .iter()
                        .any(|problem| problem.fix.as_ref() == Some(&fix))
                })
            {
                return Task::none();
            }
            app.check_generation = app.check_generation.wrapping_add(1);
            results.generation = app.check_generation;
            results.outcome = None;
            let token = Token::new();
            app.change_generation = app.change_generation.wrapping_add(1);
            app.change = Some(RunningChange::Checkup(Change {
                scope,
                token,
                status: match fix {
                    Fix::DeleteTrashEntry { .. } => "Deleting trash entry...",
                    _ => "Rolling back restore...",
                }
                .into(),
                install: None,
                problem_fixes: vec![fix.clone()],
                affects_downloads: false,
                noncancellable: true,
            }));
            checkup_work::apply_confirmed(app.change_generation, app.paths.clone(), fix)
        }
        Event::Back(scope) => {
            app.close_log();
            if let Some(results) = app.checkups.get_mut(&scope) {
                results.showing = false;
            }
            unfocus()
        }
        Event::Cancel => {
            if app
                .checkup_change()
                .is_some_and(|change| change.noncancellable)
            {
                return Task::none();
            }
            let force = app
                .checkup_change()
                .is_some_and(|change| change.token.is_canceled());
            app.cancel_checkup(force);
            Task::none()
        }
        Event::Finished(generation, result) => {
            if generation != app.change_generation || app.checkup_change().is_none() {
                return Task::none();
            }
            let Some(RunningChange::Checkup(change)) = app.change.take() else {
                unreachable!()
            };
            if let Some((_, done)) = change.install {
                done.cancel();
            }
            let results = app.checkups.entry(change.scope.clone()).or_default();
            match result {
                Ok(outcome) => {
                    results.fixed = change
                        .problem_fixes
                        .iter()
                        .filter(|fix| outcome.applied.contains(fix))
                        .count();
                    results.outcome = Some(outcome);
                }
                Err(error) => {
                    results.fixed = 0;
                    results.outcome = Some(Outcome {
                        warnings: vec![error],
                        ..Outcome::default()
                    })
                }
            }
            let scopes: Vec<_> = app
                .checkups
                .iter()
                .filter(|(scope, results)| {
                    **scope == change.scope
                        || match scope {
                            // A first check still in flight may have read before this batch.
                            Scope::Global => results.report.is_some() || results.checking,
                            Scope::Environment(_) => results.showing,
                        }
                })
                .map(|(scope, _)| scope.clone())
                .collect();
            let checks = Task::batch(scopes.into_iter().map(|scope| app.check(scope, false)));
            let reload = if change.affects_downloads {
                app.reload_after_uses_change()
            } else {
                app.reload_after_change()
            };
            Task::batch([checks, reload])
        }
        event => {
            let generation = match &event {
                Event::Progress(generation, _)
                | Event::Winetricks(generation, _)
                | Event::InstallStarted(generation, ..)
                | Event::InstallEnded(generation) => *generation,
                _ => return Task::none(),
            };
            if generation != app.change_generation {
                return Task::none();
            }
            let Some(RunningChange::Checkup(change)) = &mut app.change else {
                return Task::none();
            };
            match event {
                Event::Progress(_, status) => change.status = status,
                Event::Winetricks(_, event) => {
                    let status = match event.as_ref() {
                        WinetricksEvent::Stopping => Some("Canceling...".into()),
                        WinetricksEvent::Killing => Some("Force stopping...".into()),
                        event => crate::panel::winetricks_status(event),
                    };
                    if let Some(status) = status {
                        change.status = status;
                    }
                }
                Event::InstallStarted(_, name, done) => change.install = Some((name, done)),
                Event::InstallEnded(_) => {
                    if let Some((_, done)) = change.install.take() {
                        done.cancel();
                    }
                }
                _ => {}
            }
            Task::none()
        }
    }
}

fn unfocus() -> Task<Message> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
}

pub(crate) fn key(
    app: &mut App,
    key: &Key,
    modifiers: Modifiers,
    status: iced::event::Status,
) -> Option<Task<Message>> {
    let scope = app.checkup_key_scope()?;
    match key {
        Key::Named(Named::Escape)
            if matches!(scope, Scope::Environment(_))
                && status != iced::event::Status::Captured =>
        {
            Some(update(app, Event::Back(scope)))
        }
        Key::Named(Named::Tab) => Some(
            iced::advanced::widget::operate(focus_button::cycle(
                &app.checkup_controls(&scope),
                modifiers.shift(),
            ))
            .discard(),
        ),
        _ => None,
    }
}

impl App {
    fn check(&mut self, scope: Scope, clear_outcome: bool) -> Task<Message> {
        // Removed entries can be recreated before an old check replies.
        self.check_generation = self.check_generation.wrapping_add(1);
        let results = self.checkups.entry(scope.clone()).or_default();
        results.generation = self.check_generation;
        results.checking = true;
        results.error = None;
        results.clear_outcome_on_accept = clear_outcome;
        checkup_work::check(scope, results.generation, self.paths.clone())
    }

    pub(crate) fn refresh_feature_checkups(&mut self) -> Task<Message> {
        let mut scopes: Vec<_> = self
            .checkups
            .iter()
            .filter(|(scope, results)| match scope {
                Scope::Global => self.section == crate::sidebar::Section::Checkup,
                Scope::Environment(_) => results.showing,
            })
            .map(|(scope, _)| scope.clone())
            .collect();
        if self.section == crate::sidebar::Section::Checkup && !scopes.contains(&Scope::Global) {
            scopes.push(Scope::Global);
        }
        Task::batch(scopes.into_iter().map(|scope| self.check(scope, false)))
    }

    pub(crate) fn close_checkup(&mut self) {
        for results in self.checkups.values_mut() {
            results.showing = false;
        }
    }

    pub(crate) fn checkup_change(&self) -> Option<&Change> {
        match &self.change {
            Some(RunningChange::Checkup(change)) => Some(change),
            _ => None,
        }
    }

    pub(crate) fn cancel_checkup(&self, force: bool) {
        if let Some(change) = self.checkup_change() {
            if change.noncancellable {
                return;
            }
            if force {
                change.token.force();
            } else {
                change.token.cancel();
            }
        }
    }

    pub(crate) fn checkup_force_available(&self) -> bool {
        self.checkup_change()
            .is_some_and(|change| !change.noncancellable && change.install.is_some())
    }

    pub(crate) fn checkup_scope(&self) -> Option<Scope> {
        match self.section {
            crate::sidebar::Section::Checkup => Some(Scope::Global),
            crate::sidebar::Section::Library if self.panel_open => {
                let scope = Scope::Environment(self.selected.clone()?);
                self.checkups
                    .get(&scope)
                    .is_some_and(|results| results.showing)
                    .then_some(scope)
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn checkup_generation(&self, scope: &Scope) -> u64 {
        self.checkups[scope].generation
    }

    /// Whether the visible Checkup is asking to confirm a rollback.
    #[cfg(test)]
    pub(crate) fn checkup_recovery_open(&self) -> bool {
        self.checkup_screen().3.is_some()
    }

    /// The visible Checkup's scope, whether it is checking or showing a
    /// report, and the environment a rollback confirmation is asking about.
    pub(crate) fn checkup_screen(&self) -> (Option<Scope>, bool, bool, Option<&str>) {
        let scope = self.checkup_scope();
        let results = scope.as_ref().and_then(|scope| self.checkups.get(scope));
        (
            scope.clone(),
            results.is_some_and(|results| results.checking),
            results.is_some_and(|results| results.report.is_some()),
            results
                .and_then(|results| results.recovery_confirm.as_ref())
                .map(|fix| match fix {
                    Fix::RecoverSnapshot { environment } => environment.as_str(),
                    _ => "",
                }),
        )
    }

    pub(crate) fn checkup_visible(&self) -> bool {
        self.checkup_scope().is_some()
    }

    pub(crate) fn checkup_key_scope(&self) -> Option<Scope> {
        let scope = self.checkup_scope()?;
        let log_visible = self.log_view
            && (if scope == Scope::Global {
                self.checkup_log_scope.as_ref() == Some(&scope)
            } else {
                self.panel_log_visible()
            });
        (!log_visible
            && self.settings.is_none()
            && self.art_page.is_none()
            && self.action_menu.is_none()
            && !self.library_menu_open()
            && !self.search_focused
            && self.flow.is_none()
            && self.removal.is_none()
            && !self.trash_confirm_open()
            && !self.downloads_confirm_open()
            && !self.close_prompt)
            .then_some(scope)
    }

    fn checkup_install_log(&self, scope: &Scope) -> Option<(String, crate::worker::LogSource)> {
        let (name, done) = self
            .checkup_change()
            .filter(|change| &change.scope == scope)?
            .install
            .as_ref()?;
        (!done.is_canceled()).then(|| {
            (
                name.clone(),
                crate::worker::LogSource::Install(done.clone()),
            )
        })
    }

    pub(crate) fn checkup_log_source(&self, name: &str) -> Option<crate::worker::LogSource> {
        let (environment, done) = self.checkup_change()?.install.as_ref()?;
        (environment == name && !done.is_canceled())
            .then(|| crate::worker::LogSource::Install(done.clone()))
    }

    fn checkup_can_fix(&self, results: &Results) -> bool {
        self.checkup_can_fix_with(&Live(&self.paths), results)
    }

    fn checkup_can_fix_with(&self, facts: &dyn Facts, results: &Results) -> bool {
        !results.checking
            && !self.management_busy()
            && !self.checkup_tool_conflict_with(facts, results)
            && !results.selected().is_empty()
    }

    fn checkup_tool_conflict(&self, results: &Results) -> bool {
        self.checkup_tool_conflict_with(&Live(&self.paths), results)
    }

    fn checkup_tool_conflict_with(&self, facts: &dyn Facts, results: &Results) -> bool {
        let selected = results.selected();
        results.report.as_ref().is_some_and(|report| {
            report.problems.iter().any(|problem| {
                problem
                    .fix
                    .as_ref()
                    .is_some_and(|fix| selected.contains(fix))
                    && problem
                        .environment
                        .as_deref()
                        .is_some_and(|name| self.tool_prefix_busy_with(facts, name))
            })
        })
    }

    fn checkup_controls(&self, scope: &Scope) -> Vec<String> {
        let Some(results) = self.checkups.get(scope) else {
            return Vec::new();
        };
        let running = self
            .checkup_change()
            .is_some_and(|change| &change.scope == scope);
        let mut ids = Vec::new();
        if !results.checking
            && let Some(report) = &results.report
        {
            for (group_index, (group, problems)) in groups(report, scope).into_iter().enumerate() {
                if let Some(name) = group.environment()
                    && self.cards.iter().any(|card| card.name == name)
                {
                    ids.push(scope.id(&format!("heading-{group_index}")));
                }
                for (index, problem) in problems {
                    if !running && problem.fix.is_some() {
                        ids.push(scope.id(&format!("fix-{index}")));
                    }
                }
            }
        }
        if self.checkup_can_fix(results) {
            ids.push(scope.id("apply"));
        }
        ids.push(scope.id("check"));
        if *scope != Scope::Global {
            ids.push(scope.id("back"));
        }
        if results.recovery_confirm.is_some() {
            ids.push(scope.id("recover-cancel"));
            ids.push(scope.id("recover-confirm"));
        }
        if running
            && !self
                .checkup_change()
                .is_some_and(|change| change.noncancellable)
        {
            ids.push(scope.id("cancel"));
            if self
                .checkup_change()
                .is_some_and(|change| change.install.is_some())
            {
                ids.push(scope.id("log"));
            }
        }
        if self.toast.is_some() {
            ids.push(focus_button::UNDO_ID.into());
        }
        ids
    }

    pub(crate) fn checkup_page(&self) -> Element<'_, Message> {
        let content = if self.log_view && self.checkup_log_scope == Some(Scope::Global) {
            self.log_content()
        } else {
            self.checkup_view(Scope::Global)
        };
        column![content, self.checkup_status(&Scope::Global)]
            .spacing(10)
            .height(iced::Fill)
            .into()
    }

    pub(crate) fn checkup_view(&self, scope: Scope) -> Element<'_, Message> {
        let Some(results) = self.checkups.get(&scope) else {
            return text("Checking...").into();
        };
        let palette = self.palette;
        let running = self
            .checkup_change()
            .is_some_and(|change| change.scope == scope);
        let mut body = column![].spacing(16);
        if let Some(outcome) = &results.outcome {
            for (line, level) in outcome.lines(results.fixed) {
                body = body.push(text(line).color(match level {
                    Some(Level::Error) => palette.danger,
                    Some(Level::Warning) => palette.warning,
                    None => palette.foreground,
                }));
            }
        }
        if results.checking {
            body = body.push(
                row![spinner::spinner(palette, 16.0), text("Checking...")]
                    .spacing(8)
                    .align_y(Center),
            );
        } else if let Some(error) = &results.error {
            body = body.push(section(text(error).color(palette.danger), palette));
        } else if let Some(report) = &results.report {
            body = body.push(section(
                column![
                    section_heading("Summary", palette),
                    text(scope_summary(report, &scope))
                ]
                .spacing(8),
                palette,
            ));
            for (group_index, (group, problems)) in groups(report, &scope).into_iter().enumerate() {
                let mut issue_rows = column![].spacing(8);
                if scope == Scope::Global {
                    let heading = group.heading(&self.paths);
                    issue_rows = if let Some(name) = group.environment() {
                        issue_rows.push(control(
                            heading,
                            scope.id(&format!("heading-{group_index}")),
                            self.cards.iter().any(|card| card.name == name),
                            Event::Environment(name.to_owned()),
                            palette,
                        ))
                    } else {
                        issue_rows.push(text(heading).size(16).color(palette.foreground))
                    };
                }
                for (index, problem) in problems {
                    let (marker, color) = match problem.level {
                        Level::Error => ("x", palette.danger),
                        Level::Warning => ("!", palette.warning),
                    };
                    issue_rows = issue_rows.push(
                        row![
                            text(marker).color(color),
                            text(&problem.message)
                                .width(Fill)
                                .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        ]
                        .spacing(8),
                    );
                    if let Some(fix) = &problem.fix {
                        if let Fix::RecoverSnapshot { environment } = fix {
                            let allowed = !running
                                && !self.management_busy()
                                && !self.tool_prefix_busy_with(&self.facts, environment);
                            issue_rows = issue_rows.push(control(
                                "Roll back restore".into(),
                                scope.id(&format!("fix-{index}")),
                                allowed,
                                Event::ReviewRecovery(scope.clone(), fix.clone()),
                                palette,
                            ));
                            if let Some(reason) = self.tool_reason_with(&self.facts, environment) {
                                issue_rows =
                                    issue_rows.push(text(reason).size(12).color(palette.muted));
                            }
                            continue;
                        }
                        if let Fix::DeleteTrashEntry { .. } = fix {
                            issue_rows = issue_rows.push(control(
                                DELETE_ENTRY.into(),
                                scope.id(&format!("fix-{index}")),
                                !running && !self.management_busy(),
                                Event::ReviewRecovery(scope.clone(), fix.clone()),
                                palette,
                            ));
                            continue;
                        }
                        let checked = results.checked.contains(fix);
                        let event = Event::Toggle(scope.clone(), fix.clone(), !checked);
                        let toggle_scope = scope.clone();
                        let toggle_fix = fix.clone();
                        issue_rows = issue_rows.push(focus_button::control(
                            checkbox(checked)
                                .label(fix.label())
                                .on_toggle_maybe((!running).then_some(move |checked| {
                                    Message::Checkup(Event::Toggle(
                                        toggle_scope.clone(),
                                        toggle_fix.clone(),
                                        checked,
                                    ))
                                }))
                                .into(),
                            !running,
                            scope.id(&format!("fix-{index}")),
                            Message::Checkup(event),
                        ));
                    }
                }
                body = body.push(section(issue_rows, palette));
            }
        }
        let count = results.selected().len();
        if let Some(Fix::RecoverSnapshot { environment }) = &results.recovery_confirm {
            body = body.push(section(column![
                section_heading("Recover interrupted restore", palette),
                text(format!("Roll back the interrupted restore for {environment}?")),
                text("This restores the old prefix and config. The rollback must finish before closing.").color(palette.warning),
                row![
                    control("Cancel".into(), scope.id("recover-cancel"), true, Event::DismissRecovery(scope.clone()), palette),
                    control("Roll back restore".into(), scope.id("recover-confirm"), !self.management_busy() && !self.tool_prefix_busy_with(&self.facts, environment), Event::ConfirmRecovery(scope.clone()), palette),
                ].spacing(8)
            ].spacing(8), palette));
        }
        if let Some(Fix::DeleteTrashEntry { id }) = &results.recovery_confirm {
            body = body.push(section(
                column![
                    section_heading("Delete unreadable trash entry", palette),
                    text(format!("Permanently delete trash entry {id}?")),
                    text("Everything it kept is deleted. This can't be undone.")
                        .color(palette.danger),
                    row![
                        control(
                            "Cancel".into(),
                            scope.id("recover-cancel"),
                            true,
                            Event::DismissRecovery(scope.clone()),
                            palette
                        ),
                        control(
                            DELETE_ENTRY.into(),
                            scope.id("recover-confirm"),
                            !self.management_busy(),
                            Event::ConfirmRecovery(scope.clone()),
                            palette
                        ),
                    ]
                    .spacing(8)
                ]
                .spacing(8),
                palette,
            ));
        }
        let mut controls = row![
            control(
                format!("Fix {}", plural(count, "problem", "problems")),
                scope.id("apply"),
                self.checkup_can_fix_with(&self.facts, results),
                Event::Fix(scope.clone()),
                palette
            ),
            control(
                "Check again".into(),
                scope.id("check"),
                true,
                Event::Check(scope.clone()),
                palette
            ),
        ]
        .spacing(8);
        if scope != Scope::Global {
            controls = controls.push(control(
                "Back".into(),
                scope.id("back"),
                true,
                Event::Back(scope),
                palette,
            ));
        }
        body = body.push(section(controls, palette));
        if let Some(change) = &self.change {
            body = body.push(section(
                text(change.reason()).size(12).color(palette.muted),
                palette,
            ));
        }
        scrollable(body)
            .wineo_style(palette)
            .height(iced::Fill)
            .into()
    }

    pub(crate) fn checkup_status(&self, scope: &Scope) -> Element<'_, Message> {
        let Some(change) = self
            .checkup_change()
            .filter(|change| &change.scope == scope)
        else {
            return column![].into();
        };
        let palette = self.palette;
        let mut body = column![
            row![
                spinner::spinner(palette, 16.0),
                text(&change.status).size(14).color(palette.muted)
            ]
            .spacing(8)
            .align_y(Center)
        ]
        .spacing(8);
        let mut controls = row![control(
            if change.token.is_canceled() {
                "Force stop"
            } else {
                "Cancel"
            }
            .into(),
            scope.id("cancel"),
            !change.noncancellable,
            Event::Cancel,
            palette,
        )]
        .spacing(8);
        if change.install.is_some() {
            let message = Message::Checkup(Event::OpenInstallLog(scope.clone()));
            controls = controls.push(focus_button::control(
                secondary_button("Install log", palette)
                    .on_press(message.clone())
                    .into(),
                true,
                scope.id("log"),
                message,
            ));
        }
        body = body.push(controls);
        body.into()
    }
}

const DELETE_ENTRY: &str = "Delete this entry";

fn control(
    label: String,
    id: String,
    enabled: bool,
    event: Event,
    palette: Palette,
) -> Element<'static, Message> {
    let message = Message::Checkup(event);
    let color = if id.ends_with("-apply") {
        Some(palette.primary)
    } else if label == "Roll back restore" {
        Some(palette.warning)
    } else if label == DELETE_ENTRY {
        Some(palette.danger)
    } else {
        None
    };
    let button = button(text(label).size(14).height(Fill).align_y(Center))
        .height(crate::ui::BUTTON_HEIGHT)
        .padding([0, 12])
        .on_press_maybe(enabled.then_some(message.clone()))
        .style(move |_, status| match color {
            Some(color) => crate::ui::accent_style(color, palette.foreground, status),
            None => crate::ui::secondary_style(palette, status),
        });
    focus_button::control(button.into(), enabled, id, message)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn scope_summary(report: &Report, scope: &Scope) -> String {
    if *scope == Scope::Global && report.problems.is_empty() {
        format!(
            "No problems in {}.",
            plural(report.environments.len(), "environment", "environments")
        )
    } else {
        summary(report)
    }
}

fn summary(report: &Report) -> String {
    if report.problems.is_empty() {
        return "No problems found.".into();
    }
    let mut parts = Vec::new();
    if report.errors() > 0 {
        parts.push(plural(report.errors(), "error", "errors"));
    }
    if report.warnings() > 0 {
        parts.push(plural(report.warnings(), "warning", "warnings"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests;
