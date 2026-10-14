use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::{Outcome, Scrollable, scrollable as scrollable_operation};
use iced::widget::{Row, column, container, image, mouse_area, row, scrollable, text, toggler};
use iced::{Center, Color, ContentFit, Element, Fill, Length, Task, border, mouse};
use wineo_core::art::Kind;
use wineo_core::cancel::Token;
use wineo_core::download::{Https, Transport};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Filters, Image};

use crate::artwork;
use crate::ends::{MIN_LEFT, ends};
use crate::keys::{self, Direction};
use crate::theme::Palette;
use crate::ui::{ScrollStyle, accent_base, secondary_button};

const PAGE_SIZE: usize = 18;
const PAGE_WIDTH: f32 = 1120.0;
const THUMBNAIL_CACHE_BYTES: usize = 64 * 1024 * 1024;

type ThumbnailKey = (Kind, String);
type SharedTransport = Arc<dyn Transport + Send + Sync>;

/// One HTTPS client for everything a picker fetches. Building one starts a
/// thread and sets up TLS, which is too much to repeat per thumbnail. It is
/// built on first use, off the UI thread, and a failed build is retried.
struct Client {
    build: Box<dyn Fn() -> Result<SharedTransport, String> + Send + Sync>,
    built: Mutex<Option<SharedTransport>>,
}

impl Client {
    fn new() -> Self {
        Self::with_builder(|| {
            let https = Https::new().map_err(|error| error.to_string())?;
            Ok(Arc::new(https))
        })
    }

    fn with_builder(
        build: impl Fn() -> Result<SharedTransport, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            build: Box::new(build),
            built: Mutex::new(None),
        }
    }

    fn get(&self) -> Result<SharedTransport, String> {
        let mut built = self.built.lock().unwrap();
        if let Some(transport) = &*built {
            return Ok(transport.clone());
        }
        let transport = (self.build)()?;
        *built = Some(transport.clone());
        Ok(transport)
    }
}

struct CachedThumbnail {
    handle: image::Handle,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
pub(crate) struct ThumbnailCache {
    entries: HashMap<ThumbnailKey, CachedThumbnail>,
    bytes: usize,
    clock: u64,
}

impl ThumbnailCache {
    fn get(&mut self, kind: Kind, url: &str) -> Option<image::Handle> {
        let entry = self.entries.get_mut(&(kind, url.to_owned()))?;
        self.clock = self.clock.wrapping_add(1);
        entry.used = self.clock;
        Some(entry.handle.clone())
    }

    pub(crate) fn contains(&self, kind: Kind, url: &str) -> bool {
        self.entries.contains_key(&(kind, url.to_owned()))
    }

