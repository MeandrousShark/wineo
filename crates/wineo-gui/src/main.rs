mod art_navigation;
mod art_page;
mod art_picker;
mod artwork;
mod checkup;
mod checkup_work;
mod chips;
mod download_progress;
mod downloads_page;
mod downloads_work;
mod flow;
mod focus_button;
mod grid;
mod grid_motion;
mod keys;
mod library;
mod logview;
mod panel;
mod panel_motion;
mod picker;
mod portable_page;
mod removal;
mod remove_work;
mod runner_label;
mod settings;
mod shortcuts;
mod sidebar;
mod size;
mod snapshots_page;
mod spinner;
mod theme;
mod tools_navigation;
mod tools_page;
mod trash_page;
mod trash_work;
mod ui;
mod wheel;
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
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
use crate::panel::{LOG_ID, LogBuffer, PanelError};
use crate::theme::Palette;

pub(crate) const CARD_WIDTH: f32 = 160.0;
const CARD_ROW: f32 = 300.0;
pub(crate) const GAP: f32 = 16.0;
const SCROLL_ID: &str = "library";
const SEARCH_ID: &str = "library-search";
// iced moves 60px per wheel line, which felt too fast beside native apps.
const WHEEL_STEP_SCALE: f32 = 0.7;
const MAX_DECODES: usize = 2;
type ArtKey = (ArtFile, artwork::Kind);

pub(crate) struct App {
    paths: Paths,
    pub(crate) palette: Palette,
    search: String,
    search_focused: bool,
    cards: Vec<Card>,
    pub(crate) selected: Option<String>,
    panel_open: bool,
    panel_motion: panel_motion::PanelMotion,
    grid_motion: grid_motion::GridMotion,
    grid_scroll_after_settle: Option<String>,
    pub(crate) target: Option<String>,
    card_click_target: Option<(String, Option<String>)>,
    pub(crate) hovered: Option<String>,
    pub(crate) images: HashMap<ArtKey, image::Handle>,
    pub(crate) dimmed_images: HashMap<ArtKey, image::Handle>,
    loading: HashSet<ArtKey>,
    /// Caps concurrent art decodes, since a huge source costs far more memory
    /// than its thumbnail.
    decodes: Arc<tokio::sync::Semaphore>,
    thumbnails: Arc<Mutex<art_picker::ThumbnailCache>>,
    failed_images: HashSet<ArtKey>,
    current_art: HashMap<PathBuf, ArtIdentity>,
    error: Option<String>,
    refreshing: bool,
    /// The first library load has answered, so an empty library is real.
    loaded: bool,
    viewport_width: f32,
    viewport_height: f32,
    scroll_offset: f32,
    scroll_visible_until: Option<Instant>,
    pub(crate) operation: Option<Operation>,
    /// The game an install or uninstall request is being handed to Steam for.
    pub(crate) steam_request: Option<String>,
    pub(crate) panel_error: Option<PanelError>,
    pub(crate) hook_stop_confirm: Option<String>,
    pub(crate) hook_stop_identity: Option<wineo_core::hooks::Running>,
    /// When the panel's polite Stop may turn into Force stop, by name.
    pub(crate) force_stop_at: Option<(String, Instant)>,
    pub(crate) panel_note: Option<String>,
    pub(crate) log_view: bool,
    log_error: Option<String>,
    checkup_log_scope: Option<checkup::Scope>,
    pub(crate) log_buffer: LogBuffer,
    log_token: Option<Token>,
    log_generation: u64,
    /// True while the log view shows install.log rather than output.log.
    pub(crate) log_install: bool,
    pub(crate) spinner_angle: f32,
    spinner_start: Instant,
    settings: Option<settings::Page>,
    settings_generation: u64,
    steamgriddb_sequence: u64,
    steamgriddb_completed_sequence: u64,
    steamgriddb_availability_sequence: u64,
    steamgriddb_available: bool,
    steamgriddb_input_session_sequence: u64,
    steamgriddb_pending: HashSet<(u64, u64)>,
    steamgriddb_late_result: Option<settings::Event>,
    steam_key_work: settings::SteamKeyWork,
    art_page: Option<String>,
    art_pages: HashMap<String, art_page::Page>,
    pending_art_writes: HashSet<(String, u64, u64, art_page::EnvironmentIdentity)>,
    pending_art_cancels: HashMap<(String, u64, u64, art_page::EnvironmentIdentity), Token>,
    art_generation: u64,
    snapshots_page: Option<snapshots_page::Page>,
    portable_page: Option<String>,
    portable_pages: HashMap<String, portable_page::Page>,
    tool_page: Option<(String, tools_page::Kind)>,
    tool_pages: HashMap<(String, tools_page::Kind), tools_page::Page>,
    tool_prefixes: HashMap<(String, tools_page::Kind), PathBuf>,
    section: sidebar::Section,
    pending_section: Option<sidebar::Section>,
    pending_checkup_environment: Option<String>,
    trash_badge: sidebar::TrashBadge,
    trash_refreshing: bool,
    flow: Option<flow::Page>,
    flow_generation: u64,
    action_menu: Option<String>,
    pointer: iced::Point,
    action_menu_position: iced::Point,
    window_size: iced::Size,
    close_prompt: bool,
    closing: bool,
    removal: Option<removal::Dialog>,
    remove_generation: u64,
    change: Option<removal::Change>,
    toast: Option<removal::Toast>,
    feature_notice: Option<(String, Instant)>,
    change_generation: u64,
    library_generation: u64,
    trash_generation: u64,
    trash_page: trash_page::Page,
    downloads_page: downloads_page::Page,
    checkups: HashMap<checkup::Scope, checkup::Results>,
    check_generation: u64,
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
    /// Canceled once the winetricks install phase is over. Only a start has one.
    install_done: Option<Token>,
    /// True while winetricks is installing, which is when install.log can be read.
    installing: bool,
    after_hook: bool,
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
    InstallPhase(String, bool),
    StartFinished(String, Result<wineo_core::launch::StartOutcome, PanelError>),
    StopFinished(String, Result<Vec<String>, PanelError>),
    Install,
    Uninstall(String),
    SteamRequested(String, Result<String, PanelError>),
    ConfirmHookStop,
    CancelHookStop,
    AfterHookPhase(String, Token),
    OpenLog,
    CloseLog,
    LogChunk(u64, String),
    LogFinished(u64, Result<(), String>),
    LogAction(iced::widget::text_editor::Action),
    LogScrolled(scrollable::Viewport),
    LogFollow,
    CopyLog,
    Hover(Option<String>),
    Refresh,
    Loaded(Result<Vec<Card>, String>),
    LibraryReloaded(u64, Result<Vec<Card>, String>),
    TrashReloaded(u64, sidebar::TrashBadge),
    Section(sidebar::Section),
    TrashBadgeLoaded(sidebar::TrashBadge),
    ArtLoaded(ArtKey, Result<image::Handle, String>),
    Scrolled(scrollable::Viewport),
    GridCardLocated(String, u64, grid_motion::CardBounds),
    ScrollIdle,
    Window(window::Event),
    ThemeChanged(iced::theme::Mode),
    SpinnerTick(Instant),
    PanelTick(u64, Instant),
    GridTick(u64, Instant),
    Key(keyboard::Key, keyboard::Modifiers, iced::event::Status),
    KeyWithFocus(
        keyboard::Key,
        keyboard::Modifiers,
        iced::event::Status,
        bool,
    ),
    SearchFocusChanged(bool),
    OpenSettings,
    Settings(settings::Event),
    SteamGridDbKeyChangedAt(u64, bool),
    SteamGridDbKeyPresenceLoaded(u64, bool),
    OpenArt,
    ContextArt(String),
    ArtPage(String, u64, art_page::Event),
    SnapshotsEnvironment(String),
    Snapshots(snapshots_page::Event),
    CopyPortable(String),
    Portable(String, portable_page::Event),
    OpenTool(String, tools_page::Kind),
    Tool(String, tools_page::Kind, tools_page::Event),
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
    Remove(String),
    RemovePrefix(bool),
    RemoveSize(u64, Result<u64, String>),
    ConfirmRemove,
    CancelRemove,
    RemoveProgress(u64, wineo_core::trash::RemoveEvent),
    RemoveFinished(
        u64,
        Result<wineo_core::trash::Entry, remove_work::RemoveError>,
    ),
    RemoveFocus(iced::event::Status, bool, bool),
    Undo,
    UndoFinished(u64, Result<(), String>),
    ToastTick(Instant),
    ToastHover(bool),
    ToastFocus(bool, Instant),
    Trash(trash_page::Event),
    Downloads(downloads_page::Event),
    Checkup(checkup::Event),
}

