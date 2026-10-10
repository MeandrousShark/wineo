//! Per-environment exec and winetricks pages.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iced::widget::{column, container, row, scrollable, text, text_input};
use iced::{Center, Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::config::{self, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::installer::Output;
use wineo_core::launch::{self, ExecEvent, LaunchEvent};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::winetricks::{self, EnsureEvent, EnsureOutput, WinetricksEvent};

use crate::chips::{self, Chips};
use crate::logview::{self, LogView};
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

const LOG_ID: &str = "tool-output";
const LOG_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Exec,
    Winetricks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Back,
    Settings,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Settings,
    Program(String),
    Browse,
    Picked(u64, Result<Option<PathBuf>, String>),
    Argument(String),
    AddArgument,
    RemoveArgument(usize),
    VariableKey(String),
    VariableValue(String),
    AddVariable,
    RemoveVariable(String),
    Verb(String),
    AddVerb,
    RemoveVerb(usize),
    Extra(String),
    AddExtra,
    RemoveExtra(usize),
    Submit,
    InstallMissing,
    OpenWinetricks,
    Cancel,
    Force,
    Refresh,
    Loaded(u64, Result<Vec<(String, bool)>, String>),
    Progress(u64, String),
    Output(u64, String),
    LogStarted(u64),
    Finished(u64, Result<Option<i32>, Failure>),
    LogAction(iced::widget::text_editor::Action),
    LogScrolled(iced::widget::scrollable::Viewport),
    CopyLog,
    FollowLog,
}

impl Event {
    pub(crate) fn starts_work(&self) -> bool {
        matches!(
            self,
            Self::Submit | Self::InstallMissing | Self::OpenWinetricks
        )
    }

    pub(crate) fn is_input(&self) -> bool {
        !matches!(
            self,
            Self::Picked(..)
                | Self::Loaded(..)
                | Self::Progress(..)
                | Self::Output(..)
                | Self::LogStarted(..)
                | Self::Finished(..)
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum Work {
    Exec,
    Install,
    Run,
    Open,
}

#[derive(Debug, Clone)]
pub(crate) struct Failure {
    canceled: bool,
    message: String,
}

pub(crate) struct Page {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    program: String,
    arguments: Vec<String>,
    argument: String,
    variables: BTreeMap<String, String>,
    variable_key: String,
    variable_value: String,
    verbs: Vec<String>,
    verb: String,
    extras: Vec<String>,
    extra: String,
    declared: Result<Vec<(String, bool)>, String>,
    status: Option<String>,
    result: Option<String>,
    error: Option<String>,
    output: String,
    log: LogView,
    token: Option<Token>,
    generation: u64,
    load_generation: u64,
    picker_generation: u64,
    pending_action: Option<Action>,
    outcome: bool,
}

impl Page {
    pub(crate) fn new(paths: &Paths, name: String, kind: Kind) -> (Self, Task<Event>) {
        let mut page = Self {
            name,
            kind,
            program: String::new(),
            arguments: Vec::new(),
            argument: String::new(),
            variables: BTreeMap::new(),
            variable_key: String::new(),
            variable_value: String::new(),
            verbs: Vec::new(),
            verb: String::new(),
            extras: Vec::new(),
            extra: String::new(),
            declared: Ok(Vec::new()),
            status: None,
            result: None,
            error: None,
            output: String::new(),
            log: LogView::default(),
            token: None,
            generation: 0,
            load_generation: 0,
            picker_generation: 0,
            pending_action: None,
            outcome: false,
        };
        let task = if kind == Kind::Winetricks {
            page.refresh(paths)
        } else {
            Task::none()
        };
        (page, task)
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
    }

    pub(crate) fn blocks_launch(&self) -> bool {
        self.busy() && self.kind == Kind::Winetricks
    }

    pub(crate) fn force_available(&self) -> bool {
        self.token.as_ref().is_some_and(Token::is_canceled)
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = Some("Stopping now...".into());
            } else {
                token.cancel();
                self.status = Some("Stopping...".into());
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.pending_action.take()
    }

    pub(crate) fn take_outcome(&mut self) -> bool {
        std::mem::take(&mut self.outcome)
    }

    pub(crate) fn submit(&mut self, paths: &Paths) -> Task<Event> {
        match self.kind {
            Kind::Exec => self.start(paths, Work::Exec),
            Kind::Winetricks => self.start(paths, Work::Run),
        }
    }

    pub(crate) fn update(&mut self, paths: &Paths, event: Event) -> Task<Event> {
        match event {
            Event::Back => self.pending_action = Some(Action::Back),
            Event::Settings => self.pending_action = Some(Action::Settings),
            Event::Program(value) if !self.busy() => self.program = value,
            Event::Browse if !self.busy() => {
                self.picker_generation = self.picker_generation.wrapping_add(1);
                let generation = self.picker_generation;
                return Task::perform(
                    crate::picker::pick("Choose program".into(), false),
                    move |result| Event::Picked(generation, result),
                );
            }
            Event::Picked(generation, Ok(Some(path)))
                if generation == self.picker_generation && !self.busy() =>
            {
                self.program = path.to_string_lossy().into_owned();
            }
            Event::Picked(generation, Err(error)) if generation == self.picker_generation => {
                self.error = Some(error);
            }
            Event::Argument(value) if !self.busy() => self.argument = value,
            Event::AddArgument if !self.busy() && !self.argument.is_empty() => {
                self.arguments.push(std::mem::take(&mut self.argument));
            }
            Event::RemoveArgument(index) if !self.busy() && index < self.arguments.len() => {
                self.arguments.remove(index);
            }
            Event::VariableKey(value) if !self.busy() => self.variable_key = value,
            Event::VariableValue(value) if !self.busy() => self.variable_value = value,
            Event::AddVariable if !self.busy() => {
                match config::validate_env_key(&self.variable_key, true).and_then(|()| {
                    config::validate_env_var(&self.variable_key, &self.variable_value)
                }) {
                    Ok(()) => {
                        self.variables.insert(
                            std::mem::take(&mut self.variable_key),
                            std::mem::take(&mut self.variable_value),
                        );
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::RemoveVariable(key) if !self.busy() => {
                self.variables.remove(&key);
            }
            Event::Verb(value) if !self.busy() => self.verb = value,
            Event::AddVerb if !self.busy() => match winetricks::validate_verb(&self.verb) {
                Ok(()) => {
                    self.verbs.push(std::mem::take(&mut self.verb));
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            },
            Event::RemoveVerb(index) if !self.busy() && index < self.verbs.len() => {
                self.verbs.remove(index);
            }
            Event::Extra(value) if !self.busy() => self.extra = value,
            Event::AddExtra if !self.busy() && !self.extra.is_empty() => {
                self.extras.push(std::mem::take(&mut self.extra));
            }
            Event::RemoveExtra(index) if !self.busy() && index < self.extras.len() => {
                self.extras.remove(index);
            }
            Event::Submit => return self.submit(paths),
            Event::InstallMissing => return self.start(paths, Work::Install),
            Event::OpenWinetricks => return self.start(paths, Work::Open),
            Event::Cancel => self.cancel(false),
            Event::Force => self.cancel(true),
            Event::Refresh => return self.refresh(paths),
            Event::Loaded(generation, result) if generation == self.load_generation => {
                self.declared = result;
            }
            Event::Progress(generation, status) if generation == self.generation => {
                self.status = Some(status);
            }
            Event::Output(generation, line) if generation == self.generation => {
                self.append_output(&line);
            }
            Event::LogStarted(generation) if generation == self.generation => {
                self.output.clear();
                let _ = self.log.resync("");
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status = None;
                self.outcome = true;
                self.result = Some(match result {
                    Ok(Some(0)) if self.kind == Kind::Exec => "Program exited with code 0. Other programs may still be running in the prefix.".into(),
                    Ok(Some(0)) => "Winetricks finished.".into(),
                    Ok(Some(code)) if self.kind == Kind::Exec => format!("Program exited with code {code}. Other programs may still be running in the prefix."),
                    Ok(Some(code)) => format!("Winetricks exited with code {code}."),
                    Ok(None) => "Missing declared verbs installed.".into(),
                    Err(error) if error.canceled => "Canceled.".into(),
                    Err(error) => format!("Failed: {}", error.message),
                });
                return if self.kind == Kind::Winetricks {
                    self.refresh(paths)
                } else {
                    Task::none()
                };
            }
            Event::LogAction(action) => {
                if self.log.perform(action, || self.output.clone()) {
                    return logview::snap_to_end(LOG_ID);
                }
            }
            Event::LogScrolled(viewport) => {
                let bounds = viewport.absolute_offset();
                let content = viewport.content_bounds();
                let viewport_bounds = viewport.bounds();
                if self
                    .log
                    .scrolled(bounds.y, viewport_bounds.height, content.height, || {
                        self.output.clone()
                    })
                {
                    return logview::snap_to_end(LOG_ID);
                }
            }
            Event::CopyLog => {
                self.log.mark_copied();
                return iced::clipboard::write(self.output.clone());
            }
            Event::FollowLog if self.log.follow(|| self.output.clone()) => {
                return logview::snap_to_end(LOG_ID);
            }
            _ => {}
        }
        Task::none()
    }

    fn append_output(&mut self, line: &str) {
        let added = format!("{line}\n");
        self.output.push_str(&added);
        let mut dropped = String::new();
        if self.output.len() > LOG_LIMIT {
            let mut end = self.output.len() - LOG_LIMIT;
            while !self.output.is_char_boundary(end) {
                end += 1;
            }
            dropped = self.output.drain(..end).collect();
        }
        let _ = self.log.push(&added, &dropped, || self.output.clone());
    }

    fn refresh(&mut self, paths: &Paths) -> Task<Event> {
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        let paths = paths.clone();
        let name = self.name.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || declared(&paths, &name))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|value| value)
            },
            move |result| Event::Loaded(generation, result),
        )
    }

    fn start(&mut self, paths: &Paths, work: Work) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        self.error = None;
        if matches!(work, Work::Exec) {
            if self.program.trim().is_empty() {
                self.error = Some("Choose a program.".into());
                return Task::none();
            }
            if let Err(error) = check_exec(paths, &self.name, &self.program, &self.variables) {
                self.error = Some(error);
                return Task::none();
            }
        }
        if matches!(work, Work::Run) && self.verbs.is_empty() && self.extras.is_empty() {
            self.error = Some("Add a verb or argument.".into());
            return Task::none();
        }
        if matches!(work, Work::Run) && self.kind == Kind::Winetricks {
            for verb in &self.verbs {
                if let Err(error) = winetricks::validate_verb(verb) {
                    self.error = Some(error.to_string());
                    return Task::none();
                }
            }
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let token = Token::new();
        self.token = Some(token.clone());
        self.status = Some("Preparing...".into());
        self.result = None;
        self.output.clear();
        let _ = self.log.resync("");
        let paths = paths.clone();
        let name = self.name.clone();
        let program = self.program.clone();
        let args = self.arguments_for(work);
        let variables = self.variables.clone();
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let result = match work {
                        Work::Exec => launch::exec(
                            &paths,
                            &name,
                            &program,
                            &args,
                            &variables,
                            &token,
                            |event| send_exec(&sender, generation, event),
                        )
                        .map(|status| status.code()),
                        Work::Install | Work::Run | Work::Open => {
                            let log = Runtime::new(&paths, &name).install_log_path();
                            (|| {
                                if let Some(parent) = log.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                match work {
                                    Work::Install => winetricks::ensure_in(
                                        &paths,
                                        &name,
                                        &variables,
                                        &token,
                                        EnsureOutput::Log(&log),
                                        |event| send_ensure(&sender, generation, event),
                                    )
                                    .map(|()| None),
                                    Work::Run | Work::Open => winetricks::run_in(
                                        &paths,
                                        &name,
                                        &variables,
                                        &args,
                                        &token,
                                        Output::Log(&log),
                                        |event| send_ensure(&sender, generation, event),
                                    )
                                    .map(|status| status.code()),
                                    Work::Exec => unreachable!(),
                                }
                            })()
                        }
                    };
                    result.map_err(|error| Failure {
                        canceled: error
                            .downcast_ref::<Error>()
                            .is_some_and(|error| matches!(error, Error::Canceled(_))),
                        message: error.to_string(),
                    })
                })
                .await
                .map_err(|error| Failure {
                    canceled: false,
                    message: error.to_string(),
                })
                .and_then(|value| value)
            },
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), worker])
    }

    fn arguments_for(&self, work: Work) -> Vec<String> {
        match work {
            Work::Exec => self.arguments.clone(),
            Work::Install | Work::Open => Vec::new(),
            Work::Run => self.verbs.iter().chain(&self.extras).cloned().collect(),
        }
    }

    pub(crate) fn view(&self, palette: Palette, blocked: bool) -> Element<'_, Event> {
        let enabled = !blocked && !self.busy();
        let mut body = column![
            row![
                secondary_button("Back", palette).on_press(Event::Back),
                text(match self.kind {
                    Kind::Exec => "Run program",
                    Kind::Winetricks => "Winetricks",
                })
                .size(24)
            ]
            .spacing(16)
            .align_y(Center),
            text(format!("Environment: {}", self.name)).color(palette.muted),
        ]
        .spacing(12)
        .width(Fill);
        match self.kind {
            Kind::Exec => {
                body = body.push(
                    row![
                        text_input("winecfg or /path/to/program.exe", &self.program)
                            .on_input_maybe(enabled.then_some(Event::Program))
                            .on_submit_maybe(enabled.then_some(Event::Submit))
                            .style(move |_, status| chips::input_style(palette, status)),
                        secondary_button("Browse...", palette)
                            .on_press_maybe(enabled.then_some(Event::Browse)),
                    ]
                    .spacing(8),
                );
                body = body.push(text("Arguments (each chip is one exact argument)").size(14));
                body = body.push(
                    Chips {
                        items: &self.arguments,
                        input: &self.argument,
                        placeholder: "Add argument",
                        error: None,
                        on_input: Box::new(Event::Argument),
                        on_add: Event::AddArgument,
                        on_remove: Box::new(Event::RemoveArgument),
                        enabled,
                    }
                    .view(palette),
                );
                let preview = std::iter::once(self.program.as_str())
                    .chain(self.arguments.iter().map(String::as_str))
                    .map(|item| format!("{item:?}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                body = body.push(
                    text(format!("argv: {preview}"))
                        .size(12)
                        .color(palette.muted),
                );
            }
            Kind::Winetricks => {
                body = body.push(text("Declared verbs").size(17));
                match &self.declared {
                    Ok(verbs) if verbs.is_empty() => body = body.push(text("No verbs declared.")),
                    Ok(verbs) => {
                        for (verb, installed) in verbs {
                            body = body.push(text(format!(
                                "{verb}: {}",
                                if *installed { "Installed" } else { "Missing" }
                            )));
                        }
                    }
                    Err(error) => body = body.push(text(error).color(palette.danger)),
                }
                body = body.push(
                    row![
                        secondary_button("Install missing", palette).on_press_maybe(
                            (enabled
                                && self
                                    .declared
                                    .as_ref()
                                    .is_ok_and(|v| v.iter().any(|(_, installed)| !installed)))
                            .then_some(Event::InstallMissing)
                        ),
                        secondary_button("Edit in Settings", palette).on_press(Event::Settings),
                        secondary_button("Refresh", palette).on_press(Event::Refresh),
                    ]
                    .spacing(8),
                );
                body = body.push(text("One-off verbs").size(17));
                body = body.push(
                    Chips {
                        items: &self.verbs,
                        input: &self.verb,
                        placeholder: "Add verb",
                        error: (!self.verb.is_empty())
                            .then(|| winetricks::validate_verb(&self.verb).err())
                            .flatten()
                            .map(|error| error.to_string()),
                        on_input: Box::new(Event::Verb),
                        on_add: Event::AddVerb,
                        on_remove: Box::new(Event::RemoveVerb),
                        enabled,
                    }
                    .view(palette),
                );
                body = body.push(text("Additional arguments").size(14));
                body = body.push(
                    Chips {
                        items: &self.extras,
                        input: &self.extra,
                        placeholder: "Add exact argument",
                        error: None,
                        on_input: Box::new(Event::Extra),
                        on_add: Event::AddExtra,
                        on_remove: Box::new(Event::RemoveExtra),
                        enabled,
                    }
                    .view(palette),
                );
                body = body.push(
                    text("An installer window may open.")
                        .size(12)
                        .color(palette.muted),
                );
            }
        }
        body = body.push(text("Temporary environment variables").size(14));
        for (key, value) in &self.variables {
            body = body.push(
                row![
                    text(format!("{key}={value}")),
                    secondary_button("x", palette)
                        .on_press_maybe(enabled.then(|| Event::RemoveVariable(key.clone())))
                ]
                .spacing(8),
            );
        }
        body = body.push(
            row![
                text_input("KEY", &self.variable_key)
                    .on_input_maybe(enabled.then_some(Event::VariableKey))
                    .style(move |_, status| chips::input_style(palette, status)),
                text_input("VALUE", &self.variable_value)
                    .on_input_maybe(enabled.then_some(Event::VariableValue))
                    .style(move |_, status| chips::input_style(palette, status)),
                secondary_button("Add variable", palette)
                    .on_press_maybe(enabled.then_some(Event::AddVariable)),
            ]
            .spacing(8),
        );
        let mut controls = row![].spacing(8);
        if self.busy() {
            controls = controls.push(secondary_button("Cancel", palette).on_press(Event::Cancel));
            if self.force_available() {
                controls =
                    controls.push(secondary_button("Force stop", palette).on_press(Event::Force));
            }
        } else {
            controls = controls.push(
                accent_base(
                    match self.kind {
                        Kind::Exec => "Run",
                        Kind::Winetricks => "Run verbs",
                    },
                    palette.primary,
                )
                .on_press_maybe(enabled.then_some(Event::Submit)),
            );
            if self.kind == Kind::Winetricks {
                controls = controls.push(
                    secondary_button("Open winetricks", palette)
                        .on_press_maybe(enabled.then_some(Event::OpenWinetricks)),
                );
            }
        }
        body = body.push(controls);
        if let Some(status) = &self.status {
            body = body.push(text(status));
        }
        if let Some(result) = &self.result {
            body = body.push(text(result));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        if self.busy() || !self.output.is_empty() {
            body = body.push(text("Output").size(17));
            body = body.push(logview::controls(
                &self.log,
                !self.output.is_empty(),
                Event::CopyLog,
                Event::FollowLog,
                palette,
            ));
            body = body.push(logview::editor(
                &self.log,
                palette,
                240,
                LOG_ID,
                Event::LogAction,
                Event::LogScrolled,
            ));
        }
        body = body.push(
            text("Press Enter to run it, Esc to go back.")
                .size(12)
                .color(palette.muted),
        );
        container(scrollable(body).height(Fill))
            .padding(20)
            .width(Fill)
            .height(Fill)
            .into()
    }
}

