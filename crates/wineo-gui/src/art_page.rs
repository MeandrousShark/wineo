use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iced::widget::{
    button, column, container, image, pick_list, row, scrollable, stack, text, text_input,
};
use iced::{Center, ContentFit, Element, Fill, Task, border};
use wineo_core::art::{self, Kind, Origin, Resolved, Source};
use wineo_core::cancel::Token;
use wineo_core::config::{self, GlobalConfig};
use wineo_core::download::Https;
use wineo_core::environment::Environment;
pub(crate) use wineo_core::environment::EnvironmentIdentity;
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game};
use wineo_core::{art_fetch, desktop};

use crate::chips::secondary;
use crate::flow::artwork::{self as flow_artwork, Match};
use crate::theme::Palette;
use crate::ui::PickListStyle;
use crate::ui::{ScrollStyle, accent_base, secondary_button};
use crate::wheel;
use crate::{art_picker, artwork};

const PAGE_WIDTH: f32 = 860.0;
const KIND_ROWS: [(Kind, &str, &str, u32, u32); 4] = [
    (Kind::Icon, "Icon", "App menu and desktop entry", 88, 88),
    (Kind::Grid, "Grid", "The library card", 80, 120),
    (Kind::Hero, "Hero", "The top of the detail panel", 160, 75),
    (
        Kind::Logo,
        "Logo",
        "Over the hero, in the panel corner",
        150,
        65,
    ),
];

type Spot = (Option<String>, Kind);

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Find(Kind),
    Picker(u64, art_picker::Event),
    UseProgress(u64, u64, Option<u64>),
    Used(u64, u64, Spot, WriteOutcome),
    CancelUse,
    Settings,
    Showing(Showing),
    Browse(Kind),
    Picked(u64, Spot, Result<Option<PathBuf>, String>),
    Path(String),
    UsePath,
    CancelPath,
    Remove(Kind),
    Wrote(u64, Spot, WriteOutcome),
    Loaded(u64, Result<Snapshot, String>),
    Previewed(
        u64,
        Spot,
        Result<(image::Handle, Option<(u32, u32)>), String>,
    ),
    GameLoaded(u64, u64, Result<Option<Game>, String>),
    Change,
    SearchText(String),
    Search,
    Searched(u64, Result<Vec<Game>, String>),
    PickGame(Match),
    SaveGame,
    GameSaved(u64, Game, Result<(), String>),
    CancelChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Showing(pub(crate) Option<String>);

