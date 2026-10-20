mod app_art;
mod app_flow;
mod app_grid;
mod app_keys;
mod app_library;
mod app_operation;
mod app_selection;
mod app_window;
mod art_cache;
mod art_navigation;
mod art_page;
mod art_picker;
mod artwork;
mod checkup;
mod checkup_work;
mod chips;
mod collapse;
mod download_progress;
mod downloads_page;
mod downloads_work;
mod ends;
mod filter_control;
mod filter_tab;
mod fit_width;
mod flow;
mod focus_button;
mod fonts;
#[cfg(test)]
mod galaxy_fake;
#[cfg(test)]
mod gog_fake;
// Shared with the other crates, which include the same file.
mod gog_library;
mod grid;
mod grid_motion;
mod keys;
#[cfg(test)]
mod layout_probe;
mod library;
mod library_actions;
mod library_menu;
mod library_states;
mod logview;
mod menu_anchor;
mod order;
mod panel;
mod panel_motion;
mod picker;
mod portable_page;
mod prefix_facts;
mod progress;
mod removal;
mod remove_work;
mod rescan;
mod runner_label;
mod settings;
mod shortcuts;
mod sidebar;
mod size;
mod snapshots_page;
mod sort_control;
mod spinner;
mod task;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../wineo-core/tests/common/runners.rs"]
mod test_runners;
#[cfg(test)]
#[path = "../../wineo-core/tests/common/test_script.rs"]
mod test_script;
mod theme;
mod tile_title;
mod tools_navigation;
mod tools_page;
mod trash_page;
mod trash_work;
mod ui;
mod version_picker;
mod wake;
mod wheel;
mod worker;
mod zoom;
mod zoom_control;

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use iced::widget::{image, scrollable};
use iced::{Settings, Subscription, Task, Theme, keyboard, window};
use wineo_core::cancel::Token;
use wineo_core::paths::Paths;
#[cfg(test)]
use wineo_core::runtime::State;

#[cfg(test)]
use crate::library::playtime_label;
use crate::library::{ArtFile, ArtIdentity, Card};
use crate::panel::{LOG_ID, LogBuffer, PanelError};
use crate::theme::Palette;

pub(crate) const CARD_WIDTH: f32 = 160.0;
pub(crate) const GAP: f32 = 16.0;
const SCROLL_ID: &str = "library";
const SEARCH_ID: &str = "library-search";
// iced moves 60px per wheel line, which felt too fast beside native apps.
const WHEEL_STEP_SCALE: f32 = 0.7;
const MAX_DECODES: usize = 2;
type ArtKey = (ArtFile, artwork::Kind);
/// What a message handler hands back to `update_message`. `Break` is an early
/// exit that skips the panel bookkeeping after the match.
pub(crate) type Early = ControlFlow<Task<Message>, Task<Message>>;

