use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iced::widget::{
    Column, button, checkbox, column, container, image, pick_list, row, stack, text, text_input,
};
use iced::{Color, ContentFit, Element, Fill, Padding, Radians, Task, border, gradient};
use wineo_core::art::{self, Kind};
use wineo_core::art_fetch::{self, Mode};
use wineo_core::config::{ArtFetch, GlobalConfig};
use wineo_core::download::Https;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game};

use crate::theme::Palette;
use crate::ui::PickListStyle;

/// Width of the stacked tile block.
const WIDTH: u32 = 356;
/// Matches the hero banner in the library side panel.
const BANNER_HEIGHT: u32 = 220;
const LOGO_HEIGHT: u32 = 80;
const LOGO_MAX_WIDTH: u32 = 280;
/// Tall enough to line up with the collapsed creation form beside it.
pub(crate) const POSTER_WIDTH: u32 = 152;
const POSTER_HEIGHT: u32 = 228;
/// About the height of a labelled text input, so it sits in the Name row.
const ICON_SIZE: u32 = 56;

#[derive(Clone, Debug)]
pub(crate) enum Event {
    Path(Kind, String),
    Browse(Kind),
    Picked(Kind, u64, Result<Option<PathBuf>, String>),
    Preview(Kind, u64, Result<image::Handle, String>),
    Clear(Kind),
    ManualEntry(bool),
    Configured(u64, Result<ArtFetch, String>),
    Fetch(bool),
    SearchText(String),
    Search,
    Searched(u64, Result<Vec<Game>, String>),
    Game(Match),
    Fetched(
        Kind,
        u64,
        Result<Option<(Arc<art::Downloaded>, image::Handle)>, String>,
    ),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Match(pub(crate) Game);

impl std::fmt::Display for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.year {
            Some(year) => write!(f, "{} ({year})", self.0.name),
            None => f.write_str(&self.0.name),
        }
    }
}

#[derive(Clone, Default)]
struct Local {
    path: String,
    sequence: u64,
    chooser_sequence: u64,
    preview: Option<image::Handle>,
    error: Option<String>,
    checking: bool,
    /// Set when the flow chose this file rather than the user.
    prefilled: Option<Prefilled>,
}

#[derive(Clone)]
struct Prefilled {
    /// The source name recorded instead of the file's own.
    original: String,
    note: &'static str,
}

#[derive(Clone, Default)]
struct Remote {
    image: Option<Arc<art::Downloaded>>,
    preview: Option<image::Handle>,
    loading: bool,
    error: Option<String>,
    none_found: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Draft {
    paths: Option<Paths>,
    local: BTreeMap<Kind, Local>,
    remote: BTreeMap<Kind, Remote>,
    fetch: bool,
    kinds: Vec<Kind>,
    search: String,
    search_edited: bool,
    searching: bool,
    matches: Vec<Match>,
    selected: Option<Match>,
    sequence: u64,
    fetch_sequence: u64,
    // Mirrors fetch_sequence so running blocking jobs can see they are stale.
    fetch_generation: Arc<AtomicU64>,
    fetch_handles: BTreeMap<Kind, iced::task::Handle>,
    error: Option<String>,
    configuration_sequence: u64,
    manual_entry: bool,
    /// Kinds the user chose or removed a file for, which a prefill must not
    /// override.
    touched: BTreeSet<Kind>,
}

impl Draft {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(super) fn search_text(&self) -> &str {
        &self.search
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.fetch
            || self.search_edited
            || self.selected.is_some()
            || self.local.values().any(|local| !local.path.is_empty())
    }

    #[cfg(test)]
    pub(crate) fn is_fetching(&self) -> bool {
        self.fetch
    }

    pub(crate) fn set_title(&mut self, title: &str) -> Task<Event> {
        if !self.search_edited && self.search != title {
            self.clear_fetched();
            self.search = title.to_owned();
            self.sequence = self.sequence.wrapping_add(1);
            self.searching = false;
            self.matches.clear();
            self.selected = None;
            self.error = None;
        }
        Task::none()
    }