impl std::fmt::Display for Showing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(target) => write!(f, "Target: {target}"),
            None => f.write_str("Environment"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WriteOutcome {
    changed: bool,
    error: Option<String>,
}

impl WriteOutcome {
    fn failed(error: String) -> Self {
        Self {
            changed: false,
            error: Some(error),
        }
    }

    pub(crate) fn changed(&self) -> bool {
        self.changed
    }
}

#[derive(Debug, Clone)]
struct RowData {
    resolved: Option<Resolved>,
    preview: Option<image::Handle>,
    dimensions: Option<(u32, u32)>,
    preview_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    targets: Vec<String>,
    game: Option<u64>,
    rows: BTreeMap<Spot, Option<Resolved>>,
}

#[derive(Debug, Clone)]
struct Change {
    search: String,
    matches: Vec<Match>,
    selected: Option<Match>,
    searching: bool,
    error: Option<String>,
    sequence: u64,
}

pub(crate) struct Step {
    pub(crate) task: Task<Event>,
    pub(crate) changed: bool,
}

impl Step {
    fn none() -> Self {
        Self {
            task: Task::none(),
            changed: false,
        }
    }
    fn task(task: Task<Event>) -> Self {
        Self {
            task,
            changed: false,
        }
    }
}

struct Download {
    spot: Spot,
    token: Token,
    status: String,
}

pub(crate) struct Page {
    pub(crate) name: String,
    pub(crate) generation: u64,
    pub(crate) identity: EnvironmentIdentity,
    showing: Option<String>,
    targets: Vec<String>,
    game_id: Option<u64>,
    game: Option<Game>,
    looked_up: Option<u64>,
    pub(crate) lookup_sequence: u64,
    rows: BTreeMap<Spot, RowData>,
    row_errors: BTreeMap<Spot, String>,
    error: Option<String>,
    pub(crate) loading: bool,
    pub(crate) load_sequence: u64,
    chooser_sequence: u64,
    chooser_active: bool,
    path_kind: Option<Kind>,
    path: String,
    change: Option<Change>,
    pending_find: Option<Kind>,
    picker: Option<art_picker::Picker>,
    picker_sequence: u64,
    thumbnails: Arc<Mutex<art_picker::ThumbnailCache>>,
    download: Option<Download>,
    search_sequence: u64,
    pub(crate) write_sequence: u64,
    pub(crate) writing: bool,
}

impl Page {
    #[cfg(test)]
    pub(crate) fn new(name: String, generation: u64, identity: EnvironmentIdentity) -> Self {
        Self::with_thumbnail_cache(
            name,
            generation,
            identity,
            Arc::new(Mutex::new(art_picker::ThumbnailCache::default())),
        )
    }

    pub(crate) fn with_thumbnail_cache(
        name: String,
        generation: u64,
        identity: EnvironmentIdentity,
        thumbnails: Arc<Mutex<art_picker::ThumbnailCache>>,
    ) -> Self {
        Self {
            name,
            generation,
            identity,
            showing: None,
            targets: Vec::new(),
            game_id: None,
            game: None,
            looked_up: None,
            lookup_sequence: 0,
            rows: BTreeMap::new(),
            row_errors: BTreeMap::new(),
            error: None,
            loading: false,
            load_sequence: 0,
            chooser_sequence: 0,
            chooser_active: false,
            path_kind: None,
            path: String::new(),
            change: None,
            pending_find: None,
            picker: None,
            picker_sequence: 0,
            thumbnails,
            download: None,
            search_sequence: 0,
            write_sequence: 0,
            writing: false,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.writing
    }

    #[cfg(test)]
    pub(crate) fn is_changing(&self) -> bool {
        self.change.is_some()
    }

    pub(crate) fn finish_abandoned_write(&mut self, sequence: u64) {
        if self.write_sequence == sequence {
            self.writing = false;
            self.download = None;
        }
    }

    pub(crate) fn game_label(&self, key_available: bool) -> String {
        match (self.game_id, self.game.as_ref()) {
            (Some(id), Some(game)) if key_available && game.id == id => {
                Match(game.clone()).to_string()
            }
            (Some(id), _) => id.to_string(),
            (None, _) => "Not picked".into(),
        }
    }

    pub(crate) fn leave(&mut self) {
        self.load_sequence = self.load_sequence.wrapping_add(1);
        self.loading = false;
        self.lookup_sequence = self.lookup_sequence.wrapping_add(1);
        self.looked_up = None;
        self.chooser_sequence = self.chooser_sequence.wrapping_add(1);
        self.chooser_active = false;
        self.path_kind = None;
        self.path.clear();
        self.search_sequence = self.search_sequence.wrapping_add(1);
        self.change = None;
        self.pending_find = None;
        self.close_picker();
    }

    pub(crate) fn load(
        &mut self,
        paths: &Paths,
        _decodes: Arc<tokio::sync::Semaphore>,
    ) -> Task<Event> {
        if self.writing {
            return Task::none();
        }
        self.load_sequence = self.load_sequence.wrapping_add(1);
        let sequence = self.load_sequence;
        self.loading = true;
        let paths = paths.clone();
        let name = self.name.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load_snapshot(&paths, &name))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::Loaded(sequence, result),
        )
    }

    fn preview_tasks(
        &mut self,
        paths: &Paths,
        decodes: Arc<tokio::sync::Semaphore>,
    ) -> Task<Event> {
        let mut tasks = Vec::new();
        let sequence = self.load_sequence;
        for kind in Kind::ALL {
            let spot = (self.showing.clone(), kind);
            let Some(row) = self.rows.get(&spot) else {
                continue;
            };
            if row.preview.is_some() || row.preview_error.is_some() {
                continue;
            }
            let Some(resolved) = &row.resolved else {
                continue;
            };
            let path = resolved.found.path.clone();
            let cache = paths.art_cache_dir();
            let decodes = decodes.clone();
            let event_spot = spot.clone();
            tasks.push(Task::perform(
                async move {
                    let permit = decodes
                        .acquire_owned()
                        .await
                        .map_err(|error| error.to_string())?;
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        decode(&path, kind, &cache)
                    })
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
                },
                move |result| Event::Previewed(sequence, event_spot, result),
            ));
        }
        Task::batch(tasks)
    }

    #[cfg(test)]
    pub(crate) fn update(
        &mut self,
        paths: &Paths,
        event: Event,
        decodes: Arc<tokio::sync::Semaphore>,
    ) -> Step {
        self.update_with_key_availability(paths, event, decodes, false)
    }

    pub(crate) fn update_with_key_availability(
        &mut self,
        paths: &Paths,
        event: Event,
        decodes: Arc<tokio::sync::Semaphore>,
        key_available: bool,
    ) -> Step {
        match event {
            Event::Back => {
                self.close_picker();
                self.change = None;
                self.pending_find = None;
                Step::none()
            }
            Event::Find(kind) if !self.writing && key_available => {
                if self.game_id.is_some() {
                    Step::task(self.open_picker(paths, kind))
                } else {
                    self.pending_find = Some(kind);
                    self.update_with_key_availability(paths, Event::Change, decodes, key_available)
                }
            }
            Event::Picker(visit, event)
                if visit == self.picker_sequence && self.picker.is_some() =>
            {
                match event {
                    art_picker::Event::Back => {
                        self.close_picker();
                        Step::none()
                    }
                    art_picker::Event::Use => self.begin_use(paths),
                    art_picker::Event::UseSelected(index) => {
                        self.picker.as_mut().unwrap().select(index);
                        self.begin_use(paths)
                    }
                    art_picker::Event::Cancel => {
                        self.cancel_use();
                        Step::none()
                    }
                    event => {
                        let task = self.picker.as_mut().unwrap().update(
                            paths,
                            self.game_id.unwrap(),
                            event,
                            decodes,
                        );
                        Step::task(task.map(move |event| Event::Picker(visit, event)))
                    }
                }
            }
            Event::CancelUse => {
                self.cancel_use();
                Step::none()
            }
            Event::UseProgress(sequence, received, total)
                if self.writing && sequence == self.write_sequence =>
            {
                if let Some(download) = &mut self.download {
                    download.status = match total {
                        Some(total) => format!(
                            "Downloading {} of {}",
                            crate::size::format_bytes(received),
                            crate::size::format_bytes(total)
                        ),
                        None => format!("Downloading {}", crate::size::format_bytes(received)),
                    };
                }
                Step::none()
            }
            Event::Used(sequence, visit, spot, result)
                if self.writing && sequence == self.write_sequence =>
            {
                self.writing = false;
                self.download = None;
                if let Some(error) = result.error {
                    self.row_errors.insert(spot, error.clone());
                    if visit == self.picker_sequence
                        && let Some(picker) = &mut self.picker
                    {
                        picker.error = Some(error);
                    }
                }
                if result.changed {
                    if visit == self.picker_sequence {
                        self.close_picker();
                    }
                    Step {
                        task: self.load(paths, decodes),
                        changed: true,
                    }
                } else {
                    Step::none()
                }
            }
            Event::Settings => Step::none(),
            Event::Loaded(sequence, result) if sequence == self.load_sequence => {
                self.loading = false;
                match result {
                    Ok(snapshot) => {
                        self.error = None;
                        self.targets = snapshot.targets;
                        if self
                            .showing
                            .as_ref()
                            .is_some_and(|target| !self.targets.contains(target))
                        {
                            self.showing = None;
                        }
                        if self.game_id != snapshot.game {
                            self.close_picker();
                            self.game_id = snapshot.game;
                            self.game = None;
                            self.looked_up = None;
                            self.lookup_sequence = self.lookup_sequence.wrapping_add(1);
                        }
                        self.rows = snapshot
                            .rows
                            .into_iter()
                            .map(|(spot, resolved)| {
                                (
                                    spot,
                                    RowData {
                                        resolved,
                                        preview: None,
                                        dimensions: None,
                                        preview_error: None,
                                    },
                                )
                            })
                            .collect();
                        let preview = self.preview_tasks(paths, decodes);
                        let game = self.lookup_game(paths, key_available);
                        Step::task(Task::batch([preview, game]))
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Step::none()
                    }
                }
            }
            Event::Previewed(sequence, spot, result) if sequence == self.load_sequence => {
                if let Some(row) = self.rows.get_mut(&spot) {
                    match result {
                        Ok((preview, dimensions)) => {
                            row.preview = Some(preview);
                            row.dimensions = dimensions;
                        }
                        Err(error) => row.preview_error = Some(error),
                    }
                }
                Step::none()
            }
            Event::GameLoaded(sequence, id, result)
                if sequence == self.lookup_sequence && self.game_id == Some(id) =>
            {
                match result {
                    Ok(Some(game)) => self.game = Some(game),
                    Err(_) => self.looked_up = None,
                    Ok(None) => {}
                }
                Step::none()
            }
            Event::Showing(showing) => {
                if showing.0.is_none()
                    || showing
                        .0
                        .as_ref()
                        .is_some_and(|target| self.targets.contains(target))
                {
                    self.showing = showing.0;
                    self.chooser_sequence = self.chooser_sequence.wrapping_add(1);
                    self.path_kind = None;
                    Step::task(self.preview_tasks(paths, decodes))
                } else {
                    Step::none()
                }
            }
            Event::Browse(kind) if !self.writing => {
                self.chooser_sequence = self.chooser_sequence.wrapping_add(1);
                self.chooser_active = true;
                let sequence = self.chooser_sequence;
                let spot = (self.showing.clone(), kind);
                let event_spot = spot.clone();
                self.row_errors.remove(&spot);
                Step::task(Task::perform(
                    crate::picker::pick(format!("Select {kind} artwork"), false),
                    move |result| Event::Picked(sequence, event_spot, result),
                ))
            }
            Event::Picked(sequence, spot, result)
                if !self.writing
                    && sequence == self.chooser_sequence
                    && self.chooser_active
                    && spot.0 == self.showing =>
            {
                self.chooser_active = false;
                match result {
                    Ok(Some(path)) => self.begin_manual(paths, spot, path),
                    Ok(None) => Step::none(),
                    Err(error) => {
                        self.row_errors.insert(spot.clone(), error);
                        self.path_kind = Some(spot.1);
                        self.path.clear();
                        Step::none()
                    }
                }
            }
            Event::Path(value) if self.path_kind.is_some() => {
                self.path = value;
                Step::none()
            }
            Event::UsePath if !self.writing => {
                let Some(kind) = self.path_kind else {
                    return Step::none();
                };
                let path = match paths.config_path(&self.path) {
                    Ok(path) => path,
                    Err(error) => {
                        self.row_errors
                            .insert((self.showing.clone(), kind), error.to_string());
                        return Step::none();
                    }
                };
                let spot = (self.showing.clone(), kind);
                self.begin_manual(paths, spot, path)
            }
            Event::CancelPath => {
                self.path_kind = None;
                self.path.clear();
                Step::none()
            }
            Event::Remove(kind) if !self.writing => {
                let spot = (self.showing.clone(), kind);
                if self.rows.get(&spot).is_none_or(|row| {
                    row.resolved.as_ref().is_none_or(|resolved| {
                        self.showing.is_some() && resolved.origin == Origin::Environment
                    })
                }) {
                    return Step::none();
                }
                self.begin_write(paths, spot, None)
            }
            Event::Wrote(sequence, spot, result)
                if self.writing && sequence == self.write_sequence =>
            {
                self.writing = false;
                if let Some(error) = result.error {
                    self.row_errors.insert(spot, error);
                }
                if result.changed {
                    let task = self.load(paths, decodes);
                    Step {
                        task,
                        changed: true,
                    }
                } else {
                    Step::none()
                }
            }
            Event::Change if !self.writing && key_available => {
                self.search_sequence = self.search_sequence.wrapping_add(1);
                self.change = Some(Change {
                    search: self.name.clone(),
                    matches: Vec::new(),
                    selected: None,
                    searching: false,
                    error: None,
                    sequence: self.search_sequence,
                });
                Step::none()
            }
            Event::SearchText(value) if key_available => {
                if let Some(change) = &mut self.change {
                    self.search_sequence = self.search_sequence.wrapping_add(1);
                    change.sequence = self.search_sequence;
                    change.search = value;
                    change.matches.clear();
                    change.selected = None;
                    change.searching = false;
                    change.error = None;
                }
                Step::none()
            }
            Event::Search if key_available => {
                let Some(change) = self.change.as_mut() else {
                    return Step::none();
                };
                if change.searching || change.search.trim().is_empty() {
                    return Step::none();
                }
                change.searching = true;
                change.error = None;
                let term = change.search.clone();
                let sequence = change.sequence;
                let paths = paths.clone();
                Step::task(Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || flow_artwork::search(&paths, &term))
                            .await
                            .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    move |result| Event::Searched(sequence, result),
                ))
            }
            Event::Searched(sequence, result) => {
                if let Some(change) = &mut self.change
                    && change.sequence == sequence
                {
                    change.searching = false;
                    match result {
                        Ok(games) => {
                            change.matches = games.into_iter().map(Match).collect();
                            change.selected = change.matches.first().cloned();
                            if change.matches.is_empty() {
                                change.error = Some("No matches found".into());
                            }
                        }
                        Err(error) => change.error = Some(error),
                    }
                }
                Step::none()
            }
            Event::PickGame(game) if key_available => {
                if let Some(change) = &mut self.change {
                    change.selected = Some(game);
                }
                Step::none()
            }
            Event::SaveGame if !self.writing && key_available => {
                let Some(game) = self
                    .change
                    .as_ref()
                    .and_then(|change| change.selected.as_ref())
                    .map(|game| game.0.clone())
                else {
                    return Step::none();
                };
                self.writing = true;
                self.write_sequence = self.write_sequence.wrapping_add(1);
                self.load_sequence = self.load_sequence.wrapping_add(1);
                let sequence = self.write_sequence;
                let name = self.name.clone();
                let identity = self.identity;
                let paths = paths.clone();
                Step::task(Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            if EnvironmentIdentity::read(&paths, &name) != Some(identity) {
                                return Err(
                                    "environment changed before its game could be saved".into()
                                );
                            }
                            art::save_game(&paths, &name, game.id)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    move |result| Event::GameSaved(sequence, game.clone(), result),
                ))
            }
            Event::GameSaved(sequence, game, result)
                if self.writing && sequence == self.write_sequence =>
            {
                self.writing = false;
                match result {
                    Ok(()) => {
                        self.game_id = Some(game.id);
                        self.looked_up = Some(game.id);
                        self.lookup_sequence = self.lookup_sequence.wrapping_add(1);
                        self.game = Some(game);
                        self.change = None;
                        let load = self.load(paths, decodes);
                        let picker = self
                            .pending_find
                            .take()
                            .map(|kind| self.open_picker(paths, kind))
                            .unwrap_or_else(Task::none);
                        Step {
                            task: Task::batch([load, picker]),
                            changed: true,
                        }
                    }
                    Err(error) => {
                        if let Some(change) = &mut self.change {
                            change.error = Some(error);
                        }
                        Step::none()
                    }
                }
            }
            Event::CancelChange if !self.writing => {
                self.change = None;
                self.pending_find = None;
                Step::none()
            }
            _ => Step::none(),
        }
    }

    pub(crate) fn has_overlay(&self) -> bool {
        self.picker.is_some() || self.change.is_some()
    }

    pub(crate) fn picker_event(&self, event: art_picker::Event) -> Option<Event> {
        self.picker
            .as_ref()
            .map(|_| Event::Picker(self.picker_sequence, event))
    }

    pub(crate) fn use_token(&self) -> Option<Token> {
        self.download
            .as_ref()
            .map(|download| download.token.clone())
    }

    fn cancel_use(&self) {
        if let Some(download) = &self.download {
            download.token.cancel();
        }
    }

    fn close_picker(&mut self) {
        if let Some(picker) = self.picker.take() {
            picker.cancel_thumbnails();
        }
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
    }

    pub(crate) fn cancel_thumbnails(&self) {
        if let Some(picker) = &self.picker {
            picker.cancel_thumbnails();
        }
    }

    fn open_picker(&mut self, paths: &Paths, kind: Kind) -> Task<Event> {
        self.close_picker();
        self.path_kind = None;
        let visit = self.picker_sequence;
        let mut picker = art_picker::Picker::with_cache(kind, 0, self.thumbnails.clone());
        let task = picker.load(paths, self.game_id.unwrap());
        self.picker = Some(picker);
        task.map(move |event| Event::Picker(visit, event))
    }

    fn begin_use(&mut self, paths: &Paths) -> Step {
        if self.writing {
            return Step::none();
        }
        let Some(picker) = &self.picker else {
            return Step::none();
        };
        let Some(image) = picker.selected().cloned() else {
            return Step::none();
        };
        let spot = (self.showing.clone(), picker.kind);
        self.writing = true;
        self.write_sequence = self.write_sequence.wrapping_add(1);
        self.load_sequence = self.load_sequence.wrapping_add(1);
        let sequence = self.write_sequence;
        let visit = self.picker_sequence;
        let token = Token::new();
        self.download = Some(Download {
            spot: spot.clone(),
            token: token.clone(),
            status: "Downloading...".into(),
        });
        self.row_errors.remove(&spot);
        self.picker.as_mut().unwrap().error = None;
        let paths = paths.clone();
        let name = self.name.clone();
        let identity = self.identity;
        let event_spot = spot.clone();
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let result = (|| {
                        let key = steamgriddb::require_key(
                            &paths,
                            |key| std::env::var_os(key),
                            &SecretServiceKeyring::steamgriddb(),
                        )
                        .map_err(|error| error.to_string())?;
                        let https = Https::new().map_err(|error| error.to_string())?;
                        Ok::<_, String>(use_image_for(
                            &paths,
                            &name,
                            identity,
                            &spot,
                            &Api::new(&https, &key.value),
                            &image,
                            |received, total| {
                                let _ = sender
                                    .unbounded_send(Event::UseProgress(sequence, received, total));
                            },
                            &token,
                        ))
                    })();
                    result.unwrap_or_else(WriteOutcome::failed)
                })
                .await
                .unwrap_or_else(|error| WriteOutcome::failed(error.to_string()))
            },
            move |result| Event::Used(sequence, visit, event_spot, result),
        );
        Step::task(Task::batch([Task::run(receiver, |event| event), work]))
    }

    pub(crate) fn lookup_game(&mut self, paths: &Paths, key_available: bool) -> Task<Event> {
        let Some(id) = self.game_id.filter(|_| key_available) else {
            return Task::none();
        };
        if self.looked_up == Some(id) {
            return Task::none();
        }
        self.looked_up = Some(id);
        self.lookup_sequence = self.lookup_sequence.wrapping_add(1);
        let sequence = self.lookup_sequence;
        let paths = paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || lookup_game(&paths, id))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::GameLoaded(sequence, id, result),
        )
    }

    fn begin_manual(&mut self, paths: &Paths, spot: Spot, path: PathBuf) -> Step {
        self.path_kind = None;
        self.path.clear();
        self.begin_write(paths, spot, Some(path))
    }

    fn begin_write(&mut self, paths: &Paths, spot: Spot, path: Option<PathBuf>) -> Step {
        if self.writing {
            return Step::none();
        }
        self.writing = true;
        self.write_sequence = self.write_sequence.wrapping_add(1);
        self.load_sequence = self.load_sequence.wrapping_add(1);
        let sequence = self.write_sequence;
        self.row_errors.remove(&spot);
        let event_spot = spot.clone();
        let paths = paths.clone();
        let name = self.name.clone();
        let identity = self.identity;
        Step::task(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    write_art_for(&paths, &name, identity, &spot, path.as_deref())
                })
                .await
                .unwrap_or_else(|error| WriteOutcome::failed(error.to_string()))
            },
            move |result| Event::Wrote(sequence, event_spot, result),
        ))
    }

    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        key_available: bool,
    ) -> Element<'_, Event> {
        if let Some(picker) = &self.picker {
            let visit = self.picker_sequence;
            return picker
                .view(
                    palette,
                    &format!("{} art for {}", picker.kind, self.name),
                    self.download
                        .as_ref()
                        .map(|download| download.status.as_str()),
                )
                .map(move |event| Event::Picker(visit, event));
        }
        let mut header = row![
            secondary_button("< Back", palette).on_press(Event::Back),
            text(format!("{} art", self.name)).size(20),
        ]
        .spacing(16)
        .align_y(Center);
        header = header.push(iced::widget::Space::new().width(Fill));
        header = header.push(text("SteamGridDB game").size(12).color(palette.muted));
        header = header.push(text(self.game_label(key_available)).size(14));
        header = header.push(
            secondary_button("Change", palette)
                .on_press_maybe((key_available && !self.writing).then_some(Event::Change)),
        );
        let mut body = column![header].spacing(14);
        if !key_available {
            body = body.push(
                row![
                    text("Add a SteamGridDB key in Settings to find artwork or change the game.")
                        .size(12)
                        .color(palette.muted),
                    secondary_button("Settings", palette).on_press(Event::Settings),
                ]
                .spacing(10)
                .align_y(Center),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        if self.loading && self.rows.is_empty() {
            body = body.push(text("Loading art...").color(palette.muted));
        }
        if key_available && let Some(change) = &self.change {
            body = body.push(self.change_view(change, palette));
        } else {
            body = body.push(self.showing_view(palette));
            for (kind, title, location, width, height) in KIND_ROWS {
                body = body.push(self.row_view(
                    (kind, title, location, width, height),
                    palette,
                    key_available,
                ));
            }
            if let Some(kind) = self.path_kind {
                body = body.push(self.path_view(kind, palette));
            }
        }
        let scroll = scrollable(
            container(body)
                .max_width(PAGE_WIDTH)
                .width(Fill)
                .padding(24)
                .center_x(Fill),
        )
        .wineo_style(palette)
        .height(Fill);
        container(wheel::scale(scroll, crate::WHEEL_STEP_SCALE))
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background))
            .into()
    }

    fn showing_view(&self, palette: Palette) -> Element<'_, Event> {
        let choices: Vec<Showing> = std::iter::once(Showing(None))
            .chain(
                self.targets
                    .iter()
                    .cloned()
                    .map(|target| Showing(Some(target))),
            )
            .collect();
        let mut controls = row![text("Showing").size(13).color(palette.muted)]
            .spacing(8)
            .align_y(Center);
        if choices.len() <= 4 {
            for choice in choices {
                let chosen = self.showing == choice.0;
                let label = choice.to_string();
                controls = controls.push(
                    button(text(label).size(13))
                        .on_press(Event::Showing(choice))
                        .style(move |_, status| {
                            if chosen {
                                button::primary(&palette.theme(), status)
                            } else {
                                secondary(palette, status)
                            }
                        }),
                );
            }
        } else {
            controls = controls.push(
                pick_list(choices, Some(Showing(self.showing.clone())), Event::Showing)
                    .wineo_style(palette),
            );
        }
        controls.into()
    }

    fn row_view(
        &self,
        row: (Kind, &'static str, &'static str, u32, u32),
        palette: Palette,
        key_available: bool,
    ) -> Element<'_, Event> {
        let (kind, title, location, width, height) = row;
        let spot = (self.showing.clone(), kind);
        let row_data = self.rows.get(&spot);
        let inherited = self.inherited(kind);
        let has_override = self.has_override(kind);
        let preview: Element<'_, Event> =
            if let Some(handle) = row_data.and_then(|row| row.preview.as_ref()) {
                image(handle.clone())
                    .width(width)
                    .height(height)
                    .content_fit(ContentFit::Contain)
                    .opacity(if inherited { 0.45_f32 } else { 1.0_f32 })
                    .into()
            } else {
                container(
                    text(
                        if row_data.and_then(|row| row.resolved.as_ref()).is_some() {
                            "Loading..."
                        } else {
                            "No art"
                        },
                    )
                    .size(12)
                    .color(palette.muted),
                )
                .width(width)
                .height(height)
                .center(Fill)
                .into()
            };
        let preview = container(if kind == Kind::Logo {
            stack![checker(width, height, palette), preview].into()
        } else {
            preview
        })
        .width(width)
        .height(height);
        let mut details = column![
            text(title)
                .size(17)
                .color(
                    palette
                        .foreground
                        .scale_alpha(if inherited { 0.55 } else { 1.0 })
                ),
            text(row_data.map_or_else(|| "None".to_owned(), source_label))
                .size(13)
                .color(palette.muted),
            text(location).size(12).color(palette.muted),
        ]
        .spacing(5);
        if inherited {
            details = details.push(text("From the environment").size(12).color(palette.muted));
        }
        if let Some(error) = self
            .row_errors
            .get(&spot)
            .or_else(|| row_data.and_then(|row| row.preview_error.as_ref()))
        {
            details = details.push(text(error).size(12).color(palette.danger));
        }
        if let Some(download) = &self.download
            && download.spot == spot
        {
            details = details.push(
                row![
                    text(&download.status).size(12),
                    secondary_button("Cancel", palette).on_press(Event::CancelUse),
                ]
                .spacing(8),
            );
        }
        let actions = column![
            secondary_button("Find...", palette)
                .on_press_maybe((key_available && !self.writing).then_some(Event::Find(kind))),
            secondary_button("Browse...", palette)
                .on_press_maybe((!self.writing).then_some(Event::Browse(kind))),
            secondary_button(
                if self.showing.is_some() && has_override {
                    "Use environment art"
                } else {
                    "Remove"
                },
                palette
            )
            .on_press_maybe((!self.writing && has_override).then_some(Event::Remove(kind))),
        ]
        .spacing(5);
        container(
            row![preview, details.width(Fill), actions]
                .spacing(16)
                .align_y(Center),
        )
        .padding(14)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(
                    palette
                        .surface
                        .scale_alpha(if inherited { 0.65 } else { 1.0 }),
                )
                .border(border::rounded(8).width(1).color(palette.border))
        })
        .into()
    }

    fn inherited(&self, kind: Kind) -> bool {
        self.showing.is_some()
            && self
                .rows
                .get(&(self.showing.clone(), kind))
                .and_then(|row| row.resolved.as_ref())
                .is_some_and(|resolved| resolved.origin == Origin::Environment)
    }

    pub(crate) fn has_override(&self, kind: Kind) -> bool {
        self.rows
            .get(&(self.showing.clone(), kind))
            .and_then(|row| row.resolved.as_ref())
            .is_some_and(|resolved| self.showing.is_none() || resolved.origin == Origin::Target)
    }

    #[cfg(test)]
    pub(crate) fn has_preview(&self, spot: &Spot) -> bool {
        self.rows.get(spot).is_some_and(|row| row.preview.is_some())
    }

    #[cfg(test)]
    pub(crate) fn has_target(&self, target: &str) -> bool {
        self.targets.iter().any(|existing| existing == target)
    }

    fn path_view(&self, kind: Kind, palette: Palette) -> Element<'_, Event> {
        container(
            column![
                text(format!("Enter a path for {kind} artwork")).size(13),
                row![
                    text_input("File path", &self.path)
                        .on_input(Event::Path)
                        .on_submit(Event::UsePath)
                        .width(Fill),
                    accent_base("Use file", palette.primary).on_press_maybe(
                        (!self.path.is_empty() && !self.writing).then_some(Event::UsePath)
                    ),
                    secondary_button("Cancel", palette).on_press(Event::CancelPath),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(Fill)
        .into()
    }

    fn change_view<'a>(&self, change: &'a Change, palette: Palette) -> Element<'a, Event> {
        let mut content = column![
            row![
                text("Choose a SteamGridDB game").size(17),
                secondary_button("Back", palette)
                    .on_press_maybe((!self.writing).then_some(Event::CancelChange)),
            ]
            .spacing(12)
            .align_y(Center),
            row![
                text_input("Search games", &change.search)
                    .on_input(Event::SearchText)
                    .on_submit(Event::Search)
                    .width(Fill),
                secondary_button("Search", palette).on_press_maybe(
                    (!change.searching && !change.search.trim().is_empty())
                        .then_some(Event::Search)
                ),
            ]
            .spacing(8),
        ]
        .spacing(10);
        if change.searching {
            content = content.push(text("Searching...").color(palette.muted));
        }
        if !change.matches.is_empty() {
            content = content.push(text("Game matches").size(13).color(palette.muted));
            content = content.push(
                pick_list(
                    change.matches.clone(),
                    change.selected.clone(),
                    Event::PickGame,
                )
                .wineo_style(palette),
            );
        }
        if let Some(error) = &change.error {
            content = content.push(text(error).color(palette.danger));
        }
        content = content.push(accent_base("Use game", palette.primary).on_press_maybe(
            (change.selected.is_some() && !self.writing).then_some(Event::SaveGame),
        ));
        container(content)
            .padding(14)
            .width(Fill)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            })
            .into()
    }
}

