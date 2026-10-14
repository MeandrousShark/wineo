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
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
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

struct DropThread(std::sync::mpsc::Sender<std::thread::ThreadId>);

impl Drop for DropThread {
    fn drop(&mut self) {
        let _ = self.0.send(std::thread::current().id());
    }
}

impl Transport for DropThread {
    fn get(&self, _: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        unreachable!()
    }
}

#[test]
fn the_last_client_owner_drops_the_transport_off_the_calling_thread() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let sender = Mutex::new(sender);
    let client =
        Client::with_builder(move || Ok(Arc::new(DropThread(sender.lock().unwrap().clone()))));
    client.get().unwrap();
    drop(client);
    let dropped_on = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_ne!(dropped_on, std::thread::current().id());
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
                for (width, height) in [(360.0_f32, 300.0_f32), (360.0, 240.0), (900.0, 400.0)] {
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
                    let case = format!("{kind} filters {filters_open} {width}x{height} row {row}");
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