    pub(crate) fn insert(&mut self, kind: Kind, url: String, handle: image::Handle) {
        let bytes = match &handle {
            image::Handle::Rgba { pixels, .. } => pixels.len(),
            _ => return,
        };
        if bytes > THUMBNAIL_CACHE_BYTES {
            return;
        }
        if let Some(old) = self.entries.remove(&(kind, url.clone())) {
            self.bytes -= old.bytes;
        }
        while self.bytes + bytes > THUMBNAIL_CACHE_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
                .expect("cache has entries while over budget");
            self.bytes -= self.entries.remove(&oldest).unwrap().bytes;
        }
        self.clock = self.clock.wrapping_add(1);
        self.bytes += bytes;
        self.entries.insert(
            (kind, url),
            CachedThumbnail {
                handle,
                bytes,
                used: self.clock,
            },
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    Animated,
    Nsfw,
    Humor,
    OtherSizes,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FilterState {
    animated: bool,
    nsfw: bool,
    humor: bool,
    other_sizes: bool,
}

impl FilterState {
    fn any(self, kind: Kind) -> bool {
        self.animated || self.nsfw || self.humor || (kind == Kind::Grid && self.other_sizes)
    }

    fn set(&mut self, filter: Filter, value: bool) {
        match filter {
            Filter::Animated => self.animated = value,
            Filter::Nsfw => self.nsfw = value,
            Filter::Humor => self.humor = value,
            Filter::OtherSizes => self.other_sizes = value,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    ToggleFilters,
    Filter(Filter, bool),
    Listed(u64, Result<Vec<Image>, String>),
    LoadMore,
    Thumbnail(u64, String, Option<Result<image::Handle, String>>),
    Select(usize),
    Use,
    UseSelected(usize),
    Cancel,
    Move(Direction),
}

pub(crate) struct Picker {
    pub(crate) kind: Kind,
    list_sequence: u64,
    images: Vec<Image>,
    filtered_indices: Vec<usize>,
    visible_count: usize,
    selected: Option<usize>,
    loading: bool,
    filters_open: bool,
    filters: FilterState,
    pending_thumbnails: HashSet<String>,
    failed_thumbnails: HashSet<String>,
    thumbnails: Arc<Mutex<ThumbnailCache>>,
    thumbnail_token: Token,
    client: Arc<Client>,
    pub(crate) error: Option<String>,
    scroll_id: iced::widget::Id,
}

impl Picker {
    #[cfg(test)]
    pub(crate) fn new(kind: Kind, sequence: u64) -> Self {
        Self::with_cache(
            kind,
            sequence,
            Arc::new(Mutex::new(ThumbnailCache::default())),
        )
    }

    pub(crate) fn with_cache(
        kind: Kind,
        sequence: u64,
        thumbnails: Arc<Mutex<ThumbnailCache>>,
    ) -> Self {
        Self {
            kind,
            list_sequence: sequence,
            images: Vec::new(),
            filtered_indices: Vec::new(),
            visible_count: 0,
            selected: None,
            loading: false,
            filters_open: false,
            filters: FilterState::default(),
            pending_thumbnails: HashSet::new(),
            failed_thumbnails: HashSet::new(),
            thumbnails,
            thumbnail_token: Token::new(),
            client: Arc::new(Client::new()),
            error: None,
            scroll_id: iced::widget::Id::unique(),
        }
    }

    pub(crate) fn load(&mut self, paths: &Paths, game: u64) -> Task<Event> {
        self.start_listing(paths, game)
    }

    pub(crate) fn cancel_thumbnails(&self) {
        self.thumbnail_token.cancel();
    }

    #[cfg(test)]
    pub(crate) fn thumbnail_token(&self) -> Token {
        self.thumbnail_token.clone()
    }

    pub(crate) fn update(
        &mut self,
        paths: &Paths,
        game: u64,
        event: Event,
        decodes: Arc<tokio::sync::Semaphore>,
    ) -> Task<Event> {
        match event {
            Event::Back => {
                self.cancel_thumbnails();
                Task::none()
            }
            Event::Use | Event::UseSelected(_) | Event::Cancel => Task::none(),
            Event::ToggleFilters => {
                self.filters_open = !self.filters_open;
                Task::none()
            }
            Event::Filter(filter, value) => {
                if filter == Filter::OtherSizes && self.kind != Kind::Grid {
                    return Task::none();
                }
                if filter_value(self.filters, filter) == value {
                    return Task::none();
                }
                self.filters.set(filter, value);
                self.start_listing(paths, game)
            }
            Event::Listed(sequence, result) if sequence == self.list_sequence => {
                self.loading = false;
                self.images.clear();
                self.filtered_indices.clear();
                self.visible_count = 0;
                self.selected = None;
                match result {
                    Ok(images) => {
                        self.error = None;
                        self.images = images;
                        self.rebuild_indices();
                        self.visible_count = self.filtered_indices.len().min(page_size(self.kind));
                        self.thumbnail_tasks(0, self.visible_count, decodes)
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Listed(_, _) => Task::none(),
            Event::LoadMore => {
                if self.loading {
                    return Task::none();
                }
                let old_count = self.visible_count;
                self.visible_count =
                    (old_count + page_size(self.kind)).min(self.filtered_indices.len());
                self.thumbnail_tasks(old_count, self.visible_count, decodes)
            }
            Event::Thumbnail(sequence, url, result) => {
                if sequence != self.list_sequence || self.thumbnail_token.is_canceled() {
                    return Task::none();
                }
                self.pending_thumbnails.remove(&url);
                match result {
                    Some(Ok(handle)) => self
                        .thumbnails
                        .lock()
                        .unwrap()
                        .insert(self.kind, url, handle),
                    Some(Err(_)) => {
                        self.failed_thumbnails.insert(url);
                    }
                    None => {}
                }
                Task::none()
            }
            Event::Select(index) => {
                if index < self.visible_count.min(self.filtered_indices.len()) {
                    self.selected = Some(index);
                    self.error = None;
                }
                Task::none()
            }
            Event::Move(direction) => {
                let count = self.filtered_indices.len();
                let columns = columns(self.kind);
                let page_size = page_size(self.kind);
                let Some(index) = keys::move_index(self.selected, count, columns, direction) else {
                    return Task::none();
                };
                self.selected = Some(index);
                let old_count = self.visible_count;
                let next_count = ((index / page_size) + 1) * page_size;
                self.visible_count = self.visible_count.max(next_count.min(count));
                let thumbnails = self.thumbnail_tasks(old_count, self.visible_count, decodes);
                // The rows above and the filters differ in height, so the scroll
                // target is measured from the laid out page.
                Task::batch([
                    thumbnails,
                    iced::advanced::widget::operate(self.reveal(index)),
                ])
            }
        }
    }

    /// The scroll that brings the row of tile `index` into view.
    fn reveal(&self, index: usize) -> Reveal {
        Reveal::new(self.scroll_id.clone(), index / columns(self.kind))
    }

    pub(crate) fn selected(&self) -> Option<&Image> {
        let index = self.selected?;
        let image_index = self.filtered_indices.get(index).copied()?;
        self.images.get(image_index)
    }

    #[cfg(test)]
    pub(crate) fn list_sequence(&self) -> u64 {
        self.list_sequence
    }

    pub(crate) fn select(&mut self, index: usize) {
        if index < self.filtered_indices.len() {
            self.selected = Some(index);
            self.error = None;
        }
    }

    pub(crate) fn details(&self) -> Option<String> {
        let image = self.selected()?;
        let author = image.author.as_deref().unwrap_or("Unknown author");
        let dimensions = match (image.width, image.height) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => "Unknown size".to_owned(),
        };
        let style = image.style.as_deref().unwrap_or("Unknown style");
        let score = image
            .score
            .map_or_else(|| "Unknown score".to_owned(), |score| score.to_string());
        Some(format!(
            "SteamGridDB #{} by {author} - {dimensions} - {style} - score {score}",
            image.id
        ))
    }

    pub(crate) fn view(
        &self,
        palette: Palette,
        title: &str,
        progress: Option<&str>,
    ) -> Element<'_, Event> {
        // Back and Filters stay put. The title, filters, notices, details, and
        // grid share one scrollable, so a short window still reaches the
        // images, and the footer keeps only the progress line and Use.
        let header = ends(
            secondary_button("Back", palette).on_press(Event::Back),
            secondary_button(
                if self.filters_open {
                    "Filters ^"
                } else {
                    "Filters v"
                },
                palette,
            )
            .on_press(Event::ToggleFilters),
            14.0,
            0.0,
        );

        let mut scrolled = column![text(title.to_owned()).size(20)]
            .spacing(12)
            .width(Fill);
        if self.filters_open {
            scrolled = scrolled.push(self.filters_view(palette));
        }
        if self.loading && self.images.is_empty() {
            scrolled = scrolled.push(text("Loading SteamGridDB images...").color(palette.muted));
        }
        if let Some(error) = &self.error
            && self.selected().is_none()
        {
            scrolled = scrolled.push(text(error).size(13).color(palette.danger));
        }
        if !self.loading && self.filtered_indices.is_empty() && self.error.is_none() {
            scrolled = scrolled.push(text("No images found.").color(palette.muted));
        }
        let selected = self.details();
        let mut details = column![
            text(selected.unwrap_or_else(|| "Select an image to see its details.".to_owned()))
                .size(13)
                .color(palette.foreground),
        ]
        .spacing(8);
        if let Some(error) = &self.error
            && self.selected().is_some()
        {
            details = details.push(text(error).size(12).color(palette.danger));
        }
        scrolled = scrolled.push(details);
        scrolled = scrolled.push(self.grid_view(palette));
        let scroll = scrollable(scrolled)
            .wineo_style(palette)
            .id(self.scroll_id.clone())
            .width(Fill)
            .height(Fill);
        let has_progress = progress.is_some();
        let use_button = accent_base("Use", palette.primary).on_press_maybe(
            (!has_progress)
                .then(|| self.selected().map(|_| Event::Use))
                .flatten(),
        );
        let status: Element<'_, Event> = match progress {
            Some(status) => row![
                text(status.to_owned()).size(12).color(palette.muted),
                secondary_button("Cancel", palette).on_press(Event::Cancel),
            ]
            .spacing(10)
            .align_y(Center)
            .wrap()
            .vertical_spacing(8)
            .into(),
            None => iced::widget::Space::new().into(),
        };
        let footer = ends(status, use_button, 14.0, MIN_LEFT);
        let body = column![header, scroll, footer].spacing(12);

        container(
            container(body)
                .max_width(PAGE_WIDTH)
                .width(Fill)
                .height(Fill)
                .padding(24)
                .center_x(Fill),
        )
        .width(Fill)
        .height(Fill)
        .style(move |_| container::Style::default().background(palette.background))
        .into()
    }

    fn start_listing(&mut self, paths: &Paths, game: u64) -> Task<Event> {
        self.thumbnail_token.cancel();
        self.thumbnail_token = Token::new();
        self.list_sequence = self.list_sequence.wrapping_add(1);
        self.pending_thumbnails.clear();
        self.failed_thumbnails.clear();
        self.loading = true;
        self.error = None;
        self.images.clear();
        self.filtered_indices.clear();
        self.visible_count = 0;
        self.selected = None;
        let sequence = self.list_sequence;
        let kind = self.kind;
        let filters = request_filters(self.filters, kind);
        let paths = paths.clone();
        let client = self.client.clone();
        let listing = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    list_images(&paths, &client, game, kind, filters)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::Listed(sequence, result),
        );
        Task::batch([
            listing,
            iced::widget::operation::scroll_to(
                self.scroll_id.clone(),
                scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
            ),
        ])
    }

    fn filters_view(&self, palette: Palette) -> Element<'_, Event> {
        let switch = |label: &'static str, enabled: bool, filter: Filter| {
            toggler(enabled)
                .label(label)
                .on_toggle(move |value| Event::Filter(filter, value))
                .text_size(13)
        };
        let mut filters = row![
            switch("Animated", self.filters.animated, Filter::Animated),
            switch("NSFW", self.filters.nsfw, Filter::Nsfw),
            switch("Humor", self.filters.humor, Filter::Humor),
        ]
        .spacing(22)
        .align_y(Center);
        if self.kind == Kind::Grid {
            filters = filters.push(switch(
                "Other sizes",
                self.filters.other_sizes,
                Filter::OtherSizes,
            ));
        }
        container(filters.wrap().vertical_spacing(8))
            .padding(10)
            .width(Fill)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(6).width(1).color(palette.border))
            })
            .into()
    }

    fn grid_view(&self, palette: Palette) -> Element<'_, Event> {
        let indices = &self.filtered_indices;
        let visible = &indices[..self.visible_count.min(indices.len())];
        let columns = columns(self.kind);
        let mut grid = column![].spacing(10).width(Fill);
        for (row_index, chunk) in visible.chunks(columns).enumerate() {
            let tiles = chunk.iter().enumerate().map(|(column_index, image_index)| {
                let index = row_index * columns + column_index;
                self.tile(
                    index,
                    *image_index,
                    is_best_match(*image_index, indices),
                    palette,
                )
            });
            let mut tiles = Row::with_children(tiles)
                .spacing(10)
                .align_y(Center)
                .width(Fill);
            for _ in chunk.len()..columns {
                tiles = tiles.push(iced::widget::Space::new().width(Length::FillPortion(1)));
            }
            // Rows differ in height with their badges and Selected label, so
            // keyboard scrolling measures them by id.
            grid = grid.push(container(tiles).id(row_id(row_index)).width(Fill));
        }
        if self.visible_count < indices.len() {
            grid = grid.push(
                row![iced::widget::Space::new().width(Fill),]
                    .push(secondary_button("Load more", palette).on_press(Event::LoadMore))
                    .push(iced::widget::Space::new().width(Fill)),
            );
        }
        container(grid).width(Fill).padding([0, 2]).into()
    }

    fn tile(
        &self,
        index: usize,
        image_index: usize,
        first: bool,
        palette: Palette,
    ) -> Element<'_, Event> {
        let item = &self.images[image_index];
        let selected = self.selected == Some(index);
        let (_, height) = tile_size(self.kind);
        let url = item
            .thumb
            .as_deref()
            .filter(|url| !url.is_empty())
            .unwrap_or(&item.url);
        let cached = self.thumbnails.lock().unwrap().get(self.kind, url);
        let preview: Element<'_, Event> = match cached {
            Some(handle) => image(handle)
                .width(Fill)
                .height(Fill)
                .content_fit(ContentFit::Contain)
                .into(),
            None if self.failed_thumbnails.contains(url) => {
                container(text("Preview unavailable").size(11).color(palette.muted))
                    .center(Fill)
                    .into()
            }
            None => container(
                text(if self.pending_thumbnails.contains(url) {
                    "Loading preview..."
                } else {
                    "No preview"
                })
                .size(11)
                .color(palette.muted),
            )
            .center(Fill)
            .into(),
        };
        let preview = container(preview)
            .width(Fill)
            .height(height)
            .padding(4)
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(5).width(2).color(if selected {
                        palette.primary
                    } else {
                        palette.border
                    }))
            });
        let mut tags: Vec<Element<'_, Event>> = Vec::new();
        if first {
            tags.push(badge("Best match", palette.primary, palette));
        }
        if item.animated {
            tags.push(badge("Animated", palette.muted, palette));
        }
        if item.nsfw {
            tags.push(badge("NSFW", palette.warning, palette));
        }
        if item.humor {
            tags.push(badge("Humor", palette.muted, palette));
        }
        if self.kind == Kind::Grid && is_other_grid_size(item) {
            tags.push(badge("Other size", palette.warning, palette));
        }
        let tags = Row::with_children(tags).spacing(4).wrap();
        let mut content = column![preview, tags]
            .spacing(5)
            .width(Length::FillPortion(1));
        if selected {
            content = content.push(text("Selected").size(10).color(palette.primary));
        }
        mouse_area(content)
            .on_press(Event::Select(index))
            .on_double_click(Event::UseSelected(index))
            .interaction(mouse::Interaction::Pointer)
            .into()
    }

    #[cfg(test)]
    fn images_for_display(&self) -> &[usize] {
        &self.filtered_indices
    }

    fn rebuild_indices(&mut self) {
        self.filtered_indices = filtered_indices(self.kind, &self.images, self.filters);
    }

    fn thumbnail_tasks(
        &mut self,
        start: usize,
        end: usize,
        decodes: Arc<tokio::sync::Semaphore>,
    ) -> Task<Event> {
        let sequence = self.list_sequence;
        let mut tasks = Vec::new();
        for image_index in self
            .filtered_indices
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            let item = &self.images[*image_index];
            let url = item
                .thumb
                .as_deref()
                .filter(|url| !url.is_empty())
                .unwrap_or(&item.url)
                .to_owned();
            if self.thumbnails.lock().unwrap().contains(self.kind, &url)
                || !self.pending_thumbnails.insert(url.clone())
            {
                continue;
            }
            let client = self.client.clone();
            let url_for_result = url.clone();
            let kind = self.kind;
            let decodes = decodes.clone();
            let token = self.thumbnail_token.clone();
            tasks.push(Task::perform(
                thumbnail_work(move || fetch_thumbnail(&client, &url), kind, decodes, token),
                move |result| Event::Thumbnail(sequence, url_for_result, result),
            ));
        }
        Task::batch(tasks)
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.thumbnail_token.cancel();
    }
}

