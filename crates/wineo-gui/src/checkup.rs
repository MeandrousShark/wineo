use std::sync::Arc;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{button, checkbox, column, row, scrollable, text};
use iced::{Center, Element, Task};
use wineo_core::cancel::Token;
use wineo_core::validate::{Fix, Level, Report};
use wineo_core::winetricks::WinetricksEvent;

use crate::removal::Change as RunningChange;
use crate::{App, Message, checkup_work, focus_button, spinner};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    Environment(String),
}

impl Scope {
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Environment(name) => name,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Outcome {
    pub(crate) applied: Vec<Fix>,
    pub(crate) failed: Vec<(Fix, String)>,
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
                format!(
                    "Canceled. {} skipped.",
                    plural(self.skipped, "fix", "fixes")
                ),
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
    fixed: usize,
}

impl Results {
    fn accept(&mut self, report: Result<Report, String>) {
        self.checking = false;
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
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Open(String),
    Check(Scope),
    Checked(Scope, u64, Result<Report, String>),
    Toggle(Scope, Fix, bool),
    Fix(Scope),
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
        Event::Open(name) => {
            if app.flow.is_some()
                || app.removal.is_some()
                || app.trash_confirm_open()
                || app.settings.is_some()
                || app.close_prompt
                || !app.cards.iter().any(|card| card.name == name)
            {
                return Task::none();
            }
            app.close_log();
            app.action_menu = None;
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
            let check = if app
                .checkup_change()
                .is_some_and(|change| change.scope == scope)
            {
                Task::none()
            } else {
                app.check(scope, true)
            };
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
            let Some(results) = app.checkups.get_mut(&scope) else {
                return Task::none();
            };
            let fixes = results.selected();
            if results.checking || fixes.is_empty() {
                return Task::none();
            }
            results.generation = results.generation.wrapping_add(1);
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
            }));
            checkup_work::apply(app.change_generation, app.paths.clone(), fixes, token)
        }
        Event::Back(scope) => {
            app.close_log();
            if let Some(results) = app.checkups.get_mut(&scope) {
                results.showing = false;
            }
            unfocus()
        }
        Event::Cancel => {
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
            Task::batch([app.check(change.scope, false), app.reload_after_change()])
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
    if !app.checkup_visible()
        || app.log_view
        || app.section != crate::sidebar::Section::Library
        || app.settings.is_some()
        || app.action_menu.is_some()
    {
        return None;
    }
    match key {
        Key::Named(Named::Escape) if status != iced::event::Status::Captured => {
            let scope = Scope::Environment(app.selected.clone()?);
            Some(update(app, Event::Back(scope)))
        }
        Key::Named(Named::Tab) => {
            let scope = Scope::Environment(app.selected.clone()?);
            Some(
                iced::advanced::widget::operate(focus_button::cycle(
                    &app.checkup_controls(&scope),
                    modifiers.shift(),
                ))
                .discard(),
            )
        }
        _ => None,
    }
}

impl App {
    fn check(&mut self, scope: Scope, clear_outcome: bool) -> Task<Message> {
        let results = self.checkups.entry(scope.clone()).or_default();
        results.generation = results.generation.wrapping_add(1);
        results.checking = true;
        results.error = None;
        if clear_outcome {
            results.outcome = None;
        }
        checkup_work::check(scope, results.generation, self.paths.clone())
    }

    pub(crate) fn checkup_change(&self) -> Option<&Change> {
        match &self.change {
            Some(RunningChange::Checkup(change)) => Some(change),
            _ => None,
        }
    }

    pub(crate) fn cancel_checkup(&self, force: bool) {
        if let Some(change) = self.checkup_change() {
            if force {
                change.token.force();
            } else {
                change.token.cancel();
            }
        }
    }

    pub(crate) fn checkup_force_available(&self) -> bool {
        self.checkup_change()
            .is_some_and(|change| change.install.is_some())
    }

    pub(crate) fn checkup_visible(&self) -> bool {
        self.panel_open
            && self.selected.as_ref().is_some_and(|name| {
                self.checkups
                    .get(&Scope::Environment(name.clone()))
                    .is_some_and(|results| results.showing)
            })
    }

    pub(crate) fn checkup_log_source(&self, name: &str) -> Option<crate::worker::LogSource> {
        let (environment, done) = self.checkup_change()?.install.as_ref()?;
        (environment == name && !done.is_canceled())
            .then(|| crate::worker::LogSource::Install(done.clone()))
    }

