use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::widget::{Row, column, container, image, mouse_area, row, scrollable, text, toggler};
use iced::{Center, Color, ContentFit, Element, Fill, Length, Task, border, mouse};
use wineo_core::art::Kind;
use wineo_core::cancel::Token;
use wineo_core::download::{Https, Transport};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Filters, Image};

use crate::artwork;
use crate::keys::{self, Direction};
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

const PAGE_SIZE: usize = 18;
const PAGE_WIDTH: f32 = 1120.0;
const THUMBNAIL_CACHE_BYTES: usize = 64 * 1024 * 1024;

type ThumbnailKey = (Kind, String);

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
    Scrolled(f32, f32),
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
    pub(crate) error: Option<String>,
    scroll_id: iced::widget::Id,
    scroll_offset: f32,
    viewport_height: f32,
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
            error: None,
            scroll_id: iced::widget::Id::unique(),
            scroll_offset: 0.0,
            viewport_height: 0.0,
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
            Event::Scrolled(offset, height) => {
                self.scroll_offset = offset;
                self.viewport_height = height;
                Task::none()
            }
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
                        self.thumbnail_tasks(0, self.visible_count, paths, decodes)
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
                self.thumbnail_tasks(old_count, self.visible_count, paths, decodes)
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
                let thumbnails =
                    self.thumbnail_tasks(old_count, self.visible_count, paths, decodes);
                let row_height = tile_size(self.kind).1 as f32 + 48.0;
                let top = (index / columns) as f32 * row_height;
                let bottom = top + row_height;
                let offset = if top < self.scroll_offset {
                    Some(top)
                } else if bottom > self.scroll_offset + self.viewport_height {
                    Some((bottom - self.viewport_height).max(0.0))
                } else {
                    None
                };
                let Some(offset) = offset else {
                    return thumbnails;
                };
                Task::batch([
                    thumbnails,
                    iced::widget::operation::scroll_to(
                        self.scroll_id.clone(),
                        scrollable::AbsoluteOffset { x: 0.0, y: offset },
                    ),
                ])
            }
        }
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
        let mut header = row![
            secondary_button("< Back", palette).on_press(Event::Back),
            text(title.to_owned()).size(20),
        ]
        .spacing(14)
        .align_y(Center);
        header = header.push(iced::widget::Space::new().width(Fill));
        header = header.push(
            secondary_button(
                if self.filters_open {
                    "Filters ^"
                } else {
                    "Filters v"
                },
                palette,
            )
            .on_press(Event::ToggleFilters),
        );

        let mut body = column![header].spacing(12);
        if self.filters_open {
            body = body.push(self.filters_view(palette));
        }
        if self.loading && self.images.is_empty() {
            body = body.push(text("Loading SteamGridDB images...").color(palette.muted));
        }
        if let Some(error) = &self.error
            && self.selected().is_none()
        {
            body = body.push(text(error).size(13).color(palette.danger));
        }
        if !self.loading && self.filtered_indices.is_empty() && self.error.is_none() {
            body = body.push(text("No images found.").color(palette.muted));
        }
        body = body.push(self.grid_view(palette));
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
        if let Some(status) = progress {
            details = details.push(
                row![
                    text(status.to_owned()).size(12).color(palette.muted),
                    secondary_button("Cancel", palette).on_press(Event::Cancel),
                ]
                .spacing(10)
                .align_y(Center),
            );
        }
        let has_progress = progress.is_some();
        let use_button = if !has_progress && self.selected().is_some() {
            accent_base("Use", palette.primary).on_press(Event::Use)
        } else {
            secondary_button("Use", palette)
        };
        let footer = row![details.width(Fill), use_button]
            .spacing(14)
            .align_y(Center);
        body = body.push(footer);

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
        self.scroll_offset = 0.0;
        let sequence = self.list_sequence;
        let kind = self.kind;
        let filters = request_filters(self.filters, kind);
        let paths = paths.clone();
        let listing = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || list_images(&paths, game, kind, filters))
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
        container(filters)
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
            grid = grid.push(tiles);
        }
        if self.visible_count < indices.len() {
            grid = grid.push(
                row![iced::widget::Space::new().width(Fill),]
                    .push(secondary_button("Load more", palette).on_press(Event::LoadMore))
                    .push(iced::widget::Space::new().width(Fill)),
            );
        }
        scrollable(container(grid).width(Fill).padding([0, 2]))
            .id(self.scroll_id.clone())
            .on_scroll(|viewport| {
                Event::Scrolled(viewport.absolute_offset().y, viewport.bounds().height)
            })
            .height(Fill)
            .into()
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
        paths: &Paths,
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
            let paths = paths.clone();
            let url_for_result = url.clone();
            let kind = self.kind;
            let decodes = decodes.clone();
            let token = self.thumbnail_token.clone();
            tasks.push(Task::perform(
                thumbnail_work(move || fetch_thumbnail(&paths, &url), kind, decodes, token),
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
    let https = Https::new().map_err(|error| error.to_string())?;
    list_images_with(&https, &key.value, game, kind, filters)
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
    let acquire = decodes.acquire_owned();
    tokio::pin!(acquire);
    let permit = loop {
        if token.is_canceled() {
            return None;
        }
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(10), &mut acquire).await {
            break match result {
                Ok(permit) => permit,
                Err(error) => return Some(Err(error.to_string())),
            };
        }
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

fn fetch_thumbnail(paths: &Paths, url: &str) -> Result<Vec<u8>, String> {
    let key = steamgriddb::require_key(
        paths,
        |variable| std::env::var_os(variable),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|error| error.to_string())?;
    let https = Https::new().map_err(|error| error.to_string())?;
    Api::new(&https, &key.value)
        .download(url)
        .map_err(|error| error.to_string())
}

fn decode_thumbnail_bytes(bytes: &[u8], kind: Kind) -> Result<image::Handle, String> {
    let suffix = thumbnail_suffix(bytes);
    let mut file = tempfile::Builder::new()
        .suffix(&suffix)
        .tempfile()
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    let kind = match kind {
        Kind::Icon => artwork::Kind::Logo,
        Kind::Grid => artwork::Kind::Grid,
        Kind::Hero => artwork::Kind::Hero,
        Kind::Logo => artwork::Kind::Logo,
    };
    artwork::decode(file.path(), kind, None).map_err(|error| error.to_string())
}

fn thumbnail_suffix(bytes: &[u8]) -> String {
    let extension = if wineo_core::art::detect(bytes) == Some(wineo_core::art::Format::Svg) {
        Some("svg")
    } else {
        ::image::guess_format(bytes)
            .ok()
            .and_then(|format| format.extensions_str().first().copied())
    };
    extension.map_or_else(|| ".img".to_owned(), |extension| format!(".{extension}"))
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

        assert_eq!(thumbnail_suffix(&bytes), ".gif");
        let handle = decode_thumbnail_bytes(&bytes, Kind::Grid).unwrap();
        let image::Handle::Rgba { pixels, .. } = handle else {
            panic!("artwork decode should return pixels");
        };
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
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
}