    pub(crate) fn configure(&mut self, paths: Paths, title: String) -> Task<Event> {
        self.paths = Some(paths.clone());
        if self.search.is_empty() {
            self.search = title;
        }
        self.configuration_sequence = self.configuration_sequence.wrapping_add(1);
        let sequence = self.configuration_sequence;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let global =
                        GlobalConfig::load_or_default(&paths).map_err(|e| e.to_string())?;
                    Ok(global.art.fetch)
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
            },
            move |result| Event::Configured(sequence, result),
        )
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        self.update_with_key_availability(event, false)
    }

    pub(crate) fn update_with_key_availability(
        &mut self,
        event: Event,
        key_available: bool,
    ) -> Task<Event> {
        match event {
            Event::Path(kind, value) => {
                let paths = self.paths.clone();
                let local = self.local.entry(kind).or_default();
                local.path = value.clone();
                local.prefilled = None;
                self.touched.insert(kind);
                local.sequence = local.sequence.wrapping_add(1);
                local.chooser_sequence = local.chooser_sequence.wrapping_add(1);
                local.preview = None;
                local.error = None;
                local.checking = !value.is_empty();
                let sequence = local.sequence;
                if value.is_empty() {
                    return Task::none();
                }
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let path = resolve(paths.as_ref(), &value)?;
                            preview(&path, kind)
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    move |result| Event::Preview(kind, sequence, result),
                );
            }
            Event::Browse(kind) => {
                let local = self.local.entry(kind).or_default();
                local.chooser_sequence = local.chooser_sequence.wrapping_add(1);
                let sequence = local.chooser_sequence;
                return Task::perform(
                    crate::picker::pick(format!("Select {kind} artwork"), false),
                    move |r| Event::Picked(kind, sequence, r),
                );
            }
            Event::Picked(kind, sequence, result) => {
                if self.local.entry(kind).or_default().chooser_sequence != sequence {
                    return Task::none();
                }
                match result {
                    Ok(Some(path)) => {
                        return self.update(Event::Path(kind, path.to_string_lossy().into_owned()));
                    }
                    Ok(None) => {}
                    Err(_) => self.manual_entry = true,
                }
            }
            Event::Preview(kind, sequence, result) => {
                let local = self.local.entry(kind).or_default();
                if local.sequence == sequence {
                    local.checking = false;
                    match result {
                        Ok(handle) => local.preview = Some(handle),
                        Err(error) => local.error = Some(error),
                    }
                }
            }
            Event::Clear(kind) => return self.update(Event::Path(kind, String::new())),
            Event::ManualEntry(open) => self.manual_entry = open,
            Event::Configured(sequence, result) if sequence == self.configuration_sequence => {
                match result {
                    Ok(kinds) => {
                        self.kinds = kinds.kinds();
                        if self.fetch && (!key_available || self.kinds.is_empty()) {
                            self.fetch = false;
                            self.clear_fetched();
                            return Task::none();
                        }
                        self.remote.retain(|kind, _| self.kinds.contains(kind));
                        self.fetch_handles.retain(|kind, handle| {
                            let keep = self.kinds.contains(kind);
                            if !keep {
                                handle.abort();
                            }
                            keep
                        });
                        return self.start_fetches();
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Fetch(fetch) => {
                self.fetch = fetch && key_available && !self.kinds.is_empty();
                self.clear_fetched();
                return self.start_fetches();
            }
            Event::SearchText(value) if key_available => {
                self.clear_fetched();
                self.search = value;
                self.search_edited = true;
                self.sequence = self.sequence.wrapping_add(1);
                self.searching = false;
                self.selected = None;
                self.matches.clear();
            }
            Event::Search if key_available && !self.searching && !self.search.trim().is_empty() => {
                let Some(paths) = self.paths.clone() else {
                    return Task::none();
                };
                self.sequence = self.sequence.wrapping_add(1);
                let sequence = self.sequence;
                let term = self.search.clone();
                self.searching = true;
                self.error = None;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || search(&paths, &term))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    move |result| Event::Searched(sequence, result),
                );
            }
            Event::Searched(sequence, result) if sequence == self.sequence => {
                self.searching = false;
                match result {
                    Ok(games) => {
                        self.clear_fetched();
                        self.matches = games.into_iter().map(Match).collect();
                        self.selected = self.matches.first().cloned();
                        self.error = self
                            .matches
                            .is_empty()
                            .then(|| "No matches found".to_owned());
                        return self.start_fetches();
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Game(game) if key_available => {
                self.clear_fetched();
                self.selected = Some(game);
                return self.start_fetches();
            }
            Event::Fetched(kind, sequence, result) if sequence == self.fetch_sequence => {
                self.fetch_handles.remove(&kind);
                if let Some(remote) = self.remote.get_mut(&kind) {
                    remote.loading = false;
                    match result {
                        Ok(Some((image, preview))) => {
                            remote.image = Some(image);
                            remote.preview = Some(preview);
                        }
                        Ok(None) => remote.none_found = true,
                        Err(error) => remote.error = Some(error),
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn clear_fetched(&mut self) {
        self.fetch_sequence = self.fetch_sequence.wrapping_add(1);
        self.fetch_generation
            .store(self.fetch_sequence, Ordering::Relaxed);
        // Aborting only drops the async wrapper; running blocking jobs stop via the generation.
        for handle in std::mem::take(&mut self.fetch_handles).into_values() {
            handle.abort();
        }
        self.remote.clear();
    }

    fn start_fetches(&mut self) -> Task<Event> {
        if !self.fetch {
            return Task::none();
        }
        let (Some(paths), Some(game)) = (self.paths.clone(), self.selected.clone()) else {
            return Task::none();
        };
        let sequence = self.fetch_sequence;
        let missing: Vec<_> = self
            .kinds
            .iter()
            .copied()
            .filter(|kind| !self.remote.contains_key(kind))
            .collect();
        let generation = self.fetch_generation.clone();
        let mut tasks = Vec::new();
        for kind in missing {
            self.remote.insert(
                kind,
                Remote {
                    loading: true,
                    ..Remote::default()
                },
            );
            let paths = paths.clone();
            let game = game.clone();
            let generation = generation.clone();
            let (task, handle) = Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        fetch_preview(&paths, game.0.id, kind, &generation, sequence)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
                },
                move |result| Event::Fetched(kind, sequence, result),
            )
            .abortable();
            self.fetch_handles.insert(kind, handle);
            tasks.push(task);
        }
        Task::batch(tasks)
    }

    fn art_preview(&self, kind: Kind) -> Option<(&image::Handle, bool)> {
        let local = self.local.get(&kind);
        if local.is_some_and(|local| !local.path.is_empty()) {
            local
                .and_then(|local| local.preview.as_ref())
                .map(|preview| (preview, false))
        } else {
            self.remote
                .get(&kind)
                .and_then(|remote| remote.preview.as_ref())
                .map(|preview| (preview, true))
        }
    }

    /// Picks `path` for `kind` on the user's behalf, already decoded as
    /// `preview`. It's stored under the source name `original` and shown with
    /// `note`, and the user can remove or replace it like their own pick. Does
    /// nothing once the user has chosen or removed a file for `kind`.
    pub(crate) fn prefill(
        &mut self,
        kind: Kind,
        path: &Path,
        original: String,
        note: &'static str,
        preview: image::Handle,
    ) {
        if self.touched.contains(&kind) || self.is_selected(kind) {
            return;
        }
        let local = self.local.entry(kind).or_default();
        local.path = path.to_string_lossy().into_owned();
        local.sequence = local.sequence.wrapping_add(1);
        local.chooser_sequence = local.chooser_sequence.wrapping_add(1);
        local.preview = Some(preview);
        local.error = None;
        local.checking = false;
        local.prefilled = Some(Prefilled { original, note });
    }

    /// The source name a prefilled `kind` would be recorded under.
    #[cfg(test)]
    pub(crate) fn prefilled(&self, kind: Kind) -> Option<&str> {
        self.local
            .get(&kind)
            .and_then(|local| local.prefilled.as_ref())
            .map(|prefilled| prefilled.original.as_str())
    }

    /// A draft holding an already fetched SteamGridDB `image` for `kind`, as
    /// if the user picked match `game`.
    #[cfg(test)]
    pub(crate) fn with_fetched(kind: Kind, game: u64, image: art::Downloaded) -> Self {
        let mut draft = Self::new();
        draft.fetch = true;
        draft.kinds = vec![kind];
        draft.selected = Some(Match(Game {
            id: game,
            name: "game".into(),
            year: None,
        }));
        draft.remote.insert(
            kind,
            Remote {
                image: Some(Arc::new(image)),
                ..Remote::default()
            },
        );
        draft
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        for (kind, local) in &self.local {
            if local.checking {
                return Err(format!("Checking {kind} artwork..."));
            }
            if !local.path.is_empty()
                && let Some(error) = &local.error
            {
                return Err(format!("{kind}: {error}"));
            }
        }
        if self.fetch && self.selected.is_none() {
            return Err("Search SteamGridDB and choose a match, or turn fetching off".to_owned());
        }
        if self.remote.iter().any(|(kind, remote)| {
            remote.loading
                && self
                    .local
                    .get(kind)
                    .is_none_or(|local| local.path.is_empty())
        }) {
            return Err("Fetching artwork...".to_owned());
        }
        Ok(())
    }

    pub(crate) fn apply(&self, paths: &Paths, name: &str) -> Result<Vec<String>, String> {
        let mut notes = Vec::new();
        let mut failures = Vec::new();
        for (kind, local) in &self.local {
            if local.path.is_empty() {
                continue;
            }
            let result = resolve(Some(paths), &local.path).and_then(|path| {
                match &local.prefilled {
                    Some(prefilled) => {
                        art::set_manual_as(paths, name, None, *kind, &path, &prefilled.original)
                    }
                    None => art::set_manual(paths, name, None, *kind, &path, None),
                }
                .map_err(|e| e.to_string())
            });
            if let Err(error) = result {
                failures.push(format!("Could not store {kind}: {error}"));
            }
        }
        if self.fetch
            && let Some(game) = &self.selected
        {
            let kinds: Vec<_> = self
                .kinds
                .iter()
                .copied()
                .filter(|kind| {
                    self.local
                        .get(kind)
                        .is_none_or(|local| local.path.is_empty())
                })
                .collect();
            if !kinds.is_empty() {
                if let Err(error) = art::save_game(paths, name, game.0.id) {
                    failures.push(format!("Art fetching failed: {error}"));
                } else {
                    let mut missing = Vec::new();
                    for kind in kinds {
                        if let Some(image) = self
                            .remote
                            .get(&kind)
                            .and_then(|remote| remote.image.as_ref())
                        {
                            if let Err(error) =
                                art_fetch::publish(paths, name, None, kind, image, Mode::Fill)
                            {
                                failures.push(format!("Could not fetch {kind}: {error}"));
                            }
                        } else if self
                            .remote
                            .get(&kind)
                            .is_some_and(|remote| remote.none_found)
                        {
                            notes.push(format!("No {kind} artwork found for {}", game.0.name));
                        } else {
                            missing.push(kind);
                        }
                    }
                    if !missing.is_empty() {
                        match fetch(paths, name, &game.0, missing.into_iter()) {
                            Ok(fetched) => notes.extend(fetched),
                            Err(error) => failures.push(format!("Art fetching failed: {error}")),
                        }
                    }
                }
            }
        }
        if failures.is_empty() {
            Ok(notes)
        } else {
            Err(failures.join("\n"))
        }
    }

    pub(crate) fn view(&self, palette: Palette, key_available: bool) -> Element<'_, Event> {
        column![
            text("Artwork").size(19),
            text("Click a tile to choose an image. All four are optional.")
                .size(12)
                .color(palette.muted),
            column![
                self.tile(
                    Kind::Hero,
                    "Hero",
                    "Wide background",
                    WIDTH,
                    112,
                    true,
                    palette
                ),
                row![
                    column![
                        self.tile(Kind::Logo, "Logo", "Title graphic", 196, 78, true, palette),
                        self.tile(Kind::Icon, "Icon", "Small square", 98, 98, true, palette),
                    ]
                    .spacing(10),
                    self.tile(Kind::Grid, "Poster", "Cover art", 148, 222, true, palette),
                ]
                .spacing(12)
                // In a narrow dialog the poster moves below instead of
                // being squeezed to nothing.
                .wrap()
                .vertical_spacing(10),
            ]
            .spacing(10),
            self.control_rows(palette, key_available),
        ]
        .spacing(10)
        .into()
    }

    /// Full-width hero with the logo overlaid, like the library side panel.
    pub(crate) fn banner(&self, palette: Palette) -> Element<'_, Event> {
        let hero = self
            .art_preview(Kind::Hero)
            .map(|(handle, _)| handle.clone());
        let has_hero = hero.is_some();
        let hero_selected = self.is_selected(Kind::Hero);
        let content: Element<'_, Event> = match hero {
            Some(handle) => image(handle)
                .width(Fill)
                .height(Fill)
                .content_fit(ContentFit::Cover)
                .border_radius(6)
                .into(),
            None => container(placeholder("Hero", "Wide background", 16, palette))
                .center(Fill)
                .into(),
        };
        let hero_button = button(container(content).width(Fill).height(Fill).clip(true))
            .width(Fill)
            .height(BANNER_HEIGHT)
            .padding(2)
            .on_press(Event::Browse(Kind::Hero))
            .style(move |_, status| button::Style {
                background: Some(palette.surface.into()),
                text_color: palette.foreground,
                border: border::rounded(8)
                    .width(2)
                    .color(outline(status, hero_selected, palette)),
                ..button::Style::default()
            });

        let logo = self
            .art_preview(Kind::Logo)
            .map(|(handle, _)| handle.clone());
        let logo_selected = self.is_selected(Kind::Logo);
        let logo_button = match logo {
            Some(handle) => button(
                container(
                    image(handle)
                        .height(LOGO_HEIGHT)
                        .content_fit(ContentFit::Contain),
                )
                .max_width(LOGO_MAX_WIDTH),
            )
            .padding(4)
            .style(move |_, status| button::Style {
                background: None,
                text_color: palette.foreground,
                border: border::rounded(6).width(2).color(
                    if matches!(status, button::Status::Hovered) {
                        palette.primary
                    } else if logo_selected {
                        palette.success
                    } else {
                        Color::TRANSPARENT
                    },
                ),
                ..button::Style::default()
            }),
            None => button(container(text("Logo").size(14)).center(Fill))
                .width(140)
                .height(56)
                .style(move |_, status| button::Style {
                    background: Some(palette.surface.scale_alpha(0.9).into()),
                    text_color: palette.foreground,
                    border: border::rounded(6).width(1).color(outline(
                        status,
                        logo_selected,
                        palette,
                    )),
                    ..button::Style::default()
                }),
        }
        .on_press(Event::Browse(Kind::Logo));
        // The scrim keeps a light logo readable over a bright hero.
        let overlay = container(logo_button)
            .width(Fill)
            .height(112)
            .padding(Padding {
                top: 24.0,
                right: 16.0,
                bottom: 12.0,
                left: 16.0,
            })
            .align_left(Fill)
            .align_bottom(Fill)
            .style(move |_| {
                if has_hero {
                    let gradient = gradient::Linear::new(Radians(std::f32::consts::PI))
                        .add_stop(0.0, Color::BLACK.scale_alpha(0.0))
                        .add_stop(1.0, Color::BLACK.scale_alpha(0.78));
                    container::Style::default()
                        .background(gradient)
                        .border(border::rounded(border::bottom(6)))
                } else {
                    container::Style::default()
                }
            });
        // A pressed logo button captures the event and the stack stops there,
        // so the hero underneath never sees the click.
        stack![
            hero_button,
            container(overlay)
                .width(Fill)
                .height(BANNER_HEIGHT)
                .padding(2)
                .align_bottom(Fill),
        ]
        .into()
    }

    /// Hero, logo, and icon statuses for the banner, or `None` when there is nothing to say.
    pub(crate) fn status_line(&self, palette: Palette) -> Option<Element<'_, Event>> {
        let parts = self.banner_statuses();
        if parts.is_empty() {
            return None;
        }
        let mut line = row![].spacing(16);
        for (kind, notes, selected) in parts {
            let label = match kind {
                Kind::Hero => "Hero:",
                Kind::Logo => "Logo:",
                _ => "Icon:",
            };
            let mut part = row![text(label).size(11)].spacing(6).align_y(iced::Center);
            for note in notes {
                part = part.push(note_view(note, palette));
            }
            if selected {
                part = part.push(remove(kind));
            }
            line = line.push(part);
        }
        Some(line.wrap().vertical_spacing(4).into())
    }

    pub(crate) fn poster(&self, palette: Palette) -> Element<'_, Event> {
        self.tile(
            Kind::Grid,
            "Poster",
            "Cover art",
            POSTER_WIDTH,
            POSTER_HEIGHT,
            false,
            palette,
        )
    }

    pub(crate) fn icon(&self, palette: Palette) -> Element<'_, Event> {
        self.chooser(Kind::Icon, "Icon", "", ICON_SIZE, ICON_SIZE, palette)
    }

    /// File path entry and SteamGridDB controls, for layouts that place tiles themselves.
    pub(crate) fn controls(&self, palette: Palette, key_available: bool) -> Element<'_, Event> {
        column![
            text("Click the banner, logo, poster, or icon to choose an image. All are optional.")
                .size(12)
                .color(palette.muted),
            self.control_rows(palette, key_available),
        ]
        .spacing(10)
        .into()
    }

    fn control_rows(&self, palette: Palette, key_available: bool) -> Column<'_, Event> {
        let mut content = column![
            button(if self.manual_entry {
                "Hide file paths"
            } else {
                "Enter a file path instead"
            })
            .on_press(Event::ManualEntry(!self.manual_entry)),
        ]
        .spacing(10);
        if self.manual_entry {
            for kind in Kind::ALL {
                let value = self
                    .local
                    .get(&kind)
                    .map_or("", |local| local.path.as_str());
                content = content.push(
                    row![
                        text(format!("{kind}")).width(55),
                        text_input("Image file path", value)
                            .on_input(move |value| Event::Path(kind, value)),
                    ]
                    .spacing(8),
                );
            }
        }
        if !self.kinds.is_empty() {
            if key_available {
                content = content.push(
                    checkbox(self.fetch)
                        .label("Fetch missing artwork from SteamGridDB")
                        .on_toggle(Event::Fetch),
                );
                if self.fetch {
                    content = content.push(
                        row![
                            text_input("Game title", &self.search).on_input(Event::SearchText),
                            button(if self.searching {
                                "Searching..."
                            } else {
                                "Search"
                            })
                            .on_press_maybe((!self.searching).then_some(Event::Search)),
                        ]
                        .spacing(8),
                    );
                    if !self.matches.is_empty() {
                        content = content.push(
                            pick_list(self.matches.clone(), self.selected.clone(), Event::Game)
                                .wineo_style(palette),
                        );
                    }
                }
            } else {
                content = content.push(text("SteamGridDB fetching needs a configured key. Local artwork is always available.").color(palette.muted));
            }
        }
        if let Some(error) = &self.error {
            content = content.push(text(error).color(palette.danger));
        }
        content
    }

    fn is_selected(&self, kind: Kind) -> bool {
        self.local
            .get(&kind)
            .is_some_and(|local| !local.path.is_empty())
    }

    /// Status messages for one kind, in display order.
    fn notes(&self, kind: Kind) -> Vec<Note<'_>> {
        let mut notes = Vec::new();
        if let Some(local) = self.local.get(&kind) {
            if local.checking {
                notes.push(Note::Info("Checking image...".to_owned()));
            } else if let Some(error) = &local.error {
                notes.push(Note::Error(error));
            } else if let Some(prefilled) = &local.prefilled {
                notes.push(Note::Info(prefilled.note.to_owned()));
            }
        }
        if !self.is_selected(kind)
            && self.fetch
            && self.kinds.contains(&kind)
            && let Some(remote) = self.remote.get(&kind)
        {
            if remote.loading {
                notes.push(Note::Info("Fetching...".to_owned()));
            } else if let Some(error) = &remote.error {
                notes.push(Note::Error(error));
            } else if remote.none_found {
                notes.push(Note::Info(format!("No {kind} artwork found")));
            } else if remote.preview.is_some() {
                notes.push(Note::Info("From SteamGridDB".to_owned()));
            }
        }
        notes
    }

    /// Hero, logo, and icon entries worth showing under the banner, with
    /// whether each can be removed. The icon's status lives here so the Name
    /// row it sits in stays one line tall.
    fn banner_statuses(&self) -> Vec<(Kind, Vec<Note<'_>>, bool)> {
        [Kind::Hero, Kind::Logo, Kind::Icon]
            .into_iter()
            .map(|kind| (kind, self.notes(kind), self.is_selected(kind)))
            .filter(|(_, notes, selected)| !notes.is_empty() || *selected)
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn tile<'a>(
        &'a self,
        kind: Kind,
        title: &'static str,
        hint: &'static str,
        width: u32,
        height: u32,
        labelled: bool,
        palette: Palette,
    ) -> Element<'a, Event> {
        let mut tile = column![].spacing(5).width(width);
        if labelled {
            tile = tile.push(text(title).size(13));
        }
        tile = tile.push(self.chooser(kind, title, hint, width, height, palette));
        for note in self.notes(kind) {
            tile = tile.push(note_view(note, palette));
        }
        if self.is_selected(kind) {
            tile = tile.push(remove(kind));
        }
        tile.into()
    }

    fn chooser<'a>(
        &'a self,
        kind: Kind,
        title: &'static str,
        hint: &'static str,
        width: u32,
        height: u32,
        palette: Palette,
    ) -> Element<'a, Event> {
        let selected = self.is_selected(kind);
        let picture: Element<'_, Event> = match self.art_preview(kind) {
            Some((handle, _)) => image(handle.clone())
                .width(width - 16)
                .height(height - 16)
                .content_fit(ContentFit::Contain)
                .into(),
            None if height <= ICON_SIZE => container(text(title).size(12))
                .width(width - 16)
                .height(height - 16)
                .center(Fill)
                .into(),
            None => container(placeholder(
                title,
                hint,
                if kind == Kind::Grid { 20 } else { 16 },
                palette,
            ))
            .width(width - 16)
            .height(height - 16)
            .center(Fill)
            .into(),
        };
        button(picture)
            .width(width)
            .height(height)
            .padding(8)
            .on_press(Event::Browse(kind))
            .style(move |_, status| button::Style {
                background: Some(palette.surface.into()),
                text_color: palette.foreground,
                border: border::rounded(8)
                    .width(1)
                    .color(outline(status, selected, palette)),
                ..button::Style::default()
            })
            .into()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Note<'a> {
    Info(String),
    Error(&'a str),
}