    fn checkup_can_fix(&self, results: &Results) -> bool {
        !results.checking && !self.management_busy() && !results.selected().is_empty()
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
            && !running
            && let Some(report) = &results.report
        {
            for (index, problem) in report.problems.iter().enumerate() {
                if problem.fix.is_some() {
                    ids.push(format!("checkup-fix-{index}"));
                }
            }
        }
        if self.checkup_can_fix(results) {
            ids.push("checkup-apply".into());
        }
        ids.push("checkup-check".into());
        ids.push("checkup-back".into());
        if running {
            ids.push("checkup-cancel".into());
            if self
                .checkup_change()
                .is_some_and(|change| change.install.is_some())
            {
                ids.push("checkup-log".into());
            }
        }
        ids
    }

    pub(crate) fn checkup_view(&self, scope: Scope) -> Element<'_, Message> {
        let results = &self.checkups[&scope];
        let palette = self.palette;
        let running = self
            .checkup_change()
            .is_some_and(|change| change.scope == scope);
        let mut body = column![].spacing(10);
        if results.checking {
            body = body.push(
                row![
                    spinner::spinner(palette, self.spinner_angle, 16.0),
                    text("Checking...")
                ]
                .spacing(8)
                .align_y(Center),
            );
        } else if let Some(error) = &results.error {
            body = body.push(text(error).color(palette.danger));
        } else if let Some(report) = &results.report {
            body = body.push(text(summary(report)));
            for (index, problem) in report.problems.iter().enumerate() {
                let (marker, color) = match problem.level {
                    Level::Error => ("x", palette.danger),
                    Level::Warning => ("!", palette.warning),
                };
                body =
                    body.push(row![text(marker).color(color), text(&problem.message)].spacing(8));
                if let Some(fix) = &problem.fix {
                    let checked = results.checked.contains(fix);
                    let event = Event::Toggle(scope.clone(), fix.clone(), !checked);
                    let toggle_scope = scope.clone();
                    let toggle_fix = fix.clone();
                    body = body.push(focus_button::control(
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
                        format!("checkup-fix-{index}"),
                        Message::Checkup(event),
                    ));
                }
            }
        }
        if let Some(outcome) = &results.outcome {
            for (line, level) in outcome.lines(results.fixed) {
                body = body.push(text(line).color(match level {
                    Some(Level::Error) => palette.danger,
                    Some(Level::Warning) => palette.warning,
                    None => palette.foreground,
                }));
            }
        }
        let count = results.selected().len();
        body = body.push(
            row![
                control(
                    format!("Fix {}", plural(count, "problem", "problems")),
                    "checkup-apply",
                    self.checkup_can_fix(results),
                    Event::Fix(scope.clone())
                ),
                control(
                    "Check again".into(),
                    "checkup-check",
                    true,
                    Event::Check(scope.clone())
                ),
                control("Back".into(), "checkup-back", true, Event::Back(scope)),
            ]
            .spacing(8),
        );
        if let Some(change) = &self.change {
            body = body.push(text(change.reason()).size(12).color(palette.muted));
        }
        scrollable(body).height(iced::Fill).into()
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
                spinner::spinner(palette, self.spinner_angle, 16.0),
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
            "checkup-cancel",
            true,
            Event::Cancel,
        )]
        .spacing(8);
        if change.install.is_some() {
            controls = controls.push(focus_button::control(
                button("Install log")
                    .on_press(Message::OpenLog)
                    .padding([6, 12])
                    .into(),
                true,
                "checkup-log",
                Message::OpenLog,
            ));
        }
        body = body.push(controls);
        body.into()
    }
}

fn control(
    label: String,
    id: &'static str,
    enabled: bool,
    event: Event,
) -> Element<'static, Message> {
    let message = Message::Checkup(event);
    focus_button::control(
        button(text(label))
            .on_press_maybe(enabled.then_some(message.clone()))
            .padding([6, 12])
            .into(),
        enabled,
        id,
        message,
    )
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
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
mod tests {
    use super::*;
    use wineo_core::components::Component;
    use wineo_core::validate::{Code, Problem};
    use wineo_core::winetricks::Step;

