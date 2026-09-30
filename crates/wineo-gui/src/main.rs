mod artwork;
mod chips;
mod flow;
mod grid;
mod keys;
mod library;
mod panel;
mod picker;
mod settings;
mod theme;
mod wheel;
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use iced::widget::{image, scrollable};
use iced::{Settings, Subscription, Task, Theme, keyboard, window};
use wineo_core::cancel::Token;
use wineo_core::paths::Paths;
#[cfg(test)]
use wineo_core::runtime::State;

#[cfg(test)]
use crate::library::playtime_label;
use crate::library::{ArtFile, ArtIdentity, Card, load_library};
use crate::panel::{LogBuffer, PanelError};
use crate::theme::Palette;

pub(crate) const CARD_WIDTH: f32 = 180.0;
const CARD_ROW: f32 = 330.0;
pub(crate) const GAP: f32 = 16.0;
const SCROLL_ID: &str = "library";
const LOG_ID: &str = "panel-log";
const SEARCH_ID: &str = "library-search";
// iced moves 60px per wheel line, which felt too fast beside native apps.
const WHEEL_STEP_SCALE: f32 = 0.7;
type ArtKey = (ArtFile, artwork::Kind);

pub(crate) struct App {
    paths: Paths,
    pub(crate) palette: Palette,
    search: String,
    search_focused: bool,
    cards: Vec<Card>,
    pub(crate) selected: Option<String>,
    panel_open: bool,
    pub(crate) target: Option<String>,
    card_click_target: Option<(String, Option<String>)>,
    pub(crate) hovered: Option<String>,
    pub(crate) images: HashMap<ArtKey, image::Handle>,
    loading: HashSet<ArtKey>,
    failed_images: HashSet<ArtKey>,
    current_art: HashMap<PathBuf, ArtIdentity>,
    error: Option<String>,
    refreshing: bool,
    viewport_width: f32,
    viewport_height: f32,
    scroll_offset: f32,
    row_pitch: f32,
    scroll_visible_until: Option<Instant>,
    pub(crate) operation: Option<Operation>,
    pub(crate) panel_error: Option<PanelError>,
    pub(crate) panel_note: Option<String>,
    pub(crate) log_view: bool,
    pub(crate) log_buffer: LogBuffer,
    log_token: Option<Token>,
    log_generation: u64,
    pub(crate) spinner_frame: usize,
    settings: Option<settings::Page>,
    settings_generation: u64,
    flow: Option<flow::Page>,
    flow_generation: u64,
    action_menu: Option<String>,
    pointer: iced::Point,
    action_menu_position: iced::Point,
    window_size: iced::Size,
    close_prompt: bool,
    closing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    Start,
    Stop,
}

pub(crate) struct Operation {
    name: String,
    kind: OperationKind,
    token: Option<Token>,
    cancelled: bool,
    status: String,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    SearchChanged(String),
    Select(String),
    DoubleClick(String),
    ClosePanel,
    TargetPicked(String),
    Play,
    Stop,
    CleanUp,
    CancelStart,
    LaunchProgress(String, String),
    LaunchNote(String, String),
    StartFinished(String, Result<wineo_core::launch::StartOutcome, PanelError>),
    StopFinished(String, Result<Vec<String>, PanelError>),
    OpenLog,
    CloseLog,
    LogChunk(u64, String),
    LogFinished(u64, Result<(), String>),
    LogScrolled,
    Hover(Option<String>),
    Refresh,
    Loaded(Result<Vec<Card>, String>),
    ArtLoaded(ArtKey, Result<image::Handle, String>),
    Scrolled(scrollable::Viewport),
    ScrollIdle,
    Window(window::Event),
    ThemeChanged(iced::theme::Mode),
    SpinnerTick,
    Key(keyboard::Key, keyboard::Modifiers, iced::event::Status),
    KeyWithFocus(
        keyboard::Key,
        keyboard::Modifiers,
        iced::event::Status,
        bool,
    ),
    SearchFocusChanged(bool),
    OpenSettings,
    OpenGlobalSettings,
    Settings(settings::Event),
    Add,
    CloneEnvironment(String),
    Context(String),
    PointerMoved(iced::Point),
    OverlayInput,
    ContextSettings(String),
    CloseMenu,
    Flow(u64, flow::Event),
    FlowReturned(u64, flow::Action, Result<Vec<Card>, String>),
    KeepWorking,
    CancelAndClose,
    ForceCloseWork,
}