fn source_label(row: &RowData) -> String {
    let source = match row.resolved.as_ref() {
        Some(Resolved {
            source: Some(Source::SteamGridDb(id)),
            ..
        }) => format!("SteamGridDB {id}"),
        Some(Resolved { .. }) => "Manual".to_owned(),
        None => "None".to_owned(),
    };
    match row.dimensions {
        Some((width, height)) => format!("{source}, {width}x{height}"),
        None => source,
    }
}

fn checker<'a>(width: u32, height: u32, palette: Palette) -> Element<'a, Event> {
    let mut rows = column![];
    let cell_width = width / 10;
    let cell_height = height / 4;
    for y in 0..4 {
        let mut cells = row![];
        for x in 0..10 {
            let color = if (x + y) % 2 == 0 {
                palette.border
            } else {
                palette.surface
            };
            cells = cells.push(
                container(iced::widget::Space::new())
                    .width(cell_width)
                    .height(cell_height)
                    .style(move |_| container::Style::default().background(color)),
            );
        }
        rows = rows.push(cells);
    }
    rows.into()
}

pub(crate) fn load_snapshot(paths: &Paths, name: &str) -> Result<Snapshot, String> {
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    let config = art::ArtConfig::load(&paths.art_dir(name)).map_err(|error| error.to_string())?;
    let mut rows = BTreeMap::new();
    let targets: Vec<_> = environment.config.targets.keys().cloned().collect();
    for target in std::iter::once(None).chain(targets.iter().map(|target| Some(target.as_str()))) {
        for kind in Kind::ALL {
            let resolved = art::resolve(&paths.art_dir(name), &config, target, kind)
                .map_err(|error| error.to_string())?;
            rows.insert((target.map(str::to_owned), kind), resolved);
        }
    }
    Ok(Snapshot {
        targets,
        game: config.game,
        rows,
    })
}