fn check_exec(
    paths: &Paths,
    name: &str,
    program: &str,
    variables: &BTreeMap<String, String>,
) -> Result<(), String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    let runner = Runners::new(paths, &global)
        .map_err(|error| error.to_string())?
        .resolve(
            &environment
                .wine()
                .map_err(|error| error.to_string())?
                .runner,
        )
        .map_err(|error| error.to_string())?;
    launch::Launch::check_exec_with_resolution(
        paths,
        &global,
        &environment,
        &runner,
        program,
        variables,
        launch::ExecResolution::ExplicitPaths,
    )
    .map_err(|error| error.to_string())
}

fn declared(paths: &Paths, name: &str) -> Result<Vec<(String, bool)>, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    let runner = Runners::new(paths, &global)
        .map_err(|error| error.to_string())?
        .resolve(
            &environment
                .wine()
                .map_err(|error| error.to_string())?
                .runner,
        )
        .map_err(|error| error.to_string())?;
    let prefix = environment
        .prefix(paths)
        .map_err(|error| error.to_string())?;
    let installed =
        winetricks::installed(&runner.wine_prefix(&prefix)).map_err(|error| error.to_string())?;
    Ok(environment
        .config
        .winetricks
        .into_iter()
        .map(|verb| {
            let present = installed.contains(&verb);
            (verb, present)
        })
        .collect())
}

