//! The periodic library, trash, and Art page rescan. Every message rebuilds
//! and redraws the whole window, so a rescan only reaches `update` when its
//! result differs from what the app shows.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iced::futures::Stream;
use wineo_core::library::{self as saved, Problem};
use wineo_core::paths::Paths;

use crate::Message;
use crate::art_page::{self, EnvironmentIdentity};
use crate::library::{Card, load_library_with};
use crate::sidebar::TrashBadge;
use crate::trash_page::{Snapshot, entry_label};

pub(crate) const INTERVAL: Duration = Duration::from_secs(2);

/// What the app last applied from a library or trash read, whichever path
/// it came from, so the rescan compares against the screen rather than its
/// own last result.
#[derive(Default)]
struct Shown {
    library: Option<Result<Vec<Card>, String>>,
    /// `library.toml`'s problem, which only rescans read.
    problem: Option<Problem>,
    badge: Option<TrashBadge>,
    /// The trash page's snapshot, with its "removed N ago" labels as of
    /// when it was applied.
    trash: Option<(Result<Snapshot, String>, Vec<String>)>,
    /// The open Art page's name, its environment identity, and what it
    /// last loaded.
    art: Option<(String, EnvironmentIdentity, ArtSnapshot)>,
    revisions: Revisions,
}

type ArtSnapshot = Result<art_page::Snapshot, String>;

/// Bumped on every write and when an explicit reload starts, so a scan read
/// before that reload started can't replace its result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Revisions {
    library: u64,
    trash: u64,
    art: u64,
}

// The bound on in-flight work: at most one periodic blocking scan runs at a
// time, across subscription restarts, and every explicit reload makes scans
// read before it started stale.
#[derive(Clone, Default)]
pub(crate) struct Watch(Arc<Inner>);

#[derive(Default)]
struct Inner {
    shown: Mutex<Shown>,
    scanning: AtomicBool,
    caches: Mutex<RescanCaches>,
}

/// What rescans keep between reads, so a quiet one skips files that didn't
/// change. Held by the `Watch`, so it outlives subscription restarts.
#[derive(Default)]
pub(crate) struct RescanCaches {
    library: saved::Cache,
    stats: wineo_core::stats::Cache,
}

/// Held by the one running scan, released when its read finishes even if
/// the stream that started it is gone.
pub(crate) struct ScanGuard(Watch);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.0.0.scanning.store(false, Ordering::Release);
    }
}

impl Watch {
    fn shown(&self) -> std::sync::MutexGuard<'_, Shown> {
        self.0.shown.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// None while an earlier scan is still reading, so a tick skips rather
    /// than queues.
    pub(crate) fn begin_scan(&self) -> Option<ScanGuard> {
        (!self.0.scanning.swap(true, Ordering::AcqRel)).then(|| ScanGuard(self.clone()))
    }

    pub(crate) fn reloading_library(&self) {
        self.shown().revisions.library += 1;
    }

    pub(crate) fn reloading_trash(&self) {
        self.shown().revisions.trash += 1;
    }

    pub(crate) fn revisions(&self) -> Revisions {
        self.shown().revisions
    }

    pub(crate) fn show_library(&self, library: Result<Vec<Card>, String>) {
        let mut shown = self.shown();
        shown.library = Some(library);
        shown.revisions.library += 1;
    }

    pub(crate) fn show_problem(&self, problem: Option<Problem>) {
        self.shown().problem = problem;
    }

    pub(crate) fn show_badge(&self, badge: TrashBadge) {
        let mut shown = self.shown();
        shown.badge = Some(badge);
        shown.revisions.trash += 1;
    }

    pub(crate) fn show_trash(&self, snapshot: Result<Snapshot, String>, now: jiff::Timestamp) {
        let labels = labels(&snapshot, now);
        let mut shown = self.shown();
        shown.trash = Some((snapshot, labels));
        shown.revisions.trash += 1;
    }

    pub(crate) fn show_art(&self, name: String, identity: EnvironmentIdentity, art: ArtSnapshot) {
        let mut shown = self.shown();
        shown.art = Some((name, identity, art));
        shown.revisions.art += 1;
    }

    pub(crate) fn forget_art(&self) {
        let mut shown = self.shown();
        shown.art = None;
        shown.revisions.art += 1;
    }

    /// Drops the parts of `scan` read before a newer result was shown. The
    /// next scan compares against that result, so no change is lost.
    pub(crate) fn current(&self, mut scan: Scan) -> Scan {
        let now = self.revisions();
        if scan.revisions.library != now.library {
            scan.library = None;
            scan.problem = None;
        }
        if scan.revisions.trash != now.trash {
            scan.trash = None;
        }
        if scan.revisions.art != now.art {
            scan.art = None;
        }
        scan
    }