    fn fixture() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = wineo_core::paths::Paths::from_lookup(move |key| {
            (key == "HOME").then(|| home.as_os_str().to_owned())
        })
        .unwrap();
        let config = paths.environment_config("game");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(config, "kind = 'native'\n").unwrap();
        let mut app = App::boot(paths).0;
        app.cards = crate::library::load_library(&app.paths).unwrap();
        app.refreshing = false;
        app.trash_refreshing = false;
        (root, app)
    }

    fn scope() -> Scope {
        Scope::Environment("game".into())
    }

    fn download() -> Fix {
        Fix::Download {
            component: Component::Dxvk,
            version: Some("missing".into()),
        }
    }

    fn other_fix() -> Fix {
        Fix::RemoveStateDir {
            environment: "old".into(),
        }
    }

    fn problem(code: Code, fix: Option<Fix>) -> Problem {
        Problem {
            environment: Some("game".into()),
            template: None,
            stray: None,
            area: code.area(),
            level: code.level(),
            code,
            message: "the CLI's problem message".into(),
            fix,
        }
    }

    fn report() -> Report {
        Report {
            problems: vec![
                problem(Code::BuildMissing, Some(download())),
                problem(Code::BuildMissing, Some(download())),
                problem(Code::StateDirStray, Some(other_fix())),
            ],
            ..Report::default()
        }
    }

    fn send(app: &mut App, event: Event) {
        let _ = app.update(Message::Checkup(event));
    }

    fn loaded(app: &mut App, report: Report) {
        let generation = app.checkups[&scope()].generation;
        send(app, Event::Checked(scope(), generation, Ok(report)));
    }

    fn open(app: &mut App) {
        send(app, Event::Open("game".into()));
    }

    fn start(app: &mut App) -> u64 {
        open(app);
        loaded(app, report());
        send(app, Event::Fix(scope()));
        app.change_generation
    }

    fn lines(app: &App) -> Vec<(String, Option<Level>)> {
        let results = &app.checkups[&scope()];
        results.outcome.as_ref().unwrap().lines(results.fixed)
    }

    #[test]
    fn results_count_problem_rows_for_shared_fixes_and_use_cli_zero_copy() {
        let (_root, mut app) = fixture();
        let generation = start(&mut app);
        send(
            &mut app,
            Event::Finished(
                generation,
                Ok(Outcome {
                    applied: vec![download()],
                    ..Outcome::default()
                }),
            ),
        );
        loaded(&mut app, Report::default());
        assert_eq!(lines(&app)[0].0, "Fixed 2 problems.");
        let generation = start(&mut app);
        send(
            &mut app,
            Event::Finished(
                generation,
                Ok(Outcome {
                    failed: vec![(download(), "busy".into())],
                    ..Outcome::default()
                }),
            ),
        );
        loaded(&mut app, report());
        assert_eq!(lines(&app)[0].0, "Nothing was fixed.");
    }

    #[test]
    fn menu_opens_panel_results_and_clean_check_has_one_line() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::Context("game".into()));
        open(&mut app);
        assert!(app.action_menu.is_none());
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert!(app.panel_open && app.checkup_visible());
        assert!(app.checkups[&scope()].checking && app.spinning());
        loaded(&mut app, Report::default());
        assert_eq!(
            summary(app.checkups[&scope()].report.as_ref().unwrap()),
            "No problems found."
        );
        assert!(!app.checkup_can_fix(&app.checkups[&scope()]));
        assert!(!app.spinning());
    }

    #[test]
    fn summary_uses_cli_counts_and_messages() {
        let mut report = Report::default();
        report.problems.push(problem(Code::BuildMissing, None));
        report.problems.push(problem(Code::StateDirStray, None));
        assert_eq!(summary(&report), "1 error, 1 warning");
        assert_eq!(report.problems[0].message, "the CLI's problem message");
        report.problems.pop();
        assert_eq!(summary(&report), "1 error");
        report.problems = vec![problem(Code::StateDirStray, None)];
        assert_eq!(summary(&report), "1 warning");
    }

    #[test]
    fn shared_boxes_toggle_together_and_unchecked_fixes_are_not_applied() {
        let (_root, mut app) = fixture();
        open(&mut app);
        loaded(&mut app, report());
        let results = &app.checkups[&scope()];
        assert_eq!(results.selected(), [download(), other_fix()]);
        assert_eq!(
            results.report.as_ref().unwrap().problems[0].fix,
            results.report.as_ref().unwrap().problems[1].fix
        );
        assert_eq!(
            app.checkup_controls(&scope()),
            [
                "checkup-fix-0",
                "checkup-fix-1",
                "checkup-fix-2",
                "checkup-apply",
                "checkup-check",
                "checkup-back"
            ]
        );
        send(&mut app, Event::Toggle(scope(), download(), false));
        assert_eq!(app.checkups[&scope()].selected(), [other_fix()]);
        assert!(!app.checkups[&scope()].checked.contains(&download()));
        let mut applied = Vec::new();
        let outcome = checkup_work::run_fixes(
            &app.checkups[&scope()].selected(),
            &Token::new(),
            |fix, _| {
                applied.push(fix.clone());
                Ok(Vec::new())
            },
        );
        assert_eq!(applied, [other_fix()]);
        assert_eq!(outcome.applied.len(), 1);
        send(&mut app, Event::Toggle(scope(), download(), true));
        let outcome =
            checkup_work::run_fixes(&app.checkups[&scope()].selected(), &Token::new(), |_, _| {
                Ok(Vec::new())
            });
        assert_eq!(outcome.applied, [download(), other_fix()]);
        assert_eq!(outcome.lines(3)[0].0, "Fixed 3 problems.");
    }

    #[test]
    fn failed_checks_show_error_and_check_again_recovers() {
        let (_root, mut app) = fixture();
        open(&mut app);
        let generation = app.checkups[&scope()].generation;
        send(
            &mut app,
            Event::Checked(scope(), generation, Err("storage is unavailable".into())),
        );
        assert_eq!(
            app.checkups[&scope()].error.as_deref(),
            Some("storage is unavailable")
        );
        assert!(app.checkups[&scope()].report.is_none());
        assert!(!app.checkup_can_fix(&app.checkups[&scope()]));
        send(&mut app, Event::Check(scope()));
        assert!(app.checkups[&scope()].checking);
        loaded(&mut app, Report::default());
        assert!(app.checkups[&scope()].error.is_none());
    }

    #[test]
    fn completion_rechecks_and_reloads_both_with_persistent_partial_results() {
        let (_root, mut app) = fixture();
        let generation = start(&mut app);
        let (library, trash, check) = (
            app.library_generation,
            app.trash_generation,
            app.checkups[&scope()].generation,
        );
        let outcome = Outcome {
            applied: vec![download()],
            failed: vec![(other_fix(), "busy".into())],
            warnings: vec!["left something behind".into()],
            canceled: true,
            skipped: 1,
        };
        send(&mut app, Event::Finished(generation, Ok(outcome)));
        assert!(app.change.is_none());
        assert!(app.checkups[&scope()].checking);
        assert!(app.checkups[&scope()].generation > check);
        assert!(app.library_generation > library && app.trash_generation > trash);
        loaded(&mut app, Report::default());
        let previous = lines(&app);
        assert_eq!(
            previous
                .iter()
                .map(|(line, _)| line.as_str())
                .collect::<Vec<_>>(),
            [
                "Fixed 2 problems.",
                "error: Remove old's leftover state failed: busy",
                "warning: left something behind",
                "Canceled. 1 fix skipped."
            ]
        );
        let _ = app.update(Message::Refresh);
        let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
        let _ = app.update(Message::Section(crate::sidebar::Section::Library));
        assert_eq!(lines(&app), previous);
        send(&mut app, Event::Check(scope()));
        assert!(app.checkups[&scope()].outcome.is_none());
    }

    #[test]
    fn another_change_disables_fixes_but_checking_still_works() {
        let (_root, mut app) = fixture();
        app.change = Some(RunningChange::Restore {
            id: "entry".into(),
            name: "other".into(),
        });
        open(&mut app);
        assert!(app.checkups[&scope()].checking);
        loaded(&mut app, report());
        assert!(!app.checkup_can_fix(&app.checkups[&scope()]));
        assert!(
            !app.checkup_controls(&scope())
                .contains(&"checkup-apply".into())
        );
        assert_eq!(
            app.change.as_ref().unwrap().reason(),
            "Busy: restoring other"
        );
        let generation = app.change_generation;
        send(&mut app, Event::Fix(scope()));
        assert_eq!(app.change_generation, generation);
        assert!(matches!(app.change, Some(RunningChange::Restore { .. })));
        send(&mut app, Event::Check(scope()));
        assert!(app.checkups[&scope()].checking);
    }

    #[test]
    fn section_switch_and_panel_close_keep_the_batch_and_results() {
        let (_root, mut app) = fixture();
        let generation = start(&mut app);
        assert_eq!(app.change.as_ref().unwrap().reason(), "Busy: fixing game");
        let token = app.checkup_change().unwrap().token.clone();
        let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
        let _ = app.update(Message::ClosePanel);
        assert!(!token.is_canceled());
        send(
            &mut app,
            Event::Finished(
                generation,
                Ok(Outcome {
                    applied: vec![download()],
                    ..Outcome::default()
                }),
            ),
        );
        loaded(&mut app, Report::default());
        let _ = app.update(Message::Section(crate::sidebar::Section::Library));
        let _ = app.update(Message::Select("game".into()));
        assert!(app.checkup_visible());
        assert_eq!(lines(&app)[0].0, "Fixed 2 problems.");
        let _ = app.handle_key(Key::Named(Named::Escape), Modifiers::empty(), false, false);
        assert!(app.panel_open && !app.checkup_visible());
    }

    #[test]
    fn winetricks_progress_and_log_started_use_existing_live_log_source() {
        let (_root, mut app) = fixture();
        let generation = start(&mut app);
        assert!(app.checkup_log_source("game").is_none());
        send(
            &mut app,
            Event::Winetricks(
                generation,
                Arc::new(WinetricksEvent::Progress(Step {
                    verb: "gdiplus".into(),
                    position: 2,
                    total: 3,
                })),
            ),
        );
        assert_eq!(
            app.checkup_change().unwrap().status,
            "Installing gdiplus (2 of 3)..."
        );
        let done = Token::new();
        send(
            &mut app,
            Event::InstallStarted(generation, "game".into(), done.clone()),
        );
        assert!(matches!(
            app.log_source(&app.cards[0]),
            Some(crate::worker::LogSource::Install(_))
        ));
        assert!(
            app.checkup_controls(&scope())
                .contains(&"checkup-log".into())
        );
        let _ = app.update(Message::OpenLog);
        assert!(app.log_view && app.log_install);
        let _ = app.update(Message::CloseLog);
        assert!(app.checkup_visible());
        send(&mut app, Event::InstallEnded(generation));
        assert!(done.is_canceled());
        assert!(app.checkup_log_source("game").is_none());
        assert!(
            !app.checkup_controls(&scope())
                .contains(&"checkup-log".into())
        );
    }

    #[test]
    fn cancel_and_close_cancels_and_waits_with_force_stop_for_winetricks() {
        let (_root, mut app) = fixture();
        let generation = start(&mut app);
        let token = app.checkup_change().unwrap().token.clone();
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt && !token.is_canceled());
        let _ = app.update(Message::KeepWorking);
        assert!(!app.close_prompt);
        send(
            &mut app,
            Event::InstallStarted(generation, "game".into(), Token::new()),
        );
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        let _ = app.update(Message::CancelAndClose);
        assert!(app.closing && app.foreground_busy() && token.is_canceled());
        assert!(app.checkup_force_available());
        let _ = app.update(Message::ForceCloseWork);
        assert!(token.is_forced());
        assert!(app.change.is_some());
        send(
            &mut app,
            Event::Finished(
                generation,
                Ok(Outcome {
                    canceled: true,
                    skipped: 1,
                    ..Outcome::default()
                }),
            ),
        );
        assert!(!app.foreground_busy());
        assert!(app.checkups[&scope()].checking);
    }

    #[test]
    fn cancel_then_force_stop_use_one_batch_token() {
        let (_root, mut app) = fixture();
        start(&mut app);
        let token = app.checkup_change().unwrap().token.clone();
        send(&mut app, Event::Cancel);
        assert!(token.is_canceled() && !token.is_forced());
        send(&mut app, Event::Cancel);
        assert!(token.is_forced());
    }

    #[test]
    fn stale_check_and_fix_results_never_win() {
        let (_root, mut app) = fixture();
        open(&mut app);
        let old_check = app.checkups[&scope()].generation;
        send(&mut app, Event::Check(scope()));
        send(&mut app, Event::Checked(scope(), old_check, Ok(report())));
        assert!(app.checkups[&scope()].checking && app.checkups[&scope()].report.is_none());
        loaded(&mut app, report());
        send(&mut app, Event::Fix(scope()));
        let generation = app.change_generation;
        send(
            &mut app,
            Event::Progress(generation.wrapping_sub(1), "stale".into()),
        );
        send(
            &mut app,
            Event::Finished(generation.wrapping_sub(1), Ok(Outcome::default())),
        );
        assert!(app.change.is_some());
        assert_ne!(app.checkup_change().unwrap().status, "stale");
        send(&mut app, Event::Check(scope()));
        let concurrent_check = app.checkups[&scope()].generation;
        send(
            &mut app,
            Event::Finished(
                generation,
                Ok(Outcome {
                    applied: vec![download()],
                    ..Outcome::default()
                }),
            ),
        );
        send(
            &mut app,
            Event::Checked(scope(), concurrent_check, Ok(report())),
        );
        assert!(app.checkups[&scope()].checking);
        loaded(&mut app, Report::default());
        send(
            &mut app,
            Event::Finished(generation, Ok(Outcome::default())),
        );
        send(
            &mut app,
            Event::InstallStarted(generation, "game".into(), Token::new()),
        );
        assert!(app.checkup_change().is_none());
        assert_eq!(
            app.checkups[&scope()].outcome.as_ref().unwrap().applied,
            [download()]
        );
        assert!(
            app.checkups[&scope()]
                .report
                .as_ref()
                .unwrap()
                .problems
                .is_empty()
        );
    }
}
