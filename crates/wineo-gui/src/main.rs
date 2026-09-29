mod artwork;
mod grid;
mod keys;
mod library;
mod panel;
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
use crate::library::{Card, load_library};
use crate::panel::{LogBuffer, PanelError};
use crate::theme::Palette;

pub(crate) const CARD_WIDTH: f32 = 180.0;
const CARD_ROW: f32 = 330.0;
pub(crate) const GAP: f32 = 16.0;
const SCROLL_ID: &str = "library";
const LOG_ID: &str = "panel-log";
const SEARCH_ID: &str = "library-search";
// iced moves 60px per wheel line, which felt too fast beside native apps.
const WHEEL_STEP_SCALE: f32 = 0.45;

pub(crate) struct App {
    paths: Paths,
    pub(crate) palette: Palette,
    search: String,
    cards: Vec<Card>,
    pub(crate) selected: Option<String>,
    preferred_column: Option<usize>,
    panel_open: bool,
    pub(crate) target: Option<String>,
    pub(crate) logo_scrim: bool,
    pub(crate) hovered: Option<String>,
    pub(crate) images: HashMap<(PathBuf, artwork::Kind), image::Handle>,
    loading: HashSet<(PathBuf, artwork::Kind)>,
    failed_images: HashSet<(PathBuf, artwork::Kind)>,
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
    ArtLoaded((PathBuf, artwork::Kind), Result<image::Handle, String>),
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
}

impl App {
    fn boot(paths: Paths) -> (Self, Task<Message>) {
        let app = Self {
            paths,
            palette: Palette::DARK,
            search: String::new(),
            cards: Vec::new(),
            selected: None,
            preferred_column: None,
            panel_open: false,
            target: None,
            logo_scrim: std::env::var("WINEO_GUI_LOGO").is_ok_and(|value| value == "scrim"),
            hovered: None,
            images: HashMap::new(),
            loading: HashSet::new(),
            failed_images: HashSet::new(),
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
        Subscription::batch([
            iced::time::every(Duration::from_secs(2)).map(|_| Message::Refresh),
            iced::system::theme_changes().map(Message::ThemeChanged),
            window::events().map(|(_, event)| Message::Window(event)),
            iced::event::listen_with(|event, status, _| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    Some(Message::Key(key, modifiers, status))
                }
                _ => None,
            }),
            iced::time::every(Duration::from_millis(120)).map(|_| Message::SpinnerTick),
            idle,
        ])
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SearchChanged(search) => {
                self.search = search;
                self.preferred_column = None;
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
                    self.close_panel();
                    Task::none()
                } else {
                    self.close_log();
                    self.selected = Some(name.clone());
                    self.preferred_column = self
                        .filtered()
                        .iter()
                        .position(|card| card.name == name)
                        .map(|index| index % self.columns());
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
            Message::Refresh if !self.refreshing => {
                self.refreshing = true;
                Self::load(self.paths.clone())
            }
            Message::Refresh => Task::none(),
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
                self.viewport_width = size.width;
                self.viewport_height = (size.height - 48.0).max(0.0);
                self.load_visible_art()
            }
            Message::Window(_) => Task::none(),
            Message::ThemeChanged(mode) => {
                self.palette = Palette::from_mode(mode);
                Task::none()
            }
            Message::SpinnerTick => {
                if self.operation.is_some() {
                    self.spinner_frame = (self.spinner_frame + 1) % 4;
                }
                Task::none()
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
        }
    }
    fn merge(&mut self, cards: Vec<Card>) {
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !cards.iter().any(|card| &card.name == name))
        {
            self.selected = None;
            self.preferred_column = None;
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
            .map(|path| (path.clone(), artwork::Kind::Grid))
            .collect();
        self.load_images(paths)
    }
    fn load_images(&mut self, paths: Vec<(PathBuf, artwork::Kind)>) -> Task<Message> {
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
                        move || artwork::decode(&key.0, key.1).map_err(|e| e.to_string())
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

    fn card(name: &str, state: State, seconds: i64) -> Card {
        Card {
            name: name.into(),
            state,
            busy: false,
            busy_holder: None,
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
    #[test]
    fn app_icon_renders_from_embedded_svg() {
        assert!(app_icon().is_some());
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
}
