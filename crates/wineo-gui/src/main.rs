mod artwork;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use iced::advanced::text::Paragraph as _;
use iced::widget::{
    Row, Space, button, column, container, image, mouse_area, row, scrollable, stack, text,
    text_input,
};
use iced::{
    Center, Color, ContentFit, Element, Fill, Font, Settings, Size, Subscription, Task, Theme,
    border, font, mouse, window,
};
use wineo_core::art::{self, ArtConfig};
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::runtime::State;
use wineo_core::{stats, time};

const CARD_WIDTH: f32 = 180.0;
const CARD_ROW: f32 = 330.0;
const GAP: f32 = 16.0;
const SCROLL_ID: &str = "library";
// iced moves 60px per wheel line, which felt too fast beside native apps.
const WHEEL_STEP_SCALE: f32 = 0.45;

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[derive(Clone, Copy)]
struct Palette {
    background: Color,
    surface: Color,
    border: Color,
    foreground: Color,
    muted: Color,
    primary: Color,
    success: Color,
    warning: Color,
    danger: Color,
}

impl Palette {
    const DARK: Self = Self {
        background: rgb(0x16171b),
        surface: rgb(0x1f2126),
        border: rgb(0x2c2f36),
        foreground: rgb(0xe6e6e9),
        muted: rgb(0x8b8f98),
        primary: rgb(0x6d8cff),
        success: rgb(0x3ecf6e),
        warning: rgb(0xe5a34a),
        danger: rgb(0xe5534b),
    };
    const LIGHT: Self = Self {
        background: rgb(0xf6f7f9),
        surface: rgb(0xffffff),
        border: rgb(0xd9dde4),
        foreground: rgb(0x20242b),
        muted: rgb(0x59616d),
        primary: rgb(0x3158ca),
        success: rgb(0x198044),
        warning: rgb(0xa65d12),
        danger: rgb(0xb93431),
    };
    fn from_mode(mode: iced::theme::Mode) -> Self {
        if mode == iced::theme::Mode::Light {
            Self::LIGHT
        } else {
            Self::DARK
        }
    }
    fn theme(self) -> Theme {
        Theme::custom(
            "wineo",
            iced::theme::Palette {
                background: self.background,
                text: self.foreground,
                primary: self.primary,
                success: self.success,
                warning: self.warning,
                danger: self.danger,
            },
        )
    }
}

#[derive(Debug, Clone)]
struct Card {
    name: String,
    state: State,
    busy: bool,
    error: Option<String>,
    playtime: String,
    art: Option<PathBuf>,
}

impl Card {
    fn state_word(&self) -> &'static str {
        if self.error.is_some() {
            "error"
        } else if self.busy {
            "busy"
        } else {
            self.state.as_str()
        }
    }
    fn dot(&self, palette: Palette) -> Color {
        if self.error.is_some() || self.busy || self.state == State::Failed {
            palette.warning
        } else if self.state == State::Running {
            palette.success
        } else {
            palette.muted
        }
    }
    fn playtime(&self) -> &str {
        &self.playtime
    }
}

fn playtime_label(seconds: i64) -> String {
    if seconds <= 0 {
        "never played".into()
    } else {
        time::duration_hm(seconds)
    }
}