impl App {
    fn boot(paths: Paths) -> (Self, Task<Message>) {
        let app = Self {
            paths,
            palette: Palette::DARK,
            search: String::new(),
            search_focused: false,
            cards: Vec::new(),
            selected: None,
            panel_open: false,
            target: None,
            card_click_target: None,
            hovered: None,
            images: HashMap::new(),
            loading: HashSet::new(),
            failed_images: HashSet::new(),
            current_art: HashMap::new(),
            error: None,
            refreshing: true,
            viewport_width: 1200.0,
            viewport_height: 750.0,
            scroll_offset: 0.0,
            row_pitch: CARD_ROW,
            scroll_visible_until: None,
            operation: None,
            panel_error: None,
            panel_note: None,
            log_view: false,
            log_buffer: LogBuffer::default(),
            log_token: None,
            log_generation: 0,
            spinner_frame: 0,
            settings: None,
            settings_generation: 0,
            flow: None,
            flow_generation: 0,
            action_menu: None,
            pointer: iced::Point::ORIGIN,
            action_menu_position: iced::Point::ORIGIN,
            window_size: iced::Size::new(1200.0, 800.0),
            close_prompt: false,
            closing: false,
        };
        let load = Self::load(app.paths.clone());
        (
            app,
            Task::batch([load, iced::system::theme().map(Message::ThemeChanged)]),
        )
    }
    fn theme(&self) -> Theme {
        self.palette.theme()
    }
    fn load(paths: Paths) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load_library(&paths).map_err(|e| e.to_string()))
                    .await
                    .map_err(|e| e.to_string())?
            },
            Message::Loaded,
        )
    }
    fn subscription(&self) -> Subscription<Message> {
        let idle = if self.scroll_visible_until.is_some() {
            iced::time::every(Duration::from_millis(250)).map(|_| Message::ScrollIdle)
        } else {
            Subscription::none()
        };
        // Every message rebuilds the view, so an always-on tick keeps the app busy while idle.
        let spinner =
            if self.operation.is_some() || self.settings.as_ref().is_some_and(|page| page.saving) {
                iced::time::every(Duration::from_millis(120)).map(|_| Message::SpinnerTick)
            } else {
                Subscription::none()
            };
        Subscription::batch([
            iced::time::every(Duration::from_secs(2)).map(|_| Message::Refresh),
            iced::system::theme_changes().map(Message::ThemeChanged),
            window::events().map(|(_, event)| Message::Window(event)),
            iced::event::listen_with(|event, status, _| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    Some(Message::Key(key, modifiers, status))
                }
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::PointerMoved(position))
                }
                _ => None,
            }),
            spinner,
            idle,
        ])
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        if self.flow.is_some()
            && matches!(
                message,
                Message::SearchChanged(_)
                    | Message::Select(_)
                    | Message::DoubleClick(_)
                    | Message::ClosePanel
                    | Message::TargetPicked(_)
                    | Message::Play
                    | Message::Stop
                    | Message::CleanUp
                    | Message::CancelStart
                    | Message::OpenLog
                    | Message::CloseLog
                    | Message::Context(_)
                    | Message::ContextSettings(_)
                    | Message::OpenSettings
                    | Message::OpenGlobalSettings
                    | Message::KeyWithFocus(..)
                    | Message::Scrolled(_)
                    | Message::Hover(_)
            )
        {
            return Task::none();
        }
        let task = match message {
            Message::SearchChanged(search) => {
                self.search = search;
                self.scroll_offset = 0.0;
                Task::batch([
                    iced::widget::operation::scroll_to(
                        iced::widget::Id::new(SCROLL_ID),
                        scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                    ),
                    self.load_visible_art(),
                ])
            }
            Message::Select(name) => {
                if self.selected.as_deref() == Some(&name) && self.panel_open {
                    self.card_click_target = Some((name, self.target.clone()));
                    self.close_panel();
                    Task::none()
                } else {
                    self.close_log();
                    self.selected = Some(name.clone());
                    self.panel_open = true;
                    self.target = self
                        .cards
                        .iter()
                        .find(|card| card.name == name)
                        .and_then(|card| card.default_target.clone());
                    self.panel_error = None;
                    self.panel_note = None;
                    self.load_panel_art()
                }
            }
            Message::DoubleClick(name) => {
                let prior_target = self
                    .card_click_target
                    .take()
                    .and_then(|(clicked, target)| (clicked == name).then_some(target));
                let panel_art = if self.selected.as_deref() != Some(&name) || !self.panel_open {
                    self.update(Message::Select(name))
                } else {
                    Task::none()
                };
                if let Some(target) = prior_target {
                    self.target = target;
                }
                Task::batch([panel_art, self.load_panel_art(), self.start_selected()])
            }
            Message::ClosePanel => {
                self.close_panel();
                Task::none()
            }
            Message::TargetPicked(target) => {
                self.target = Some(target);
                self.load_panel_art()
            }
            Message::Play => self.start_selected(),
            Message::Stop | Message::CleanUp => self.stop_selected(),
            Message::CancelStart => {
                if let Some(operation) = &mut self.operation
                    && operation.kind == OperationKind::Start
                {
                    if let Some(token) = &operation.token {
                        if operation.cancelled {
                            token.force();
                            operation.status = "Force stopping...".to_owned();
                        } else {
                            token.cancel();
                            operation.status = "Canceling...".to_owned();
                        }
                    }
                    operation.cancelled = true;
                }
                Task::none()
            }
            Message::LaunchProgress(name, status) => {
                if let Some(operation) = &mut self.operation
                    && operation.name == name
                    && operation.kind == OperationKind::Start
                    && !operation.cancelled
                {
                    operation.status = status;
                }
                Task::none()
            }
            Message::LaunchNote(name, note) => {
                if self.selected.as_deref() == Some(&name) {
                    self.panel_note = Some(note);
                }
                Task::none()
            }
            Message::StartFinished(name, result) => {
                let cancelled = self
                    .operation
                    .as_ref()
                    .is_some_and(|operation| operation.name == name && operation.cancelled);
                if self.operation.as_ref().is_some_and(|operation| {
                    operation.name == name && operation.kind == OperationKind::Start
                }) {
                    self.operation = None;
                }
                if self.selected.as_deref() == Some(&name)
                    && let Err(error) = result
                    && !cancelled
                {
                    self.panel_error = Some(error);
                }
                if self.refreshing {
                    Task::none()
                } else {
                    self.refreshing = true;
                    Self::load(self.paths.clone())
                }
            }
            Message::StopFinished(name, result) => {
                if self.operation.as_ref().is_some_and(|operation| {
                    operation.name == name && operation.kind == OperationKind::Stop
                }) {
                    self.operation = None;
                }
                if self.selected.as_deref() == Some(&name) {
                    match result {
                        Ok(messages) => {
                            self.panel_note = (!messages.is_empty()).then(|| messages.join("\n"))
                        }
                        Err(error) => self.panel_error = Some(error),
                    }
                }
                if self.refreshing {
                    Task::none()
                } else {
                    self.refreshing = true;
                    Self::load(self.paths.clone())
                }
            }
            Message::OpenLog => self.open_log(),
            Message::CloseLog => {
                self.close_log();
                Task::none()
            }
            Message::LogChunk(generation, chunk) => {
                if self.log_view && self.log_generation == generation {
                    self.log_buffer.push(&chunk);
                    iced::widget::operation::snap_to_end(iced::widget::Id::new(LOG_ID))
                } else {
                    Task::none()
                }
            }
            Message::LogFinished(generation, result) => {
                if self.log_view
                    && self.log_generation == generation
                    && let Err(error) = result
                {
                    self.panel_note = Some(error);
                }
                Task::none()
            }
            Message::LogScrolled => Task::none(),
            Message::Hover(name) => {
                self.hovered = name;
                Task::none()
            }
            Message::Refresh => {
                let library = if self.refreshing {
                    Task::none()
                } else {
                    self.refreshing = true;
                    Self::load(self.paths.clone())
                };
                Task::batch([library, self.settings_tick()])
            }
            Message::Loaded(Ok(cards)) => {
                self.refreshing = false;
                self.merge(cards);
                Task::batch([self.load_visible_art(), self.load_panel_art()])
            }
            Message::Loaded(Err(error)) => {
                self.refreshing = false;
                self.error = Some(error);
                Task::none()
            }
            Message::ArtLoaded(key, result) => {
                self.loading.remove(&key);
                if self.current_art.get(&key.0.path) != Some(&key.0.identity) {
                    return Task::none();
                }
                match result {
                    Ok(handle) => {
                        self.images.insert(key, handle);
                    }
                    Err(_) => {
                        self.failed_images.insert(key);
                    }
                }
                Task::none()
            }
            Message::Scrolled(viewport) => {
                self.viewport_width =
                    viewport.bounds().width + if self.panel_open { 420.0 } else { 0.0 };
                self.viewport_height = viewport.bounds().height;
                let columns = self.columns();
                let rows = self.filtered().len().div_ceil(columns);
                if rows > 0 {
                    self.row_pitch = (viewport.content_bounds().height - 32.0 + GAP) / rows as f32;
                }
                self.scroll_offset = viewport.absolute_offset().y;
                self.scroll_visible_until = Some(Instant::now() + Duration::from_millis(850));
                self.load_visible_art()
            }
            Message::ScrollIdle => {
                if self
                    .scroll_visible_until
                    .is_some_and(|until| Instant::now() >= until)
                {
                    self.scroll_visible_until = None;
                }
                Task::none()
            }
            Message::Window(window::Event::Resized(size) | window::Event::Opened { size, .. }) => {
                self.window_size = size;
                self.viewport_width = size.width;
                self.viewport_height = (size.height - 48.0).max(0.0);
                self.load_visible_art()
            }
            Message::Window(window::Event::CloseRequested) => {
                if self.foreground_busy() {
                    self.close_prompt = true;
                    Task::none()
                } else {
                    iced::exit()
                }
            }
            Message::Window(_) => Task::none(),
            Message::ThemeChanged(mode) => {
                self.palette = Palette::from_mode(mode);
                Task::none()
            }
            Message::SpinnerTick => {
                if self.operation.is_some()
                    || self.settings.as_ref().is_some_and(|page| page.saving)
                {
                    self.spinner_frame = (self.spinner_frame + 1) % 4;
                }
                Task::none()
            }
            Message::Key(key, _, _) if self.close_prompt => {
                if key == keyboard::Key::Named(keyboard::key::Named::Escape) && !self.closing {
                    self.close_prompt = false;
                }
                Task::none()
            }
            Message::Key(key, modifiers, status) if self.flow.is_some() => {
                use iced::keyboard::{Key, key::Named};
                match key.as_ref() {
                    Key::Character(choice) if matches!(self.flow, Some(flow::Page::Choose(_))) => {
                        let event = match choice {
                            "1" => flow::Event::New,
                            "2" => flow::Event::Prefix,
                            "3" => flow::Event::Heroic,
                            _ => return Task::none(),
                        };
                        self.flow_update(self.flow_generation, event)
                    }
                    Key::Named(Named::Enter) if modifiers.control() => {
                        let generation = self.flow_generation;
                        let task = self
                            .flow
                            .as_mut()
                            .unwrap()
                            .submit()
                            .map(move |event| Message::Flow(generation, event));
                        task.chain(self.flow_update(generation, flow::Event::AfterSubmit))
                    }
                    Key::Named(Named::Tab) if modifiers.shift() => {
                        iced::widget::operation::focus_previous()
                    }
                    Key::Named(Named::Tab) => iced::widget::operation::focus_next(),
                    Key::Named(Named::Escape)
                        if status != iced::event::Status::Captured && !self.close_prompt =>
                    {
                        self.flow_update(self.flow_generation, flow::Event::Back)
                    }
                    _ => Task::none(),
                }
            }
            Message::Key(key, modifiers, status) if self.settings.is_some() => {
                self.settings_key(&key, modifiers, status == iced::event::Status::Captured)
            }
            Message::Key(key, modifiers, _)
                if modifiers.control()
                    && matches!(key.as_ref(), keyboard::Key::Character("n" | "N")) =>
            {
                self.update(Message::Add)
            }
            Message::Key(key, modifiers, status) => iced::widget::operation::is_focused(
                iced::widget::Id::new(SEARCH_ID),
            )
            .map(move |focused| Message::KeyWithFocus(key.clone(), modifiers, status, focused)),
            Message::KeyWithFocus(key, modifiers, status, focused) => self.handle_key(
                key,
                modifiers,
                focused,
                status == iced::event::Status::Captured,
            ),
            Message::SearchFocusChanged(focused) => {
                self.search_focused = focused;
                Task::none()
            }
            Message::OpenSettings => self.open_settings(),
            Message::OpenGlobalSettings => self.open_global_settings(),
            Message::Settings(event) => self.settings_update(event),
            Message::Add => {
                if self.foreground_busy() || self.settings.is_some() || self.flow.is_some() {
                    return Task::none();
                }
                self.close_log();
                self.action_menu = None;
                self.flow_generation = self.flow_generation.wrapping_add(1);
                self.flow = Some(flow::Page::choose(self.paths.clone()));
                iced::advanced::widget::operate(
                    iced::advanced::widget::operation::focusable::unfocus(),
                )
            }
            Message::CloneEnvironment(name) => {
                if self.foreground_busy() || self.settings.is_some() || self.flow.is_some() {
                    return Task::none();
                }
                self.close_log();
                self.action_menu = None;
                self.flow_generation = self.flow_generation.wrapping_add(1);
                let generation = self.flow_generation;
                let (page, task) = flow::Page::clone(self.paths.clone(), name);
                self.flow = Some(page);
                Task::batch([
                    task.map(move |event| Message::Flow(generation, event)),
                    iced::advanced::widget::operate(
                        iced::advanced::widget::operation::focusable::unfocus(),
                    ),
                ])
            }
            Message::Context(name) => {
                self.action_menu_position = self.pointer;
                self.action_menu = if self.action_menu.as_deref() == Some(&name) {
                    None
                } else {
                    Some(name)
                };
                Task::none()
            }
            Message::PointerMoved(position) => {
                self.pointer = position;
                Task::none()
            }
            Message::OverlayInput => Task::none(),
            Message::CloseMenu => {
                self.action_menu = None;
                Task::none()
            }
            Message::ContextSettings(name) => {
                self.action_menu = None;
                self.selected = Some(name);
                self.open_settings()
            }
            Message::Flow(generation, event) => self.flow_update(generation, event),
            Message::FlowReturned(generation, action, result)
                if generation == self.flow_generation =>
            {
                self.flow_returned(action, result)
            }
            Message::FlowReturned(..) => Task::none(),
            Message::KeepWorking => {
                self.close_prompt = false;
                Task::none()
            }
            event @ (Message::CancelAndClose | Message::ForceCloseWork) => {
                let force = matches!(event, Message::ForceCloseWork);
                self.closing = true;
                if let Some(page) = &mut self.flow {
                    page.cancel(force);
                }
                if let Some(operation) = &mut self.operation
                    && let Some(token) = &operation.token
                {
                    if force {
                        token.force();
                    } else {
                        token.cancel();
                    }
                    operation.cancelled = true;
                }
                if !self.foreground_busy() {
                    iced::exit()
                } else {
                    Task::none()
                }
            }
        };
        if self.closing && !self.foreground_busy() {
            task.chain(iced::exit())
        } else {
            task
        }
    }
    fn foreground_busy(&self) -> bool {
        self.operation.is_some()
            || self.flow.as_ref().is_some_and(flow::Page::busy)
            || self.settings.as_ref().is_some_and(|page| page.saving)
    }

    fn flow_update(&mut self, generation: u64, event: flow::Event) -> Task<Message> {
        if generation != self.flow_generation {
            return Task::none();
        }
        let Some(page) = &mut self.flow else {
            return Task::none();
        };
        let choose_back =
            matches!(page, flow::Page::Choose(_)) && matches!(event, flow::Event::Back);
        let task = page
            .update(event)
            .map(move |event| Message::Flow(generation, event));
        if self.closing && !page.busy() {
            return task.chain(iced::exit());
        }
        if choose_back {
            self.flow = None;
            return task;
        }
        if let Some(action) = page.take_action() {
            if matches!(action, flow::Action::Choose) {
                self.flow_generation = self.flow_generation.wrapping_add(1);
                self.flow = Some(flow::Page::choose(self.paths.clone()));
                return Task::none();
            }
            self.flow = None;
            self.flow_generation = self.flow_generation.wrapping_add(1);
            let generation = self.flow_generation;
            let paths = self.paths.clone();
            return Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        load_library(&paths).map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
                },
                move |cards| Message::FlowReturned(generation, action.clone(), cards),
            );
        }
        task
    }

    fn flow_returned(
        &mut self,
        action: flow::Action,
        result: Result<Vec<Card>, String>,
    ) -> Task<Message> {
        if self.closing {
            return iced::exit();
        }
        match result {
            Ok(cards) => self.merge(cards),
            Err(error) => {
                self.error = Some(error);
                return Task::none();
            }
        }
        let (name, launch, settings) = match action {
            flow::Action::Choose => return Task::none(),
            flow::Action::Library { name, launch } => (name, launch, false),
            flow::Action::Settings { name, launch } => (Some(name), launch, true),
        };
        let Some(name) = name else {
            return Task::none();
        };
        self.search.clear();
        self.selected = Some(name.clone());
        self.panel_open = true;
        self.target = self
            .cards
            .iter()
            .find(|card| card.name == name)
            .and_then(|card| card.default_target.clone());
        self.panel_error = None;
        self.panel_note = None;
        let mut tasks = vec![self.load_visible_art(), self.load_panel_art()];
        if launch {
            tasks.push(self.start_selected());
        }
        if settings {
            tasks.push(self.open_settings());
            if self
                .cards
                .iter()
                .find(|card| card.name == name)
                .is_some_and(|card| card.targets.is_empty())
                && let Some(page) = &mut self.settings
            {
                page.open_add_after_load = true;
            }
        }
        Task::batch(tasks)
    }
    fn merge(&mut self, cards: Vec<Card>) {
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !cards.iter().any(|card| &card.name == name))
        {
            self.selected = None;
            self.close_panel();
        }
        if let Some(card) = self
            .selected
            .as_ref()
            .and_then(|name| cards.iter().find(|card| &card.name == name))
            && self
                .target
                .as_ref()
                .is_some_and(|target| !card.targets.contains(target))
        {
            self.target = card.default_target.clone();
        }
        self.cards = cards;
        self.current_art = self
            .cards
            .iter()
            .flat_map(|card| {
                card.art.iter().chain(
                    card.panel_art
                        .values()
                        .flat_map(|art| art.hero.iter().chain(art.logo.iter())),
                )
            })
            .map(|art| (art.path.clone(), art.identity))
            .collect();
        self.images
            .retain(|(art, _), _| self.current_art.get(&art.path) == Some(&art.identity));
        self.loading
            .retain(|(art, _)| self.current_art.get(&art.path) == Some(&art.identity));
        self.failed_images
            .retain(|(art, _)| self.current_art.get(&art.path) == Some(&art.identity));
        self.error = None;
    }
    pub(crate) fn filtered(&self) -> Vec<&Card> {
        let needle = self.search.to_lowercase();
        self.cards
            .iter()
            .filter(|card| card.name.to_lowercase().contains(&needle))
            .collect()
    }
    fn columns(&self) -> usize {
        let width = self.viewport_width - if self.panel_open { 420.0 } else { 0.0 };
        ((width - 32.0 + GAP) / (CARD_WIDTH + GAP)).floor().max(1.0) as usize
    }
    fn load_visible_art(&mut self) -> Task<Message> {
        let columns = self.columns();
        let first = (self.scroll_offset.max(0.0) / self.row_pitch).floor() as usize;
        let last = ((self.scroll_offset + self.viewport_height) / self.row_pitch).ceil() as usize;
        let paths: Vec<_> = self
            .filtered()
            .into_iter()
            .skip(first * columns)
            .take(last.saturating_sub(first) * columns)
            .filter_map(|card| card.art.as_ref())
            .map(|art| (art.clone(), artwork::Kind::Grid))
            .collect();
        self.load_images(paths)
    }
    fn load_images(&mut self, paths: Vec<ArtKey>) -> Task<Message> {
        let mut tasks = Vec::new();
        for key in paths {
            if self.images.contains_key(&key)
                || self.failed_images.contains(&key)
                || !self.loading.insert(key.clone())
            {
                continue;
            }
            tasks.push(Task::perform(
                async move {
                    let decoded = tokio::task::spawn_blocking({
                        let key = key.clone();
                        move || artwork::decode(&key.0.path, key.1).map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string());
                    (key, decoded)
                },
                |(key, decoded)| Message::ArtLoaded(key, decoded.unwrap_or_else(Err)),
            ));
        }
        Task::batch(tasks)
    }
}