fn send_exec(
    sender: &iced::futures::channel::mpsc::UnboundedSender<Event>,
    generation: u64,
    event: ExecEvent,
) {
    let message = match event {
        ExecEvent::Launch(event) => {
            launch_status(&event).map(|status| Event::Progress(generation, status))
        }
        ExecEvent::Started => Some(Event::Progress(generation, "Running...".into())),
        ExecEvent::OutputLine(line) => Some(Event::Output(generation, line)),
        ExecEvent::Stopping => Some(Event::Progress(generation, "Stopping...".into())),
        ExecEvent::Killing => Some(Event::Progress(generation, "Stopping now...".into())),
        ExecEvent::Exited { .. } => None,
    };
    if let Some(message) = message {
        let _ = sender.unbounded_send(message);
    }
}

fn send_ensure(
    sender: &iced::futures::channel::mpsc::UnboundedSender<Event>,
    generation: u64,
    event: EnsureEvent,
) {
    let message = match event {
        EnsureEvent::Launch(event) => {
            launch_status(&event).map(|status| Event::Progress(generation, status))
        }
        EnsureEvent::Winetricks(event) => match event {
            WinetricksEvent::Installing(verbs) => Some(Event::Progress(
                generation,
                format!("Installing {}...", verbs.join(", ")),
            )),
            WinetricksEvent::OutputLine(line) => Some(Event::Output(generation, line)),
            WinetricksEvent::Progress(step) => Some(Event::Progress(
                generation,
                format!(
                    "Installing {} ({} of {})...",
                    step.verb, step.position, step.total
                ),
            )),
            WinetricksEvent::LogStarted => Some(Event::LogStarted(generation)),
            WinetricksEvent::Stopping => Some(Event::Progress(generation, "Stopping...".into())),
            WinetricksEvent::Killing => Some(Event::Progress(generation, "Stopping now...".into())),
            WinetricksEvent::Warning(note) => {
                Some(Event::Output(generation, format!("Warning: {note}")))
            }
            WinetricksEvent::Done(_) => None,
        },
    };
    if let Some(message) = message {
        let _ = sender.unbounded_send(message);
    }
}