    /// Keeps only the parts of a scan that differ from what is shown. Nothing
    /// is reported before the app has shown a first result, since its own
    /// first load is still on the way.
    pub(crate) fn changes(&self, scan: Scan, now: jiff::Timestamp) -> Option<Scan> {
        let shown = self.shown();
        let library = scan
            .library
            .filter(|library| shown.library.as_ref().is_some_and(|shown| shown != library));
        let problem = scan
            .problem
            .filter(|problem| shown.library.is_some() && shown.problem != *problem);
        let trash = scan.trash.filter(|trash| match trash {
            Trash::Badge(badge) => shown.badge.as_ref().is_some_and(|shown| shown != badge),
            Trash::Page(snapshot) => shown.trash.as_ref().is_some_and(|(shown, labels)| {
                !same_snapshot(shown, snapshot) || *labels != self::labels(snapshot, now)
            }),
        });
        let art = scan.art.filter(|art| {
            shown
                .art
                .as_ref()
                .is_some_and(|(name, identity, snapshot)| {
                    *name == art.name
                        && (art.identity != Some(*identity) || *snapshot != art.snapshot)
                })
        });
        (library.is_some() || problem.is_some() || trash.is_some() || art.is_some()).then_some(
            Scan {
                revisions: scan.revisions,
                library,
                problem,
                trash,
                art,
            },
        )
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Scan {
    /// What was shown when the read started.
    pub(crate) revisions: Revisions,
    pub(crate) library: Option<Result<Vec<Card>, String>>,
    /// `library.toml`'s problem, read with `library`.
    pub(crate) problem: Option<Option<Problem>>,
    pub(crate) trash: Option<Trash>,
    pub(crate) art: Option<Art>,
}

/// What the open Art page reads: a recreated environment closes it, and any
/// change to its rows, saved game, or art files reloads it.
#[derive(Debug, Clone)]
pub(crate) struct Art {
    pub(crate) name: String,
    pub(crate) identity: Option<EnvironmentIdentity>,
    pub(crate) snapshot: ArtSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) enum Trash {
    Badge(TrashBadge),
    /// The trash page reads the whole snapshot, which also gives the badge.
    Page(Result<Snapshot, String>),
}

/// The subscription's identity. Opening or leaving the trash page or an Art
/// page restarts it, since that changes what is read.
pub(crate) struct Rescan {
    pub(crate) paths: Paths,
    pub(crate) watch: Watch,
    pub(crate) trash_page: bool,
    pub(crate) art_page: Option<String>,
}

impl Hash for Rescan {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.trash_page.hash(state);
        self.art_page.hash(state);
    }
}

/// A rescan through the caches `watch` holds.
pub(crate) fn read(paths: &Paths, watch: &Watch, trash_page: bool, art_page: Option<&str>) -> Scan {
    let mut caches = watch
        .0
        .caches
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    read_with(paths, watch, &mut caches, trash_page, art_page)
}

pub(crate) fn read_with(
    paths: &Paths,
    watch: &Watch,
    caches: &mut RescanCaches,
    trash_page: bool,
    art_page: Option<&str>,
) -> Scan {
    let revisions = watch.revisions();
    let saved = caches.library.read(paths);
    Scan {
        revisions,
        library: Some(
            load_library_with(paths, saved, Some(&mut caches.stats))
                .map_err(|error| error.to_string()),
        ),
        problem: Some(saved.problem.clone()),
        trash: Some(if trash_page {
            Trash::Page(crate::trash_work::read(paths))
        } else {
            Trash::Badge(TrashBadge::read(paths))
        }),
        art: art_page.map(|name| Art {
            name: name.to_owned(),
            identity: EnvironmentIdentity::read(paths, name),
            snapshot: art_page::load_snapshot(paths, name),
        }),
    }
}

/// Sleeps first: a new stream starts at boot, whose load is already running,
/// and on refocus, which reloads explicitly.
pub(crate) fn stream(rescan: &Rescan) -> impl Stream<Item = Message> + use<> {
    let paths = rescan.paths.clone();
    let watch = rescan.watch.clone();
    let trash_page = rescan.trash_page;
    let art_page = rescan.art_page.clone();
    iced::stream::channel(1, async move |mut output| {
        use iced::futures::SinkExt;
        loop {
            tokio::time::sleep(INTERVAL).await;
            let Some(guard) = watch.begin_scan() else {
                continue;
            };
            let (paths, reader, art_page) = (paths.clone(), watch.clone(), art_page.clone());
            let Ok(scan) = tokio::task::spawn_blocking(move || {
                let scan = read(&paths, &reader, trash_page, art_page.as_deref());
                drop(guard);
                scan
            })
            .await
            else {
                continue;
            };
            if let Some(scan) = watch.changes(scan, jiff::Timestamp::now())
                && output
                    .send(Message::Rescanned(Box::new(scan)))
                    .await
                    .is_err()
            {
                return;
            }
        }
    })
}

// Pending markers come back in directory order.
fn same_snapshot(a: &Result<Snapshot, String>, b: &Result<Snapshot, String>) -> bool {
    match (a, b) {
        (Ok(a), Ok(b)) => {
            let pending = |snapshot: &Snapshot| {
                let mut pending: Vec<_> = snapshot
                    .pending
                    .iter()
                    .map(|entry| (entry.id.clone(), entry.name.clone(), entry.finished))
                    .collect();
                pending.sort();
                pending
            };
            a.entries == b.entries && pending(a) == pending(b)
        }
        (Err(a), Err(b)) => a == b,
        _ => false,
    }
}

fn labels(snapshot: &Result<Snapshot, String>, now: jiff::Timestamp) -> Vec<String> {
    snapshot.as_ref().map_or_else(
        |_| Vec::new(),
        |snapshot| {
            snapshot
                .entries
                .iter()
                .map(|entry| entry_label(entry, now))
                .collect()
        },
    )
}

#[cfg(test)]
mod tests;
