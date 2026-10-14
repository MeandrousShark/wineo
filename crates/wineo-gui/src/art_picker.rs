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
use crate::task;
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

// A blocking reqwest client joins its runtime thread when dropped, which can
// wait on a slow DNS lookup, so the last owner must not be the UI thread.
impl Drop for Client {
    fn drop(&mut self) {
        let built = self
            .built
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(transport) = built {
            std::thread::spawn(move || drop(transport));
        }
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
        let listing = task::blocking(
            move || list_images(&paths, &client, game, kind, filters),
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
mod tests;