impl App {
    fn boot(paths: Paths) -> (Self, Task<Message>) {
        let mut app = Self {
            paths,
            palette: Palette::DARK,
            search: String::new(),
            search_focused: false,
            cards: Vec::new(),
            selected: None,
            panel_open: false,
            panel_motion: panel_motion::PanelMotion::default(),
            grid_motion: grid_motion::GridMotion::default(),
            grid_scroll_after_settle: None,
            target: None,
            card_click_target: None,
            hovered: None,
            images: HashMap::new(),
            dimmed_images: HashMap::new(),
            loading: HashSet::new(),
            decodes: Arc::new(tokio::sync::Semaphore::new(MAX_DECODES)),
            thumbnails: Arc::new(Mutex::new(art_picker::ThumbnailCache::default())),
            failed_images: HashSet::new(),
            current_art: HashMap::new(),
            error: None,
            refreshing: true,
            loaded: false,
            viewport_width: 1200.0 - sidebar::WIDTH,
            viewport_height: 750.0,
            scroll_offset: 0.0,
            scroll_visible_until: None,
            operation: None,
            steam_request: None,
            panel_error: None,
            hook_stop_confirm: None,
            hook_stop_identity: None,
            force_stop_at: None,
            panel_note: None,
            log_view: false,
            log_error: None,
            checkup_log_scope: None,
            log_buffer: LogBuffer::default(),
            log_token: None,
            log_generation: 0,
            log_install: false,
            spinner_angle: 0.0,
            spinner_start: Instant::now(),
            settings: None,
            settings_generation: 0,
            steamgriddb_sequence: 1,
            steamgriddb_completed_sequence: 0,
            steamgriddb_availability_sequence: 0,
            steamgriddb_available: false,
            steamgriddb_input_session_sequence: 0,
            steamgriddb_pending: HashSet::new(),
            steamgriddb_late_result: None,
            steam_key_work: settings::SteamKeyWork::default(),
            art_page: None,
            art_pages: HashMap::new(),
            pending_art_writes: HashSet::new(),
            pending_art_cancels: HashMap::new(),
            art_generation: 0,
            snapshots_page: None,
            portable_page: None,
            portable_pages: HashMap::new(),
            tool_page: None,
            tool_pages: HashMap::new(),
            tool_prefixes: HashMap::new(),
            section: sidebar::Section::Library,
            pending_section: None,
            pending_checkup_environment: None,
            trash_badge: sidebar::TrashBadge::default(),
            trash_refreshing: true,
            flow: None,
            flow_generation: 0,
            action_menu: None,
            pointer: iced::Point::ORIGIN,
            action_menu_position: iced::Point::ORIGIN,
            window_size: iced::Size::new(1200.0, 800.0),
            close_prompt: false,
            closing: false,
            removal: None,
            remove_generation: 0,
            change: None,
            toast: None,
            feature_notice: None,
            change_generation: 0,
            library_generation: 0,
            trash_generation: 0,
            trash_page: trash_page::Page::default(),
            downloads_page: downloads_page::Page::default(),
            checkups: HashMap::new(),
            check_generation: 0,
        };
        let load = app.reload_library();
        let badge = app.reload_badge();
        let cache = app.paths.art_cache_dir();
        let prune = Task::future(async move {
            let _ = tokio::task::spawn_blocking(move || artwork::prune(&cache)).await;
        })
        .discard();
        let key_presence = settings::key_presence_task(app.paths.clone(), app.steamgriddb_sequence);
        (
            app,
            Task::batch([
                load,
                badge,
                prune,
                key_presence,
                iced::system::theme().map(Message::ThemeChanged),
            ]),
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
    fn reload_library(&mut self) -> Task<Message> {
        self.library_generation = self.library_generation.wrapping_add(1);
        let generation = self.library_generation;
        Self::load(self.paths.clone()).map(move |message| match message {
            Message::Loaded(result) => Message::LibraryReloaded(generation, result),
            message => message,
        })
    }

    fn reload_badge(&mut self) -> Task<Message> {
        self.trash_generation = self.trash_generation.wrapping_add(1);
        let generation = self.trash_generation;
        Self::load_trash_badge(self.paths.clone()).map(move |message| match message {
            Message::TrashBadgeLoaded(badge) => Message::TrashReloaded(generation, badge),
            message => message,
        })
    }

    fn management_busy(&self) -> bool {
        self.filesystem_busy()
            || self.change.is_some()
            || self.art_busy()
            || self.flow.as_ref().is_some_and(flow::Page::busy)
            || self.settings.as_ref().is_some_and(|page| page.saving)
    }

    fn spinning(&self) -> bool {
        self.filesystem_busy()
            || self.tools_busy()
            || self.operation.is_some()
            || self.steam_request.is_some()
            || self.art_busy()
            || self.checkups.values().any(|results| results.checking)
            || self.change.is_some()
            || (self.section == sidebar::Section::Trash && self.trash_page.reviewing)
            || self
                .settings
                .as_ref()
                .is_some_and(|page| page.saving || page.key_busy())
            || self.flow.as_ref().is_some_and(flow::Page::spinning)
    }

    fn set_steamgriddb_availability(&mut self, sequence: u64, available: bool) -> Task<Message> {
        if sequence <= self.steamgriddb_availability_sequence {
            return Task::none();
        }
        self.steamgriddb_availability_sequence = sequence;
        self.steamgriddb_sequence = self.steamgriddb_sequence.max(sequence);
        if self.steamgriddb_available == available {
            return Task::none();
        }
        self.steamgriddb_available = available;
        if !available {
            return Task::none();
        }
        let paths = self.paths.clone();
        Task::batch(self.art_pages.iter_mut().map(|(name, page)| {
            let name = name.clone();
            let generation = page.generation;
            page.lookup_game(&paths, true)
                .map(move |event| Message::ArtPage(name.clone(), generation, event))
        }))
    }

    fn subscription(&self) -> Subscription<Message> {
        let idle = if self.scroll_visible_until.is_some() {
            iced::time::every(Duration::from_millis(250)).map(|_| Message::ScrollIdle)
        } else {
            Subscription::none()
        };
        // Every message rebuilds the view, so an always-on tick keeps the app busy while idle.
        let spinner = if self.spinning() {
            window::frames().map(Message::SpinnerTick)
        } else {
            Subscription::none()
        };
        let panel_motion = if !self.panel_hidden() && self.panel_motion.active() {
            let generation = self.panel_motion.generation();
            window::frames()
                .with(generation)
                .map(|(generation, now)| Message::PanelTick(generation, now))
        } else {
            Subscription::none()
        };
        let grid_motion = if !self.panel_hidden() && self.grid_motion.active() {
            let generation = self.grid_motion.generation();
            window::frames()
                .with(generation)
                .map(|(generation, now)| Message::GridTick(generation, now))
        } else {
            Subscription::none()
        };
        Subscription::batch([
            iced::time::every(Duration::from_secs(2)).map(|_| Message::Refresh),
            iced::system::theme_changes().map(Message::ThemeChanged),
            window::events().map(|(_, event)| Message::Window(event)),
            iced::event::listen_with(|event, status, _| event_message(event, status)),
            spinner,
            panel_motion,
            grid_motion,
            idle,
            if self.toast.is_some() || self.feature_notice.is_some() {
                iced::time::every(Duration::from_millis(100)).map(Message::ToastTick)
            } else {
                Subscription::none()
            },
        ])
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        let panel_was_open = self.panel_open;
        if self
            .feature_notice
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.feature_notice = None;
        }
        if (self.flow.is_some()
            || self.removal.is_some()
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
            || self.art_page.is_some()
            || self.tool_page.is_some()
            || self.portable_page.is_some())
            && matches!(
                message,
                Message::SearchChanged(_)
                    | Message::Select(_)
                    | Message::DoubleClick(_)
                    | Message::ClosePanel
                    | Message::TargetPicked(_)
                    | Message::Play
                    | Message::Install
                    | Message::Uninstall(_)
                    | Message::Stop
                    | Message::CleanUp
                    | Message::CancelStart
                    | Message::OpenLog
                    | Message::CloseLog
                    | Message::Context(_)
                    | Message::ContextSettings(_)
                    | Message::ContextArt(_)
                    | Message::OpenSettings
                    | Message::OpenArt
                    | Message::KeyWithFocus(..)
                    | Message::Scrolled(_)
                    | Message::Hover(_)
                    | Message::Add
                    | Message::CloneEnvironment(_)
                    | Message::Checkup(
                        checkup::Event::Fix(_)
                            | checkup::Event::Check(_)
                            | checkup::Event::OpenInstallLog(_)
                            | checkup::Event::Environment(_)
                    )
            )
        {
            return Task::none();
        }
        let task = match message {
            Message::SnapshotsEnvironment(name) => self.open_snapshots(Some(name)),
            Message::Snapshots(event) => self.snapshots_update(event),
            Message::CopyPortable(name) => self.open_portable(name),
            Message::Portable(name, event) => self.portable_update(name, event),
            Message::OpenTool(name, kind) => self.open_tool(name, kind),
            Message::Tool(name, kind, event) => self.tool_update(name, kind, event),
            Message::Checkup(event) => checkup::update(self, event),
            Message::Trash(event) => trash_page::update(self, event),
            Message::Downloads(event) => downloads_page::update(self, event),
            message @ (Message::Remove(_)
            | Message::RemovePrefix(_)
            | Message::RemoveSize(..)
            | Message::ConfirmRemove
            | Message::CancelRemove
            | Message::RemoveProgress(..)
            | Message::RemoveFinished(..)
            | Message::RemoveFocus(..)
            | Message::Undo
            | Message::UndoFinished(..)
            | Message::ToastHover(_)
            | Message::ToastTick(_)
            | Message::ToastFocus(..)) => removal::update(self, message),
            Message::SearchChanged(search) => {
                self.search = search;
                self.grid_motion
                    .snap(self.panel_open, self.filtered().len());
                self.grid_scroll_after_settle = None;
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
                    let was_open = self.panel_open;
                    self.close_log();
                    if self.selected.as_deref() != Some(&name) {
                        self.close_checkup();
                    }
                    self.selected = Some(name.clone());
                    self.panel_open = true;
                    if !was_open {
                        self.panel_motion.open(Instant::now());
                    }
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
            Message::Install => self.install_selected(),
            Message::Uninstall(name) => self.uninstall(name),
            Message::SteamRequested(name, result) => {
                if self.steam_request.as_deref() == Some(&name) {
                    self.steam_request = None;
                }
                if self.selected.as_deref() == Some(&name) {
                    match result {
                        Ok(note) => self.panel_note = Some(note),
                        Err(error) => self.panel_error = Some(error),
                    }
                }
                if self.refreshing {
                    Task::none()
                } else {
                    self.refreshing = true;
                    self.reload_library()
                }
            }
            Message::Stop | Message::CleanUp => self.stop_selected(),
            Message::ConfirmHookStop => self.confirm_hook_stop(),
            Message::CancelHookStop => {
                self.hook_stop_confirm = None;
                self.hook_stop_identity = None;
                Task::none()
            }
            Message::AfterHookPhase(name, token) => {
                if let Some(operation) = &mut self.operation
                    && operation.name == name
                    && operation.kind == OperationKind::Start
                {
                    operation.token = Some(token);
                    operation.status = "Running its after hook".to_owned();
                    operation.after_hook = true;
                }
                Task::none()
            }
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
            Message::InstallPhase(name, installing) => {
                if let Some(operation) = &mut self.operation
                    && operation.name == name
                    && operation.kind == OperationKind::Start
                {
                    operation.installing = installing;
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
                    self.reload_library()
                }
            }
            Message::StopFinished(name, result) => {
                if self.operation.as_ref().is_some_and(|operation| {
                    operation.name == name && operation.kind == OperationKind::Stop
                }) {
                    self.operation = None;
                }
                if self
                    .force_stop_at
                    .as_ref()
                    .is_some_and(|(stopping, _)| *stopping == name)
                {
                    self.force_stop_at = None;
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
                    self.reload_library()
                }
            }
            Message::OpenLog => self.open_log(),
            Message::CloseLog => {
                self.close_log();
                Task::none()
            }
            Message::LogChunk(generation, chunk) => Self::follow_if(
                self.log_view && self.log_generation == generation && self.log_buffer.push(&chunk),
            ),
            Message::LogFinished(generation, result) => {
                if self.log_view
                    && self.log_generation == generation
                    && let Err(error) = result
                {
                    self.log_error = Some(error);
                }
                Task::none()
            }
            Message::LogAction(action) => {
                Self::follow_if(self.log_view && self.log_buffer.act(action))
            }
            Message::LogScrolled(viewport) => {
                Self::follow_if(self.log_view && self.log_buffer.scrolled(viewport))
            }
            Message::LogFollow => Self::follow_if(self.log_buffer.follow()),
            Message::CopyLog => iced::clipboard::write(self.log_buffer.copy()),
            Message::Hover(name) => {
                self.hovered = name;
                Task::none()
            }
            Message::Refresh => {
                let library = if self.refreshing {
                    Task::none()
                } else {
                    self.refreshing = true;
                    self.reload_library()
                };
                let badge = if self.trash_refreshing {
                    Task::none()
                } else {
                    self.trash_refreshing = true;
                    if self.section == sidebar::Section::Trash {
                        self.reload_trash_page()
                    } else {
                        self.reload_badge()
                    }
                };
                Task::batch([library, badge, self.settings_tick()])
            }
            Message::Section(section) => self.navigate_section(section),
            Message::LibraryReloaded(generation, result) => {
                if generation == self.library_generation {
                    self.update(Message::Loaded(result))
                } else {
                    Task::none()
                }
            }
            Message::TrashReloaded(generation, badge) => {
                if generation == self.trash_generation {
                    self.update(Message::TrashBadgeLoaded(badge))
                } else {
                    Task::none()
                }
            }
            Message::TrashBadgeLoaded(badge) => {
                self.trash_refreshing = false;
                self.trash_badge = badge;
                Task::none()
            }
            Message::Loaded(Ok(cards)) => {
                self.refreshing = false;
                self.loaded = true;
                if self.hook_stop_confirm.as_ref().is_some_and(|name| {
                    !cards.iter().any(|card| {
                        &card.name == name
                            && card.busy_holder
                                == Some(wineo_core::operation::Operation::Hook(
                                    wineo_core::hooks::Phase::After,
                                ))
                    })
                }) {
                    self.hook_stop_confirm = None;
                    self.hook_stop_identity = None;
                }
                let art = self.refresh_art(&cards);
                self.merge(cards);
                Task::batch([art, self.load_visible_art(), self.load_panel_art()])
            }
            Message::Loaded(Err(error)) => {
                self.refreshing = false;
                self.loaded = true;
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
                        self.refresh_dimmed_images();
                    }
                    Err(_) => {
                        self.failed_images.insert(key);
                    }
                }
                Task::none()
            }
            Message::Scrolled(viewport) => {
                self.viewport_width = viewport.bounds().width;
                self.viewport_height = viewport.bounds().height;
                self.scroll_offset = viewport.absolute_offset().y;
                self.scroll_visible_until = Some(Instant::now() + Duration::from_millis(850));
                self.load_visible_art()
            }
            Message::GridCardLocated(name, generation, bounds) => {
                if self.selected.as_deref() != Some(&name)
                    || self.panel_hidden()
                    || generation != self.grid_motion.generation()
                {
                    return Task::none();
                }
                let top = bounds.card.y - bounds.viewport.y;
                let bottom = top + bounds.card.height;
                let offset = if top < bounds.offset.y {
                    top
                } else if bottom > bounds.offset.y + bounds.viewport.height {
                    bottom - bounds.viewport.height
                } else {
                    return Task::none();
                };
                self.scroll_offset = offset.max(0.0);
                iced::widget::operation::scroll_to(
                    iced::widget::Id::new(SCROLL_ID),
                    scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: self.scroll_offset,
                    },
                )
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
                self.viewport_width = (size.width - sidebar::WIDTH).max(0.0);
                self.viewport_height = (size.height - 48.0).max(0.0);
                self.grid_motion
                    .snap(self.panel_open, self.filtered().len());
                self.load_visible_art()
            }
            Message::Window(window::Event::CloseRequested) => {
                if self.foreground_busy() {
                    self.close_prompt = true;
                    Task::none()
                } else {
                    for page in self.art_pages.values() {
                        page.cancel_thumbnails();
                    }
                    self.cancel_remove_dialog();
                    iced::exit()
                }
            }
            Message::Window(_) => Task::none(),
            Message::ThemeChanged(mode) => {
                self.palette = Palette::from_mode(mode);
                Task::none()
            }
            Message::SpinnerTick(now) => {
                if self.spinning() {
                    self.spinner_angle = spinner::angle(self.spinner_start, now);
                }
                Task::none()
            }
            Message::PanelTick(generation, now) => {
                if self.section == sidebar::Section::Library {
                    self.panel_motion.tick(generation, now);
                }
                Task::none()
            }
            Message::GridTick(generation, now) => {
                if self.section == sidebar::Section::Library {
                    let was_active = self.grid_motion.active();
                    self.grid_motion.tick(generation, now);
                    if was_active
                        && !self.grid_motion.active()
                        && let Some(name) = self.grid_scroll_after_settle.take()
                        && self.selected.as_deref() == Some(&name)
                    {
                        return self.scroll_selection();
                    }
                }
                Task::none()
            }
            Message::Key(key, _, _) if self.close_prompt => {
                if key == keyboard::Key::Named(keyboard::key::Named::Escape) && !self.closing {
                    self.close_prompt = false;
                }
                Task::none()
            }
            Message::Key(key, modifiers, _) if self.hook_stop_confirm.is_some() => {
                self.hook_stop_key(&key, modifiers)
            }
            Message::Key(key, modifiers, _) if self.removal.is_some() => {
                removal::dialog_key(self, &key, modifiers)
            }
            Message::Key(key, modifiers, _) if self.downloads_confirm_open() => {
                downloads_page::key(self, &key, modifiers, false)
            }
            Message::Key(key, modifiers, _) if self.trash_confirm_open() => {
                trash_page::key(self, &key, modifiers)
            }
            Message::Key(key, modifiers, status) if self.flow.is_some() => {
                use iced::keyboard::{Key, key::Named};
                if sidebar::shortcut(&key, modifiers).is_some() {
                    return Task::none();
                }
                let generation = self.flow_generation;
                let width = self.flow_width();
                let own = if status == iced::event::Status::Captured {
                    None
                } else {
                    self.flow.as_mut().unwrap().key(&key, modifiers, width)
                };
                if let Some(task) = own {
                    task.map(move |event| Message::Flow(generation, event))
                } else {
                    match key.as_ref() {
                        Key::Character(choice)
                            if matches!(self.flow, Some(flow::Page::Choose(_))) =>
                        {
                            let event = match choice {
                                "1" => flow::Event::New,
                                "2" => flow::Event::Prefix,
                                "3" => flow::Event::Heroic,
                                "4" => flow::Event::Steam,
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
            }
            Message::Key(key, modifiers, status) => {
                let results_tab = self.checkup_key_scope().is_some()
                    && key == keyboard::Key::Named(keyboard::key::Named::Tab);
                if !results_tab && let Some(task) = removal::key(self, &key, modifiers, status) {
                    return task;
                }
                // shortcuts::capture normally takes these first. This covers a
                // key an open overlay saw and left uncaptured.
                if let Some(section) = sidebar::shortcut(&key, modifiers) {
                    if status == iced::event::Status::Captured {
                        Task::none()
                    } else {
                        self.navigate_section(section)
                    }
                } else if self.settings.is_some() {
                    self.settings_key(&key, modifiers, status == iced::event::Status::Captured)
                } else if self.art_page.is_some() {
                    self.art_key(&key, modifiers, status == iced::event::Status::Captured)
                } else if self.feature_page_open() {
                    self.feature_key(&key, modifiers, status == iced::event::Status::Captured)
                } else if self.section == sidebar::Section::Downloads {
                    downloads_page::key(
                        self,
                        &key,
                        modifiers,
                        status == iced::event::Status::Captured,
                    )
                } else if modifiers.control()
                    && matches!(key.as_ref(), keyboard::Key::Character("n" | "N"))
                {
                    self.update(Message::Add)
                } else {
                    iced::widget::operation::is_focused(iced::widget::Id::new(SEARCH_ID)).map(
                        move |focused| {
                            Message::KeyWithFocus(key.clone(), modifiers, status, focused)
                        },
                    )
                }
            }
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
            Message::Settings(event) => self.settings_update(event),
            Message::SteamGridDbKeyChangedAt(sequence, available) => {
                if sequence != self.steamgriddb_completed_sequence {
                    return Task::none();
                }
                self.set_steamgriddb_availability(sequence, available)
            }
            Message::SteamGridDbKeyPresenceLoaded(sequence, available) => {
                if sequence <= self.steamgriddb_completed_sequence {
                    return Task::none();
                }
                self.steamgriddb_sequence = self.steamgriddb_sequence.max(sequence);
                self.steamgriddb_completed_sequence = sequence;
                self.set_steamgriddb_availability(sequence, available)
            }
            Message::OpenArt => match self.selected.clone() {
                Some(name) => self.open_art(name),
                None => Task::none(),
            },
            Message::ContextArt(name) => self.open_art(name),
            Message::ArtPage(name, generation, event) => self.art_update(name, generation, event),
            Message::Add => {
                if self.management_busy()
                    || self.operation.is_some()
                    || self.settings.is_some()
                    || self.art_page.is_some()
                    || self.flow.is_some()
                    || self.section != sidebar::Section::Library
                {
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
                if self.management_busy()
                    || !self.transfer_actions(&name)
                    || self.operation.is_some()
                    || self.remove_reason(&name).is_some()
                    || self.settings.is_some()
                    || self.art_page.is_some()
                    || self.flow.is_some()
                {
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
                if self.selected.as_deref() != Some(&name) {
                    self.close_checkup();
                }
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
                self.cancel_management();
                self.cancel_features(force);
                for page in self.art_pages.values() {
                    page.cancel_thumbnails();
                }
                for token in self.pending_art_cancels.values() {
                    token.cancel();
                }
                self.cancel_checkup(force);
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
        if !panel_was_open && self.panel_open {
            self.panel_motion.open(Instant::now());
        }
        let task = if panel_was_open != self.panel_open {
            if self.panel_hidden() {
                self.grid_motion
                    .snap(self.panel_open, self.filtered().len());
            } else {
                self.grid_motion
                    .retarget(self.panel_open, self.filtered().len(), Instant::now());
            }
            task.chain(self.load_visible_art())
        } else {
            task
        };
        if self.panel_motion.active() && self.panel_hidden() {
            self.panel_motion.snap(self.panel_open);
        }
        if self.grid_motion.active() && self.panel_hidden() {
            self.grid_motion
                .snap(self.panel_open, self.filtered().len());
            self.grid_scroll_after_settle = None;
        }
        let task = if panel_was_open && !self.panel_open {
            task.chain(iced::advanced::widget::operate(
                iced::advanced::widget::operation::focusable::unfocus(),
            ))
        } else {
            task
        };
        if self.closing && !self.foreground_busy() {
            task.chain(iced::exit())
        } else {
            task
        }
    }
    fn follow_if(snap: bool) -> Task<Message> {
        if snap {
            logview::snap_to_end(LOG_ID)
        } else {
            Task::none()
        }
    }
    fn foreground_busy(&self) -> bool {
        self.filesystem_busy()
            || self.tools_busy()
            || self.change.is_some()
            || self.art_busy()
            || self.operation.is_some()
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
            .update_with_key_availability(event, self.steamgriddb_available)
            .map(move |event| Message::Flow(generation, event));
        if self.closing && !page.busy() {
            return task;
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
            if matches!(action, flow::Action::SteamKey) {
                return self.open_steam_key_settings();
            }
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
            return if self.foreground_busy() {
                Task::none()
            } else {
                iced::exit()
            };
        }
        match result {
            Ok(cards) => self.merge(cards),
            Err(error) => {
                self.error = Some(error);
                return Task::none();
            }
        }
        let (name, launch, settings) = match action {
            flow::Action::Choose | flow::Action::SteamKey => return Task::none(),
            flow::Action::Library { name, launch } => (name, launch, false),
            flow::Action::Settings { name, launch } => (Some(name), launch, true),
        };
        let Some(name) = name else {
            return Task::none();
        };
        self.search.clear();
        if self.selected.as_deref() != Some(&name) {
            self.close_checkup();
        }
        self.selected = Some(name.clone());
        let was_open = self.panel_open;
        self.panel_open = true;
        if !was_open {
            self.panel_motion.open(Instant::now());
        }
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
        let needle = self.search.to_lowercase();
        let old_layout: Vec<_> = self
            .filtered()
            .iter()
            .map(|card| (&card.name, &card.display_title))
            .collect();
        let new_layout: Vec<_> = cards
            .iter()
            .filter(|card| {
                card.display_title.to_lowercase().contains(&needle)
                    || card.name.to_lowercase().contains(&needle)
            })
            .map(|card| (&card.name, &card.display_title))
            .collect();
        let layout_changed = old_layout != new_layout;
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
        if layout_changed {
            self.grid_motion
                .snap(self.panel_open, self.filtered().len());
            self.grid_scroll_after_settle = None;
        }
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
        self.refresh_dimmed_images();
        self.error = None;
    }
    fn refresh_dimmed_images(&mut self) {
        let keys: HashSet<_> = self
            .cards
            .iter()
            .filter(|card| card.not_installed())
            .flat_map(|card| {
                // The panel's hero and logo dim along with the poster.
                let panel = card.panel_art.values().flat_map(|art| {
                    art.hero
                        .iter()
                        .map(|hero| (hero.clone(), artwork::Kind::Hero))
                        .chain(
                            art.logo
                                .iter()
                                .map(|logo| (logo.clone(), artwork::Kind::Logo)),
                        )
                });
                card.art
                    .iter()
                    .map(|art| (art.clone(), artwork::Kind::Grid))
                    .chain(panel)
                    .collect::<Vec<_>>()
            })
            .collect();
        self.dimmed_images.retain(|key, _| keys.contains(key));
        for key in keys {
            if !self.dimmed_images.contains_key(&key)
                && let Some(handle) = self.images.get(&key).and_then(artwork::grayscale)
            {
                self.dimmed_images.insert(key, handle);
            }
        }
    }
    pub(crate) fn filtered(&self) -> Vec<&Card> {
        let needle = self.search.to_lowercase();
        self.cards
            .iter()
            .filter(|card| {
                card.display_title.to_lowercase().contains(&needle)
                    || card.name.to_lowercase().contains(&needle)
            })
            .collect()
    }
    fn columns(&self) -> usize {
        grid_motion::columns_for_viewport(self.viewport_width, self.panel_open)
    }
    fn panel_reserved(&self) -> bool {
        self.section == sidebar::Section::Library && self.panel_motion.reserved()
    }
    fn panel_hidden(&self) -> bool {
        self.section != sidebar::Section::Library
            || self.settings.is_some()
            || self.art_page.is_some()
            || self.tool_page.is_some()
            || self.portable_page.is_some()
            || self.flow.is_some()
            || self.removal.is_some()
            || self.close_prompt
    }
    fn load_visible_art(&mut self) -> Task<Message> {
        let pitch = CARD_ROW;
        let first = ((self.scroll_offset - 2.0 * pitch).max(0.0) / pitch).floor() as usize;
        let last =
            ((self.scroll_offset + self.viewport_height + 2.0 * pitch) / pitch).ceil() as usize;
        let filtered = self.filtered();
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        for columns in [
            grid_motion::columns_for_viewport(self.viewport_width, false),
            grid_motion::columns_for_viewport(self.viewport_width, true),
        ] {
            for card in filtered
                .iter()
                .skip(first * columns)
                .take(last.saturating_sub(first) * columns)
            {
                if let Some(art) = &card.art {
                    let key = (art.clone(), artwork::Kind::Grid);
                    if seen.insert(key.clone()) {
                        paths.push(key);
                    }
                }
            }
        }
        self.load_images(paths)
    }
    fn load_images(&mut self, paths: Vec<ArtKey>) -> Task<Message> {
        let mut tasks = Vec::new();
        let cache = self.paths.art_cache_dir();
        for key in paths {
            if self.images.contains_key(&key)
                || self.failed_images.contains(&key)
                || !self.loading.insert(key.clone())
            {
                continue;
            }
            let decodes = self.decodes.clone();
            let cache = cache.clone();
            tasks.push(Task::perform(
                async move {
                    let decoded = match decodes.acquire_owned().await {
                        Ok(permit) => tokio::task::spawn_blocking({
                            let key = key.clone();
                            move || {
                                let decoded = artwork::decode(&key.0.path, key.1, Some(&cache))
                                    .map_err(|e| e.to_string());
                                drop(permit);
                                decoded
                            }
                        })
                        .await
                        .map_err(|e| e.to_string()),
                        Err(error) => Err(error.to_string()),
                    };
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

/// Maps a runtime event to an app message. The mouse Back button becomes an
/// uncaptured Escape so every page's existing Escape handling runs unchanged.
pub(crate) fn event_message(event: iced::Event, status: iced::event::Status) -> Option<Message> {
    match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Message::Key(key, modifiers, status))
        }
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::PointerMoved(position))
        }
        // Ignored regardless of status: a text input or button that saw the
        // click must not turn Back into a captured key that pages skip.
        iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Back)) => {
            Some(Message::Key(
                keyboard::Key::Named(keyboard::key::Named::Escape),
                keyboard::Modifiers::empty(),
                iced::event::Status::Ignored,
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn card(name: &str, state: State, seconds: i64) -> Card {
        Card {
            name: name.into(),
            kind: None,
            display_title: name.into(),
            steam: None,
            state,
            lost: false,
            prefix: None,
            busy: false,
            busy_holder: None,
            unresolved: false,
            error: None,
            playtime: playtime_label(seconds),
            sessions: "0".to_owned(),
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

    fn tool_app() -> App {
        let mut app = app();
        app.cards = ["notes", "alias", "editor"]
            .into_iter()
            .map(|name| {
                let mut card = card(name, State::Inactive, 0);
                card.prefix = Some(PathBuf::from(if name == "editor" {
                    "/tmp/editor-prefix"
                } else {
                    "/tmp/notes-prefix"
                }));
                card
            })
            .collect();
        let _ = app.update(Message::OpenTool(
            "notes".into(),
            tools_page::Kind::Winetricks,
        ));
        let _ = app.update(Message::Tool(
            "notes".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::OpenWinetricks,
        ));
        app
    }

    /// Snapshots, Run program, and Winetricks are Wine-only. Clone and Copy
    /// to portable work for native environments too, but never for a proxied
    /// one, whose launcher owns it.
    #[test]
    fn the_context_menu_follows_each_card_kind() {
        use crate::library::CardKind;

        let mut app = app();
        app.cards = [
            ("notes", Some(CardKind::Wine), true),
            ("dusk", Some(CardKind::Native), false),
            ("bg3", Some(CardKind::Proxied), false),
            ("broken", None, false),
        ]
        .into_iter()
        .map(|(name, kind, prefix)| {
            let mut card = card(name, State::Inactive, 0);
            card.kind = kind;
            card.prefix = prefix.then(|| PathBuf::from("/tmp/notes-prefix"));
            card
        })
        .collect();
        for (name, wine, transfer) in [
            ("notes", true, true),
            ("dusk", false, true),
            ("bg3", false, false),
            ("broken", false, true),
        ] {
            assert_eq!(app.wine_actions(name), wine, "{name}");
            assert_eq!(app.transfer_actions(name), transfer, "{name}");
        }

        let _ = app.update(Message::Section(sidebar::Section::Library));
        let _ = app.update(Message::CloneEnvironment("bg3".into()));
        assert!(app.flow.is_none());
        let _ = app.update(Message::CopyPortable("bg3".into()));
        assert!(app.portable_page.is_none());
        let _ = app.update(Message::OpenTool("bg3".into(), tools_page::Kind::Exec));
        assert!(app.tool_page.is_none());
        let _ = app.update(Message::OpenTool(
            "bg3".into(),
            tools_page::Kind::Winetricks,
        ));
        assert!(app.tool_page.is_none());
        let _ = app.update(Message::SnapshotsEnvironment("bg3".into()));
        assert!(app.snapshots_page.is_none());

        let _ = app.update(Message::CopyPortable("dusk".into()));
        assert_eq!(app.portable_page.as_deref(), Some("dusk"));
    }

    #[test]
    fn native_cards_can_still_be_cloned() {
        use crate::library::CardKind;

        let mut app = app();
        let mut native = card("dusk", State::Inactive, 0);
        native.kind = Some(CardKind::Native);
        app.cards = vec![native];
        let _ = app.update(Message::Section(sidebar::Section::Library));
        let _ = app.update(Message::CloneEnvironment("dusk".into()));
        assert!(app.flow.is_some());
    }

    #[test]
    fn tools_block_shared_prefix_changes_but_leave_other_environments_usable() {
        let mut app = tool_app();
        assert!(app.tools_busy());
        assert!(!app.management_busy());
        assert!(app.remove_reason("notes").is_some());
        assert!(app.remove_reason("alias").is_some());
        assert!(app.remove_reason("editor").is_none());
        assert!(app.tool_blocks_launch("alias"));
        assert!(!app.tool_blocks_launch("editor"));
        app.cards[0].prefix = Some("/tmp/replaced-prefix".into());
        assert!(app.tool_prefix_busy("alias"));
        let _ = app.update(Message::Section(sidebar::Section::Library));
        let _ = app.update(Message::CloneEnvironment("editor".into()));
        assert!(app.flow.is_some());
        assert!(app.tools_busy());
    }

    #[test]
    fn hidden_tool_messages_keep_worker_but_cannot_navigate_or_start_work() {
        let mut app = tool_app();
        let _ = app.update(Message::Section(sidebar::Section::Library));
        let _ = app.update(Message::Tool(
            "notes".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::Settings,
        ));
        assert!(app.settings.is_none());
        assert!(app.tools_busy());
        let _ = app.update(Message::Tool(
            "notes".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::Finished(999, Ok(Some(0))),
        ));
        assert!(app.tools_busy());
        let _ = app.update(Message::Tool(
            "notes".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::Finished(1, Ok(Some(0))),
        ));
        assert!(!app.tools_busy());
        assert!(app.tool_page.is_none());
        let _ = app.update(Message::OpenTool(
            "notes".into(),
            tools_page::Kind::Winetricks,
        ));
        assert!(!app.tools_busy());
        assert_eq!(app.tool_pages.len(), 1);
    }

    #[test]
    fn closing_waits_for_all_tools_after_leaving_their_pages() {
        let mut app = tool_app();
        let _ = app.update(Message::OpenTool(
            "editor".into(),
            tools_page::Kind::Winetricks,
        ));
        let _ = app.update(Message::Tool(
            "editor".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::OpenWinetricks,
        ));
        let _ = app.update(Message::Section(sidebar::Section::Downloads));
        let _ = app.update(Message::Window(window::Event::CloseRequested));
        assert!(app.close_prompt);
        let _ = app.update(Message::CancelAndClose);
        assert!(app.closing && app.foreground_busy());
        assert!(
            app.tool_pages
                .values()
                .all(tools_page::Page::force_available)
        );
        let _ = app.update(Message::Tool(
            "notes".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::Finished(1, Ok(Some(0))),
        ));
        assert!(app.foreground_busy());
        let _ = app.update(Message::Tool(
            "editor".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::Finished(1, Ok(Some(0))),
        ));
        assert!(!app.foreground_busy());
    }

    #[test]
    fn portable_review_uses_the_slot_and_survives_sidebar_navigation() {
        let mut app = app();
        let _ = app.update(Message::CopyPortable("native".into()));
        let _ = app.update(Message::Portable(
            "native".into(),
            portable_page::Event::SourceLoaded(Ok(true)),
        ));
        let _ = app.update(Message::Portable(
            "native".into(),
            portable_page::Event::Folder("/tmp/portable-review".into()),
        ));
        let _ = app.update(Message::Portable(
            "native".into(),
            portable_page::Event::Review,
        ));
        assert!(app.filesystem_busy() && app.management_busy());
        let _ = app.update(Message::Section(sidebar::Section::Checkup));
        assert!(app.portable_page.is_none());
        assert!(app.filesystem_busy());
        let _ = app.update(Message::CopyPortable("second".into()));
        let _ = app.update(Message::Portable(
            "second".into(),
            portable_page::Event::SourceLoaded(Ok(true)),
        ));
        let _ = app.update(Message::Portable(
            "second".into(),
            portable_page::Event::Folder("/tmp/second-copy".into()),
        ));
        let _ = app.update(Message::Portable(
            "second".into(),
            portable_page::Event::Review,
        ));
        assert!(!app.portable_pages["second"].busy());
        assert!(app.portable_pages["native"].busy());
    }

    #[test]
    fn snapshot_shortcut_preserves_downloads_shortcut_and_page_results() {
        assert_eq!(
            sidebar::shortcut(
                &keyboard::Key::Character("4".into()),
                keyboard::Modifiers::CTRL
            ),
            Some(sidebar::Section::Downloads)
        );
        assert_eq!(
            sidebar::shortcut(
                &keyboard::Key::Character("5".into()),
                keyboard::Modifiers::CTRL
            ),
            Some(sidebar::Section::Snapshots)
        );
        let mut app = app();
        let _ = app.update(Message::SnapshotsEnvironment("notes".into()));
        assert_eq!(app.section, sidebar::Section::Snapshots);
        let _ = app.update(Message::Section(sidebar::Section::Library));
        let _ = app.update(Message::Section(sidebar::Section::Snapshots));
        assert_eq!(
            app.snapshots_page.as_ref().unwrap().selected_name(),
            Some("notes")
        );
    }

    #[test]
    fn cached_winetricks_page_refreshes_declarations_on_reopen() {
        let mut app = app();
        let mut item = card("notes", State::Inactive, 0);
        item.prefix = Some("/tmp/notes-prefix".into());
        app.cards.push(item);
        let _ = app.update(Message::OpenTool(
            "notes".into(),
            tools_page::Kind::Winetricks,
        ));
        let _ = app.update(Message::Section(sidebar::Section::Library));
        let task = app.update(Message::OpenTool(
            "notes".into(),
            tools_page::Kind::Winetricks,
        ));
        assert!(task.units() > 0);
        assert_eq!(app.tool_pages.len(), 1);
        assert!(!app.tools_busy());
    }

    #[test]
    fn winetricks_refuses_running_shared_prefix_but_exec_can_coexist() {
        let mut app = app();
        app.cards = ["notes", "alias"]
            .into_iter()
            .map(|name| {
                let mut card = card(
                    name,
                    if name == "alias" {
                        State::Running
                    } else {
                        State::Inactive
                    },
                    0,
                );
                card.prefix = Some("/tmp/shared-prefix".into());
                card
            })
            .collect();
        assert!(
            app.tool_start_reason("notes", tools_page::Kind::Winetricks)
                .is_some()
        );
        assert!(
            app.tool_start_reason("notes", tools_page::Kind::Exec)
                .is_none()
        );
        assert!(app.remove_reason("notes").is_some());
        let _ = app.update(Message::OpenTool(
            "notes".into(),
            tools_page::Kind::Winetricks,
        ));
        let _ = app.update(Message::Tool(
            "notes".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::OpenWinetricks,
        ));
        assert!(!app.tools_busy());
    }

    #[test]
    fn restore_markers_block_ordinary_changes_without_disabling_recovery() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let marker = paths.restore_marker("notes");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, "invalid marker").unwrap();
        let app = App::boot(paths).0;
        assert_eq!(
            app.remove_reason("notes").as_deref(),
            Some("Roll back restore first.")
        );
        assert!(
            app.tool_start_reason("notes", tools_page::Kind::Exec)
                .is_some()
        );
        assert!(
            app.tool_start_reason("notes", tools_page::Kind::Winetricks)
                .is_some()
        );
        assert!(app.prefix_change_reason("notes").is_none());
        let marker = app.paths.restore_marker("notes");
        std::fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing-marker"), marker).unwrap();
        assert!(app.restore_pending("notes"));
        assert!(app.remove_reason("notes").is_some());
        assert!(
            app.tool_start_reason("notes", tools_page::Kind::Exec)
                .is_some()
        );
        assert!(app.prefix_change_reason("notes").is_none());
    }

    #[test]
    fn portable_success_notifies_destination_and_enter_returns_to_library() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        std::fs::create_dir_all(paths.environments_dir()).unwrap();
        std::fs::write(paths.environment_config("native"), "kind = 'native'\n").unwrap();
        let folder = root.path().join("portable");
        let destination = Paths::select(
            |key| (key == "HOME").then(|| root.path().as_os_str().to_owned()),
            Some(&folder),
        )
        .unwrap();
        let review = wineo_core::portable::inspect_native(&paths, &destination, "native").unwrap();
        let mut app = App::boot(paths).0;
        let _ = app.update(Message::CopyPortable("native".into()));
        for event in [
            portable_page::Event::SourceLoaded(Ok(true)),
            portable_page::Event::Folder(folder.display().to_string()),
            portable_page::Event::Review,
            portable_page::Event::ReviewLoaded(
                1,
                Ok(Box::new(portable_page::Review::Native(Box::new(review)))),
            ),
            portable_page::Event::Copy,
            portable_page::Event::CopyFinished(2, Ok(())),
        ] {
            let _ = app.update(Message::Portable("native".into(), event));
        }
        assert_eq!(
            app.feature_notice.as_ref().unwrap().0,
            format!("Copied native to {}.", folder.display())
        );
        let _ = app.feature_key(
            &keyboard::Key::Named(keyboard::key::Named::Enter),
            keyboard::Modifiers::empty(),
            false,
        );
        assert!(app.portable_page.is_none());
        assert!(!app.filesystem_busy());
    }

    #[test]
    fn overlays_preserve_library_scroll_state() {
        use iced::advanced::widget::{Tree, tree};

        fn scroll_state(widget: &Tree, tag: tree::Tag) -> Option<*const ()> {
            if widget.tag == tag
                && let tree::State::Some(state) = &widget.state
            {
                return Some(std::ptr::from_ref(state.as_ref()).cast());
            }
            widget
                .children
                .iter()
                .find_map(|child| scroll_state(child, tag))
        }

        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let scroll: iced::Element<'_, Message> = scrollable(iced::widget::Space::new()).into();
        let tag = scroll.as_widget().tag();
        let mut tree = Tree::new(app.view().as_widget());
        // The offset lives in iced's private scrollable state, not App::scroll_offset.
        let state = scroll_state(&tree, tag).unwrap();

        let _ = app.update(Message::Context("game".into()));
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));
        let _ = app.update(Message::CloseMenu);
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));

        let _ = app.update(Message::Select("game".into()));
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));
        let _ = app.update(Message::ClosePanel);
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));

        app.flow = Some(flow::Page::choose(app.paths.clone()));
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));
        app.close_prompt = true;
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));
        app.flow = None;
        app.close_prompt = false;
        tree.diff(app.view().as_widget());
        assert_eq!(scroll_state(&tree, tag), Some(state));
    }

    #[test]
    fn late_art_and_add_results_use_current_key_availability() {
        for (initial, available, deliver_results_after) in [
            (false, true, false),
            (false, true, true),
            (true, false, false),
            (true, false, true),
        ] {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().to_owned();
            let paths =
                Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                    .unwrap();
            let config = paths.environment_config("game");
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();

            let mut app = App::boot(paths.clone()).0;
            let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, initial));
            let identity = art_page::EnvironmentIdentity::read(&paths, "game").unwrap();
            app.art_page = Some("game".to_owned());
            app.art_pages.insert(
                "game".to_owned(),
                art_page::Page::new("game".to_owned(), 7, identity),
            );
            let snapshot = art_page::load_snapshot(&paths, "game").unwrap();
            app.flow_generation = 1;
            app.flow = Some(flow::Page::choose(paths.clone()));
            let _ = app.flow_update(1, flow::Event::New);
            let configured = || {
                flow::Event::Create(Box::new(flow::new::Event::Art(
                    flow::artwork::Event::Configured(1, Ok(wineo_core::config::ArtFetch::Icon)),
                )))
            };
            let loaded = || {
                Message::ArtPage(
                    "game".to_owned(),
                    7,
                    art_page::Event::Loaded(1, Ok(snapshot.clone())),
                )
            };

            if !deliver_results_after {
                let _ = app.update(loaded());
                let _ = app.flow_update(1, configured());
            }
            app.steamgriddb_completed_sequence = 2;
            let _ = app.update(Message::SteamGridDbKeyChangedAt(2, available));
            if deliver_results_after {
                let _ = app.update(loaded());
                let _ = app.flow_update(1, configured());
            }

            assert_eq!(app.steamgriddb_available, available);
            let _ = app.art_update(
                "game".to_owned(),
                7,
                art_page::Event::Find(wineo_core::art::Kind::Grid),
            );
            assert_eq!(app.art_pages["game"].is_changing(), available);
            if available {
                let _ = app.art_update("game".to_owned(), 7, art_page::Event::CancelChange);
            }
            let _ = app.art_update("game".to_owned(), 7, art_page::Event::Change);
            assert_eq!(app.art_pages["game"].is_changing(), available);
            let _ = app.flow_update(
                1,
                flow::Event::Create(Box::new(flow::new::Event::Art(
                    flow::artwork::Event::Fetch(true),
                ))),
            );
            assert_eq!(app.flow.as_ref().unwrap().art_fetching(), available);

            let _ = app.update(Message::SteamGridDbKeyChangedAt(1, !available));
            assert_eq!(app.steamgriddb_available, available);
        }
    }

    #[test]
    fn steamgriddb_availability_never_regresses_to_an_older_sequence() {
        let mut app = app();
        let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(3, true));
        let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(2, false));
        assert!(app.steamgriddb_available);
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
    fn an_empty_library_stays_shown_through_background_refreshes() {
        let mut app = app();
        assert!(!app.loaded, "nothing claims empty before the first load");
        let _ = app.update(Message::Loaded(Ok(Vec::new())));
        assert!(app.loaded);
        let _ = app.update(Message::Refresh);
        assert!(app.refreshing);
        assert!(app.loaded, "a refresh must not hide the empty state");
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
        let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
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
        let mut named = card("dusk_id", State::Inactive, 0);
        named.display_title = "Dusklight".into();
        app.cards = vec![named, card("Other", State::Inactive, 0)];
        app.search = "LIGHT".into();
        assert_eq!(app.filtered().len(), 1);
        assert_eq!(app.filtered()[0].display_title, "Dusklight");
        assert_eq!(app.filtered()[0].name, "dusk_id");
        app.search = "DUSK_ID".into();
        assert_eq!(app.filtered().len(), 1);
        assert_eq!(app.filtered()[0].name, "dusk_id");
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
    fn steam_install_changes_reuse_art_and_update_the_grayscale_cache() {
        let mut app = app();
        let art = art_file("/tmp/steam-grid.png", 10);
        let key = (art.clone(), artwork::Kind::Grid);
        let mut card = card_with_art(art);
        card.kind = Some(crate::library::CardKind::Proxied);
        card.steam = Some(crate::library::SteamDetails {
            install: wineo_core::steam::AppState::Installed.into(),
            proton: None,
            size: None,
        });
        app.merge(vec![card.clone()]);
        let _ = app.update(Message::ArtLoaded(
            key.clone(),
            Ok(image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255])),
        ));
        assert!(app.dimmed_images.is_empty());
        card.steam.as_mut().unwrap().install = wineo_core::steam::AppState::NotInstalled.into();
        app.merge(vec![card.clone()]);
        assert!(app.dimmed_images.contains_key(&key));
        assert!(app.images.contains_key(&key));
        card.steam.as_mut().unwrap().install =
            wineo_core::steam::AppState::Unknown("Missing Steam".to_owned()).into();
        app.merge(vec![card]);
        assert!(app.dimmed_images.is_empty());
        assert!(app.images.contains_key(&key));
        app.merge(Vec::new());
        assert!(app.images.is_empty());
    }

    fn steam_card(name: &str, install: wineo_core::steam::AppState) -> Card {
        let mut card = card(name, State::Inactive, 0);
        card.kind = Some(crate::library::CardKind::Proxied);
        card.tags = vec!["game".into()];
        card.targets = vec!["steam".into()];
        card.default_target = Some("steam".into());
        card.steam = Some(crate::library::SteamDetails {
            install: install.into(),
            proton: None,
            size: None,
        });
        card
    }

    #[test]
    fn the_steam_key_link_opens_global_settings_at_the_key_row() {
        let mut app = app();
        let _ = app.update(Message::Add);
        let generation = app.flow_generation;
        let _ = app.flow_update(generation, flow::Event::Steam);
        assert!(app.flow.is_some());
        app.flow = None;
        let _ = app.open_steam_key_settings();
        let page = app.settings.as_ref().unwrap();
        assert!(page.is_global());
        assert!(page.reveal_steam_key);
    }

    #[test]
    fn install_replaces_play_and_asks_steam_once() {
        use wineo_core::steam::AppState;
        let mut app = app();
        app.merge(vec![
            steam_card("owned", AppState::NotInstalled),
            steam_card("queued", AppState::InstallQueued),
            steam_card("installed", AppState::Installed),
        ]);
        for (name, installable) in [("owned", true), ("queued", true), ("installed", false)] {
            let card = app.cards.iter().find(|card| card.name == name).unwrap();
            assert_eq!(card.installable(), installable, "{name}");
            assert_eq!(panel::install_enabled(card, false), installable, "{name}");
            assert!(!panel::install_enabled(card, true), "{name}");
        }

        let _ = app.update(Message::Select("installed".into()));
        assert_eq!(app.update(Message::Install).units(), 0);
        assert!(app.steam_request.is_none());

        let _ = app.update(Message::Select("owned".into()));
        app.panel_note = Some("old".into());
        assert!(app.update(Message::Install).units() > 0);
        assert_eq!(app.steam_request.as_deref(), Some("owned"));
        assert!(app.panel_note.is_none());
        assert!(app.spinning());
        // A second press waits for Steam's answer.
        assert_eq!(app.update(Message::Install).units(), 0);

        let _ = app.update(Message::SteamRequested(
            "owned".into(),
            Ok("Asked Steam to install owned.".into()),
        ));
        assert!(app.steam_request.is_none());
        assert_eq!(
            app.panel_note.as_deref(),
            Some("Asked Steam to install owned.")
        );
        assert!(!app.spinning());
    }

    #[test]
    fn start_stays_disabled_until_steam_has_the_game() {
        use wineo_core::steam::AppState;
        for (install, playable) in [
            (AppState::NotInstalled, false),
            (AppState::InstallQueued, false),
            (AppState::Installing(Some(40)), false),
            (AppState::Installing(None), false),
            (AppState::Uninstalling, false),
            (AppState::Updating, true),
            (AppState::Installed, true),
        ] {
            let label = format!("{install:?}");
            let mut app = app();
            app.merge(vec![steam_card("game", install)]);
            let card = &app.cards[0];
            assert_eq!(
                panel::play_enabled(card, false, Some("steam")),
                playable,
                "{label}"
            );
            let _ = app.update(Message::Select("game".into()));
            let _ = app.update(Message::Play);
            assert_eq!(app.operation.is_some(), playable, "{label}");
        }
    }

    #[test]
    fn steam_request_refusals_show_in_the_panel() {
        let mut app = app();
        app.merge(vec![steam_card(
            "game",
            wineo_core::steam::AppState::Installed,
        )]);
        let _ = app.update(Message::Select("game".into()));
        app.steam_request = Some("game".into());
        let _ = app.update(Message::SteamRequested(
            "game".into(),
            Err(PanelError::from_error(&anyhow::anyhow!(
                "Stop game before uninstalling it."
            ))),
        ));
        assert!(app.steam_request.is_none());
        assert_eq!(
            app.panel_error.as_ref().unwrap().text,
            "Stop game before uninstalling it."
        );
    }

    #[test]
    fn uninstall_is_offered_only_for_installed_steam_games() {
        use wineo_core::steam::AppState;
        let mut app = app();
        let mut wine = card("wine", State::Inactive, 0);
        wine.kind = Some(crate::library::CardKind::Wine);
        wine.prefix = Some(PathBuf::from("/tmp/wine-prefix"));
        let mut native = card("native", State::Inactive, 0);
        native.kind = Some(crate::library::CardKind::Native);
        let mut broken = steam_card("broken", AppState::Installed);
        broken.error = Some("broken config".into());
        app.merge(vec![
            steam_card("installed", AppState::Installed),
            steam_card("updating", AppState::Updating),
            steam_card("owned", AppState::NotInstalled),
            steam_card("queued", AppState::InstallQueued),
            steam_card("installing", AppState::Installing(Some(5))),
            steam_card("uninstalling", AppState::Uninstalling),
            steam_card("missing", AppState::Unknown("Steam isn't found".into())),
            broken,
            wine,
            native,
        ]);
        for (name, offered) in [
            ("installed", true),
            ("updating", true),
            ("owned", false),
            ("queued", false),
            ("installing", false),
            ("uninstalling", false),
            ("missing", false),
            ("broken", false),
            ("wine", false),
            ("native", false),
        ] {
            assert_eq!(app.uninstall_action(name), offered, "{name}");
        }

        assert_eq!(app.update(Message::Uninstall("owned".into())).units(), 0);
        assert!(app.steam_request.is_none());

        // The menu's Uninstall selects the game, so Steam's answer has a place.
        app.action_menu = Some("installed".into());
        assert!(app.update(Message::Uninstall("installed".into())).units() > 0);
        assert!(app.action_menu.is_none());
        assert_eq!(app.selected.as_deref(), Some("installed"));
        assert!(app.panel_open);
        assert_eq!(app.steam_request.as_deref(), Some("installed"));
        assert_eq!(app.update(Message::Uninstall("updating".into())).units(), 0);
    }

    #[test]
    fn steam_requests_use_the_install_path_and_its_notes() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let steamapps = root.path().join(".local/share/Steam/steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        std::fs::create_dir_all(paths.environments_dir()).unwrap();
        std::fs::write(
            paths.environment_config("native"),
            "kind = 'native'
",
        )
        .unwrap();
        std::fs::write(
            paths.environment_config("game"),
            "kind = 'proxied'\nlauncher = 'steam'\napp_id = '123'\n",
        )
        .unwrap();
        let request = |name, action| {
            worker::steam_request_now(&paths, name, action).map_err(|error| error.to_string())
        };
        use wineo_core::steam_install::Action;
        assert_eq!(
            request("native", Action::Install).unwrap_err(),
            "Only Steam games can be installed."
        );
        assert_eq!(
            request("game", Action::Uninstall).unwrap(),
            "Not installed."
        );
        std::fs::write(
            steamapps.join("appmanifest_123.acf"),
            r#""AppState" { "appid" "123" "name" "Game" "StateFlags" "4" }"#,
        )
        .unwrap();
        assert_eq!(
            request("game", Action::Install).unwrap(),
            "Already installed."
        );
        std::fs::write(
            steamapps.join("appmanifest_123.acf"),
            r#""AppState" { "appid" "123" "name" "Game" "StateFlags" "1026" }"#,
        )
        .unwrap();
        assert_eq!(
            request("game", Action::Install).unwrap(),
            "Steam is already installing game."
        );
    }

    #[test]
    fn panel_art_is_dimmed_for_games_steam_doesnt_have() {
        use wineo_core::steam::AppState;
        let mut app = app();
        let hero = art_file("/tmp/steam-hero.png", 10);
        let logo = art_file("/tmp/steam-logo.png", 11);
        let mut card = steam_card("game", AppState::NotInstalled);
        card.panel_art.insert(
            "steam".into(),
            crate::library::ArtPaths {
                hero: Some(hero.clone()),
                logo: Some(logo.clone()),
            },
        );
        app.merge(vec![card.clone()]);
        let blue = || image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
        for key in [
            (hero.clone(), artwork::Kind::Hero),
            (logo.clone(), artwork::Kind::Logo),
        ] {
            let _ = app.update(Message::ArtLoaded(key.clone(), Ok(blue())));
            assert!(app.dimmed_images.contains_key(&key));
        }
        let gray = |handle: Option<&image::Handle>| match handle {
            Some(image::Handle::Rgba { pixels, .. }) => {
                pixels[0] == pixels[1] && pixels[1] == pixels[2]
            }
            _ => panic!("expected decoded art"),
        };
        let (shown_hero, shown_logo) = app.panel_images(&app.cards[0], Some(&hero), Some(&logo));
        assert!(gray(shown_hero));
        assert!(gray(shown_logo));

        card.steam.as_mut().unwrap().install = AppState::Installed.into();
        app.merge(vec![card]);
        assert!(app.dimmed_images.is_empty());
        let (shown_hero, shown_logo) = app.panel_images(&app.cards[0], Some(&hero), Some(&logo));
        assert!(!gray(shown_hero));
        assert!(!gray(shown_logo));
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

    fn mouse_button(button: iced::mouse::Button) -> iced::Event {
        iced::Event::Mouse(iced::mouse::Event::ButtonPressed(button))
    }

    fn back_key(status: iced::event::Status) -> Message {
        event_message(mouse_button(iced::mouse::Button::Back), status)
            .expect("Back should map to a key message")
    }

    /// Delivers a mouse Back press to a page that handles keys directly.
    pub(crate) fn press_back(app: &mut App, status: iced::event::Status) {
        let _ = app.update(back_key(status));
    }

    /// Delivers a mouse Back press on the library, where the Key message
    /// continues as KeyWithFocus once the search focus is known.
    fn press_back_in_library(app: &mut App, status: iced::event::Status) {
        let Message::Key(key, modifiers, status) = back_key(status) else {
            panic!("Back should map to a key message");
        };
        let _ = app.update(Message::Key(key.clone(), modifiers, status));
        let _ = app.update(Message::KeyWithFocus(key, modifiers, status, false));
    }

    #[test]
    fn mouse_back_maps_to_uncaptured_escape_and_forward_does_nothing() {
        for status in [iced::event::Status::Ignored, iced::event::Status::Captured] {
            assert!(matches!(
                event_message(mouse_button(iced::mouse::Button::Back), status),
                Some(Message::Key(
                    keyboard::Key::Named(keyboard::key::Named::Escape),
                    modifiers,
                    iced::event::Status::Ignored,
                )) if modifiers.is_empty()
            ));
            for button in [
                iced::mouse::Button::Forward,
                iced::mouse::Button::Left,
                iced::mouse::Button::Right,
            ] {
                assert!(event_message(mouse_button(button), status).is_none());
            }
        }
    }

    #[test]
    fn mouse_back_closes_log_then_panel_then_clears_search() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Select("game".into()));
        app.log_view = true;
        app.search = "ga".into();
        // A captured press must still act, like an uncaptured Escape.
        press_back_in_library(&mut app, iced::event::Status::Captured);
        assert!(!app.log_view);
        assert!(app.panel_open);
        press_back_in_library(&mut app, iced::event::Status::Ignored);
        assert!(!app.panel_open);
        assert_eq!(app.search, "ga");
        press_back_in_library(&mut app, iced::event::Status::Ignored);
        assert!(app.search.is_empty());
    }

    #[test]
    fn closing_panel_cancels_log_before_motion_finishes() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Select("game".into()));
        let opened = app.panel_motion.generation();
        let _ = app.update(Message::PanelTick(
            opened,
            Instant::now() + Duration::from_millis(200),
        ));
        assert!(app.panel_motion.interactive());
        let token = Token::new();
        app.log_token = Some(token.clone());
        app.log_view = true;

        let _ = app.update(Message::ClosePanel);
        assert!(!app.panel_open);
        assert!(!app.log_view);
        assert!(token.is_canceled());
        assert!(app.panel_reserved());
        assert!(!app.panel_motion.interactive());
        let closing = app.panel_motion.generation();
        let _ = app.update(Message::PanelTick(
            closing,
            Instant::now() + Duration::from_millis(200),
        ));
        assert!(!app.panel_reserved());
    }

    #[test]
    fn changing_selection_does_not_replay_panel_entrance() {
        let mut app = app();
        app.cards = vec![
            card("one", State::Inactive, 0),
            card("two", State::Inactive, 0),
        ];
        let _ = app.update(Message::Select("one".into()));
        let opened = app.panel_motion.generation();
        let _ = app.update(Message::PanelTick(
            opened,
            Instant::now() + Duration::from_millis(200),
        ));
        let _ = app.update(Message::Select("two".into()));
        assert_eq!(app.selected.as_deref(), Some("two"));
        assert_eq!(app.panel_motion.generation(), opened);
        assert!(app.panel_motion.interactive());
    }

    #[test]
    fn grid_motion_keeps_scroll_tree_and_ignores_stale_bounds() {
        let mut app = app();
        app.cards = (1..=30)
            .map(|index| card(&format!("game-{index}"), State::Inactive, 0))
            .collect();
        let _ = app.update(Message::Select("game-21".into()));
        assert!(app.grid_motion.active());
        let generation = app.grid_motion.generation();
        let _ = app.scroll_selection();
        assert_eq!(app.grid_scroll_after_settle.as_deref(), Some("game-21"));

        let bounds = grid_motion::CardBounds {
            card: iced::Rectangle::new(
                iced::Point::new(32.0, 1650.0),
                iced::Size::new(160.0, 300.0),
            ),
            viewport: iced::Rectangle::new(
                iced::Point::new(0.0, 50.0),
                iced::Size::new(800.0, 600.0),
            ),
            offset: iced::Vector::ZERO,
        };
        let _ = app.update(Message::GridCardLocated(
            "game-21".into(),
            generation,
            bounds,
        ));
        assert_eq!(app.scroll_offset, 1300.0);
        let _ = app.update(Message::GridCardLocated(
            "game-21".into(),
            generation - 1,
            grid_motion::CardBounds {
                card: iced::Rectangle::new(
                    iced::Point::new(0.0, 0.0),
                    iced::Size::new(160.0, 300.0),
                ),
                ..bounds
            },
        ));
        assert_eq!(app.scroll_offset, 1300.0);

        let _ = app.update(Message::GridTick(
            generation,
            Instant::now() + Duration::from_millis(310),
        ));
        assert!(!app.grid_motion.active());
        assert!(app.grid_scroll_after_settle.is_none());
    }

    #[test]
    fn unchanged_refresh_does_not_cut_grid_motion_short() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Select("game".into()));
        let generation = app.grid_motion.generation();
        let _ = app.update(Message::GridTick(
            generation,
            Instant::now() + Duration::from_millis(90),
        ));
        let position = app.grid_motion.progress(0);
        app.merge(app.cards.clone());
        assert!(app.grid_motion.active());
        assert_eq!(app.grid_motion.generation(), generation);
        assert_eq!(app.grid_motion.progress(0), position);
    }

    #[test]
    fn leaving_library_finishes_motion_and_ignores_stale_tick() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Select("game".into()));
        let stale = app.panel_motion.generation();
        assert!(app.panel_motion.active());
        let _ = app.update(Message::Section(sidebar::Section::Trash));
        assert!(!app.panel_motion.active());
        let _ = app.update(Message::PanelTick(
            stale,
            Instant::now() + Duration::from_secs(1),
        ));
        assert_eq!(app.panel_motion.progress(), 1.0);
        let _ = app.update(Message::Section(sidebar::Section::Library));
        assert!(app.panel_motion.interactive());
    }

    #[test]
    fn checkup_from_a_closed_panel_starts_its_presentation() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Checkup(checkup::Event::Open(
            checkup::Scope::Environment("game".into()),
        )));
        assert!(app.panel_open);
        assert!(app.panel_reserved());
        assert!(app.panel_motion.active());
        let opened = app.panel_motion.generation();
        let _ = app.update(Message::PanelTick(
            opened,
            Instant::now() + Duration::from_millis(200),
        ));
        assert!(app.panel_motion.interactive());
    }

    #[test]
    fn opening_a_flow_finishes_hidden_panel_motion() {
        let mut app = app();
        app.cards = vec![card("game", State::Inactive, 0)];
        let _ = app.update(Message::Select("game".into()));
        assert!(app.panel_motion.active());
        let _ = app.update(Message::Add);
        assert!(app.flow.is_some());
        assert!(app.panel_hidden());
        assert!(!app.panel_motion.active());
        assert_eq!(app.panel_motion.progress(), 1.0);
    }

    #[test]
    fn double_click_keeps_open_panels_picker_choice() {
        let mut app = app();
        let mut game = card("game", State::Inactive, 0);
        game.targets = vec!["game".into(), "alternate".into()];
        game.display_title = "My Game".into();
        app.cards = vec![game];
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::TargetPicked("alternate".into()));
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::DoubleClick("game".into()));
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert_eq!(app.cards[0].name, "game");
        assert_eq!(app.cards[0].display_title, "My Game");
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

    fn hook_app(phase: wineo_core::hooks::Phase) -> App {
        let mut app = app();
        let mut game = card("game", State::Inactive, 0);
        game.busy = true;
        game.busy_holder = Some(wineo_core::operation::Operation::Hook(phase));
        app.cards = vec![game];
        let _ = app.update(Message::Select("game".into()));
        app
    }

    fn record_after_hook_identity(paths: &Paths, name: &str) {
        let running = wineo_core::hooks::Running {
            group: 12345,
            boot_id: "test-boot".into(),
            leader_start: 67890,
        };
        std::fs::write(
            paths.locks_dir().join(format!("name-{name}.lock")),
            format!(
                "hook-after {} {} {} {}\n",
                std::process::id(),
                running.group,
                running.boot_id,
                running.leader_start
            ),
        )
        .unwrap();
    }

    #[test]
    fn stop_after_opens_confirmation_and_cancel_keeps_hook_busy() {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        let _ = app.update(Message::Stop);
        assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
        assert!(app.operation.is_none());
        let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
        let _ = app.update(Message::Section(sidebar::Section::Downloads));
        let _ = app.update(Message::ClosePanel);
        let _ = app.update(Message::Select("other".into()));
        assert_eq!(app.section, sidebar::Section::Library);
        assert!(app.panel_open);
        assert_eq!(app.selected.as_deref(), Some("game"));
        let _ = app.update(Message::CancelHookStop);
        assert!(app.hook_stop_confirm.is_none());
        assert!(app.cards[0].busy);
        assert!(app.operation.is_none());
        let _ = app.update(Message::Stop);
        let _ = app.update(Message::Key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::empty(),
            iced::event::Status::Ignored,
        ));
        assert!(app.hook_stop_confirm.is_none());
        assert!(app.panel_open);
    }

    #[test]
    fn stop_while_our_own_stop_waits_on_after_opens_confirmation() {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.operation = Some(Operation {
            name: "game".into(),
            kind: OperationKind::Stop,
            token: None,
            cancelled: false,
            status: "Stopping...".into(),
            install_done: None,
            installing: false,
            after_hook: false,
        });
        let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
        let _ = app.update(Message::Stop);
        assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
    }

    #[test]
    fn confirm_after_uses_force_stop_only_while_hook_is_active() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.paths = paths;
        let guard = wineo_core::operation::Locks::new(&app.paths)
            .acquire(
                wineo_core::operation::Operation::Hook(wineo_core::hooks::Phase::After),
                "game",
                &[],
            )
            .unwrap();
        record_after_hook_identity(&app.paths, "game");
        let _ = app.update(Message::Stop);
        let task = app.update(Message::ConfirmHookStop);
        assert!(task.units() > 0);
        assert!(app.hook_stop_confirm.is_none());
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
        drop(guard);
        app.operation = None;
        let _ = app.update(Message::Stop);
        let task = app.update(Message::ConfirmHookStop);
        assert_eq!(task.units(), 0);
        assert!(app.operation.is_none());
    }

    #[test]
    fn confirm_after_refreshes_missing_hook_identity() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.paths = paths.clone();
        let _ = app.update(Message::Stop);
        assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
        assert!(app.hook_stop_identity.is_none());

        let guard = wineo_core::operation::Locks::new(&paths)
            .acquire(
                wineo_core::operation::Operation::Hook(wineo_core::hooks::Phase::After),
                "game",
                &[],
            )
            .unwrap();
        record_after_hook_identity(&paths, "game");
        let task = app.update(Message::ConfirmHookStop);
        assert!(task.units() > 0);
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
        drop(task);
        drop(guard);
    }

    #[test]
    fn confirm_after_without_matching_hook_shows_note() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.paths = paths;
        let _ = app.update(Message::Stop);
        assert!(app.hook_stop_identity.is_none());

        let task = app.update(Message::ConfirmHookStop);
        assert_eq!(task.units(), 0);
        assert_eq!(
            app.panel_note.as_deref(),
            Some(worker::AFTER_HOOK_ENDED_NOTE)
        );
        assert!(app.operation.is_none());
    }

    #[test]
    fn reload_dismisses_confirmation_when_after_is_no_longer_busy() {
        for holder in [
            None,
            Some(wineo_core::operation::Operation::Hook(
                wineo_core::hooks::Phase::Before,
            )),
        ] {
            let mut app = hook_app(wineo_core::hooks::Phase::After);
            let _ = app.update(Message::Stop);
            let mut loaded = app.cards[0].clone();
            loaded.busy_holder = holder;
            loaded.busy = holder.is_some();
            let _ = app.update(Message::LibraryReloaded(
                app.library_generation,
                Ok(vec![loaded]),
            ));
            assert!(app.hook_stop_confirm.is_none());
            assert!(app.hook_stop_identity.is_none());
        }
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        let _ = app.update(Message::Stop);
        let _ = app.update(Message::Loaded(Ok(Vec::new())));
        assert!(app.hook_stop_confirm.is_none());
    }

    #[test]
    fn reload_keeps_confirmation_for_an_active_after_hook() {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        let _ = app.update(Message::Stop);
        let _ = app.update(Message::Loaded(Ok(app.cards.clone())));
        assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
    }

    #[test]
    fn stop_turns_into_force_stop_only_after_the_delay() {
        let mut app = selected_app(State::Running);
        let task = app.update(Message::Stop);
        assert!(task.units() > 0);
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
        let (_, at) = app.force_stop_at.clone().unwrap();
        let card = app.cards[0].clone();
        assert!(!app.force_stop_ready(&card, at - Duration::from_millis(1)));
        assert!(app.force_stop_ready(&card, at));

        // A second click inside the delay, like a double-click, does nothing.
        let task = app.update(Message::Stop);
        assert_eq!(task.units(), 0);
        assert!(!app.operation.as_ref().unwrap().cancelled);

        app.force_stop_at = Some(("game".into(), Instant::now() - Duration::from_secs(1)));
        let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
        let task = app.update(Message::Stop);
        assert!(task.units() > 0);
        let operation = app.operation.as_ref().unwrap();
        assert!(operation.cancelled);
        assert_eq!(operation.status, "Force stopping...");
        assert!(app.force_stop_at.is_none());
        assert_eq!(app.update(Message::Stop).units(), 0);

        let _ = app.update(Message::StopFinished("game".into(), Ok(Vec::new())));
        assert!(app.operation.is_none());
    }

    #[test]
    fn force_stop_waits_for_the_after_hook_confirmation_instead() {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.operation = Some(Operation {
            name: "game".into(),
            kind: OperationKind::Stop,
            token: None,
            cancelled: false,
            status: "Stopping...".into(),
            install_done: None,
            installing: false,
            after_hook: false,
        });
        app.force_stop_at = Some(("game".into(), Instant::now() - Duration::from_secs(1)));
        let _ = app.update(Message::Stop);
        assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
        assert!(!app.operation.as_ref().unwrap().cancelled);
    }

    #[test]
    fn steam_waiting_and_client_runs_use_stop_and_force_stop() {
        for state in [State::Starting, State::SteamRunning] {
            let mut app = selected_app(state);
            let card = &mut app.cards[0];
            card.kind = Some(crate::library::CardKind::Proxied);
            card.steam = Some(crate::library::SteamDetails {
                install: wineo_core::steam::AppState::Installed.into(),
                proton: Some("proton_experimental".to_owned()),
                size: Some(2048),
            });
            card.default_target = Some("steam".into());
            card.targets = vec!["steam".into()];
            app.target = Some("steam".into());
            let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
            assert_eq!(app.update(Message::Play).units(), 0);
            assert_eq!(app.remove_reason("game").as_deref(), Some("Stop it first"));
            assert!(app.update(Message::Stop).units() > 0);
            assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
            app.force_stop_at = Some(("game".into(), Instant::now() - Duration::from_secs(1)));
            assert!(app.update(Message::Stop).units() > 0);
            assert_eq!(app.operation.as_ref().unwrap().status, "Force stopping...");
        }
    }

    #[test]
    fn steam_stop_during_handoff_switches_to_the_stop_worker() {
        let mut app = selected_app(State::Starting);
        app.cards[0].kind = Some(crate::library::CardKind::Proxied);
        app.operation = Some(installing_start("game", false));
        assert!(app.update(Message::Stop).units() > 0);
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
        let _ = app.update(Message::StartFinished(
            "game".into(),
            Ok(wineo_core::launch::StartOutcome::SteamRequested),
        ));
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
    }

    #[test]
    fn stop_during_hookless_start_does_not_cancel_start_token() {
        let mut app = selected_app(State::Running);
        let token = Token::new();
        app.operation = Some(installing_start("game", false));
        app.operation.as_mut().unwrap().token = Some(token.clone());
        let task = app.update(Message::Stop);
        assert_eq!(task.units(), 0);
        assert!(!token.is_canceled());
        assert!(!app.operation.as_ref().unwrap().cancelled);
    }

    #[test]
    fn stop_before_uses_the_launch_cancel_token() {
        let mut app = hook_app(wineo_core::hooks::Phase::Before);
        let token = Token::new();
        app.operation = Some(installing_start("game", false));
        app.operation.as_mut().unwrap().token = Some(token.clone());
        let _ = app.update(Message::Stop);
        assert!(token.is_canceled());
        assert!(app.hook_stop_confirm.is_none());
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Start);
    }

    #[test]
    fn gui_cancel_interrupts_a_real_before_hook() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("game");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(config, "schema_version = 1\nkind = 'native'\n[hooks]\nbefore = 'sleep 30'\n[targets.game]\nexe = '/missing'\n").unwrap();
        let mut app = hook_app(wineo_core::hooks::Phase::Before);
        app.paths = paths.clone();
        let token = Token::new();
        app.operation = Some(installing_start("game", false));
        app.operation.as_mut().unwrap().token = Some(token.clone());
        let capture = tempfile::tempfile().unwrap();
        let work = std::thread::spawn(move || {
            wineo_core::launch::prepare(
                &paths,
                "game",
                &wineo_core::launch::Overrides {
                    target: Some("game".into()),
                    ..Default::default()
                },
                &token,
                wineo_core::hooks::Output::File(capture),
                |_| {},
            )
        });
        let locks = wineo_core::operation::Locks::new(&app.paths);
        let deadline = Instant::now() + Duration::from_secs(5);
        while locks.busy_with("game").unwrap()
            != Some(wineo_core::operation::Operation::Hook(
                wineo_core::hooks::Phase::Before,
            ))
        {
            if work.is_finished() || Instant::now() >= deadline {
                app.operation
                    .as_ref()
                    .unwrap()
                    .token
                    .as_ref()
                    .unwrap()
                    .force();
                panic!("before hook did not acquire its lock");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = app.update(Message::CancelStart);
        let error = work
            .join()
            .unwrap()
            .err()
            .expect("before hook should be canceled");
        assert!(matches!(
            error.downcast_ref::<wineo_core::Error>(),
            Some(wineo_core::Error::Canceled("launch"))
        ));
        assert!(locks.busy_with("game").unwrap().is_none());
    }

    #[test]
    fn launcher_after_cleanup_gets_confirmation_and_its_fresh_token() {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.cards[0].busy = false;
        app.cards[0].busy_holder = None;
        app.operation = Some(installing_start("game", false));
        let fresh = Token::new();
        let _ = app.update(Message::AfterHookPhase("game".into(), fresh.clone()));
        assert_eq!(
            app.operation.as_ref().unwrap().status,
            "Running its after hook"
        );
        let _ = app.update(Message::Stop);
        assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
        assert!(!fresh.is_canceled());
    }

    #[test]
    fn after_cleanup_preserves_canceled_launch_error_suppression() {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        app.operation = Some(installing_start("game", false));
        let _ = app.update(Message::CancelStart);
        let _ = app.update(Message::AfterHookPhase("game".into(), Token::new()));
        assert!(app.operation.as_ref().unwrap().cancelled);
        let error = anyhow::Error::new(wineo_core::Error::Canceled("launch"));
        let _ = app.update(Message::StartFinished(
            "game".into(),
            Err(PanelError::from_error(&error)),
        ));
        assert!(app.panel_error.is_none());
    }

    #[test]
    fn before_error_keeps_output_in_panel_and_cancel_hides_it() {
        for cancelled in [false, true] {
            let mut app = hook_app(wineo_core::hooks::Phase::Before);
            app.operation = Some(installing_start("game", false));
            app.operation.as_mut().unwrap().cancelled = cancelled;
            let error = PanelError {
                text: "Launch canceled: the before hook exited with code 4.".into(),
                cleanup: false,
                output: Some("last line".into()),
            };
            let _ = app.update(Message::StartFinished("game".into(), Err(error.clone())));
            assert_eq!(app.panel_error, (!cancelled).then_some(error));
            let _ = iced::advanced::widget::Tree::new(app.panel(&app.cards[0]).as_widget());
        }
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
            install_done: None,
            installing: false,
            after_hook: false,
        });
        let _ = app.update(Message::CancelStart);
        assert!(token.is_canceled());
        assert!(!token.is_forced());
        assert!(app.operation.as_ref().unwrap().cancelled);
        let _ = app.update(Message::CancelStart);
        assert!(token.is_forced());
    }

    fn installing_start(name: &str, installing: bool) -> Operation {
        Operation {
            name: name.into(),
            kind: OperationKind::Start,
            token: Some(Token::new()),
            cancelled: false,
            status: "Installing...".into(),
            install_done: Some(Token::new()),
            installing,
            after_hook: false,
        }
    }

    fn selected_app(state: State) -> App {
        let mut app = app();
        app.cards = vec![card("game", state, 0)];
        app.selected = Some("game".into());
        app.panel_open = true;
        app
    }

    #[test]
    fn install_phase_message_follows_the_start_operation() {
        let mut app = selected_app(State::Inactive);
        app.operation = Some(installing_start("game", false));
        let _ = app.update(Message::InstallPhase("other".into(), true));
        assert!(!app.operation.as_ref().unwrap().installing);
        let _ = app.update(Message::InstallPhase("game".into(), true));
        assert!(app.operation.as_ref().unwrap().installing);
        let _ = app.update(Message::InstallPhase("game".into(), false));
        assert!(!app.operation.as_ref().unwrap().installing);
    }

    #[test]
    fn log_is_offered_for_an_inactive_card_only_while_installing() {
        let mut app = selected_app(State::Inactive);
        assert!(app.log_source(&app.cards[0]).is_none());
        app.operation = Some(installing_start("game", false));
        assert!(app.log_source(&app.cards[0]).is_none());
        app.operation = Some(installing_start("game", true));
        assert!(matches!(
            app.log_source(&app.cards[0]),
            Some(worker::LogSource::Install(_))
        ));
        app.operation = Some(installing_start("other", true));
        assert!(app.log_source(&app.cards[0]).is_none());
    }

    #[test]
    fn open_log_picks_the_file_for_the_phase() {
        let mut app = selected_app(State::Inactive);
        let _ = app.open_log();
        assert!(!app.log_view);

        app.operation = Some(installing_start("game", true));
        let _ = app.open_log();
        assert!(app.log_view);
        assert!(app.log_install);

        // Once the game runs, the same button shows its output.
        app.close_log();
        app.operation = None;
        app.cards = vec![card("game", State::Running, 0)];
        let _ = app.open_log();
        assert!(app.log_view);
        assert!(!app.log_install);
    }

    #[test]
    fn switching_logs_drops_chunks_from_the_old_one() {
        let mut app = selected_app(State::Inactive);
        app.operation = Some(installing_start("game", true));
        let _ = app.open_log();
        let install = app.log_generation;
        let _ = app.update(Message::LogChunk(install, "install line\n".into()));

        app.operation = None;
        app.cards = vec![card("game", State::Running, 0)];
        let _ = app.open_log();
        assert_ne!(app.log_generation, install);
        let _ = app.update(Message::LogChunk(install, "late install line\n".into()));
        let _ = app.update(Message::LogChunk(app.log_generation, "game line\n".into()));
        assert_eq!(app.log_buffer.display(), "game line\n");
    }

    #[test]
    fn finishing_the_install_keeps_the_open_install_log() {
        let mut app = selected_app(State::Inactive);
        app.operation = Some(installing_start("game", true));
        let _ = app.open_log();
        let generation = app.log_generation;
        let _ = app.update(Message::InstallPhase("game".into(), false));
        let _ = app.update(Message::LogFinished(generation, Ok(())));
        assert!(app.log_view);
        assert!(app.log_install);
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