fn run() -> anyhow::Result<()> {
    let paths = Paths::from_env(None)?;
    iced::application(move || App::boot(paths.clone()), App::update, App::view)
        .title("wineo")
        .theme(App::theme)
        .settings(Settings {
            default_text_size: 14.into(),
            ..Settings::default()
        })
        .window(window_settings())
        .window_size((1200.0, 800.0))
        .subscription(App::subscription)
        .run()?;
    Ok(())
}

fn app_icon() -> Option<window::Icon> {
    let svg = include_bytes!("../assets/wineo-gui.svg");
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );

    let rgba = pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let alpha = u16::from(pixel[3]);
            if alpha == 0 {
                [0, 0, 0, 0]
            } else {
                let straight =
                    |channel: u8| ((u16::from(channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                [
                    straight(pixel[0]),
                    straight(pixel[1]),
                    straight(pixel[2]),
                    pixel[3],
                ]
            }
        })
        .collect();

    window::icon::from_rgba(rgba, size.width(), size.height()).ok()
}

fn window_settings() -> window::Settings {
    window::Settings {
        exit_on_close_request: false,
        icon: app_icon(),
        platform_specific: window::settings::PlatformSpecific {
            application_id: "wineo-gui".to_owned(),
            ..window::settings::PlatformSpecific::default()
        },
        ..window::Settings::default()
    }
}

