//! How the import grids turn a poster file into a tile image: capped to the
//! tile size, kept as a small copy in the art cache, and decoded under the
//! same permits as the library's art so a long list never decodes at once.

use std::io::Read;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;

use iced::futures::executor::block_on;
use iced::futures::future::{Either, select};
use iced::widget::image::Handle;
use tokio::sync::Semaphore;
use wineo_core::art::{self, Format, Kind};
use wineo_core::cancel::Token;

use crate::artwork;

/// Enough to recognise every format by its start, short of a long SVG prolog.
const HEAD: u64 = 64 * 1024;

/// Decodes the poster at `path`, and says which format it is stored as. Checks
/// the file like the art draft does, so a file that is not a Grid image is
/// refused before any decoding.
pub(super) fn decode(path: &Path, cache: &Path) -> Result<(Handle, Format), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > art::MAX_BYTES {
        return Err("Artwork exceeds 32 MiB".to_owned());
    }
    let mut head = Vec::new();
    (&mut file)
        .take(HEAD)
        .read_to_end(&mut head)
        .map_err(|e| e.to_string())?;
    let mut format = art::detect(&head);
    // A prolog longer than the head is rare, so only then read it all.
    if format.is_none() && length > HEAD {
        file.read_to_end(&mut head).map_err(|e| e.to_string())?;
        format = art::detect(&head);
    }
    let format = format
        .filter(|format| Kind::Grid.accepts(*format))
        .ok_or_else(|| format!("Unsupported image format for {}", Kind::Grid))?;
    drop(head);
    let handle = artwork::decode_as(path, artwork::TILE, Some(cache), format == Format::Svg)
        .map_err(|e| e.to_string())?;
    Ok((handle, format))
}

/// Runs `work` on a blocking thread once one of the shared decode permits is
/// free, holding it until `work` returns.
pub(super) async fn bounded<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    bounded_by(artwork::decode_slots(), work).await
}

async fn bounded_by<T: Send + 'static>(
    slots: Arc<Semaphore>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let permit = slots.acquire_owned().await.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let done = work();
        drop(permit);
        done
    })
    .await
    .map_err(|e| e.to_string())
}

/// `decode` for a caller already on a worker thread, once a shared permit is
/// free, so it counts against the same limit as the grids' own decodes. Gives
/// up with `None` if `token` is canceled while it waits. Take no permit before
/// a download and none is held across one.
pub(super) fn decode_waiting(
    path: &Path,
    cache: &Path,
    token: Option<&Token>,
) -> Option<Result<(Handle, Format), String>> {
    waiting(artwork::decode_slots(), token, || decode(path, cache))
}

fn waiting<T>(slots: Arc<Semaphore>, token: Option<&Token>, work: impl FnOnce() -> T) -> Option<T> {
    let permit = block_on(async {
        let acquire = pin!(slots.acquire_owned());
        match token {
            None => acquire.await.ok(),
            Some(token) => match select(acquire, pin!(token.canceled())).await {
                Either::Left((permit, _)) => permit.ok(),
                Either::Right(_) => None,
            },
        }
    })?;
    let done = work();
    drop(permit);
    Some(done)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn no_more_decodes_run_at_once_than_there_are_permits() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let slots = Arc::new(Semaphore::new(2));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs: Vec<_> = (0..40)
            .map(|index| {
                let (running, peak) = (running.clone(), peak.clone());
                bounded_by(slots.clone(), move || {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                    running.fetch_sub(1, Ordering::SeqCst);
                    index
                })
            })
            .collect();
        let done = runtime.block_on(iced::futures::future::join_all(jobs));
        // Every poster still arrives, in the order asked for.
        let done: Vec<_> = done.into_iter().map(Result::unwrap).collect();
        assert_eq!(done, (0..40).collect::<Vec<_>>());
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(slots.available_permits(), 2);
    }

    #[test]
    fn worker_thread_decodes_share_the_limit_with_the_grids() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let slots = Arc::new(Semaphore::new(2));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let work = |running: Arc<AtomicUsize>, peak: Arc<AtomicUsize>| {
            move || {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
                running.fetch_sub(1, Ordering::SeqCst);
            }
        };
        let threads: Vec<_> = (0..20)
            .map(|_| {
                let (slots, run) = (slots.clone(), work(running.clone(), peak.clone()));
                std::thread::spawn(move || waiting(slots, None, run).unwrap())
            })
            .collect();
        let jobs: Vec<_> = (0..20)
            .map(|_| bounded_by(slots.clone(), work(running.clone(), peak.clone())))
            .collect();
        for done in runtime.block_on(iced::futures::future::join_all(jobs)) {
            done.unwrap();
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(slots.available_permits(), 2);
    }

    #[test]
    fn a_canceled_worker_gives_up_its_place_in_line() {
        let slots = Arc::new(Semaphore::new(0));
        let token = Token::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = {
            let (slots, token) = (slots.clone(), token.clone());
            std::thread::spawn(move || {
                let ran = waiting(slots, Some(&token), || ());
                sender.send(ran).unwrap();
            })
        };
        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        token.cancel();
        let ran = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("the wait outlived the cancel");
        assert_eq!(ran, None);
        waiter.join().unwrap();
        // A freed permit lets a waiting worker decode.
        let release = slots.clone();
        let late = std::thread::spawn(move || waiting(slots, None, || 7));
        release.add_permits(1);
        assert_eq!(late.join().unwrap(), Some(7));
    }

    #[test]
    fn a_poster_is_capped_cached_and_reports_its_format() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("poster.png");
        image::RgbImage::from_pixel(600, 900, image::Rgb([40, 100, 180]))
            .save(&source)
            .unwrap();
        let cache = dir.path().join("cache");
        let (handle, format) = decode(&source, &cache).unwrap();
        assert_eq!(format, Format::Png);
        let Handle::Rgba { width, height, .. } = handle else {
            panic!("not decoded to pixels");
        };
        assert_eq!(
            (width, height),
            (
                crate::flow::artwork::POSTER_WIDTH,
                crate::flow::artwork::POSTER_HEIGHT
            )
        );
        assert!(cache.exists(), "the shrunk copy is cached");
        let (again, _) = decode(&source, &cache).unwrap();
        assert!(matches!(again, Handle::Rgba { width: 152, .. }));
    }

    #[test]
    fn a_poster_needs_no_extension() {
        // Heroic caches its images under a bare hash.
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let png = dir.path().join("0123abcd");
        image::RgbImage::from_pixel(4, 6, image::Rgb([40, 100, 180]))
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode(&png, &cache).unwrap().1, Format::Png);
        let svg = dir.path().join("4567ef01");
        std::fs::write(
            &svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"3\"><rect width=\"2\" height=\"3\"/></svg>",
        )
        .unwrap();
        assert_eq!(decode(&svg, &cache).unwrap().1, Format::Svg);
    }

    #[test]
    fn a_file_that_is_not_a_grid_image_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let text = dir.path().join("poster.png");
        std::fs::write(&text, "not an image").unwrap();
        assert!(decode(&text, &cache).is_err());
        let ico = dir.path().join("poster.ico");
        image::RgbaImage::from_pixel(16, 16, image::Rgba([1, 2, 3, 255]))
            .save_with_format(&ico, image::ImageFormat::Ico)
            .unwrap();
        assert!(decode(&ico, &cache).unwrap_err().contains("Unsupported"));
        assert!(decode(&dir.path().join("gone.png"), &cache).is_err());
    }
}