fn note_view(note: Note<'_>, palette: Palette) -> Element<'_, Event> {
    match note {
        Note::Info(message) => text(message).size(11).color(palette.muted).into(),
        Note::Error(message) => text(message).size(11).color(palette.danger).into(),
    }
}

fn remove<'a>(kind: Kind) -> Element<'a, Event> {
    button("Remove")
        .on_press(Event::Clear(kind))
        .padding(2)
        .into()
}

fn placeholder<'a>(
    title: &'static str,
    hint: &'static str,
    size: u32,
    palette: Palette,
) -> Column<'a, Event> {
    column![
        text(title).size(size),
        text(hint).size(11).color(palette.muted),
        text("Choose image").size(11).color(palette.primary),
    ]
    .align_x(iced::Center)
    .spacing(6)
}

fn outline(status: button::Status, selected: bool, palette: Palette) -> Color {
    if matches!(status, button::Status::Hovered) {
        palette.primary
    } else if selected {
        palette.success
    } else {
        palette.border
    }
}

/// Largest decoded preview per kind, sized for where each kind is shown so
/// the hero banner stays sharp while small tiles stay cheap.
fn preview_bounds(kind: Kind) -> (u32, u32) {
    match kind {
        Kind::Hero => (1000, 500),
        Kind::Logo => (LOGO_MAX_WIDTH, LOGO_HEIGHT),
        Kind::Grid => (POSTER_WIDTH, POSTER_HEIGHT),
        Kind::Icon => (160, 100),
    }
}