fn launch_status(event: &LaunchEvent) -> Option<String> {
    match event {
        LaunchEvent::BuildingPrefix { .. } => Some("Building prefix...".into()),
        LaunchEvent::Components(_) => Some("Preparing components...".into()),
        LaunchEvent::Desktop(_) => Some("Updating desktop entries...".into()),
        LaunchEvent::Winetricks(_) => Some("Preparing winetricks...".into()),
        LaunchEvent::Starting | LaunchEvent::Started => Some("Starting...".into()),
        LaunchEvent::NoWaylandSession | LaunchEvent::AfterHook(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(kind: Kind) -> (Paths, Page) {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let (page, _) = Page::new(&paths, "notes".into(), kind);
        (paths, page)
    }

    #[test]
    fn chips_keep_exact_arguments_and_open_uses_no_arguments() {
        let (paths, mut exec) = page(Kind::Exec);
        let _ = exec.update(&paths, Event::Argument("two words".into()));
        let _ = exec.update(&paths, Event::AddArgument);
        let _ = exec.update(&paths, Event::Argument("/quiet".into()));
        let _ = exec.update(&paths, Event::AddArgument);
        assert_eq!(exec.arguments_for(Work::Exec), ["two words", "/quiet"]);
        let (paths, mut page) = page(Kind::Winetricks);
        let _ = page.update(&paths, Event::Verb("corefonts".into()));
        let _ = page.update(&paths, Event::AddVerb);
        let _ = page.update(&paths, Event::Extra("-q".into()));
        let _ = page.update(&paths, Event::AddExtra);
        assert_eq!(page.arguments_for(Work::Run), ["corefonts", "-q"]);
        assert!(page.arguments_for(Work::Open).is_empty());
        assert!(page.arguments_for(Work::Install).is_empty());
    }

    #[test]
    fn variable_validation_and_stale_worker_events() {
        let (paths, mut page) = page(Kind::Exec);
        let _ = page.update(&paths, Event::VariableKey("WINEPREFIX".into()));
        let _ = page.update(&paths, Event::VariableValue("/tmp/other".into()));
        let _ = page.update(&paths, Event::AddVariable);
        assert!(page.variables.is_empty());
        assert!(page.error.is_some());
        let _ = page.update(&paths, Event::VariableKey("FAKE_VAR".into()));
        let _ = page.update(&paths, Event::AddVariable);
        assert_eq!(
            page.variables.get("FAKE_VAR").map(String::as_str),
            Some("/tmp/other")
        );
        page.generation = 3;
        let _ = page.update(&paths, Event::Output(2, "old".into()));
        let _ = page.update(&paths, Event::Progress(2, "old".into()));
        assert!(page.output.is_empty());
        assert!(page.status.is_none());
        let _ = page.update(&paths, Event::Output(3, "new".into()));
        assert_eq!(page.output, "new\n");
    }

    #[test]
    fn invalid_run_stays_idle_and_completion_requests_refresh() {
        let (paths, mut exec) = page(Kind::Exec);
        let _ = exec.submit(&paths);
        assert!(!exec.busy());
        assert_eq!(exec.error.as_deref(), Some("Choose a program."));

        let (paths, mut tricks) = page(Kind::Winetricks);
        let _ = tricks.submit(&paths);
        assert!(!tricks.busy());
        assert_eq!(tricks.error.as_deref(), Some("Add a verb or argument."));
        tricks.token = Some(Token::new());
        tricks.generation = 4;
        let _ = tricks.update(&paths, Event::Finished(3, Ok(None)));
        assert!(tricks.busy());
        let _ = tricks.update(
            &paths,
            Event::Finished(
                4,
                Err(Failure {
                    canceled: false,
                    message: "test failure".into(),
                }),
            ),
        );
        assert!(!tricks.busy());
        assert!(tricks.take_outcome());
        assert!(!tricks.take_outcome());
        assert!(tricks.result.as_deref().unwrap().contains("test failure"));
    }
}