fn list_images(
    paths: &Paths,
    client: &Client,
    game: u64,
    kind: Kind,
    filters: Filters,
) -> Result<Vec<Image>, String> {
    let key = steamgriddb::require_key(
        paths,
        |variable| std::env::var_os(variable),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|error| error.to_string())?;
    let https = client.get()?;
    list_images_with(&*https, &key.value, game, kind, filters)
}

fn list_images_with(
    transport: &dyn Transport,
    key: &str,
    game: u64,
    kind: Kind,
    filters: Filters,
) -> Result<Vec<Image>, String> {
    let mut images = Api::new(transport, key)
        .images(game, kind, filters)
        .map_err(|error| error.to_string())?;
    rank_images(kind, &mut images);
    Ok(images)
}

fn rank_images(kind: Kind, images: &mut [Image]) {
    let language = steamgriddb::language(|variable| std::env::var_os(variable));
    steamgriddb::rank(kind, images, &language);
}

pub(crate) async fn thumbnail_work<F>(
    fetch: F,
    kind: Kind,
    decodes: Arc<tokio::sync::Semaphore>,
    token: Token,
) -> Option<Result<image::Handle, String>>
where
    F: FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
{
    if token.is_canceled() {
        return None;
    }
    let fetch_token = token.clone();
    let bytes =
        match tokio::task::spawn_blocking(move || (!fetch_token.is_canceled()).then(fetch)).await {
            Ok(Some(Ok(bytes))) => bytes,
            Ok(Some(Err(error))) => return (!token.is_canceled()).then_some(Err(error)),
            Ok(None) => return None,
            Err(error) => return (!token.is_canceled()).then_some(Err(error.to_string())),
        };
    if token.is_canceled() {
        return None;
    }
    // Waiting for a slot ends as soon as the picker moves on.
    let canceled = std::pin::pin!(token.canceled());
    let acquire = std::pin::pin!(decodes.acquire_owned());
    let permit = match iced::futures::future::select(acquire, canceled).await {
        iced::futures::future::Either::Left((Ok(permit), _)) => permit,
        iced::futures::future::Either::Left((Err(error), _)) => {
            return Some(Err(error.to_string()));
        }
        iced::futures::future::Either::Right(_) => return None,
    };
    if token.is_canceled() {
        return None;
    }
    let decode_token = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        (!decode_token.is_canceled()).then(|| decode_thumbnail_bytes(&bytes, kind))
    })
    .await
    .unwrap_or_else(|error| Some(Err(error.to_string())));
    (!token.is_canceled()).then_some(result).flatten()
}