fn resolve(paths: Option<&Paths>, value: &str) -> Result<PathBuf, String> {
    match paths {
        Some(paths) => paths.config_path(value).map_err(|e| e.to_string()),
        None if Path::new(value).is_absolute() => Ok(PathBuf::from(value)),
        None => Err("Enter an absolute file path".to_owned()),
    }
}

fn preview(path: &Path, kind: Kind) -> Result<image::Handle, String> {
    preview_with_format(path, kind).map(|(handle, _)| handle)
}

/// Decodes `path` for `kind`, and says which format it is stored as.
pub(crate) fn preview_with_format(
    path: &Path,
    kind: Kind,
) -> Result<(image::Handle, art::Format), String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(art::MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > art::MAX_BYTES {
        return Err("Artwork exceeds 32 MiB".to_owned());
    }
    let handle = preview_bytes(&bytes, kind)?;
    let format =
        art::detect(&bytes).ok_or_else(|| format!("Unsupported image format for {kind}"))?;
    Ok((handle, format))
}

fn preview_bytes(bytes: &[u8], kind: Kind) -> Result<image::Handle, String> {
    let format = art::detect(bytes)
        .filter(|format| kind.accepts(*format))
        .ok_or_else(|| format!("Unsupported image format for {kind}"))?;
    let (max_width, max_height) = preview_bounds(kind);
    if format == art::Format::Svg {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default())
            .map_err(|e| e.to_string())?;
        let size = tree.size();
        let scale = (max_width as f32 / size.width())
            .min(max_height as f32 / size.height())
            .min(1.0);
        let width = (size.width() * scale).ceil().max(1.0) as u32;
        let height = (size.height() * scale).ceil().max(1.0) as u32;
        let mut pixels =
            resvg::tiny_skia::Pixmap::new(width, height).ok_or("Invalid image dimensions")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixels.as_mut(),
        );
        let mut rgba = pixels.take();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u16::from(*channel) * 255)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
        Ok(image::Handle::from_rgba(width, height, rgba))
    } else {
        let decoded = ::image::load_from_memory(bytes)
            .map_err(|e| e.to_string())?
            .thumbnail(max_width, max_height)
            .into_rgba8();
        Ok(image::Handle::from_rgba(
            decoded.width(),
            decoded.height(),
            decoded.into_raw(),
        ))
    }
}