fn decode(
    path: &Path,
    kind: Kind,
    cache: &Path,
) -> Result<(image::Handle, Option<(u32, u32)>), String> {
    let dimensions = if path.extension().is_some_and(|extension| extension == "svg") {
        fs::read(path)
            .ok()
            .and_then(|bytes| {
                resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()).ok()
            })
            .map(|tree| tree.size().to_int_size())
            .map(|size| (size.width(), size.height()))
    } else {
        ::image::image_dimensions(path).ok()
    };
    let kind = match kind {
        Kind::Icon => artwork::Kind::Logo,
        Kind::Grid => artwork::Kind::Grid,
        Kind::Hero => artwork::Kind::Hero,
        Kind::Logo => artwork::Kind::Logo,
    };
    let preview = artwork::decode(path, kind, Some(cache)).map_err(|error| error.to_string())?;
    Ok((preview, dimensions))
}

fn lookup_game(paths: &Paths, id: u64) -> Result<Option<Game>, String> {
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|error| error.to_string())?;
    let https = Https::new().map_err(|error| error.to_string())?;
    Api::new(&https, &key.value)
        .game(id)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn canceled_use_outcome() -> WriteOutcome {
    WriteOutcome::failed("download canceled".into())
}

#[cfg(test)]
pub(crate) fn write_art(
    paths: &Paths,
    name: &str,
    spot: &Spot,
    path: Option<&Path>,
) -> WriteOutcome {
    let Some(identity) = EnvironmentIdentity::read(paths, name) else {
        return WriteOutcome::failed("environment no longer exists".into());
    };
    write_art_for(paths, name, identity, spot, path)
}

