use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iced::widget::{
    Column, button, checkbox, column, container, image, pick_list, row, text, text_input,
};
use iced::{Color, ContentFit, Element, Fill, Task, border};
use wineo_core::art::{self, Kind};
use wineo_core::art_fetch::{self, Mode};
use wineo_core::cancel::Token;
use wineo_core::config::{ArtFetch, GlobalConfig};
use wineo_core::desktop;
use wineo_core::download::{Https, Lazy, Transport};
use wineo_core::environment::{Currency, EnvironmentIdentity};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game};

use crate::task;
use crate::theme::Palette;
use crate::ui::{PickListStyle, secondary_button};

/// Width of the stacked tile block.
const WIDTH: u32 = 356;
const LOGO_HEIGHT: u32 = 80;
const LOGO_MAX_WIDTH: u32 = 280;
/// Tall enough to line up with the collapsed creation form beside it.
pub(crate) const POSTER_WIDTH: u32 = 152;
pub(crate) const POSTER_HEIGHT: u32 = 228;
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

    pub(crate) fn configure(&mut self, paths: Paths, title: String) -> Task<Event> {
        self.paths = Some(paths.clone());
        if self.search.is_empty() {
            self.search = title;
        }
        self.configuration_sequence = self.configuration_sequence.wrapping_add(1);
        let sequence = self.configuration_sequence;
        task::blocking(
            move || {
                // A fallback to defaults isn't reported here, since the checkup
                // already lists it as `global_config_invalid`.
                let loaded = GlobalConfig::load_or_default(&paths).map_err(|e| e.to_string())?;
                Ok(loaded.config.art.fetch)
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
                return task::blocking(
                    move || {
                        let path = resolve(paths.as_ref(), &value)?;
                        preview(&path, kind)
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
                return task::blocking(
                    move || search(&paths, &term),
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
            let (task, handle) = task::blocking(
                move || fetch_preview(&paths, game.0.id, kind, &generation, sequence),
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

    /// The kinds a fetch fills: the configured ones with no file chosen.
    fn fetch_kinds(&self) -> Vec<Kind> {
        self.kinds
            .iter()
            .copied()
            .filter(|kind| {
                self.local
                    .get(kind)
                    .is_none_or(|local| local.path.is_empty())
            })
            .collect()
    }

    /// Whether `apply` would write anything.
    fn has_work(&self) -> bool {
        self.local.values().any(|local| !local.path.is_empty())
            || (self.fetch && self.selected.is_some() && !self.fetch_kinds().is_empty())
    }

    /// Stores the draft's art in `name` while the environment is still the
    /// one `identity` names, the identity the flow's creation returned.
    /// Without one, one is acquired, and only when there is something to
    /// write, so a draft with nothing in it never gives an environment an
    /// id.
    pub(crate) fn apply(
        &self,
        paths: &Paths,
        name: &str,
        identity: Option<EnvironmentIdentity>,
    ) -> Fetched {
        if !self.has_work() {
            return Fetched {
                art: Ok(Vec::new()),
                identity,
                icon: false,
            };
        }
        let identity = match identity {
            Some(identity) => Ok(identity),
            // A name core would refuse, or a missing environment, gets no
            // identity, and that error for each kind.
            None => EnvironmentIdentity::acquire_existing(paths, name).map_err(|e| e.to_string()),
        };
        self.apply_for(paths, name, identity)
    }

    /// `apply`, storing the art only while the environment is still the one
    /// `identity` was read from. Without one, each kind fails with why there
    /// is none.
    fn apply_for(
        &self,
        paths: &Paths,
        name: &str,
        identity: Result<EnvironmentIdentity, String>,
    ) -> Fetched {
        let held_id = identity
            .as_ref()
            .is_ok_and(|identity| identity.id().is_some());
        let mut notes = Vec::new();
        let mut failures = Vec::new();
        let mut icon = false;
        for (kind, local) in &self.local {
            if local.path.is_empty() {
                continue;
            }
            let could_not = |error: String| format!("Could not store {kind}: {error}");
            let path = match resolve(Some(paths), &local.path) {
                Ok(path) => path,
                Err(error) => {
                    failures.push(FetchError::Other(could_not(error)));
                    continue;
                }
            };
            let identity = match &identity {
                Ok(identity) => *identity,
                Err(error) => {
                    failures.push(FetchError::Other(could_not(error.clone())));
                    continue;
                }
            };
            let stored = match &local.prefilled {
                Some(prefilled) => art::set_manual_as(
                    paths,
                    name,
                    None,
                    *kind,
                    &path,
                    &prefilled.original,
                    identity,
                ),
                None => art::set_manual(paths, name, None, *kind, &path, identity),
            };
            match stored {
                Ok(_) => icon |= *kind == Kind::Icon,
                Err(error) => failures.push(FetchError::from_core(error, held_id, could_not)),
            }
        }
        let kinds = self.fetch_kinds();
        if self.fetch
            && let Some(game) = &self.selected
            && !kinds.is_empty()
        {
            let failed = |error: String| format!("Art fetching failed: {error}");
            let saved = match &identity {
                Ok(identity) => art::save_game_for(paths, name, game.0.id, *identity, None)
                    .map(|()| *identity)
                    .map_err(|error| FetchError::from_core(error, held_id, failed)),
                Err(error) => Err(FetchError::Other(failed(error.clone()))),
            };
            match saved {
                Err(error) => failures.push(error),
                Ok(identity) => {
                    let missing = self.publish_held(
                        paths,
                        name,
                        &game.0,
                        &kinds,
                        identity,
                        (&mut notes, &mut failures, &mut icon),
                    );
                    if !missing.is_empty() {
                        let (fetched, stored_icon) = fetch(
                            (paths, name),
                            &game.0,
                            &missing,
                            identity,
                            (&Lazy::new(), &Token::new()),
                        );
                        icon |= stored_icon;
                        match fetched {
                            Ok(fetched) => notes.extend(fetched),
                            Err(error) => failures.push(error.described(failed)),
                        }
                    }
                }
            }
        }
        Fetched {
            art: FetchError::combine(failures).map_or(Ok(notes), Err),
            identity: identity.ok(),
            icon,
        }
    }

    /// Publishes the kinds already downloaded for the preview, only into the
    /// environment `identity` was read from, and returns the kinds that still
    /// need fetching.
    fn publish_held(
        &self,
        paths: &Paths,
        name: &str,
        game: &Game,
        kinds: &[Kind],
        identity: EnvironmentIdentity,
        (notes, failures, icon): (&mut Vec<String>, &mut Vec<FetchError>, &mut bool),
    ) -> Vec<Kind> {
        let held_id = identity.id().is_some();
        let mut missing = Vec::new();
        for &kind in kinds {
            if let Some(image) = self
                .remote
                .get(&kind)
                .and_then(|remote| remote.image.as_ref())
            {
                match art_fetch::publish_for(paths, name, None, kind, image, Mode::Fill, identity) {
                    Ok(art_fetch::Fetched::Stored { .. }) => *icon |= kind == Kind::Icon,
                    Ok(_) => {}
                    Err(error) => failures.push(FetchError::from_error(&error, held_id, |error| {
                        format!("Could not fetch {kind}: {error}")
                    })),
                }
            } else if self
                .remote
                .get(&kind)
                .is_some_and(|remote| remote.none_found)
            {
                notes.push(format!("No {kind} artwork found for {}", game.name));
            } else {
                missing.push(kind);
            }
        }
        missing
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

    fn control_rows(&self, palette: Palette, key_available: bool) -> Column<'_, Event> {
        let mut content = column![
            secondary_button(
                if self.manual_entry {
                    "Hide file paths"
                } else {
                    "Enter a file path instead"
                },
                palette
            )
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
                            secondary_button(
                                if self.searching {
                                    "Searching..."
                                } else {
                                    "Search"
                                },
                                palette
                            )
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
            tile = tile.push(remove(kind, palette));
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

fn remove<'a>(kind: Kind, palette: Palette) -> Element<'a, Event> {
    secondary_button("Remove", palette)
        .on_press(Event::Clear(kind))
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
    search_with(paths, term, &Lazy::new())
}

fn search_with(paths: &Paths, term: &str, https: &Lazy<Https>) -> Result<Vec<Game>, String> {
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())?;
    let https = https.https().map_err(|e| e.to_string())?;
    Api::new(https, &key.value)
        .search(term)
        .map_err(|e| e.to_string())
}

fn fetch(
    (paths, name): (&Paths, &str),
    game: &Game,
    kinds: &[Kind],
    identity: EnvironmentIdentity,
    (https, token): (&Lazy<Https>, &Token),
) -> (Result<Vec<String>, FetchError>, bool) {
    let connected = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())
    .and_then(|key| Ok((https.https().map_err(|e| e.to_string())?, key)));
    match connected {
        Ok((https, key)) => {
            let api = Api::new(https, &key.value);
            fetch_with((paths, name), &api, game, kinds, identity, token)
        }
        Err(error) => (Err(FetchError::Other(error)), false),
    }
}

/// Fetches `kinds` of `game` into `name`, and says whether an icon was
/// stored, which the desktop entries show even when a later kind failed.
fn fetch_with(
    (paths, name): (&Paths, &str),
    api: &Api,
    game: &Game,
    kinds: &[Kind],
    identity: EnvironmentIdentity,
    token: &Token,
) -> (Result<Vec<String>, FetchError>, bool) {
    let held_id = identity.id().is_some();
    let language = steamgriddb::language(|key| std::env::var_os(key));
    let fetched =
        match art_fetch::fetch_game(paths, name, api, game.id, kinds, &language, identity, token) {
            Ok(fetched) => fetched,
            Err(error) => {
                return (
                    Err(FetchError::from_error(&error, held_id, |error| error)),
                    false,
                );
            }
        };
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    let mut icon = false;
    for (kind, result) in fetched {
        match result {
            Ok(art_fetch::Fetched::NoneFound) => {
                notes.push(format!("No {kind} artwork found for {}", game.name))
            }
            Ok(art_fetch::Fetched::Stored { .. }) => icon |= kind == Kind::Icon,
            Ok(_) => {}
            Err(error) => failures.push(FetchError::from_error(&error, held_id, |error| {
                format!("Could not fetch {kind}: {error}")
            })),
        }
    }
    (FetchError::combine(failures).map_or(Ok(notes), Err), icon)
}

/// Looks `title` up and fetches the configured kinds for `name`, writing
/// only while it is still the environment `identity` names. A batch passes
/// the same `https` for every game, so they share one client.
pub(crate) fn fetch_default(
    paths: &Paths,
    name: &str,
    title: &str,
    identity: Option<EnvironmentIdentity>,
    https: &Lazy<Https>,
    token: &Token,
) -> Fetched {
    fetch_default_over(paths, name, title, identity, token, &|| {
        let key = steamgriddb::require_key(
            paths,
            |key| std::env::var_os(key),
            &SecretServiceKeyring::steamgriddb(),
        )
        .map_err(|e| e.to_string())?;
        let https = https.https().map_err(|e| e.to_string())?;
        Ok((Box::new(https.clone()) as Box<dyn Transport>, key.value))
    })
}

/// The connection art is fetched over and the key to use on it, which tests
/// replace so nothing reaches SteamGridDB.
pub(crate) type Connect<'a> = &'a dyn Fn() -> Result<(Box<dyn Transport>, String), String>;

/// `fetch_default` over `connect`. The identity is settled before the
/// search: the flow's own, or when no creation step handed one over, one
/// acquired here, once there are kinds to fetch.
pub(crate) fn fetch_default_over(
    paths: &Paths,
    name: &str,
    title: &str,
    identity: Option<EnvironmentIdentity>,
    token: &Token,
    connect: Connect<'_>,
) -> Fetched {
    let unwritten = |art| Fetched {
        art,
        identity,
        icon: false,
    };
    let loaded = match GlobalConfig::load_or_default(paths) {
        Ok(loaded) => loaded,
        Err(error) => return unwritten(Err(FetchError::Other(error.to_string()))),
    };
    // Reported through the same notes as every other outcome here.
    let mut notes: Vec<String> = loaded
        .warning
        .iter()
        .map(|warning| format!("Warning: {}", wineo_core::error::one_line(warning)))
        .collect();
    let kinds = loaded.config.art.fetch.kinds();
    if kinds.is_empty() {
        return unwritten(Ok(notes));
    }
    if token.is_canceled() {
        return unwritten(Err(FetchError::Other(
            wineo_core::error::Error::Canceled("artwork").to_string(),
        )));
    }
    let identity = match identity {
        Some(identity) => identity,
        None => match EnvironmentIdentity::acquire_existing(paths, name) {
            Ok(identity) => identity,
            Err(error) => return unwritten(Err(FetchError::Other(error.to_string()))),
        },
    };
    let held = |art, icon| Fetched {
        art,
        identity: Some(identity),
        icon,
    };
    let (transport, key) = match connect() {
        Ok(connected) => connected,
        Err(error) => return held(Err(FetchError::Other(error)), false),
    };
    let api = Api::new(&*transport, &key);
    match api.search(title).map(|games| games.into_iter().next()) {
        Err(error) => held(Err(FetchError::Other(error.to_string())), false),
        Ok(Some(game)) => {
            let (fetched, icon) = fetch_with((paths, name), &api, &game, &kinds, identity, token);
            held(
                fetched.map(|more| {
                    notes.extend(more);
                    notes
                }),
                icon,
            )
        }
        Ok(None) => {
            notes.push(format!("No artwork match for {title}"));
            held(Ok(notes), false)
        }
    }
}

/// What a flow says when its art wasn't stored because its environment is
/// no longer the one it holds: replaced, for an id, or changed, for the
/// fingerprint kept when the environment has no id.
pub(crate) fn changed_message(id: bool) -> &'static str {
    if id {
        "Artwork wasn't stored because the environment was replaced."
    } else {
        "Artwork wasn't stored because the environment changed."
    }
}

/// What the add flows and the Art page say when an icon was stored but the
/// desktop entries were left alone because `library.toml` can't be read.
pub(crate) fn entries_unreadable(name: &str) -> String {
    format!("library.toml can't be read. The desktop entries for `{name}` weren't updated.")
}

/// Why art after creation wasn't stored, sorted so a flow can offer what
/// fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchError {
    /// The environment isn't the one the flow holds, which `id` says was an
    /// id rather than the fingerprint. Retrying with the same identity can
    /// only fail again, so only Skip is offered.
    Changed {
        id: bool,
    },
    /// `library.toml` can't be read: its sentence, then any other failures.
    LibraryUnreadable(String),
    Other(String),
}

impl FetchError {
    pub(crate) fn is_changed(&self) -> bool {
        matches!(self, Self::Changed { .. })
    }

    /// Sorts a failed write by core's error, with `describe` wording any
    /// that isn't about the environment or its library.
    fn from_error(
        error: &anyhow::Error,
        held_id: bool,
        describe: impl FnOnce(String) -> String,
    ) -> Self {
        match error
            .chain()
            .find_map(|cause| cause.downcast_ref::<wineo_core::Error>())
        {
            Some(wineo_core::Error::ArtEnvironmentChanged(_)) => Self::Changed { id: held_id },
            Some(unreadable @ wineo_core::Error::ArtLibraryUnreadable { .. }) => {
                Self::LibraryUnreadable(unreadable.to_string())
            }
            _ => Self::Other(describe(error.to_string())),
        }
    }

    fn from_core(
        error: wineo_core::Error,
        held_id: bool,
        describe: impl FnOnce(String) -> String,
    ) -> Self {
        Self::from_error(&anyhow::Error::from(error), held_id, describe)
    }

    /// Rewords an `Other` failure, leaving the sorted kinds as they are.
    fn described(self, describe: impl FnOnce(String) -> String) -> Self {
        match self {
            Self::Other(error) => Self::Other(describe(error)),
            error => error,
        }
    }

    /// One failure for several: "changed" if any kind was, since nothing
    /// more can be stored, then a library that can't be read, said once.
    fn combine(failures: Vec<Self>) -> Option<Self> {
        if let Some(changed) = failures.iter().find(|failure| failure.is_changed()) {
            return Some(changed.clone());
        }
        let mut unreadable = None;
        let mut lines: Vec<String> = Vec::new();
        for failure in failures {
            match failure {
                Self::LibraryUnreadable(line) => {
                    unreadable.get_or_insert(line);
                }
                Self::Other(line) => lines.push(line),
                Self::Changed { .. } => unreachable!("returned above"),
            }
        }
        match unreadable {
            Some(sentence) => {
                lines.insert(0, sentence);
                Some(Self::LibraryUnreadable(lines.join("\n")))
            }
            None if lines.is_empty() => None,
            None => Some(Self::Other(lines.join("\n"))),
        }
    }

    /// This failure with the desktop entries' after it, in one message.
    pub(crate) fn with_entries(self, entries: &EntriesError) -> Self {
        match (self, entries) {
            (Self::Other(error), EntriesError::Failed(entry)) => {
                Self::Other(format!("{error}; desktop entry: {entry}"))
            }
            (Self::LibraryUnreadable(error), EntriesError::Failed(entry)) => {
                Self::LibraryUnreadable(format!("{error}; desktop entry: {entry}"))
            }
            (
                Self::Other(error) | Self::LibraryUnreadable(error),
                EntriesError::LibraryUnreadable(entry),
            ) => Self::LibraryUnreadable(format!("{error}\n{entry}")),
            // Changed leaves the entries alone, so they can't have failed.
            (changed @ Self::Changed { .. }, _) => changed,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed { id } => f.write_str(changed_message(*id)),
            Self::LibraryUnreadable(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

/// Why the desktop entries weren't brought in line after a fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntriesError {
    /// Skipped after an icon was stored, since `library.toml` can't be read
    /// to tell whether the environment is still the same one.
    LibraryUnreadable(String),
    Failed(String),
}

impl std::fmt::Display for EntriesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryUnreadable(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

/// What a fetch after creation came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fetched {
    pub(crate) art: Result<Vec<String>, FetchError>,
    /// The identity it wrote with, which the flow keeps for a Retry. None
    /// when it took none.
    pub(crate) identity: Option<EnvironmentIdentity>,
    /// Whether an icon was stored.
    pub(crate) icon: bool,
}

impl Fetched {
    /// What a fake fetcher answers: `art`, written with the identity it was
    /// handed, and no icon.
    #[cfg(test)]
    pub(crate) fn answer(
        art: Result<Vec<String>, FetchError>,
        identity: Option<EnvironmentIdentity>,
    ) -> Self {
        Self {
            art,
            identity,
            icon: false,
        }
    }
}

/// Fetches art for an environment, from its name, the title to search for,
/// the identity the flow holds, and the token that stops it. Tests swap in
/// their own, so nothing they run reaches SteamGridDB.
pub(crate) type Fetcher =
    Arc<dyn Fn(&Paths, &str, &str, Option<EnvironmentIdentity>, &Token) -> Fetched + Send + Sync>;

pub(crate) fn default_fetcher() -> Fetcher {
    Arc::new(|paths, name, title, identity, token| {
        fetch_default(paths, name, title, identity, &Lazy::new(), token)
    })
}

/// What a flow's Details offers for fetching art after the environment
/// registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtOffer {
    /// The settings haven't been read yet.
    Unknown,
    /// Art to fetch has no kind on.
    Off,
    NoKey,
    /// The checkbox, and whether it's on.
    Fetch(bool),
}

impl ArtOffer {
    /// `fetch_kinds` is whether Art to fetch has any kind on, once read, and
    /// `choice` is what the user set the checkbox to, if they did. The key is
    /// read when the page draws, so a default follows it.
    pub(crate) fn new(
        fetch_kinds: Option<bool>,
        key_available: bool,
        choice: Option<bool>,
    ) -> Self {
        match fetch_kinds {
            None => Self::Unknown,
            Some(false) => Self::Off,
            Some(true) if !key_available => Self::NoKey,
            Some(true) => Self::Fetch(choice.unwrap_or(true)),
        }
    }

    /// The checkbox, or a note on why there's none. `after` names the button
    /// whose result the Art page picks up from, like "Create".
    pub(crate) fn view<'a, M: Clone + 'a>(
        self,
        palette: Palette,
        after: &str,
        on_toggle: impl Fn(bool) -> M + 'a,
    ) -> Option<Element<'a, M>> {
        let note = match self {
            Self::Unknown => return None,
            Self::Fetch(on) => {
                return Some(
                    checkbox(on)
                        .label("Fetch art using the top SteamGridDB match")
                        .on_toggle(on_toggle)
                        .into(),
                );
            }
            Self::Off => "Art to fetch is off in Settings.".to_owned(),
            Self::NoKey => {
                format!(
                    "Add a SteamGridDB key in Settings to fetch art, or choose it after {after}."
                )
            }
        };
        Some(text(note).size(12).color(palette.muted).into())
    }
}

/// What the fetch and the desktop entry reapply after it each came to, apart
/// so a caller can tell which half failed, and the identity to retry with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FetchParts {
    pub(crate) art: Result<Vec<String>, FetchError>,
    pub(crate) entries: Result<(), EntriesError>,
    pub(crate) identity: Option<EnvironmentIdentity>,
}

/// The fetch after an environment registers, then the desktop entry again,
/// since its icon comes from the art. `identity` is the flow's, None when no
/// creation step handed one over.
pub(crate) fn fetch_art_parts(
    paths: &Paths,
    name: &str,
    title: &str,
    identity: Option<EnvironmentIdentity>,
    token: &Token,
    fetcher: &Fetcher,
) -> FetchParts {
    let fetched = fetcher(paths, name, title, identity, token);
    let parts = |entries| FetchParts {
        art: fetched.art.clone(),
        entries,
        identity: fetched.identity,
    };
    // Skipped, so nothing more is written; a later launch refreshes the entry.
    if fetched.art.is_err() && token.is_canceled() {
        return parts(Ok(()));
    }
    // A replacement environment's entries are not this fetch's to touch.
    if fetched.art.as_ref().is_err_and(FetchError::is_changed) {
        return parts(Ok(()));
    }
    match fetched.identity.map(|held| held.is_current(paths, name)) {
        None | Some(Currency::Current) => {}
        Some(Currency::Changed) => return parts(Ok(())),
        // Said only when the entries would have shown a new icon, and never
        // for a portable root, which has none.
        Some(Currency::LibraryUnreadable) => {
            let said = fetched.icon && desktop::supported(paths).is_ok();
            return parts(if said {
                Err(EntriesError::LibraryUnreadable(entries_unreadable(name)))
            } else {
                Ok(())
            });
        }
    }
    parts(
        desktop::apply(paths, name, false)
            .map(|_| ())
            .map_err(|error| EntriesError::Failed(error.to_string())),
    )
}

/// `fetch_art_parts` as one result, for a page that shows a single error.
pub(crate) fn fetch_art(
    paths: &Paths,
    name: &str,
    title: &str,
    identity: Option<EnvironmentIdentity>,
    token: &Token,
    fetcher: &Fetcher,
) -> (Result<Vec<String>, FetchError>, Option<EnvironmentIdentity>) {
    let parts = fetch_art_parts(paths, name, title, identity, token, fetcher);
    let art = match (parts.art, parts.entries) {
        (Ok(notes), Ok(())) => Ok(notes),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(EntriesError::Failed(error))) => Err(FetchError::Other(error)),
        (Ok(_), Err(EntriesError::LibraryUnreadable(error))) => {
            Err(FetchError::LibraryUnreadable(error))
        }
        (Err(error), Err(entries)) => Err(error.with_entries(&entries)),
    };
    (art, parts.identity)
}

#[cfg(test)]
pub(crate) mod canned;
#[cfg(test)]
mod tests;