fn fetch_thumbnail(client: &Client, url: &str) -> Result<Vec<u8>, String> {
    let https = client.get()?;
    // Thumbnails come from a CDN and `download` never sends the key, so none is
    // looked up.
    Api::new(&*https, "")
        .download(url)
        .map_err(|error| error.to_string())
}

fn decode_thumbnail_bytes(bytes: &[u8], kind: Kind) -> Result<image::Handle, String> {
    let kind = match kind {
        Kind::Icon => artwork::Kind::Logo,
        Kind::Grid => artwork::Kind::Grid,
        Kind::Hero => artwork::Kind::Hero,
        Kind::Logo => artwork::Kind::Logo,
    };
    artwork::decode_bytes(bytes, kind).map_err(|error| error.to_string())
}

fn filtered_indices(kind: Kind, images: &[Image], filters: FilterState) -> Vec<usize> {
    images
        .iter()
        .enumerate()
        .filter_map(|(index, image)| {
            let visible = (filters.animated || !image.animated)
                && (filters.nsfw || !image.nsfw)
                && (filters.humor || !image.humor)
                && (kind != Kind::Grid || filters.other_sizes || !is_other_grid_size(image));
            visible.then_some(index)
        })
        .collect()
}

fn is_other_grid_size(image: &Image) -> bool {
    image
        .width
        .zip(image.height)
        .is_some_and(|size| !matches!(size, (600, 900) | (342, 482) | (660, 930)))
}

fn filter_value(filters: FilterState, filter: Filter) -> bool {
    match filter {
        Filter::Animated => filters.animated,
        Filter::Nsfw => filters.nsfw,
        Filter::Humor => filters.humor,
        Filter::OtherSizes => filters.other_sizes,
    }
}

fn request_filters(filters: FilterState, kind: Kind) -> Filters {
    if filters.any(kind) {
        Filters::None
    } else {
        Filters::Ranked
    }
}

fn row_id(row: usize) -> iced::widget::Id {
    iced::widget::Id::from(format!("art-picker-row-{row}"))
}

/// Measures the tile row `row` in the scrolled content, then scrolls so it
/// shows. The runtime rebuilds the interface after an update before it runs
/// widget operations, so the row is already laid out with its Selected label
/// and any rows a Load more added.
struct Reveal {
    scroll: iced::widget::Id,
    row: usize,
    target: iced::widget::Id,
    /// The viewport's height, the content's top, and the current offset.
    viewport: Option<(f32, f32, f32)>,
    found: Option<iced::Rectangle>,
}

impl Reveal {
    fn new(scroll: iced::widget::Id, row: usize) -> Self {
        Self {
            scroll,
            row,
            target: row_id(row),
            viewport: None,
            found: None,
        }
    }
}

impl<T: Send + 'static> Operation<T> for Reveal {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::widget::Id>, bounds: iced::Rectangle) {
        if id == Some(&self.target) {
            self.found = Some(bounds);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        translation: iced::Vector,
        _state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.scroll) {
            self.viewport = Some((bounds.height, content_bounds.y, translation.y));
        }
    }

    fn finish(&self) -> Outcome<T> {
        let (Some((viewport, content_top, current)), Some(bounds)) = (self.viewport, self.found)
        else {
            return Outcome::None;
        };
        let top = bounds.y - content_top;
        let offset = reveal_offset(current, viewport, top, top + bounds.height, self.row == 0);
        if (offset - current).abs() < 0.5 {
            return Outcome::None;
        }
        Outcome::Chain(Box::new(scrollable_operation::scroll_to(
            self.scroll.clone(),
            scrollable_operation::AbsoluteOffset {
                x: None,
                y: Some(offset),
            },
        )))
    }
}

/// The smallest scroll offset that shows the row spanning `top..bottom` of
/// the content in a `viewport` tall window now scrolled to `current`. A row
/// taller than the window shows its top, and the first row shows the filters
/// above it when both fit.
fn reveal_offset(current: f32, viewport: f32, top: f32, bottom: f32, first: bool) -> f32 {
    let lowest = bottom - viewport;
    if first && lowest <= 0.0 {
        return 0.0;
    }
    if lowest > top {
        return top;
    }
    current.clamp(lowest, top)
}

fn is_best_match(image_index: usize, filtered: &[usize]) -> bool {
    filtered.first() == Some(&image_index)
}

fn columns(kind: Kind) -> usize {
    match kind {
        Kind::Hero => 3,
        Kind::Logo => 4,
        Kind::Grid => 5,
        Kind::Icon => 6,
    }
}

fn page_size(kind: Kind) -> usize {
    let columns = columns(kind);
    PAGE_SIZE.div_ceil(columns) * columns
}

fn tile_size(kind: Kind) -> (u32, u32) {
    let columns = columns(kind) as f32;
    // Account for page padding, scroll container padding, and gaps between tiles.
    let width = (PAGE_WIDTH - 48.0 - 4.0 - (columns - 1.0) * 10.0) / columns;
    let aspect = match kind {
        Kind::Grid => 2.0 / 3.0,
        Kind::Hero => 3.1,
        Kind::Logo => 2.0,
        // Icon thumbnails decode at 128px, so a taller box would only upscale them.
        Kind::Icon => return (width.round() as u32, 116),
    };
    (width.round() as u32, (width / aspect).round() as u32)
}