fn load_library(paths: &Paths) -> anyhow::Result<Vec<Card>> {
    let overview = report::overview(paths, OverviewFilter::default())?;
    let played: HashMap<_, _> = stats::overview(paths)?
        .rows
        .into_iter()
        .map(|(name, summary)| (name, summary.seconds))
        .collect();
    let mut cards = Vec::new();
    for item in overview.environments {
        let (error, art) = match Environment::load(paths, &item.name) {
            Ok(env) => {
                let dir = paths.art_dir(&item.name);
                let resolved = (|| {
                    let config = ArtConfig::load(&dir)?;
                    let target = env
                        .config
                        .targets
                        .iter()
                        .find(|(_, target)| target.default)
                        .map(|(name, _)| name.as_str())
                        .or_else(|| {
                            if env.config.targets.len() == 1 {
                                env.config.targets.keys().next().map(String::as_str)
                            } else {
                                None
                            }
                        });
                    Ok::<_, wineo_core::Error>(
                        art::resolve(&dir, &config, target, art::Kind::Grid)?
                            .map(|found| found.found.path),
                    )
                })();
                match resolved {
                    Ok(path) => (None, path),
                    Err(error) => (Some(error.to_string()), None),
                }
            }
            Err(error) => (Some(error.to_string()), None),
        };
        cards.push(Card {
            playtime: playtime_label(played.get(&item.name).copied().unwrap_or(0)),
            name: item.name,
            state: item.status.state,
            busy: item.busy.is_some() || item.unresolved,
            error,
            art,
        });
    }
    for error in overview.errors {
        if cards.iter().any(|card| card.name == error.name) {
            continue;
        }
        cards.push(Card {
            playtime: playtime_label(played.get(&error.name).copied().unwrap_or(0)),
            name: error.name,
            state: State::Inactive,
            busy: false,
            error: Some(error.message),
            art: None,
        });
    }
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(cards)
}

struct App {
    paths: Paths,
    palette: Palette,
    search: String,
    cards: Vec<Card>,
    selected: Option<String>,
    hovered: Option<String>,
    images: HashMap<PathBuf, image::Handle>,
    loading: HashSet<PathBuf>,
    failed_images: HashSet<PathBuf>,
    error: Option<String>,
    refreshing: bool,
    viewport_width: f32,
    viewport_height: f32,
    scroll_offset: f32,
    row_pitch: f32,
    scroll_visible_until: Option<Instant>,
    mouse_down: bool,
}

#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    Select(String),
    Hover(Option<String>),
    Refresh,
    Loaded(Result<Vec<Card>, String>),
    ArtLoaded(PathBuf, Result<image::Handle, String>),
    Scrolled(scrollable::Viewport),
    ScrollIdle,
    Window(window::Event),
    ThemeChanged(iced::theme::Mode),
    MouseButton(bool),
}