fn write_art_for(
    paths: &Paths,
    name: &str,
    identity: EnvironmentIdentity,
    spot: &Spot,
    path: Option<&Path>,
) -> WriteOutcome {
    if EnvironmentIdentity::read(paths, name) != Some(identity) {
        return WriteOutcome::failed("environment changed before artwork could be written".into());
    }
    let result = match path {
        Some(path) => flow_artwork::preview_with_format(path, spot.1).and_then(|_| {
            if EnvironmentIdentity::read(paths, name) != Some(identity) {
                return Err("environment changed before artwork could be written".into());
            }
            art::set_manual(paths, name, spot.0.as_deref(), spot.1, path, Some(identity))
                .map(|_| ())
                .map_err(|error| error.to_string())
        }),
        None => {
            if EnvironmentIdentity::read(paths, name) != Some(identity) {
                return WriteOutcome::failed(
                    "environment changed before artwork could be removed".into(),
                );
            }
            art::remove(paths, name, spot.0.as_deref(), spot.1)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    };
    if let Err(error) = result {
        return WriteOutcome::failed(error);
    }
    let error = if spot.1 == Kind::Icon {
        if EnvironmentIdentity::read(paths, name) == Some(identity) {
            reconcile_desktop(paths, name).err()
        } else {
            Some("environment changed before desktop entries could be reconciled".into())
        }
    } else {
        None
    };
    WriteOutcome {
        changed: true,
        error,
    }
}

#[allow(clippy::too_many_arguments)]
fn use_image_for(
    paths: &Paths,
    name: &str,
    identity: EnvironmentIdentity,
    spot: &Spot,
    api: &Api,
    image: &steamgriddb::Image,
    mut progress: impl FnMut(u64, Option<u64>),
    token: &Token,
) -> WriteOutcome {
    if EnvironmentIdentity::read(paths, name) != Some(identity) {
        return WriteOutcome::failed("environment changed before artwork could be written".into());
    }
    let result = art_fetch::use_image(
        paths,
        name,
        spot.0.as_deref(),
        spot.1,
        api,
        image,
        |received, total| {
            if EnvironmentIdentity::read(paths, name) != Some(identity) {
                token.cancel();
            }
            progress(received, total);
        },
        token,
        Some(identity),
    );
    if let Err(error) = result {
        return WriteOutcome::failed(error.to_string());
    }
    let error = if spot.1 == Kind::Icon && EnvironmentIdentity::read(paths, name) == Some(identity)
    {
        reconcile_desktop(paths, name).err()
    } else {
        None
    };
    WriteOutcome {
        changed: true,
        error,
    }
}

fn reconcile_desktop(paths: &Paths, name: &str) -> Result<(), String> {
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    let global = GlobalConfig::load_or_default(paths).map_err(|error| error.to_string())?;
    match &environment.config.kind {
        config::Kind::Native => desktop::reconcile_native(paths, &global, &environment),
        // Proxied environments get no entries yet.
        config::Kind::Proxied(_) => Ok(Vec::new()),
        config::Kind::Wine(wine) => {
            let runner = Runners::new(paths, &global)
                .and_then(|runners| runners.resolve(&wine.runner))
                .map_err(|error| error.to_string())?;
            let prefix = environment
                .prefix(paths)
                .map_err(|error| error.to_string())?;
            desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)
        }
    }
    .map(|_| ())
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