fn badge<'a>(label: &'a str, color: Color, palette: Palette) -> Element<'a, Event> {
    container(text(label).size(10).color(palette.foreground))
        .padding([2, 5])
        .style(move |_| {
            container::Style::default()
                .background(color.scale_alpha(0.35))
                .border(border::rounded(4).width(1).color(color))
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};
    use std::sync::Mutex;
    use std::time::Duration;
    use wineo_core::download::Response;

    struct FakeTransport {
        status: u16,
        requests: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn new(status: u16) -> Self {
            Self {
                status,
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, _url: &str) -> anyhow::Result<Box<dyn Read>> {
            anyhow::bail!("unexpected CDN request")
        }

        fn request(&self, url: &str, _bearer: Option<&str>) -> anyhow::Result<Response> {
            self.requests.lock().unwrap().push(url.to_owned());
            let page = url
                .split("page=")
                .nth(1)
                .and_then(|page| page.parse::<u32>().ok())
                .unwrap_or_default();
            let body = if self.status == 200 {
                match page {
                    0 => {
                        r#"{"success":true,"data":[{"id":4,"url":"https://cdn.example/4.png","score":1},{"id":8,"url":"https://cdn.example/8.png","score":10}]}"#
                    }
                    1 => {
                        r#"{"success":true,"data":[{"id":8,"url":"https://cdn.example/8.png","score":10},{"id":2,"url":"https://cdn.example/2.png","score":4}]}"#
                    }
                    _ => r#"{"success":true,"data":[]}"#,
                }
            } else {
                "{}"
            };
            Ok(Response {
                status: self.status,
                body: Box::new(Cursor::new(body.as_bytes().to_vec())),
            })
        }
    }

    fn paths() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                .unwrap();
        (root, paths)
    }

    fn item(id: u64) -> Image {
        Image {
            id,
            url: format!("https://cdn.example/{id}.png"),
            width: Some(600),
            height: Some(900),
            ..Image::default()
        }
    }

    #[test]
    fn kind_layouts_use_larger_previews_and_complete_pages() {
        for (kind, expected_columns, expected_page_size, expected_size) in [
            (Kind::Hero, 3, 18, (349, 113)),
            (Kind::Logo, 4, 20, (260, 130)),
            (Kind::Grid, 5, 20, (206, 308)),
            (Kind::Icon, 6, 18, (170, 116)),
        ] {
            assert_eq!(columns(kind), expected_columns);
            assert_eq!(page_size(kind), expected_page_size);
            assert_eq!(tile_size(kind), expected_size);
            assert_eq!(page_size(kind) % columns(kind), 0);
        }
    }

    #[test]
    fn listing_and_load_more_use_complete_rows_for_each_kind() {
        let (_root, paths) = paths();
        let decodes = Arc::new(tokio::sync::Semaphore::new(2));
        for kind in [Kind::Hero, Kind::Logo, Kind::Grid, Kind::Icon] {
            let mut picker = Picker::new(kind, 0);
            let page_size = page_size(kind);
            let count = page_size * 2 + 1;
            let images = (1..=count as u64).map(item).collect();
            let _ = picker.update(&paths, 123, Event::Listed(0, Ok(images)), decodes.clone());
            assert_eq!(picker.visible_count, page_size);
            assert_eq!(picker.pending_thumbnails.len(), page_size);

            let _ = picker.update(&paths, 123, Event::LoadMore, decodes.clone());
            assert_eq!(picker.visible_count, page_size * 2);
            assert_eq!(picker.pending_thumbnails.len(), page_size * 2);

            let _ = picker.update(&paths, 123, Event::LoadMore, decodes.clone());
            assert_eq!(picker.visible_count, count);
            assert_eq!(picker.pending_thumbnails.len(), count);
        }
    }

    #[test]
    fn keyboard_moves_by_kind_columns_and_reveals_complete_pages() {
        let (_root, paths) = paths();
        let decodes = Arc::new(tokio::sync::Semaphore::new(2));
        for kind in [Kind::Hero, Kind::Logo, Kind::Grid, Kind::Icon] {
            let mut picker = Picker::new(kind, 0);
            let page_size = page_size(kind);
            let images = (1..=(page_size * 3) as u64).map(item).collect();
            let _ = picker.update(&paths, 123, Event::Listed(0, Ok(images)), decodes.clone());
            picker.selected = Some(0);

            let _ = picker.update(&paths, 123, Event::Move(Direction::Down), decodes.clone());
            assert_eq!(picker.selected, Some(columns(kind)));
            let _ = picker.update(&paths, 123, Event::Move(Direction::Up), decodes.clone());
            assert_eq!(picker.selected, Some(0));

            picker.selected = Some(page_size - columns(kind));
            let _ = picker.update(&paths, 123, Event::Move(Direction::Down), decodes.clone());
            assert_eq!(picker.selected, Some(page_size));
            assert_eq!(picker.visible_count, page_size * 2);
            assert_eq!(picker.pending_thumbnails.len(), page_size * 2);
        }
    }

    #[test]
    fn thumbnail_cache_evicts_least_recently_used_by_decoded_bytes() {
        let mut cache = ThumbnailCache::default();
        let large = image::Handle::from_rgba(4096, 2048, vec![0; 32 * 1024 * 1024]);
        cache.insert(Kind::Grid, "first".into(), large.clone());
        cache.insert(Kind::Hero, "second".into(), large);
        assert_eq!(cache.bytes, THUMBNAIL_CACHE_BYTES);
        assert!(cache.get(Kind::Grid, "first").is_some());

        cache.insert(
            Kind::Logo,
            "third".into(),
            image::Handle::from_rgba(1, 1, vec![0; 4]),
        );

        assert!(cache.contains(Kind::Grid, "first"));
        assert!(!cache.contains(Kind::Hero, "second"));
        assert!(cache.contains(Kind::Logo, "third"));
        assert_eq!(cache.bytes, 32 * 1024 * 1024 + 4);
        cache.insert(
            Kind::Grid,
            "first".into(),
            image::Handle::from_rgba(1, 1, vec![0; 4]),
        );
        assert_eq!(cache.bytes, 8);
    }

    #[test]
    fn canceled_thumbnail_never_fetches_or_waits_for_decode() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let token = Token::new();
        token.cancel();
        runtime.block_on(async {
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                thumbnail_work(
                    || panic!("abandoned thumbnail must not fetch"),
                    Kind::Grid,
                    Arc::new(tokio::sync::Semaphore::new(0)),
                    token,
                ),
            )
            .await
            .unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn canceling_while_queued_for_a_decode_slot_ends_the_wait() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let token = Token::new();
        let cancel = token.clone();
        runtime.block_on(async {
            let work = thumbnail_work(
                || Ok(vec![0; 4]),
                Kind::Grid,
                Arc::new(tokio::sync::Semaphore::new(0)),
                token,
            );
            let canceler = async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                cancel.cancel();
            };
            let (result, ()) = tokio::time::timeout(
                Duration::from_secs(2),
                iced::futures::future::join(work, canceler),
            )
            .await
            .expect("the wait for a slot outlived the cancel");
            assert!(result.is_none());
        });
    }

    #[test]
    fn a_freed_decode_slot_lets_a_queued_thumbnail_decode() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let decodes = Arc::new(tokio::sync::Semaphore::new(0));
        let release = decodes.clone();
        runtime.block_on(async {
            let work = thumbnail_work(|| Ok(vec![0; 4]), Kind::Grid, decodes, Token::new());
            let freer = async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                release.add_permits(1);
            };
            let (result, ()) = tokio::time::timeout(
                Duration::from_secs(2),
                iced::futures::future::join(work, freer),
            )
            .await
            .expect("the queued thumbnail never got its slot");
            // The bytes are not an image, but reaching the decode proves the
            // permit arrived.
            assert!(matches!(result, Some(Err(_))));
        });
    }

    #[test]
    fn relisting_and_back_cancel_thumbnail_tokens_and_ignore_old_results() {
        let (_root, paths) = paths();
        let mut picker = Picker::new(Kind::Grid, 0);
        let decodes = Arc::new(tokio::sync::Semaphore::new(1));
        let old = picker.thumbnail_token();
        let _ = picker.update(
            &paths,
            123,
            Event::Filter(Filter::Animated, true),
            decodes.clone(),
        );
        assert!(old.is_canceled());
        assert!(!picker.thumbnail_token.is_canceled());
        let handle = image::Handle::from_rgba(1, 1, vec![0; 4]);
        let _ = picker.update(
            &paths,
            123,
            Event::Thumbnail(0, "old".into(), Some(Ok(handle.clone()))),
            decodes.clone(),
        );
        assert!(
            !picker
                .thumbnails
                .lock()
                .unwrap()
                .contains(Kind::Grid, "old")
        );
        let current = picker.thumbnail_token();
        let _ = picker.update(&paths, 123, Event::Back, decodes.clone());
        assert!(current.is_canceled());
        let _ = picker.update(
            &paths,
            123,
            Event::Thumbnail(picker.list_sequence, "closed".into(), Some(Ok(handle))),
            decodes,
        );
        assert!(
            !picker
                .thumbnails
                .lock()
                .unwrap()
                .contains(Kind::Grid, "closed")
        );
    }

    #[test]
    fn ranked_list_uses_core_order_before_display_paging() {
        let mut images = vec![
            Image {
                id: 4,
                score: Some(1),
                ..item(4)
            },
            Image {
                id: 8,
                score: Some(10),
                ..item(8)
            },
            Image {
                id: 2,
                score: Some(4),
                ..item(2)
            },
        ];
        steamgriddb::rank(Kind::Grid, &mut images, "en");
        assert_eq!(
            images.iter().map(|image| image.id).collect::<Vec<_>>(),
            [8, 2, 4]
        );
        let mut picker = Picker::new(Kind::Grid, 0);
        picker.images = images;
        picker.rebuild_indices();
        picker.visible_count = 2;
        assert_eq!(picker.images_for_display()[..picker.visible_count], [0, 1]);
        assert_eq!(picker.images[0].id, 8);
        assert!(is_best_match(0, &[0, 1]));
        assert!(!is_best_match(1, &[0, 1]));
    }

    #[test]
    fn listing_walks_pages_deduplicates_and_ranks_in_order() {
        let fake = FakeTransport::new(200);
        let images = list_images_with(&fake, "key", 123, Kind::Grid, Filters::Ranked).unwrap();

        assert_eq!(
            images.iter().map(|image| image.id).collect::<Vec<_>>(),
            [8, 2, 4]
        );
        let requests = fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].contains("types=static&nsfw=false&humor=false"));
        assert!(requests[0].contains("page=0"));
        assert!(requests[1].contains("page=1"));
        assert!(requests[2].contains("page=2"));
    }

    #[test]
    fn listing_reports_rejected_key() {
        let fake = FakeTransport::new(403);
        let error = list_images_with(&fake, "key", 123, Kind::Grid, Filters::Ranked).unwrap_err();

        assert_eq!(error, "SteamGridDB rejected the key");
    }

    #[test]
    fn load_more_does_not_relist_and_filters_choose_request_mode() {
        let (_root, paths) = paths();
        let mut picker = Picker::new(Kind::Grid, 5);
        picker.images = (1..=25).map(item).collect();
        picker.rebuild_indices();
        picker.visible_count = page_size(picker.kind);
        let sequence = picker.list_sequence();
        let _ = picker.update(
            &paths,
            123,
            Event::LoadMore,
            Arc::new(tokio::sync::Semaphore::new(2)),
        );
        assert_eq!(picker.visible_count, 25);
        assert_eq!(picker.list_sequence(), sequence);

        let _ = picker.update(
            &paths,
            123,
            Event::Filter(Filter::Nsfw, true),
            Arc::new(tokio::sync::Semaphore::new(2)),
        );
        assert_eq!(picker.list_sequence(), sequence + 1);
        assert_eq!(request_filters(picker.filters, Kind::Grid), Filters::None);

        let _ = picker.update(
            &paths,
            123,
            Event::Filter(Filter::Nsfw, false),
            Arc::new(tokio::sync::Semaphore::new(2)),
        );
        assert_eq!(picker.list_sequence(), sequence + 2);
        assert_eq!(request_filters(picker.filters, Kind::Grid), Filters::Ranked);
        assert_eq!(
            request_filters(FilterState::default(), Kind::Grid),
            Filters::Ranked
        );
        assert_eq!(
            request_filters(
                FilterState {
                    other_sizes: true,
                    ..FilterState::default()
                },
                Kind::Grid
            ),
            Filters::None
        );
        assert_eq!(
            request_filters(
                FilterState {
                    other_sizes: true,
                    ..FilterState::default()
                },
                Kind::Hero
            ),
            Filters::Ranked
        );
        assert_eq!(
            request_filters(Picker::new(Kind::Grid, 9).filters, Kind::Grid),
            Filters::Ranked
        );
    }

    #[test]
    fn filters_hide_categories_and_standard_grid_sizes_only() {
        let mut animated = item(2);
        animated.animated = true;
        let mut nsfw = item(3);
        nsfw.nsfw = true;
        let mut humor = item(4);
        humor.humor = true;
        let mut other_size = item(5);
        other_size.width = Some(900);
        let images = vec![item(1), animated, nsfw, humor, other_size];

        assert_eq!(
            filtered_indices(Kind::Grid, &images, FilterState::default()),
            [0]
        );
        assert_eq!(
            filtered_indices(
                Kind::Grid,
                &images,
                FilterState {
                    animated: true,
                    nsfw: false,
                    humor: false,
                    other_sizes: false,
                }
            ),
            [0, 1]
        );
        assert_eq!(
            filtered_indices(
                Kind::Grid,
                &images,
                FilterState {
                    animated: true,
                    nsfw: true,
                    humor: true,
                    other_sizes: true,
                }
            ),
            [0, 1, 2, 3, 4]
        );
        assert_eq!(
            filtered_indices(Kind::Hero, &images, FilterState::default()),
            [0, 4]
        );
    }

    #[test]
    fn selection_details_include_image_metadata() {
        let mut picker = Picker::new(Kind::Grid, 7);
        picker.images = vec![Image {
            id: 42,
            author: Some("Someone".to_owned()),
            width: Some(600),
            height: Some(900),
            style: Some("official".to_owned()),
            score: Some(12),
            ..Image::default()
        }];
        picker.rebuild_indices();
        picker.visible_count = 1;
        picker.selected = Some(0);

        assert_eq!(picker.selected().unwrap().id, 42);
        assert_eq!(
            picker.details().as_deref(),
            Some("SteamGridDB #42 by Someone - 600x900 - official - score 12")
        );
    }

    #[test]
    fn thumbnail_decoding_sniffs_opaque_urls_and_uses_animated_first_frame() {
        let first = ::image::RgbaImage::from_pixel(1, 1, ::image::Rgba([255, 0, 0, 255]));
        let second = ::image::RgbaImage::from_pixel(1, 1, ::image::Rgba([0, 0, 255, 255]));
        let delay = ::image::Delay::from_numer_denom_ms(100, 1);
        let frames = [
            ::image::Frame::from_parts(first, 0, 0, delay),
            ::image::Frame::from_parts(second, 0, 0, delay),
        ];
        let mut bytes = Vec::new();
        ::image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames(frames)
            .unwrap();

        let handle = decode_thumbnail_bytes(&bytes, Kind::Grid).unwrap();
        let image::Handle::Rgba { pixels, .. } = handle else {
            panic!("artwork decode should return pixels");
        };
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }

    #[derive(Default)]
    struct RecordingTransport {
        seen: Mutex<Vec<(String, Option<String>)>>,
    }

    impl Transport for RecordingTransport {
        fn get(&self, _: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
            unreachable!("thumbnails use request")
        }

        fn request(
            &self,
            url: &str,
            bearer: Option<&str>,
        ) -> anyhow::Result<wineo_core::download::Response> {
            self.seen
                .lock()
                .unwrap()
                .push((url.to_owned(), bearer.map(str::to_owned)));
            Ok(wineo_core::download::Response {
                status: 200,
                body: Box::new(std::io::Cursor::new(b"bytes".to_vec())),
            })
        }
    }

    #[test]
    fn thumbnails_share_one_client_and_send_no_key() {
        let transport = Arc::new(RecordingTransport::default());
        let builds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = {
            let transport = transport.clone();
            let builds = builds.clone();
            Client::with_builder(move || {
                builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(transport.clone())
            })
        };
        for number in 0..30 {
            let url = format!("https://cdn.example/{number}.png");
            assert_eq!(fetch_thumbnail(&client, &url).unwrap(), b"bytes");
        }
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);
        let seen = transport.seen.lock().unwrap();
        assert_eq!(seen.len(), 30);
        assert!(seen.iter().all(|(_, bearer)| bearer.is_none()));
    }

    #[test]
    fn a_failed_client_build_is_retried() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = {
            let attempts = attempts.clone();
            Client::with_builder(move || {
                if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err("no tls".to_owned())
                } else {
                    Ok(Arc::new(RecordingTransport::default()))
                }
            })
        };
        assert_eq!(
            fetch_thumbnail(&client, "https://cdn.example/a.png").unwrap_err(),
            "no tls"
        );
        assert!(fetch_thumbnail(&client, "https://cdn.example/a.png").is_ok());
    }

    #[test]
    fn visible_rows_load_thumb_urls_and_fall_back_to_originals() {
        let (_root, paths) = paths();
        let mut picker = Picker::new(Kind::Grid, 0);
        let mut images: Vec<_> = (1..=24).map(item).collect();
        images[0].thumb = Some("https://cdn.example/first-thumb.png".into());
        images[1].thumb = Some(String::new());
        let decodes = Arc::new(tokio::sync::Semaphore::new(2));
        let _ = picker.update(&paths, 123, Event::Listed(0, Ok(images)), decodes.clone());
        assert_eq!(picker.pending_thumbnails.len(), page_size(picker.kind));
        assert!(
            picker
                .pending_thumbnails
                .contains("https://cdn.example/first-thumb.png")
        );
        assert!(
            !picker
                .pending_thumbnails
                .contains("https://cdn.example/1.png")
        );
        assert!(
            picker
                .pending_thumbnails
                .contains("https://cdn.example/2.png")
        );
        assert!(
            !picker
                .pending_thumbnails
                .contains("https://cdn.example/21.png")
        );
        let _ = picker.update(&paths, 123, Event::LoadMore, decodes);
        assert_eq!(picker.pending_thumbnails.len(), 24);
        assert!(
            picker
                .pending_thumbnails
                .contains("https://cdn.example/21.png")
        );
        assert_eq!(picker.list_sequence(), 0);
    }

    fn loaded(kind: Kind, filters_open: bool) -> Picker {
        let mut picker = Picker::new(kind, 0);
        picker.images = (1..=40).map(item).collect();
        picker.filtered_indices = (0..40).collect();
        picker.visible_count = page_size(kind).min(40);
        picker.selected = Some(0);
        picker.filters_open = filters_open;
        picker
    }

    fn nodes_of_size(layout: iced::advanced::Layout<'_>, size: iced::Size) -> Vec<iced::Rectangle> {
        let bounds = layout.bounds();
        let mut found: Vec<_> = layout
            .children()
            .flat_map(|child| nodes_of_size(child, size))
            .collect();
        if (bounds.width - size.width).abs() < 0.01 && (bounds.height - size.height).abs() < 0.01 {
            found.push(bounds);
        }
        found
    }

    fn natural(button: iced::widget::Button<'static, Event>) -> iced::Size {
        let mut element: Element<'static, Event> = button.into();
        crate::layout_probe::layout(&mut element, iced::Size::new(4000.0, 400.0)).size()
    }

    #[test]
    fn a_short_window_scrolls_the_title_and_filters_with_the_grid_and_pins_the_footer() {
        use crate::layout_probe::probe;
        let palette = Palette::DARK;
        let title = "Grid art for an extremely long environment name that goes on and on";
        let use_size = natural(accent_base("Use", palette.primary));
        let cancel_size = natural(secondary_button("Cancel", palette));
        for kind in [Kind::Grid, Kind::Hero] {
            for filters_open in [false, true] {
                for progress in [None, Some("Downloading 123.4 MB of 456.7 MB, nearly there")] {
                    for (width, height) in [(360.0_f32, 300.0_f32), (360.0, 240.0), (900.0, 400.0)]
                    {
                        let picker = loaded(kind, filters_open);
                        let size = iced::Size::new(width, height);
                        let case =
                            format!("{kind} filters {filters_open} {progress:?} {width}x{height}");
                        let mut view = picker.view(palette, title, progress);
                        let frame = iced::Rectangle::with_size(size);
                        let probe = probe(&mut view, size);
                        let spilled = probe.outside(frame);
                        assert!(spilled.is_empty(), "{case}: {spilled:?}");
                        assert_eq!(probe.scrollables.len(), 1, "{case}");
                        let (viewport, _) = probe.scrollables[0];
                        assert!(viewport.height > 0.0, "{case}: no room for the images");
                        let layout = iced::advanced::Layout::new(&probe.node);
                        let mut footer = vec![(use_size, "Use")];
                        if progress.is_some() {
                            footer.push((cancel_size, "Cancel"));
                        }
                        for (size, label) in footer {
                            let found = nodes_of_size(layout, size);
                            assert_eq!(found.len(), 1, "{case}: {label} was squeezed");
                            let found = found[0];
                            assert!(
                                found.y >= viewport.y + viewport.height - 0.01,
                                "{case}: {label} is not below the scroll"
                            );
                            assert!(
                                found.y + found.height <= height + 0.01,
                                "{case}: {label} is cut off by the window"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Where a tile row and the scrollable are, as seen by an operation.
    #[derive(Default)]
    struct Spotted {
        row: Option<iced::Rectangle>,
        scroll: Option<(iced::Rectangle, iced::Rectangle, iced::Vector)>,
    }

    struct Where {
        row: iced::widget::Id,
        scroll: iced::widget::Id,
        out: Arc<Mutex<Spotted>>,
    }

    impl Operation<()> for Where {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
            operate(self);
        }

        fn container(&mut self, id: Option<&iced::widget::Id>, bounds: iced::Rectangle) {
            if id == Some(&self.row) {
                self.out.lock().unwrap().row = Some(bounds);
            }
        }

        fn scrollable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: iced::Rectangle,
            content_bounds: iced::Rectangle,
            translation: iced::Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&self.scroll) {
                self.out.lock().unwrap().scroll = Some((bounds, content_bounds, translation));
            }
        }
    }

    /// The row's top and bottom on screen, and the viewport's.
    fn on_screen(
        laid: &mut crate::layout_probe::Laid<'_, Event>,
        picker: &Picker,
        row: usize,
    ) -> (f32, f32, iced::Rectangle) {
        let out = Arc::new(Mutex::new(Spotted::default()));
        laid.run(Box::new(Where {
            row: row_id(row),
            scroll: picker.scroll_id.clone(),
            out: out.clone(),
        }));
        let out = out.lock().unwrap();
        let row = out.row.expect("row is laid out");
        let (viewport, content, translation) = out.scroll.expect("scrollable");
        let top = viewport.y + (row.y - content.y) - translation.y;
        (top, top + row.height, viewport)
    }

    /// Rows differ in height with their badges and the Selected label, so the
    /// reveal has to measure them.
    #[test]
    fn keyboard_reveal_lands_on_rows_of_different_heights() {
        use crate::layout_probe::lay_out;
        let palette = Palette::DARK;
        for kind in [Kind::Grid, Kind::Hero] {
            for filters_open in [false, true] {
                for (width, height) in [(360.0_f32, 300.0_f32), (360.0, 900.0), (900.0, 400.0)] {
                    let mut probe_picker = loaded(kind, filters_open);
                    for (index, image) in probe_picker.images.iter_mut().enumerate() {
                        // Alternate rows wrap into several badge lines.
                        let badged = (index / columns(kind)).is_multiple_of(2);
                        image.animated = badged;
                        image.nsfw = badged;
                        image.humor = badged;
                    }
                    let rows = probe_picker.visible_count.div_ceil(columns(kind));
                    for row in [0, rows / 2, rows - 1] {
                        let mut picker = loaded(kind, filters_open);
                        picker.images = probe_picker.images.clone();
                        picker.selected = Some(row * columns(kind));
                        let case =
                            format!("{kind} filters {filters_open} {width}x{height} row {row}");
                        let mut laid = lay_out(
                            picker.view(palette, "Grid art for my-game", None),
                            iced::Size::new(width, height),
                        );
                        // Start from the far end so every row has to move.
                        for from in [rows - 1, 0, row] {
                            laid.run(Box::new(Reveal::new(picker.scroll_id.clone(), from)));
                        }
                        let (top, bottom, viewport) = on_screen(&mut laid, &picker, row);
                        let taller = bottom - top > viewport.height;
                        let first = on_screen(&mut laid, &picker, 0);
                        let second = on_screen(&mut laid, &picker, 1);
                        assert!(
                            ((first.1 - first.0) - (second.1 - second.0)).abs() > 10.0,
                            "{case}: rows should differ in height"
                        );
                        assert!(top >= viewport.y - 0.5, "{case}: row starts above the view");
                        assert!(
                            taller || bottom <= viewport.y + viewport.height + 0.5,
                            "{case}: row ends below the view"
                        );
                        assert!(
                            top <= viewport.y + viewport.height,
                            "{case}: row top is below the view"
                        );
                    }
                }
            }
        }
    }

    /// Drives the runtime's order: the update moves the selection (and loads
    /// more rows), the interface is rebuilt with the scroll position it had,
    /// and only then does the reveal run.
    #[test]
    fn moving_the_selection_reveals_the_new_row_after_the_rebuild() {
        use crate::layout_probe::lay_out;
        let (_root, paths) = paths();
        let palette = Palette::DARK;
        let decodes = Arc::new(tokio::sync::Semaphore::new(2));
        let kind = Kind::Grid;
        // 19 to 20 crosses into rows a Load more appends.
        for (from, direction) in [
            (19, Direction::Right),
            (9, Direction::Right),
            (10, Direction::Left),
            (14, Direction::Down),
        ] {
            for (width, height) in [(360.0_f32, 900.0_f32), (360.0, 240.0)] {
                let mut picker = loaded(kind, true);
                for (index, image) in picker.images.iter_mut().enumerate() {
                    let badged = (index / columns(kind)).is_multiple_of(2);
                    image.animated = badged;
                    image.nsfw = badged;
                    image.humor = badged;
                }
                // Thumbnails are not under test, and fetching them needs a network.
                picker.pending_thumbnails = picker.images.iter().map(|i| i.url.clone()).collect();
                picker.visible_count = page_size(kind);
                picker.selected = Some(from);
                let size = iced::Size::new(width, height);
                let case = format!("{from} {direction:?} {width}x{height}");
                let mut laid = lay_out(picker.view(palette, "Grid art for my-game", None), size);
                laid.run(Box::new(picker.reveal(from)));
                let offset = laid.offsets()[0];
                drop(laid);

                let _ = picker.update(&paths, 123, Event::Move(direction), decodes.clone());
                let to = picker.selected.expect("a selection");
                assert_ne!(to, from, "{case}");
                assert_eq!(
                    picker.visible_count,
                    if from == 19 { 40 } else { 20 },
                    "{case}"
                );
                let mut laid = lay_out(picker.view(palette, "Grid art for my-game", None), size);
                laid.run(Box::new(scrollable_operation::scroll_to(
                    picker.scroll_id.clone(),
                    scrollable_operation::AbsoluteOffset {
                        x: None,
                        y: Some(offset),
                    },
                )));
                laid.run(Box::new(picker.reveal(to)));
                let (top, bottom, viewport) = on_screen(&mut laid, &picker, to / columns(kind));
                assert!(top >= viewport.y - 0.5, "{case}: row starts above the view");
                assert!(
                    bottom - top > viewport.height || bottom <= viewport.y + viewport.height + 0.5,
                    "{case}: row ends below the view"
                );
                assert!(
                    top <= viewport.y + viewport.height,
                    "{case}: row top is below the view"
                );
            }
        }
    }

    #[test]
    fn reveal_offset_keeps_visible_rows_and_shows_the_first_row_whole() {
        // Already in view: no move.
        assert_eq!(reveal_offset(100.0, 300.0, 150.0, 250.0, false), 100.0);
        // Below the window: its bottom lines up with the bottom edge.
        assert_eq!(reveal_offset(0.0, 300.0, 400.0, 500.0, false), 200.0);
        // Above it: its top lines up with the top edge.
        assert_eq!(reveal_offset(500.0, 300.0, 100.0, 200.0, false), 100.0);
        // The first row scrolls back to show the filters above it.
        assert_eq!(reveal_offset(80.0, 300.0, 60.0, 160.0, true), 0.0);
        // A row taller than the window shows its top.
        assert_eq!(reveal_offset(0.0, 100.0, 200.0, 500.0, false), 200.0);
    }
}