fn fetch_preview(
    paths: &Paths,
    game: u64,
    kind: Kind,
    generation: &AtomicU64,
    sequence: u64,
) -> Result<Option<(Arc<art::Downloaded>, image::Handle)>, String> {
    let stale = || generation.load(Ordering::Relaxed) != sequence;
    if stale() {
        return Ok(None);
    }
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())?;
    let https = Https::new().map_err(|e| e.to_string())?;
    if stale() {
        return Ok(None);
    }
    let api = Api::new(&https, &key.value);
    let language = steamgriddb::language(|key| std::env::var_os(key));
    let chosen = art_fetch::choose(&api, game, kind, &language).map_err(|e| e.to_string())?;
    decode_if_current(generation, sequence, chosen, |downloaded| {
        let preview = preview_bytes(&downloaded.bytes, kind)?;
        Ok((Arc::new(downloaded), preview))
    })
}

// Decoding is the expensive step, so a superseded selection skips it.
fn decode_if_current<T, U>(
    generation: &AtomicU64,
    sequence: u64,
    chosen: Option<T>,
    decode: impl FnOnce(T) -> Result<U, String>,
) -> Result<Option<U>, String> {
    if generation.load(Ordering::Relaxed) != sequence {
        return Ok(None);
    }
    chosen.map(decode).transpose()
}

pub(crate) fn search(paths: &Paths, term: &str) -> Result<Vec<Game>, String> {
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())?;
    let https = Https::new().map_err(|e| e.to_string())?;
    Api::new(&https, &key.value)
        .search(term)
        .map_err(|e| e.to_string())
}