fn main() -> ExitCode {
    if let Some(result) = wineo_core::supervisor::dispatch_if_supervise() {
        return if result.is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn card(name: &str, state: State, seconds: i64) -> Card {
        Card {
            name: name.into(),
            state,
            busy: false,
            busy_holder: None,
            unresolved: false,
            error: None,
            playtime: playtime_label(seconds),
            sessions: 0,
            last_played: "never".into(),
            tags: Vec::new(),
            targets: vec!["game".into()],
            default_target: Some("game".into()),
            art: None,
            panel_art: HashMap::new(),
        }
    }
    fn app() -> App {
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| "/tmp/wineo-gui-tests".into()))
                .unwrap();
        App::boot(paths).0
    }
    fn art_file(path: &str, len: u64) -> ArtFile {
        ArtFile {
            path: PathBuf::from(path),
            identity: ArtIdentity {
                modified: SystemTime::UNIX_EPOCH,
                len,
            },
        }
    }
    fn card_with_art(art: ArtFile) -> Card {
        let mut card = card("game", State::Inactive, 0);
        card.art = Some(art);
        card
    }
    #[test]
    fn app_icon_renders_from_embedded_svg() {
        assert!(app_icon().is_some());
    }

    #[test]
    fn back_from_new_returns_to_add_choices_without_closing() {
        let mut app = app();
        let _ = app.update(Message::Add);
        let generation = app.flow_generation;
        let _ = app.flow_update(generation, flow::Event::New);
        let _ = app.flow_update(generation, flow::Event::Back);
        assert!(matches!(app.flow, Some(flow::Page::Choose(_))));
        assert!(!app.closing);
        assert_ne!(app.flow_generation, generation);
        let _ = app.flow_update(generation, flow::Event::Back);
        assert!(app.flow.is_some(), "stale Back cannot dismiss Add");
    }

    #[test]
    fn creation_overlay_rejects_background_library_actions() {
        let mut app = app();
        app.search = "original".into();
        let _ = app.update(Message::Add);
        let _ = app.update(Message::SearchChanged("hidden".into()));
        let _ = app.update(Message::Select("other".into()));
        let _ = app.update(Message::OpenGlobalSettings);
        let _ = app.update(Message::Play);
        assert_eq!(app.search, "original");
        assert!(app.selected.is_none());
        assert!(app.settings.is_none());
        assert!(app.operation.is_none());
        assert!(matches!(app.flow, Some(flow::Page::Choose(_))));
    }

    #[test]
    fn flow_result_selects_new_card_even_when_search_hides_it() {
        let mut app = app();
        app.search = "other".into();
        let _ = app.flow_returned(
            flow::Action::Library {
                name: Some("new".into()),
                launch: false,
            },
            Ok(vec![card("new", State::Inactive, 0)]),
        );
        assert!(app.search.is_empty());
        assert_eq!(app.selected.as_deref(), Some("new"));
        assert_eq!(app.target.as_deref(), Some("game"));
        assert!(app.panel_open);
        assert!(app.operation.is_none());
    }

    #[test]
    fn closing_ignores_pending_launch_and_stale_flow_replies() {
        let mut app = app();
        app.flow = Some(flow::Page::choose(app.paths.clone()));
        app.flow_generation = 5;
        let _ = app.flow_update(4, flow::Event::New);
        assert!(matches!(app.flow, Some(flow::Page::Choose(_))));
        let _ = app.update(Message::FlowReturned(
            4,
            flow::Action::Settings {
                name: "old".into(),
                launch: true,
            },
            Ok(vec![card("old", State::Inactive, 0)]),
        ));
        assert!(app.selected.is_none());
        assert!(app.settings.is_none());
        app.closing = true;
        let _ = app.flow_returned(
            flow::Action::Library {
                name: Some("new".into()),
                launch: true,
            },
            Ok(vec![card("new", State::Inactive, 0)]),
        );
        assert!(app.operation.is_none());
        assert!(app.selected.is_none());
    }
    #[test]
    fn search_is_case_insensitive() {
        let mut app = app();
        app.cards = vec![
            card("Dusklight", State::Inactive, 0),
            card("Other", State::Inactive, 0),
        ];
        app.search = "LIGHT".into();
        assert_eq!(app.filtered().len(), 1);
        assert_eq!(app.filtered()[0].name, "Dusklight");
    }
    #[test]
    fn card_state_and_playtime() {
        let palette = Palette::DARK;
        let running = card("game", State::Running, 11520);
        assert_eq!(running.state_word(), "running");
        assert_eq!(running.dot(palette), palette.success);
        assert_eq!(running.playtime(), "3 h 12 m");
        let idle = card("game", State::Inactive, 0);
        assert_eq!(idle.dot(palette), palette.muted);
        assert_eq!(idle.playtime(), "never played");
        let failed = card("game", State::Failed, 0);
        assert_eq!(failed.dot(palette), palette.warning);
        let mut busy = card("game", State::Running, 0);
        busy.busy = true;
        assert_eq!(busy.state_word(), "busy");
        assert_eq!(busy.dot(palette), palette.warning);
        busy.error = Some("broken config".into());
        assert_eq!(busy.state_word(), "error");
        assert_eq!(busy.dot(palette), palette.warning);
    }
    #[test]
    fn refresh_preserves_search_and_selection_until_removed() {
        let mut app = app();
        app.search = "ga".into();
        app.selected = Some("game".into());
        app.scroll_offset = 140.0;
        let _ = app.update(Message::Loaded(Ok(vec![card("game", State::Running, 60)])));
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert_eq!(app.search, "ga");
        assert_eq!(app.scroll_offset, 140.0);
        let _ = app.update(Message::Loaded(Ok(vec![card("other", State::Inactive, 0)])));
        assert_eq!(app.selected, None);
    }

    #[test]
    fn changed_art_identity_reloads_and_evicts_old_handle() {
        let mut app = app();
        let old = art_file("/tmp/grid.png", 10);
        let new = art_file("/tmp/grid.png", 20);
        let old_key = (old.clone(), artwork::Kind::Grid);
        let new_key = (new.clone(), artwork::Kind::Grid);
        app.merge(vec![card_with_art(old)]);
        let _ = app.load_visible_art();
        assert!(app.loading.contains(&old_key));
        let handle = image::Handle::from_rgba(1, 1, vec![1, 2, 3, 255]);
        let _ = app.update(Message::ArtLoaded(old_key.clone(), Ok(handle)));
        assert!(app.images.contains_key(&old_key));
        app.merge(vec![card_with_art(new)]);
        assert!(!app.images.contains_key(&old_key));
        let _ = app.load_visible_art();
        assert!(app.loading.contains(&new_key));
        assert!(!app.loading.contains(&old_key));
    }

    #[test]
    fn failed_art_retries_only_after_identity_changes() {
        let mut app = app();
        let old = art_file("/tmp/broken.png", 10);
        let new = art_file("/tmp/broken.png", 20);
        let old_key = (old.clone(), artwork::Kind::Grid);
        let new_key = (new.clone(), artwork::Kind::Grid);
        app.merge(vec![card_with_art(old)]);
        let _ = app.load_visible_art();
        let _ = app.update(Message::ArtLoaded(old_key.clone(), Err("invalid".into())));
        let _ = app.load_visible_art();
        assert!(app.failed_images.contains(&old_key));
        assert!(app.loading.is_empty());
        app.merge(vec![card_with_art(new)]);
        assert!(!app.failed_images.contains(&old_key));
        let _ = app.load_visible_art();
        assert!(app.loading.contains(&new_key));
    }

    #[test]
    fn library_keeps_native_and_broken_configs() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_lookup(|key| match key {
            "HOME" | "XDG_CONFIG_HOME" | "XDG_DATA_HOME" | "XDG_STATE_HOME" => {
                Some(root.path().as_os_str().to_owned())
            }
            _ => None,
        })
        .unwrap();
        std::fs::create_dir_all(paths.environments_dir()).unwrap();
        std::fs::write(
            paths.environment_config("native"),
            "schema_version = 1\nkind = \"native\"\n[targets.game]\nexe = \"missing-game\"\ndefault = true\n",
        )
        .unwrap();
        std::fs::write(paths.environment_config("broken"), "this is [").unwrap();
        let cards = load_library(&paths).unwrap();
        assert_eq!(cards.len(), 2);
        assert!(
            cards
                .iter()
                .any(|card| card.name == "native" && card.error.is_none())
        );
        assert!(
            cards
                .iter()
                .any(|card| card.name == "broken" && card.error.is_some())
        );
    }

    #[test]
    fn play_target_uses_default_single_target_or_picker_choice() {
        let mut app = app();
        let mut one = card("one", State::Inactive, 0);
        one.targets = vec!["only".into()];
        one.default_target = Some("only".into());
        assert_eq!(app.chosen_target(&one), Some("only"));
        let mut several = card("many", State::Inactive, 0);
        several.targets = vec!["a".into(), "b".into()];
        several.default_target = Some("b".into());
        assert_eq!(app.chosen_target(&several), Some("b"));
        app.target = Some("a".into());
        assert_eq!(app.chosen_target(&several), Some("a"));
    }

    #[test]
    fn double_click_opens_panel_and_requests_start() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Select("game".into()));
        assert!(app.panel_open);
        let _ = app.update(Message::Select("game".into()));
        assert!(!app.panel_open);
        let _ = app.update(Message::DoubleClick("game".into()));
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert!(app.panel_open);
        assert_eq!(app.target.as_deref(), Some("game"));
        assert!(app.operation.as_ref().is_some_and(|operation| {
            operation.name == "game" && operation.kind == OperationKind::Start
        }));
    }

    #[test]
    fn double_click_keeps_open_panels_picker_choice() {
        let mut app = app();
        let mut game = card("game", State::Inactive, 0);
        game.targets = vec!["game".into(), "alternate".into()];
        app.cards = vec![game];
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::TargetPicked("alternate".into()));
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::DoubleClick("game".into()));
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert!(app.panel_open);
        assert_eq!(app.target.as_deref(), Some("alternate"));
        assert!(app.operation.as_ref().is_some_and(|operation| {
            operation.name == "game" && operation.kind == OperationKind::Start
        }));
    }

    #[test]
    fn double_click_cannot_start_disabled_card() {
        let mut app = app();
        app.cards = vec![card("game", State::Running, 0)];
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::DoubleClick("game".into()));
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert!(app.panel_open);
        assert!(app.operation.is_none());
    }

    #[test]
    fn cancel_then_force_stops_the_same_token() {
        let mut app = app();
        let token = Token::new();
        app.operation = Some(Operation {
            name: "game".into(),
            kind: OperationKind::Start,
            token: Some(token.clone()),
            cancelled: false,
            status: "Starting...".into(),
        });
        let _ = app.update(Message::CancelStart);
        assert!(token.is_canceled());
        assert!(!token.is_forced());
        assert!(app.operation.as_ref().unwrap().cancelled);
        let _ = app.update(Message::CancelStart);
        assert!(token.is_forced());
    }

    #[test]
    fn refresh_keeps_open_panel_target_and_log() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        app.selected = Some("game".into());
        app.panel_open = true;
        app.target = Some("game".into());
        app.log_view = true;
        let _ = app.update(Message::Loaded(Ok(vec![card("game", State::Running, 60)])));
        assert!(app.panel_open);
        assert!(app.log_view);
        assert_eq!(app.target.as_deref(), Some("game"));
        assert_eq!(app.cards[0].state, State::Running);
    }

    #[test]
    fn focused_widget_keeps_its_arrow_keys() {
        let mut app = app();
        app.cards = vec![card("a", State::Inactive, 0), card("b", State::Inactive, 0)];
        app.selected = Some("a".into());
        let right = keyboard::Key::Named(keyboard::key::Named::ArrowRight);
        let _ = app.handle_key(right.clone(), keyboard::Modifiers::empty(), false, true);
        assert_eq!(app.selected.as_deref(), Some("a"));
        let _ = app.handle_key(right, keyboard::Modifiers::empty(), false, false);
        assert_eq!(app.selected.as_deref(), Some("b"));
    }

    #[test]
    fn down_from_search_selects_first_match_then_grid_keys_move() {
        let mut app = app();
        app.cards = vec![
            card("alpha", State::Inactive, 0),
            card("amber", State::Inactive, 0),
        ];
        app.search = "a".into();
        let down = keyboard::Key::Named(keyboard::key::Named::ArrowDown);
        let _ = app.handle_key(down, keyboard::Modifiers::empty(), true, true);
        assert!(app.search_focused);
        assert_eq!(app.selected.as_deref(), Some("alpha"));
        let _ = app.update(Message::SearchFocusChanged(false));
        assert!(!app.search_focused);
        let right = keyboard::Key::Named(keyboard::key::Named::ArrowRight);
        let _ = app.handle_key(
            right,
            keyboard::Modifiers::empty(),
            app.search_focused,
            false,
        );
        assert_eq!(app.selected.as_deref(), Some("amber"));
    }
}