impl App {
    fn boot(paths: Paths) -> (Self, Task<Message>) {
        let app = Self {
            paths,
            palette: Palette::DARK,
            search: String::new(),
            cards: Vec::new(),
            selected: None,
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
            mouse_down: false,
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
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    Some(Message::MouseButton(true))
                }
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::MouseButton(false))
                }
                _ => None,
            }),
            idle,
        ])
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
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
                self.selected = Some(name);
                Task::none()
            }
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
                self.load_visible_art()
            }
            Message::Loaded(Err(error)) => {
                self.refreshing = false;
                self.error = Some(error);
                Task::none()
            }
            Message::ArtLoaded(path, result) => {
                self.loading.remove(&path);
                match result {
                    Ok(handle) => {
                        self.images.insert(path, handle);
                    }
                    Err(_) => {
                        self.failed_images.insert(path);
                    }
                }
                Task::none()
            }
            Message::Scrolled(viewport) => {
                self.viewport_width = viewport.bounds().width;
                self.viewport_height = viewport.bounds().height;
                let columns = self.columns();
                let rows = self.filtered().len().div_ceil(columns);
                if rows > 0 {
                    self.row_pitch = (viewport.content_bounds().height - 32.0 + GAP) / rows as f32;
                }
                let raw = viewport.absolute_offset().y;
                let factor = if self.mouse_down {
                    1.0
                } else {
                    WHEEL_STEP_SCALE
                };
                let corrected = self.scroll_offset + (raw - self.scroll_offset) * factor;
                self.scroll_offset = corrected;
                self.scroll_visible_until = Some(Instant::now() + Duration::from_millis(850));
                Task::batch([
                    iced::widget::operation::scroll_to(
                        iced::widget::Id::new(SCROLL_ID),
                        scrollable::AbsoluteOffset {
                            x: 0.0,
                            y: corrected,
                        },
                    ),
                    self.load_visible_art(),
                ])
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
            Message::MouseButton(down) => {
                self.mouse_down = down;
                Task::none()
            }
        }
    }
    fn merge(&mut self, cards: Vec<Card>) {
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !cards.iter().any(|card| &card.name == name))
        {
            self.selected = None;
        }
        self.cards = cards;
        self.error = None;
    }
    fn filtered(&self) -> Vec<&Card> {
        let needle = self.search.to_lowercase();
        self.cards
            .iter()
            .filter(|card| card.name.to_lowercase().contains(&needle))
            .collect()
    }
    fn columns(&self) -> usize {
        ((self.viewport_width - 32.0 + GAP) / (CARD_WIDTH + GAP))
            .floor()
            .max(1.0) as usize
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
            .filter(|path| {
                !self.images.contains_key(*path)
                    && !self.loading.contains(*path)
                    && !self.failed_images.contains(*path)
            })
            .cloned()
            .collect();
        let mut tasks = Vec::new();
        for path in paths {
            if !self.loading.insert(path.clone()) {
                continue;
            }
            tasks.push(Task::perform(
                async move {
                    let decoded = tokio::task::spawn_blocking({
                        let path = path.clone();
                        move || artwork::decode(&path).map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string());
                    (path, decoded)
                },
                |(path, decoded)| Message::ArtLoaded(path, decoded.unwrap_or_else(Err)),
            ));
        }
        Task::batch(tasks)
    }
    fn view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let mut page = column![
            self.top_bar(),
            container(Space::new())
                .height(1)
                .style(move |_| container::Style::default().background(palette.border))
        ];
        if let Some(error) = &self.error {
            page = page.push(
                container(text(format!("Failed to load library: {error}")).color(palette.danger))
                    .padding(16),
            );
        }
        let visible = self.scroll_visible_until.is_some();
        let scroll = scrollable(self.grid())
            .id(iced::widget::Id::new(SCROLL_ID))
            .width(Fill)
            .height(Fill)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new().width(6).scroller_width(4),
            ))
            .on_scroll(Message::Scrolled)
            .style(move |theme, status| scroll_style(theme, status, palette, visible));
        container(page.push(scroll))
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background))
            .into()
    }
    fn top_bar(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let input = text_input("Search library...", &self.search)
            .on_input(Message::SearchChanged)
            .width(320)
            .padding(iced::Padding::from([6, 10]).right(30))
            .style(move |_, status| text_input::Style {
                background: palette.surface.into(),
                border: border::rounded(6).width(1).color(
                    if matches!(status, text_input::Status::Focused { .. }) {
                        palette.primary
                    } else {
                        palette.border
                    },
                ),
                icon: palette.muted,
                placeholder: palette.muted,
                value: palette.foreground,
                selection: palette.primary.scale_alpha(0.4),
            });
        // Keep the tree shape stable so clearing text does not drop input focus.
        let clear: Element<'_, Message> = if self.search.is_empty() {
            Space::new().into()
        } else {
            button(text("x").size(15).color(palette.muted))
                .on_press(Message::SearchChanged(String::new()))
                .padding(4)
                .style(|_, _| button::Style::default())
                .into()
        };
        let search = stack![
            input,
            container(clear)
                .width(Fill)
                .height(Fill)
                .align_right(Fill)
                .center_y(Fill)
                .padding([0, 6])
        ];
        container(
            row![
                text("wineo")
                    .size(18)
                    .font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                    .color(palette.foreground),
                search,
            ]
            .spacing(16)
            .align_y(Center),
        )
        .padding([8, 16])
        .width(Fill)
        .into()
    }
    fn grid(&self) -> Element<'_, Message> {
        let cards = self.filtered().into_iter().map(|card| self.card(card));
        container(
            Row::with_children(cards)
                .spacing(GAP)
                .wrap()
                .vertical_spacing(GAP),
        )
        .padding(16)
        .width(Fill)
        .into()
    }
    fn card<'a>(&'a self, card: &'a Card) -> Element<'a, Message> {
        let palette = self.palette;
        let hovered = self.hovered.as_ref() == Some(&card.name);
        let selected = self.selected.as_ref() == Some(&card.name);
        let art = card.art.as_ref().and_then(|path| self.images.get(path));
        let poster: Element<'_, Message> = match art {
            Some(handle) => image(handle.clone())
                .width(Fill)
                .height(Fill)
                .content_fit(ContentFit::Cover)
                .border_radius(4)
                .opacity(if hovered { 0.85_f32 } else { 1.0_f32 })
                .into(),
            None => container(text(&card.name).size(14).color(palette.muted).center())
                .width(Fill)
                .height(Fill)
                .padding(12)
                .center(Fill)
                .style(move |_| {
                    container::Style::default().background(
                        palette
                            .surface
                            .scale_alpha(if hovered { 0.85 } else { 1.0 }),
                    )
                })
                .into(),
        };
        let framed = container(container(poster).clip(true))
            .width(CARD_WIDTH)
            .height(270)
            .padding(2)
            .style(move |_| {
                container::Style::default().border(border::rounded(6).width(2).color(if selected {
                    palette.primary
                } else {
                    Color::TRANSPARENT
                }))
            });
        let label = truncate_name(&card.name, CARD_WIDTH);
        let dot_color = card.dot(palette);
        let meta = row![
            container(Space::new())
                .width(8)
                .height(8)
                .style(move |_| container::Style::default()
                    .background(dot_color)
                    .border(border::rounded(4))),
            text(card.state_word()).size(12).color(palette.muted),
            text(card.playtime()).size(12).color(palette.muted),
        ]
        .spacing(8)
        .align_y(Center);
        let content = column![
            framed,
            text(label)
                .size(14)
                .font(Font {
                    weight: font::Weight::Semibold,
                    ..Font::DEFAULT
                })
                .color(palette.foreground)
                .wrapping(iced::widget::text::Wrapping::None),
            meta,
        ]
        .spacing(4)
        .width(CARD_WIDTH);
        mouse_area(content)
            .on_press(Message::Select(card.name.clone()))
            .on_enter(Message::Hover(Some(card.name.clone())))
            .on_exit(Message::Hover(None))
            .interaction(mouse::Interaction::Pointer)
            .into()
    }
}