fn fetch(
    paths: &Paths,
    name: &str,
    game: &Game,
    kinds: impl Iterator<Item = Kind>,
) -> Result<Vec<String>, String> {
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())?;
    let https = Https::new().map_err(|e| e.to_string())?;
    let api = Api::new(&https, &key.value);
    let language = steamgriddb::language(|key| std::env::var_os(key));
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    art::save_game(paths, name, game.id).map_err(|e| e.to_string())?;
    for kind in kinds {
        match art_fetch::fetch_kind(
            paths,
            name,
            None,
            kind,
            &api,
            game.id,
            &language,
            Mode::Fill,
        ) {
            Ok(art_fetch::Fetched::NoneFound) => {
                notes.push(format!("No {kind} artwork found for {}", game.name))
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("Could not fetch {kind}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(notes)
    } else {
        Err(failures.join("\n"))
    }
}

pub(crate) fn fetch_default(paths: &Paths, name: &str, title: &str) -> Result<Vec<String>, String> {
    let global = GlobalConfig::load_or_default(paths).map_err(|e| e.to_string())?;
    let kinds = global.art.fetch.kinds();
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    match search(paths, title)?.into_iter().next() {
        Some(game) => fetch(paths, name, &game, kinds.into_iter()),
        None => Ok(vec![format!("No artwork match for {title}")]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wineo_core::environment::Environment;

    /// The editor that Heroic details and Import prefix embed, at the content
    /// width of the narrowest and widest import dialogs.
    #[test]
    fn the_editor_fits_narrow_and_wide_dialogs() {
        use crate::layout_probe::probe;
        let draft = Draft::new();
        // Page padding and the scrollbar gutter come off the dialog width.
        for width in [280.0_f32 - 48.0 - 16.0, 780.0 - 16.0] {
            let mut view: Element<'_, Event> =
                iced::widget::scrollable(draft.view(Palette::DARK, true))
                    .height(400)
                    .into();
            let probe = probe(&mut view, iced::Size::new(width, 400.0));
            let frame = iced::Rectangle::with_size(iced::Size::new(width, 400.0));
            let spilled = probe.outside(frame);
            assert!(spilled.is_empty(), "{width}: {spilled:?}");
            // A squeezed row would leave the poster tile a sliver.
            fn sizes(layout: iced::advanced::Layout<'_>, found: &mut Vec<iced::Size>) {
                found.push(layout.bounds().size());
                for child in layout.children() {
                    sizes(child, found);
                }
            }
            let mut found = Vec::new();
            sizes(iced::advanced::Layout::new(&probe.node), &mut found);
            assert!(
                found
                    .iter()
                    .any(|size| size.width >= 148.0 && size.height >= 222.0),
                "{width}: the poster tile lost its size"
            );
        }
    }

    #[test]
    fn stale_generation_skips_decode() {
        let generation = AtomicU64::new(2);
        let result = decode_if_current(&generation, 1, Some(()), |()| -> Result<u8, String> {
            panic!("decoded a stale download")
        });
        assert_eq!(result, Ok(None));
        let result = decode_if_current(&generation, 2, Some(()), |()| Ok(7u8));
        assert_eq!(result, Ok(Some(7)));
    }

    #[test]
    fn clearing_bumps_shared_generation() {
        let mut draft = Draft::new();
        let generation = draft.fetch_generation.clone();
        draft.clear_fetched();
        assert_eq!(generation.load(Ordering::Relaxed), draft.fetch_sequence);
    }

    #[test]
    fn fetch_uses_supplied_availability_without_storing_key_presence() {
        let mut draft = Draft::new();
        draft.kinds = vec![Kind::Icon];
        draft.remote.insert(Kind::Icon, Remote::default());
        let _ = draft.update_with_key_availability(Event::Fetch(true), false);
        assert!(!draft.fetch);
        let _ = draft.update_with_key_availability(Event::Fetch(true), true);
        assert!(draft.fetch);
        assert!(draft.remote.is_empty());
    }

    #[test]
    fn fetch_handles_are_pruned_and_dropped_kinds_are_aborted() {
        let mut draft = Draft::new();
        draft.fetch = true;
        draft.kinds = vec![Kind::Icon, Kind::Hero];
        let (_, icon) = Task::<Event>::none().abortable();
        let (_, hero) = Task::<Event>::none().abortable();
        draft.fetch_handles.insert(Kind::Icon, icon.clone());
        draft.fetch_handles.insert(Kind::Hero, hero.clone());
        draft.configuration_sequence = 1;
        let _ = draft.update_with_key_availability(Event::Configured(1, Ok(ArtFetch::Icon)), true);
        assert!(hero.is_aborted());
        assert!(!icon.is_aborted());
        assert_eq!(
            draft.fetch_handles.keys().copied().collect::<Vec<_>>(),
            vec![Kind::Icon]
        );
        let _ = draft.update(Event::Fetched(Kind::Icon, draft.fetch_sequence, Ok(None)));
        assert!(draft.fetch_handles.is_empty());
    }

    #[test]
    fn typing_title_does_not_discard_pending_art_configuration() {
        let mut draft = Draft::new();
        draft.configuration_sequence = 1;
        let _ = draft.set_title("new title");
        let _ = draft.update(Event::Configured(1, Ok(ArtFetch::default())));
        assert!(!draft.kinds.is_empty());
    }

    #[test]
    fn edited_search_keeps_selected_match_and_preview_when_title_changes() {
        let mut draft = Draft::new();
        draft.search = "Chosen match".to_owned();
        draft.search_edited = true;
        draft.selected = Some(Match(Game {
            id: 7,
            name: "Chosen match".to_owned(),
            year: None,
        }));
        draft.remote.insert(
            Kind::Icon,
            Remote {
                preview: Some(image::Handle::from_rgba(1, 1, vec![0; 4])),
                ..Remote::default()
            },
        );
        let sequence = draft.fetch_sequence;
        let _ = draft.set_title("Renamed environment");
        assert_eq!(draft.selected.as_ref().unwrap().0.id, 7);
        assert!(draft.remote[&Kind::Icon].preview.is_some());
        assert_eq!(draft.fetch_sequence, sequence);
    }

    #[test]
    fn failed_search_keeps_selected_match_and_preview() {
        let root = tempfile::tempdir().unwrap();
        let mut draft = Draft::new();
        draft.paths = Some(paths(root.path()));
        draft.search = "game".to_owned();
        draft.selected = Some(Match(Game {
            id: 7,
            name: "game".to_owned(),
            year: None,
        }));
        draft.remote.insert(
            Kind::Icon,
            Remote {
                preview: Some(image::Handle::from_rgba(1, 1, vec![0; 4])),
                ..Remote::default()
            },
        );
        let sequence = draft.fetch_sequence;
        let _ = draft.update_with_key_availability(Event::Search, true);
        assert!(draft.searching);
        assert_eq!(draft.fetch_sequence, sequence);
        let _ = draft.update(Event::Searched(draft.sequence, Err("offline".to_owned())));
        assert_eq!(draft.selected.as_ref().unwrap().0.id, 7);
        assert!(draft.remote[&Kind::Icon].preview.is_some());
        assert_eq!(draft.fetch_sequence, sequence);
    }

    #[test]
    fn reconfiguring_keeps_loaded_previews_and_fetches_only_added_kinds() {
        let root = tempfile::tempdir().unwrap();
        let mut draft = Draft::new();
        draft.paths = Some(paths(root.path()));
        draft.fetch = true;
        draft.kinds = vec![Kind::Icon];
        draft.selected = Some(Match(Game {
            id: 7,
            name: "game".to_owned(),
            year: None,
        }));
        let image = Arc::new(art::Downloaded {
            asset: 42,
            bytes: vec![1],
            format: art::Format::Png,
        });
        draft.remote.insert(
            Kind::Icon,
            Remote {
                image: Some(image.clone()),
                preview: Some(image::Handle::from_rgba(1, 1, vec![0; 4])),
                ..Remote::default()
            },
        );
        draft.configuration_sequence = 1;
        let sequence = draft.fetch_sequence;
        let _ = draft.update_with_key_availability(Event::Configured(1, Ok(ArtFetch::Icon)), true);
        assert_eq!(draft.remote.len(), 1);
        assert!(Arc::ptr_eq(
            draft.remote[&Kind::Icon].image.as_ref().unwrap(),
            &image
        ));
        assert!(draft.remote[&Kind::Icon].preview.is_some());
        assert_eq!(draft.fetch_sequence, sequence);

        draft.configuration_sequence = 2;
        let _ = draft.update_with_key_availability(
            Event::Configured(2, Ok(ArtFetch::Selected([true, true, false, false]))),
            true,
        );
        assert_eq!(draft.remote.len(), 2);
        assert!(Arc::ptr_eq(
            draft.remote[&Kind::Icon].image.as_ref().unwrap(),
            &image
        ));
        assert!(!draft.remote[&Kind::Icon].loading);
        assert!(draft.remote[&Kind::Grid].loading);
        assert_eq!(draft.fetch_sequence, sequence);
    }

    fn paths(root: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
    }

    #[test]
    fn local_art_works_without_fetching_and_preserves_source_bytes() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
        let source = root.path().join("my-image.data");
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="red"/></svg>"#;
        std::fs::write(&source, bytes).unwrap();
        let mut draft = Draft::new();
        draft.local.insert(
            Kind::Icon,
            Local {
                path: source.to_string_lossy().into_owned(),
                ..Local::default()
            },
        );
        assert!(!draft.fetch);
        assert!(draft.kinds.is_empty());
        assert!(draft.validate().is_ok());
        assert!(preview(&source, Kind::Icon).is_ok());
        assert!(draft.apply(&paths, "game").unwrap().is_empty());
        draft.fetch = true;
        draft.kinds = vec![Kind::Icon];
        draft.selected = Some(Match(Game {
            id: 123,
            name: "game".into(),
            year: None,
        }));
        assert!(draft.apply(&paths, "game").unwrap().is_empty());
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert_eq!(
            std::fs::read(paths.art_dir("game").join("icon.svg")).unwrap(),
            bytes
        );
        assert_eq!(
            art::ArtConfig::load(&paths.art_dir("game")).unwrap().kinds[&Kind::Icon].source,
            art::Source::Manual("my-image.data".to_owned())
        );
    }

    #[test]
    fn stale_previews_and_chooser_results_cannot_replace_an_edit() {
        let mut draft = Draft::new();
        draft.local.insert(
            Kind::Grid,
            Local {
                path: "/new.png".to_owned(),
                sequence: 3,
                chooser_sequence: 3,
                ..Local::default()
            },
        );
        let _ = draft.update(Event::Picked(
            Kind::Grid,
            2,
            Ok(Some(PathBuf::from("/old.png"))),
        ));
        let _ = draft.update(Event::Preview(Kind::Grid, 2, Err("old error".to_owned())));
        assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
        assert!(draft.local[&Kind::Grid].error.is_none());
        let _ = draft.update(Event::Picked(Kind::Grid, 3, Ok(None)));
        assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
        draft.local.get_mut(&Kind::Grid).unwrap().checking = true;
        let _ = draft.update(Event::Browse(Kind::Grid));
        let _ = draft.update(Event::Picked(Kind::Grid, 4, Ok(None)));
        assert!(!draft.manual_entry);
        let _ = draft.update(Event::Browse(Kind::Grid));
        let _ = draft.update(Event::Picked(
            Kind::Grid,
            5,
            Err("portal unavailable".to_owned()),
        ));
        assert!(draft.manual_entry);
        assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
        assert!(draft.local[&Kind::Grid].error.is_none());
        let _ = draft.update(Event::Preview(
            Kind::Grid,
            3,
            Ok(image::Handle::from_rgba(1, 1, vec![0; 4])),
        ));
        assert!(!draft.local[&Kind::Grid].checking);
        assert!(draft.local[&Kind::Grid].preview.is_some());
    }

    #[test]
    fn title_change_refreshes_unedited_search_without_accepting_stale_results() {
        let mut draft = Draft::new();
        draft.search = "Old title".to_owned();
        draft.searching = true;
        draft.sequence = 5;
        let _ = draft.set_title("New title");
        assert_eq!(draft.search, "New title");
        assert!(!draft.searching);
        let _ = draft.update(Event::Searched(
            5,
            Ok(vec![Game {
                id: 1,
                name: "Old title".to_owned(),
                year: None,
            }]),
        ));
        assert!(draft.matches.is_empty());
        let _ =
            draft.update_with_key_availability(Event::SearchText("Custom match".to_owned()), true);
        let _ = draft.set_title("Third title");
        assert_eq!(draft.search, "Custom match");
    }

    #[test]
    fn failed_art_does_not_remove_registration() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
        let mut draft = Draft::new();
        draft.local.insert(
            Kind::Logo,
            Local {
                path: root
                    .path()
                    .join("missing.png")
                    .to_string_lossy()
                    .into_owned(),
                ..Local::default()
            },
        );
        assert!(draft.apply(&paths, "game").is_err());
        assert!(Environment::load(&paths, "game").is_ok());
    }

    #[test]
    fn stale_fetched_art_is_ignored_after_match_change() {
        let root = tempfile::tempdir().unwrap();
        let mut draft = Draft::new();
        draft.paths = Some(paths(root.path()));
        draft.fetch = true;
        draft.kinds = vec![Kind::Grid];
        draft.selected = Some(Match(Game {
            id: 1,
            name: "Old".into(),
            year: None,
        }));
        draft.remote.insert(
            Kind::Grid,
            Remote {
                loading: true,
                ..Remote::default()
            },
        );
        let old_sequence = draft.fetch_sequence;
        let _ = draft.update_with_key_availability(
            Event::Game(Match(Game {
                id: 2,
                name: "New".into(),
                year: None,
            })),
            true,
        );
        let image = Arc::new(art::Downloaded {
            asset: 1,
            bytes: vec![1],
            format: art::Format::Png,
        });
        let handle = image::Handle::from_rgba(1, 1, vec![0; 4]);
        let _ = draft.update(Event::Fetched(
            Kind::Grid,
            old_sequence,
            Ok(Some((image, handle))),
        ));
        assert!(draft.remote[&Kind::Grid].loading);
        assert!(draft.remote[&Kind::Grid].image.is_none());
    }

    #[test]
    fn current_fetched_result_stores_image_and_preview() {
        let mut draft = Draft::new();
        draft.remote.insert(
            Kind::Grid,
            Remote {
                loading: true,
                ..Remote::default()
            },
        );
        let downloaded = Arc::new(art::Downloaded {
            asset: 42,
            bytes: vec![1],
            format: art::Format::Png,
        });
        let preview = image::Handle::from_rgba(1, 1, vec![0; 4]);
        let _ = draft.update(Event::Fetched(
            Kind::Grid,
            draft.fetch_sequence,
            Ok(Some((downloaded.clone(), preview))),
        ));
        assert!(!draft.remote[&Kind::Grid].loading);
        assert!(Arc::ptr_eq(
            draft.remote[&Kind::Grid].image.as_ref().unwrap(),
            &downloaded
        ));
        assert!(draft.remote[&Kind::Grid].preview.is_some());
    }

    #[test]
    fn none_found_preview_skips_fetch_and_reports_note() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
        let mut draft = Draft::new();
        draft.fetch = true;
        draft.kinds = vec![Kind::Grid];
        draft.selected = Some(Match(Game {
            id: 7,
            name: "game".to_owned(),
            year: None,
        }));
        draft.remote.insert(
            Kind::Grid,
            Remote {
                loading: true,
                ..Remote::default()
            },
        );
        let _ = draft.update(Event::Fetched(Kind::Grid, draft.fetch_sequence, Ok(None)));
        assert!(draft.remote[&Kind::Grid].none_found);
        assert!(!draft.remote[&Kind::Grid].loading);
        assert_eq!(
            draft.apply(&paths, "game").unwrap(),
            vec!["No grid artwork found for game"]
        );
    }

    #[test]
    fn local_selection_allows_validation_while_remote_is_loading() {
        let mut draft = Draft::new();
        draft.remote.insert(
            Kind::Grid,
            Remote {
                loading: true,
                ..Remote::default()
            },
        );
        draft.local.insert(
            Kind::Grid,
            Local {
                path: "/grid.png".to_owned(),
                ..Local::default()
            },
        );
        assert!(draft.validate().is_ok());
    }

    #[test]
    fn local_selection_wins_over_fetched_preview() {
        let mut draft = Draft::new();
        let local_preview = image::Handle::from_rgba(1, 1, vec![0; 4]);
        draft.local.insert(
            Kind::Icon,
            Local {
                path: "/icon.png".into(),
                preview: Some(local_preview),
                ..Local::default()
            },
        );
        draft.remote.insert(
            Kind::Icon,
            Remote {
                preview: Some(image::Handle::from_rgba(1, 1, vec![255; 4])),
                ..Remote::default()
            },
        );
        let (shown, fetched) = draft.art_preview(Kind::Icon).unwrap();
        assert!(!fetched);
        assert!(std::ptr::eq(
            shown,
            draft.local[&Kind::Icon].preview.as_ref().unwrap()
        ));
    }

    fn decoded_size(handle: &image::Handle) -> (u32, u32) {
        match handle {
            image::Handle::Rgba { width, height, .. } => (*width, *height),
            _ => panic!("expected decoded pixels"),
        }
    }

    #[test]
    fn hero_previews_decode_large_enough_for_the_banner() {
        let mut png = Vec::new();
        ::image::RgbaImage::new(1920, 620)
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        assert_eq!(
            decoded_size(&preview_bytes(&png, Kind::Hero).unwrap()),
            (1000, 323)
        );
        let (width, height) = decoded_size(&preview_bytes(&png, Kind::Icon).unwrap());
        assert!(width <= 160 && height <= 100);
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1920" height="620"><rect width="1920" height="620" fill="red"/></svg>"#;
        assert_eq!(
            decoded_size(&preview_bytes(svg, Kind::Hero).unwrap()),
            (1000, 323)
        );
        assert_eq!(preview_bounds(Kind::Grid), (POSTER_WIDTH, POSTER_HEIGHT));
    }

    #[test]
    fn banner_statuses_list_hero_logo_and_icon_with_something_to_say() {
        let mut draft = Draft::new();
        assert!(draft.banner_statuses().is_empty());
        draft.fetch = true;
        draft.kinds = Kind::ALL.to_vec();
        draft.remote.insert(
            Kind::Logo,
            Remote {
                loading: true,
                ..Remote::default()
            },
        );
        draft.remote.insert(
            Kind::Icon,
            Remote {
                none_found: true,
                ..Remote::default()
            },
        );
        draft.local.insert(
            Kind::Hero,
            Local {
                path: "/hero.png".to_owned(),
                ..Local::default()
            },
        );
        assert_eq!(
            draft.banner_statuses(),
            vec![
                (Kind::Hero, vec![], true),
                (
                    Kind::Logo,
                    vec![Note::Info("Fetching...".to_owned())],
                    false
                ),
                (
                    Kind::Icon,
                    vec![Note::Info("No icon artwork found".to_owned())],
                    false
                ),
            ]
        );
        assert_eq!(
            draft.notes(Kind::Icon),
            vec![Note::Info("No icon artwork found".to_owned())]
        );
    }

    #[test]
    fn local_selection_hides_fetch_status() {
        let mut draft = Draft::new();
        draft.fetch = true;
        draft.kinds = vec![Kind::Hero];
        draft.remote.insert(
            Kind::Hero,
            Remote {
                error: Some("offline".to_owned()),
                ..Remote::default()
            },
        );
        assert_eq!(draft.notes(Kind::Hero), vec![Note::Error("offline")]);
        draft.local.insert(
            Kind::Hero,
            Local {
                path: "/hero.png".to_owned(),
                checking: true,
                ..Local::default()
            },
        );
        assert_eq!(
            draft.notes(Kind::Hero),
            vec![Note::Info("Checking image...".to_owned())]
        );
    }

    #[test]
    fn apply_stores_held_art_without_a_request() {
        const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
        let mut draft = Draft::new();
        draft.fetch = true;
        draft.kinds = vec![Kind::Grid];
        draft.selected = Some(Match(Game {
            id: 7,
            name: "game".into(),
            year: None,
        }));
        draft.remote.insert(
            Kind::Grid,
            Remote {
                image: Some(Arc::new(art::Downloaded {
                    asset: 42,
                    bytes: PNG.to_vec(),
                    format: art::Format::Png,
                })),
                ..Remote::default()
            },
        );
        assert!(draft.apply(&paths, "game").unwrap().is_empty());
        assert_eq!(
            std::fs::read(paths.art_dir("game").join("grid.png")).unwrap(),
            PNG
        );
        assert_eq!(art::saved_game(&paths, "game").unwrap(), Some(7));
        assert_eq!(
            art::ArtConfig::load(&paths.art_dir("game")).unwrap().kinds[&Kind::Grid].source,
            art::Source::SteamGridDb(42)
        );
    }

    #[test]
    fn a_prefilled_pick_is_stored_under_its_name_until_removed_or_replaced() {
        const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        const FETCHED: &[u8] = b"\xff\xd8\xff\xe0 fetched";
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let poster = root.path().join("0123abcd");
        std::fs::write(&poster, PNG).unwrap();
        let mine = root.path().join("mine.png");
        std::fs::write(&mine, PNG).unwrap();
        let fetched = || {
            Draft::with_fetched(
                Kind::Grid,
                7,
                art::Downloaded {
                    asset: 42,
                    bytes: FETCHED.to_vec(),
                    format: art::Format::Jpeg,
                },
            )
        };
        let prefill = |draft: &mut Draft| {
            draft.prefill(
                Kind::Grid,
                &poster,
                "heroic-a.png".to_owned(),
                "From Heroic",
                image::Handle::from_rgba(1, 1, vec![0; 4]),
            )
        };
        let grid = |env: &str| {
            art::ArtConfig::load(&paths.art_dir(env)).unwrap().kinds[&Kind::Grid]
                .source
                .clone()
        };
        for env in ["kept", "removed", "replaced"] {
            Environment::create_native(&paths, &GlobalConfig::default(), env, None, false).unwrap();
        }

        let mut draft = fetched();
        prefill(&mut draft);
        assert!(draft.is_selected(Kind::Grid));
        assert_eq!(
            draft.notes(Kind::Grid),
            [Note::Info("From Heroic".to_owned())]
        );
        draft.apply(&paths, "kept").unwrap();
        assert_eq!(grid("kept"), art::Source::Manual("heroic-a.png".into()));
        assert!(!paths.art_dir("kept").join("grid.jpg").exists());

        let mut draft = fetched();
        prefill(&mut draft);
        let _ = draft.update(Event::Clear(Kind::Grid));
        assert!(!draft.is_selected(Kind::Grid));
        draft.apply(&paths, "removed").unwrap();
        assert_eq!(grid("removed"), art::Source::SteamGridDb(42));

        let mut draft = fetched();
        prefill(&mut draft);
        let _ = draft.update(Event::Path(Kind::Grid, mine.to_string_lossy().into_owned()));
        assert_eq!(draft.prefilled(Kind::Grid), None);
        draft.apply(&paths, "replaced").unwrap();
        assert_eq!(grid("replaced"), art::Source::Manual("mine.png".into()));
    }
}