pub(crate) struct App {
    paths: Paths,
    pub(crate) palette: Palette,
    search: String,
    search_focused: bool,
    cards: Vec<Card>,
    /// How the library is sorted. The open window keeps its own, whatever
    /// another window or process saves.
    query: order::Query,
    /// The library grid's zoom in whole percent, from the saved view. While
    /// a tween runs this is where it's heading, which steps count from.
    zoom: u16,
    zooming: zoom::Zoom,
    /// Titles hide until a poster is pointed at or selected.
    poster_only: bool,
    /// Store, platform, and tag filters and Show hidden, which reset each launch.
    filters: order::Filters,
    /// The held modifier keys, for a wheel turn the filter tab hands to the grid.
    modifiers: keyboard::Modifiers,
    /// The shown order, rebuilt by `reorder`.
    order: order::Order,
    /// The sort menu is open.
    sort_menu: bool,
    sort_menu_position: iced::Point,
    view_writes: sort_control::Writes,
    /// Favorite and hidden writes in flight.
    flag_writes: library_actions::Writes,
    sort_opening: library_menu::Opening,
    /// The Filters popup is open.
    filter_menu: bool,
    filter_menu_position: iced::Point,
    filter_opening: library_menu::Opening,
    /// The Filters button's bounds as last laid out, which the filter tab
    /// hangs from.
    filter_button: Option<iced::Rectangle>,
    /// What `view` reads in place of the disk; see `prefix_facts`.
    facts: prefix_facts::Stored,
    pub(crate) selected: Option<String>,
    panel_open: bool,
    panel_motion: panel_motion::PanelMotion,
    grid_motion: grid_motion::GridMotion,
    collapse: collapse::Collapse,
    /// The desktop asked for reduced motion, read once at startup.
    reduced_motion: bool,
    grid_scroll_after_settle: Option<String>,
    pub(crate) target: Option<String>,
    card_click_target: Option<(String, Option<String>)>,
    pub(crate) hovered: Option<String>,
    pub(crate) images: art_cache::ArtCache<ArtKey>,
    /// Art decoding, by key and whether it is decoded dimmed.
    loading: HashSet<(ArtKey, bool)>,
    /// The grid art `load_visible_art` last found around the viewport, which
    /// eviction leaves alone.
    visible_art: HashSet<ArtKey>,
    /// Caps concurrent art decodes, since a huge source costs far more memory
    /// than its thumbnail.
    decodes: Arc<tokio::sync::Semaphore>,
    thumbnails: Arc<Mutex<art_picker::ThumbnailCache>>,
    failed_images: HashSet<ArtKey>,
    current_art: HashMap<PathBuf, ArtIdentity>,
    error: Option<String>,
    /// Why `library.toml` couldn't be used, as of the last read.
    library_problem: Option<wineo_core::library::Problem>,
    refreshing: bool,
    /// The first library load has answered, so an empty library is real.
    loaded: bool,
    viewport_width: f32,
    viewport_height: f32,
    scroll_offset: f32,
    scroll_visible_until: Option<Instant>,
    scroll_wake: wake::Wake,
    pub(crate) operation: Option<Operation>,
    /// The game an install or uninstall request is being handed to Steam for.
    pub(crate) steam_request: Option<String>,
    pub(crate) panel_error: Option<PanelError>,
    pub(crate) hook_stop_confirm: Option<String>,
    pub(crate) hook_stop_identity: Option<wineo_core::process::Identity>,
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
    gog_work: settings::GogWork,
    /// What the background check found, by environment. Kept here rather
    /// than on cards, which every library refresh rebuilds.
    pub(crate) gog_updates: gog_library::GogState,
    /// The Version picker, which takes the panel's details' place like Log.
    pub(crate) version: Option<version_picker::Picker>,
    version_generation: u64,
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
    /// The cursor at the last input that could open the action menu.
    menu_anchor: iced::Point,
    action_menu_position: iced::Point,
    window_size: iced::Size,
    close_prompt: bool,
    /// A close waiting only on sort writes, which shows no prompt.
    close_quietly: bool,
    closing: bool,
    removal: Option<removal::Dialog>,
    remove_generation: u64,
    change: Option<removal::Change>,
    toast: Option<removal::Toast>,
    toast_wake: wake::Wake,
    /// When a close waiting on sort writes gives up and exits anyway.
    close_by: Option<Instant>,
    close_wake: wake::Wake,
    /// The window chose to exit, so late results only drop.
    exiting: bool,
    feature_notice: Option<(String, Instant)>,
    change_generation: u64,
    library_generation: u64,
    trash_generation: u64,
    /// What the periodic rescan compares its reads against.
    shown: rescan::Watch,
    /// True until the window reports losing focus, since some compositors
    /// never send focus events.
    focused: bool,
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
    /// The parts of a periodic rescan that differ from what is shown.
    Rescanned(Box<rescan::Scan>),
    SettingsTick,
    /// A key or mouse button press, which counts as focus while unfocused.
    Pressed,
    Loaded(Result<Vec<Card>, String>),
    LibraryReloaded(u64, Result<Vec<Card>, String>),
    /// An explicit reload's cards with `library.toml`'s problem from the
    /// same read.
    LibraryRead(
        u64,
        Result<Vec<Card>, String>,
        Option<wineo_core::library::Problem>,
    ),
    TrashReloaded(u64, sidebar::TrashBadge),
    Section(sidebar::Section),
    /// A sidebar row was clicked, as opposed to a keyboard shortcut.
    SectionClicked(sidebar::Section),
    Rebuild,
    CollapseTick(u64, Instant),
    ReducedMotion(bool),
    TrashBadgeLoaded(sidebar::TrashBadge),
    ArtLoaded(ArtKey, bool, Result<image::Handle, String>),
    Scrolled(scrollable::Viewport),
    GridCardLocated(String, u64, grid_motion::CardBounds),
    /// The scrollbar fade deadline, tagged with the wakeup that scheduled it.
    ScrollIdle(u64),
    Window(window::Event),
    ThemeChanged(iced::theme::Mode),
    PanelTick(u64, Instant),
    GridTick(u64, Instant),
    Key(keyboard::Key, keyboard::Modifiers, iced::event::Status),
    Modifiers(keyboard::Modifiers),
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
    /// Updates a GOG game, or finishes its interrupted update.
    GogUpdate(String),
    GogChecked(
        HashMap<String, u64>,
        Result<Option<wineo_core::gog::update::Batch>, String>,
    ),
    /// One game checked again after an update changed it.
    GogRechecked(
        String,
        u64,
        Result<Option<Box<wineo_core::gog::update::Status>>, String>,
    ),
    /// The Version picker in a GOG game's panel.
    Version(version_picker::Event),
    Context(String),
    /// Where the cursor was when an input that can open the action menu arrived.
    MenuAnchor(iced::Point),
    ForceStopDue,
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
    UndoFinished(u64, Result<Vec<String>, String>),
    /// The toast or feature notice deadline, tagged with its wakeup.
    ToastTick(u64),
    /// A key came up or a finger went down, by which time any focus move it
    /// caused has happened.
    FocusRecheck,
    ToastHover(bool),
    ToastFocus(bool, Instant),
    /// A focus answer for the toast with this serial.
    ToastAnswer(u64, bool, Instant),
    Trash(trash_page::Event),
    Downloads(downloads_page::Event),
    Checkup(checkup::Event),
    Sort(sort_control::Event),
    Filter(filter_control::Event),
    /// Favoriting and hiding a card.
    Mark(library_actions::Event),
    Zoom(zoom::Event),
    /// The wait for `library.toml` writes on close is over, tagged with its
    /// wakeup.
    CloseWaitOver(u64),
}