fn text_width(content: &str) -> f32 {
    iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: Size::INFINITE,
        size: 14.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font: Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        },
        align_x: iced::advanced::text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::None,
    })
    .min_bounds()
    .width
}

fn truncate_name(name: &str, max_width: f32) -> String {
    if text_width(name) <= max_width {
        return name.to_owned();
    }
    let chars: Vec<_> = name.chars().collect();
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = chars[..mid].iter().collect::<String>() + "...";
        if text_width(&candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    chars[..low]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_owned()
        + "..."
}

fn scroll_style(
    theme: &Theme,
    status: scrollable::Status,
    palette: Palette,
    visible: bool,
) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    let show = visible || !matches!(status, scrollable::Status::Active { .. });
    for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
        rail.background = None;
        rail.border = border::rounded(3);
        rail.scroller.background = if show {
            palette.border.into()
        } else {
            Color::TRANSPARENT.into()
        };
        rail.scroller.border = border::rounded(3);
    }
    style
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
        .window_size((1200.0, 800.0))
        .subscription(App::subscription)
        .run()?;
    Ok(())
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
            error: None,
            playtime: playtime_label(seconds),
            art: None,
        }
    }
    fn app() -> App {
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| "/tmp/wineo-gui-tests".into()))
                .unwrap();
        App::boot(paths).0
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
}