impl App {
    fn boot(paths: Paths) -> (Self, Task<Message>) {
        let mut app = Self {
            paths,
            palette: Palette::DARK,
            search: String::new(),
            search_focused: false,
            cards: Vec::new(),
            query: order::Query::default(),
            zoom: wineo_core::library::ZOOM_DEFAULT,
            zooming: zoom::Zoom::default(),
            poster_only: false,
            filters: order::Filters::default(),
            modifiers: keyboard::Modifiers::empty(),
            order: order::Order::default(),
            sort_menu: false,
            sort_opening: library_menu::Opening::default(),
            filter_menu: false,
            filter_menu_position: iced::Point::ORIGIN,
            filter_opening: library_menu::Opening::default(),
            filter_button: None,
            sort_menu_position: iced::Point::ORIGIN,
            view_writes: sort_control::Writes::default(),
            flag_writes: library_actions::Writes::default(),
            facts: prefix_facts::Stored::default(),
            selected: None,
            panel_open: false,
            panel_motion: panel_motion::PanelMotion::default(),
            grid_motion: grid_motion::GridMotion::default(),
            collapse: collapse::Collapse::default(),
            reduced_motion: false,
            grid_scroll_after_settle: None,
            target: None,
            card_click_target: None,
            hovered: None,
            images: art_cache::ArtCache::default(),
            loading: HashSet::new(),
            visible_art: HashSet::new(),
            decodes: Arc::new(tokio::sync::Semaphore::new(MAX_DECODES)),
            thumbnails: Arc::new(Mutex::new(art_picker::ThumbnailCache::default())),
            failed_images: HashSet::new(),
            current_art: HashMap::new(),
            error: None,
            library_problem: None,
            refreshing: true,
            loaded: false,
            viewport_width: 1200.0 - sidebar::WIDTH,
            viewport_height: 750.0,
            scroll_offset: 0.0,
            scroll_visible_until: None,
            scroll_wake: wake::Wake::default(),
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
            gog_work: settings::GogWork::default(),
            gog_updates: gog_library::GogState::default(),
            version: None,
            version_generation: 0,
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
            menu_anchor: iced::Point::ORIGIN,
            action_menu_position: iced::Point::ORIGIN,
            window_size: iced::Size::new(1200.0, 800.0),
            close_prompt: false,
            close_quietly: false,
            closing: false,
            removal: None,
            remove_generation: 0,
            change: None,
            toast: None,
            toast_wake: wake::Wake::default(),
            close_by: None,
            close_wake: wake::Wake::default(),
            exiting: false,
            feature_notice: None,
            change_generation: 0,
            library_generation: 0,
            trash_generation: 0,
            shown: rescan::Watch::default(),
            focused: true,
            trash_page: trash_page::Page::default(),
            downloads_page: downloads_page::Page::default(),
            checkups: HashMap::new(),
            check_generation: 0,
        };
        // Read once, here, so the first cards already show in the saved
        // order. A read takes no lock and never writes.
        let saved = wineo_core::library::load(&app.paths);
        app.zoom = saved.view.zoom;
        app.poster_only = saved.view.poster_only;
        app.query = saved.view.into();
        // The banner shows from the first frame, before the cards arrive.
        app.problem_read(saved.problem);
        app.reorder();
        let load = app.reload_library();
        let badge = app.reload_badge();
        let cache = app.paths.art_cache_dir();
        let prune = Task::future(async move {
            let _ = tokio::task::spawn_blocking(move || artwork::prune(&cache)).await;
        })
        .discard();
        let key_presence = settings::key_presence_task(app.paths.clone(), app.steamgriddb_sequence);
        let gog_check = gog_library::check_task(app.paths.clone(), app.gog_updates.asked_all());
        (
            app,
            Task::batch([
                load,
                badge,
                prune,
                key_presence,
                gog_check,
                iced::system::theme().map(Message::ThemeChanged),
                read_reduced_motion(),
            ]),
        )
    }
    fn theme(&self) -> Theme {
        self.palette.theme()
    }

    fn management_busy(&self) -> bool {
        self.filesystem_busy()
            || self.change.is_some()
            || self.art_busy()
            || self.flow.as_ref().is_some_and(flow::Page::busy)
            || self.settings.as_ref().is_some_and(|page| page.saving)
    }

    // Only tests ask now: spinners animate themselves while drawn.
    #[cfg(test)]
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
        let panel_motion = if !self.panel_hidden() && self.panel_motion.active() {
            let generation = self.panel_motion.generation();
            window::frames()
                .with(generation)
                .map(|(generation, now)| Message::PanelTick(generation, now))
        } else {
            Subscription::none()
        };
        let grid_motion = if self.panel_hidden() {
            Subscription::none()
        } else {
            self.grid_motion.subscription()
        };
        let zoom = if self.panel_hidden() {
            Subscription::none()
        } else {
            self.zooming.subscription()
        };
        let collapse = if !self.panel_hidden() && self.collapse.animating() {
            window::frames()
                .with(self.collapse.generation())
                .map(|(generation, now)| Message::CollapseTick(generation, now))
        } else {
            Subscription::none()
        };
        // Paused while unfocused; focus coming back reloads at once.
        let rescan = if self.focused {
            Subscription::run_with(
                rescan::Rescan {
                    paths: self.paths.clone(),
                    watch: self.shown.clone(),
                    trash_page: self.section == sidebar::Section::Trash,
                    art_page: self.art_page.clone(),
                },
                rescan::stream,
            )
        } else {
            Subscription::none()
        };
        // The settings page compares its own disk copy, and the tick also
        // keeps its busy refusal current, so it keeps a plain timer.
        let settings_tick = if self.focused && self.settings.is_some() {
            iced::time::every(rescan::INTERVAL).map(|_| Message::SettingsTick)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            rescan,
            settings_tick,
            iced::system::theme_changes().map(Message::ThemeChanged),
            iced::event::listen_with(|event, _, _| window_or_press(event)),
            iced::event::listen_with(|event, status, _| event_message(event, status)),
            panel_motion,
            grid_motion,
            zoom,
            collapse,
            // Only a toast has focus to recheck.
            if self.toast.is_some() {
                iced::event::listen_with(|event, _, _| focus_input(event))
            } else {
                Subscription::none()
            },
        ])
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        let poll_focus = self.toast_focus_may_have_changed(&message);
        let screen = self.screen_key();
        let task = self.update_message(message);
        // One rule for every way a screen opens: see `prefix_facts`.
        if self.screen_key() != screen {
            self.refresh_facts();
        }
        self.settle_toast();
        // Leaving the Library or opening anything over it closes the menu,
        // so it doesn't come back on its own later.
        if self.sort_menu && !self.sort_menu_shown() {
            self.sort_menu = false;
        }
        if self.filter_menu && !self.filter_menu_shown() {
            self.filter_menu = false;
        }
        // Whichever way a close got here, once only `library.toml` writes
        // are left they get a bounded wait.
        if self.closing
            && !self.exiting
            && self.close_by.is_none()
            && !self.foreground_busy()
            && self.library_writes_pending()
        {
            self.close_by = Some(Instant::now() + app_window::SORT_SAVE_WAIT);
        }
        let focus = if poll_focus {
            self.poll_toast_focus()
        } else {
            Task::none()
        };
        // Armed after every update so each deadline change is seen.
        let wakes = Task::batch([
            self.scroll_wake
                .arm(self.scroll_visible_until, Message::ScrollIdle),
            self.toast_wake
                .arm(self.next_toast_deadline(), Message::ToastTick),
            self.close_wake.arm(self.close_by, Message::CloseWaitOver),
        ]);
        Task::batch([task, focus, wakes])
    }

    fn update_message(&mut self, message: Message) -> Task<Message> {
        let panel_was_open = self.panel_open;
        if self
            .feature_notice
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.feature_notice = None;
        }
        if self.message_blocked(&message) {
            return Task::none();
        }
        let task = match self.route(message) {
            ControlFlow::Break(task) => return task,
            ControlFlow::Continue(task) => task,
        };
        self.settle_panel(panel_was_open, task)
    }

    /// Messages ignored while an overlay or the card collapse owns the input.
    fn message_blocked(&self, message: &Message) -> bool {
        if (self.flow.is_some()
            || self.close_prompt
            || self.removal.is_some()
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
            || self.art_page.is_some()
            || self.tool_page.is_some()
            || self.portable_page.is_some())
            && (matches!(
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
                    | Message::GogUpdate(_)
                    | Message::Checkup(
                        checkup::Event::Fix(_)
                            | checkup::Event::Check(_)
                            | checkup::Event::OpenInstallLog(_)
                            | checkup::Event::Environment(_)
                    )
            ) || matches!(message, Message::Version(event) if event.is_input())
                || matches!(message, Message::Sort(event) if event.is_input())
                || matches!(message, Message::Filter(event) if event.is_input())
                || matches!(message, Message::Mark(event) if event.is_input())
                || matches!(message, Message::Zoom(event) if event.is_input()))
        {
            return true;
        }
        // The sidebar shortcuts and Undo are taken before any key handler,
        // so they're stopped here.
        if self.library_menu_shown()
            && matches!(
                message,
                Message::Section(_) | Message::SectionClicked(_) | Message::Undo | Message::Add
            )
        {
            return true;
        }
        if !self.collapse.idle()
            && matches!(
                message,
                Message::Select(_) | Message::DoubleClick(_) | Message::Context(_)
            )
        {
            return true;
        }
        false
    }

    /// Routes one message to its handler. `Break` skips the bookkeeping
    /// `update_message` does after a handler that ran to the end.
    fn route(&mut self, message: Message) -> Early {
        ControlFlow::Continue(match message {
            Message::SnapshotsEnvironment(name) => self.open_snapshots(Some(name)),
            Message::Snapshots(event) => self.snapshots_update(event),
            Message::CopyPortable(name) => self.open_portable(name),
            Message::Portable(name, event) => self.portable_update(name, event),
            Message::OpenTool(name, kind) => self.open_tool(name, kind),
            Message::Tool(name, kind, event) => self.tool_update(name, kind, event),
            Message::Checkup(event) => checkup::update(self, event),
            Message::Version(event) => version_picker::update(self, event),
            Message::Trash(event) => trash_page::update(self, event),
            Message::Downloads(event) => downloads_page::update(self, event),
            Message::Sort(event) => sort_control::update(self, event),
            Message::Filter(event) => filter_control::update(self, event),
            Message::Mark(event) => library_actions::update(self, event),
            Message::Zoom(event) => zoom::update(self, event),
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
            | Message::ToastFocus(..)
            | Message::ToastAnswer(..)) => removal::update(self, message),
            Message::SearchChanged(search) => self.search_changed(search),
            Message::Select(name) => self.select_card(name),
            Message::DoubleClick(name) => self.double_click_card(name),
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
            Message::SteamRequested(name, result) => self.steam_requested(name, result),
            Message::Stop | Message::CleanUp => self.stop_selected(),
            Message::ConfirmHookStop => self.confirm_hook_stop(),
            Message::CancelHookStop => {
                self.hook_stop_confirm = None;
                self.hook_stop_identity = None;
                Task::none()
            }
            Message::AfterHookPhase(name, token) => self.after_hook_phase(name, token),
            Message::CancelStart => self.cancel_start(),
            Message::LaunchProgress(name, status) => self.launch_progress(name, status),
            Message::InstallPhase(name, installing) => self.install_phase(name, installing),
            Message::LaunchNote(name, note) => {
                if self.selected.as_deref() == Some(&name) {
                    self.panel_note = Some(note);
                }
                Task::none()
            }
            Message::StartFinished(name, result) => self.start_finished(name, result),
            Message::StopFinished(name, result) => self.stop_finished(name, result),
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
            Message::Refresh => self.refresh(),
            Message::Rescanned(scan) => self.rescanned(*scan),
            Message::SettingsTick => self.settings_tick(),
            Message::Pressed | Message::Window(window::Event::Focused) => {
                if self.focused {
                    Task::none()
                } else {
                    self.focused = true;
                    self.update(Message::Refresh)
                }
            }
            Message::Section(section) => self.navigate_section(section),
            Message::SectionClicked(section) => {
                let task = self.navigate_section(section);
                if section == sidebar::Section::Library
                    && self.collapse.clicks.record(Instant::now())
                {
                    self.start_collapse(Instant::now());
                    task
                } else {
                    task
                }
            }
            Message::Rebuild => {
                if self.collapse.restorable() {
                    self.collapse.restore(Instant::now());
                }
                Task::none()
            }
            Message::CollapseTick(generation, now) => {
                if self.section == sidebar::Section::Library {
                    self.collapse.tick(generation, now);
                }
                Task::none()
            }
            Message::ReducedMotion(reduced) => {
                self.reduced_motion = reduced;
                Task::none()
            }
            Message::LibraryRead(generation, result, problem) => {
                if generation == self.library_generation {
                    self.problem_read(problem);
                }
                self.update(Message::LibraryReloaded(generation, result))
            }
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
                self.shown.show_badge(badge.clone());
                self.trash_badge = badge;
                Task::none()
            }
            Message::Loaded(Ok(cards)) => self.cards_loaded(cards),
            Message::Loaded(Err(error)) => {
                self.refreshing = false;
                self.loaded = true;
                self.shown.show_library(Err(error.clone()));
                self.error = Some(error);
                Task::none()
            }
            Message::ArtLoaded(key, dimmed, result) => self.art_result(key, dimmed, result)?,
            Message::Scrolled(viewport) => self.grid_scrolled(viewport),
            Message::GridCardLocated(name, generation, bounds) => {
                self.grid_card_located(name, generation, bounds)?
            }
            // Handled in `update`, which polls the toast's focus for it.
            Message::FocusRecheck => Task::none(),
            Message::ScrollIdle(generation) => {
                if self.scroll_wake.fired(generation)
                    && self
                        .scroll_visible_until
                        .is_some_and(|until| Instant::now() >= until)
                {
                    self.scroll_visible_until = None;
                }
                Task::none()
            }
            Message::Window(window::Event::Resized(size) | window::Event::Opened { size, .. }) => {
                self.window_resized(size)
            }
            Message::Window(window::Event::CloseRequested) => self.close_requested(),
            Message::CloseWaitOver(generation) => {
                if self.close_wake.fired(generation) && self.close_by.is_some() {
                    self.exit_now()
                } else {
                    Task::none()
                }
            }
            Message::Window(window::Event::Unfocused) => {
                self.focused = false;
                // Keys released elsewhere never report here, so Ctrl
                // can't be trusted to still be down.
                self.modifiers = keyboard::Modifiers::empty();
                self.zooming.modifiers_changed(self.modifiers);
                Task::none()
            }
            Message::Window(_) => Task::none(),
            Message::ThemeChanged(mode) => {
                self.palette = Palette::from_mode(mode);
                Task::none()
            }
            Message::PanelTick(generation, now) => {
                if self.section == sidebar::Section::Library {
                    self.panel_motion.tick(generation, now);
                }
                Task::none()
            }
            Message::GridTick(generation, now) => self.grid_tick(generation, now)?,
            Message::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                self.zooming.modifiers_changed(modifiers);
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
            // Only the menu's own keys work while it's open.
            Message::Key(key, modifiers, _) if self.sort_menu_shown() => {
                sort_control::key(self, &key, modifiers)
            }
            Message::Key(key, modifiers, _) if self.filter_menu_shown() => {
                filter_control::key(self, &key, modifiers)
            }
            Message::Key(key, modifiers, status) if self.flow.is_some() => {
                self.flow_key(key, modifiers, status)?
            }
            Message::Key(key, modifiers, status) => self.page_key(key, modifiers, status)?,
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
                    return ControlFlow::Break(Task::none());
                }
                self.set_steamgriddb_availability(sequence, available)
            }
            Message::SteamGridDbKeyPresenceLoaded(sequence, available) => {
                if sequence <= self.steamgriddb_completed_sequence {
                    return ControlFlow::Break(Task::none());
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
            Message::Add => self.add_environment()?,
            Message::GogChecked(asked, result) => {
                gog_library::checked_since(self, &asked, result);
                Task::none()
            }
            Message::GogRechecked(name, generation, result) => {
                gog_library::rechecked(self, name, generation, result);
                Task::none()
            }
            Message::GogUpdate(name) => self.gog_update_requested(name)?,
            Message::CloneEnvironment(name) => self.clone_environment(name)?,
            Message::Context(name) => {
                let open = self.action_menu.take();
                self.close_library_menus();
                self.action_menu = open;
                self.action_menu_position = self.menu_anchor;
                self.action_menu = if self.action_menu.as_deref() == Some(&name) {
                    None
                } else {
                    Some(name)
                };
                Task::none()
            }
            Message::ForceStopDue => Task::none(),
            Message::MenuAnchor(position) => {
                self.menu_anchor = position;
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
                self.cancel_and_close(event)
            }
        })
    }

    /// Keeps the panel, grid, and collapse animations consistent with where
    /// the handler left the panel.
    fn settle_panel(&mut self, panel_was_open: bool, task: Task<Message>) -> Task<Message> {
        if !panel_was_open && self.panel_open {
            self.panel_motion.open(Instant::now());
        }
        let task = if panel_was_open != self.panel_open {
            self.move_grid();
            task.chain(self.load_visible_art())
        } else {
            task
        };
        if self.panel_motion.active() && self.panel_hidden() {
            self.panel_motion.snap(self.panel_open);
        }
        if self.grid_motion.active() && self.panel_hidden() {
            self.snap_grid();
        }
        // Whatever covers the grid takes the pointer, so a drag ends here,
        // and with every motion snapped a waiting scroll has nothing to do.
        let task = if self.panel_hidden() {
            let stopped = self.stop_zoom();
            let _ = self.motion_ended(false);
            task.chain(stopped)
        } else {
            task
        };
        // Leaving the Library, an overlay, or the panel opening by any path
        // puts the cards back at once.
        if !self.collapse.idle() && (self.panel_hidden() || self.panel_open) {
            self.collapse.reset();
        }
        let task = if panel_was_open && !self.panel_open {
            task.chain(iced::advanced::widget::operate(
                iced::advanced::widget::operation::focusable::unfocus(),
            ))
        } else {
            task
        };
        if self.closing && !self.exit_waits() {
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
    /// What closing the window waits for: foreground work, and
    /// `library.toml` writes that haven't reached the file yet.
    fn exit_waits(&self) -> bool {
        self.foreground_busy() || self.library_writes_pending()
    }

    /// A sort, favorite, or hidden write is in flight.
    fn library_writes_pending(&self) -> bool {
        self.view_writes.pending() || self.flag_writes.pending()
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

    /// Rereads the disk facts `view` shows reasons from. Cheap enough to run
    /// per user action: one lstat and one canonicalize per environment.
    pub(crate) fn refresh_facts(&mut self) {
        self.facts = prefix_facts::Stored::read(&self.paths, &self.cards);
    }

    /// Records a new scroll offset. Scrolling moves the heap out of view, so
    /// the cards go back at once rather than leaving nothing to click.
    fn on_scroll_offset(&mut self, offset: f32) {
        if offset != self.scroll_offset {
            self.collapse.reset();
        }
        self.scroll_offset = offset;
    }
    /// Drops the visible cards, or fades them with reduced motion. The panel
    /// closes without animating so the cards fall from their closed slots.
    fn start_collapse(&mut self, now: Instant) {
        if !self.collapse.idle()
            || self.section != sidebar::Section::Library
            || self.panel_hidden()
            || self.filtered().is_empty()
        {
            return;
        }
        if self.panel_open {
            self.close_panel();
        }
        self.panel_motion.snap(false);
        self.snap_grid();
        let _ = self.motion_ended(false);
        self.action_menu = None;
        self.hovered = None;
        if self.reduced_motion {
            self.collapse.fade(now);
        } else {
            let layout = self.grid_layout();
            let filtered = self.filtered();
            let heights: Vec<f32> = filtered
                .iter()
                .map(|card| grid::card_height(card, layout))
                .collect();
            let columns = grid_motion::columns_for_viewport(self.viewport_width, false, layout);
            let floor = self.scroll_offset + self.viewport_height;
            let visible = grid_motion::cards_between(&heights, columns, self.scroll_offset, floor);
            // Nothing in view would leave only hidden cards and nothing to click.
            if visible.is_empty() {
                return;
            }
            // 2x3 so it keeps the poster's shape when fit into the frame.
            let placeholder =
                image::Handle::from_rgba(2, 3, self.palette.surface.into_rgba8().repeat(6));
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos() as u64);
            let count = heights.len();
            self.collapse.fall(
                now,
                count,
                visible,
                collapse::Area {
                    floor,
                    width: self.viewport_width,
                    columns,
                    half_poster: {
                        let frame = layout.art_frame();
                        iced::Vector::new(frame.width / 2.0, frame.height / 2.0)
                    },
                    // A partly visible top row can start a card height above.
                    drop: self.viewport_height + heights.iter().copied().fold(0.0, f32::max),
                },
                placeholder,
                seed,
            );
        }
    }
    /// The shown cards in shown order, from the cache `reorder` keeps.
    pub(crate) fn filtered(&self) -> Vec<&Card> {
        self.order.shown(self.order_inputs())
    }

    /// What the shown order is built from.
    pub(crate) fn order_inputs(&self) -> order::Of<'_> {
        order::Of {
            cards: &self.cards,
            search: &self.search,
            query: self.query,
            filters: &self.filters,
        }
    }

    /// Rebuilds the shown order after the cards, search, sort, or filters
    /// changed.
    pub(crate) fn reorder(&mut self) {
        let of = order::Of {
            cards: &self.cards,
            search: &self.search,
            query: self.query,
            filters: &self.filters,
        };
        self.order.rebuild(of);
    }
    fn columns(&self) -> usize {
        grid_motion::columns_for_viewport(self.viewport_width, self.panel_open, self.grid_layout())
    }
    pub(crate) fn grid_layout(&self) -> grid_motion::GridLayout {
        match self.zooming.shown() {
            Some(zoom) => grid_motion::GridLayout::at(zoom),
            None => grid_motion::GridLayout::new(self.zoom),
        }
    }
    /// Changes whenever the cards' drawn places may have: the dock's motion
    /// or the zoom. Both counters only grow, so their sum does too, and a
    /// card's location measured under an older one is stale.
    pub(crate) fn grid_revision(&self) -> u64 {
        self.grid_motion
            .generation()
            .wrapping_add(self.zooming.revision())
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
}

// One value, because `Application::settings` replaces everything set before it.
fn app_settings() -> Settings {
    Settings {
        fonts: vec![fonts::REGULAR.into(), fonts::SEMIBOLD.into()],
        default_font: fonts::DEFAULT,
        default_text_size: 14.into(),
        ..Settings::default()
    }
}

fn run() -> anyhow::Result<()> {
    let paths = Paths::from_env(None)?;
    fonts::install();
    iced::application(move || App::boot(paths.clone()), App::update, App::view)
        .title("wineo")
        .theme(App::theme)
        .settings(app_settings())
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

/// Reads GNOME's animation switch through the desktop portal, the same
/// D-Bus service the file chooser uses. Any failure means full motion.
fn read_reduced_motion() -> Task<Message> {
    Task::perform(
        async {
            let Ok(settings) = ashpd::desktop::settings::Settings::new().await else {
                return false;
            };
            settings
                .read::<bool>("org.gnome.desktop.interface", "enable-animations")
                .await
                .is_ok_and(|enabled| !enabled)
        },
        Message::ReducedMotion,
    )
}

/// Maps a runtime event to an app message. The mouse Back button becomes an
/// uncaptured Escape so every page's existing Escape handling runs unchanged.
/// Window events and presses share one subscription so focus changes and
/// input arrive in the order they happened. X11 maps windows unfocused, and
/// without a window manager no Focused follows, so a press counts as focus.
pub(crate) fn window_or_press(event: iced::Event) -> Option<Message> {
    match event {
        iced::Event::Window(event) => Some(Message::Window(event)),
        iced::Event::Keyboard(keyboard::Event::KeyPressed { .. })
        | iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => Some(Message::Pressed),
        _ => None,
    }
}

/// Focus moves by key (Tab, a dialog opening) finish before the key comes up,
/// unlike at the press, so this is when the toast re-checks its Undo focus.
/// A touch is the other way focus leaves it, and the widgets have handled the
/// press by the time a listener sees it.
fn focus_input(event: iced::Event) -> Option<Message> {
    matches!(
        event,
        iced::Event::Keyboard(keyboard::Event::KeyReleased { .. })
            | iced::Event::Touch(iced::touch::Event::FingerPressed { .. })
    )
    .then_some(Message::FocusRecheck)
}

pub(crate) fn event_message(event: iced::Event, status: iced::event::Status) -> Option<Message> {
    match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Message::Key(key, modifiers, status))
        }
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::Modifiers(modifiers))
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
mod tests;
