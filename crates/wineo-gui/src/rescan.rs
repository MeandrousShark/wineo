//! The periodic library and trash rescan. Every message rebuilds and redraws
//! the whole window, so a rescan only reaches `update` when its result
//! differs from what the app shows.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iced::futures::Stream;
use wineo_core::paths::Paths;

use crate::Message;
use crate::library::{Card, load_library};
use crate::sidebar::TrashBadge;
use crate::trash_page::{Snapshot, entry_label};

pub(crate) const INTERVAL: Duration = Duration::from_secs(2);

/// What the app last applied from a library or trash read, whichever path
/// it came from, so the rescan compares against the screen rather than its
/// own last result.
#[derive(Default)]
struct Shown {
    library: Option<Result<Vec<Card>, String>>,
    badge: Option<TrashBadge>,
    /// The trash page's snapshot, with its "removed N ago" labels as of
    /// when it was applied.
    trash: Option<(Result<Snapshot, String>, Vec<String>)>,
}

#[derive(Clone, Default)]
pub(crate) struct Watch(Arc<Mutex<Shown>>);

impl Watch {
    fn shown(&self) -> std::sync::MutexGuard<'_, Shown> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn show_library(&self, library: Result<Vec<Card>, String>) {
        self.shown().library = Some(library);
    }

    pub(crate) fn show_badge(&self, badge: TrashBadge) {
        self.shown().badge = Some(badge);
    }

    pub(crate) fn show_trash(&self, snapshot: Result<Snapshot, String>, now: jiff::Timestamp) {
        let labels = labels(&snapshot, now);
        self.shown().trash = Some((snapshot, labels));
    }

    /// Keeps only the parts of a scan that differ from what is shown. Nothing
    /// is reported before the app has shown a first result, since its own
    /// first load is still on the way.
    pub(crate) fn changes(&self, scan: Scan, now: jiff::Timestamp) -> Option<Scan> {
        let shown = self.shown();
        let library = scan
            .library
            .filter(|library| shown.library.as_ref().is_some_and(|shown| shown != library));
        let trash = scan.trash.filter(|trash| match trash {
            Trash::Badge(badge) => shown.badge.as_ref().is_some_and(|shown| shown != badge),
            Trash::Page(snapshot) => shown.trash.as_ref().is_some_and(|(shown, labels)| {
                !same_snapshot(shown, snapshot) || *labels != self::labels(snapshot, now)
            }),
        });
        (library.is_some() || trash.is_some()).then_some(Scan { library, trash })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Scan {
    pub(crate) library: Option<Result<Vec<Card>, String>>,
    pub(crate) trash: Option<Trash>,
}

#[derive(Debug, Clone)]
pub(crate) enum Trash {
    Badge(TrashBadge),
    /// The trash page reads the whole snapshot, which also gives the badge.
    Page(Result<Snapshot, String>),
}

/// The subscription's identity. Opening or leaving the trash page restarts
/// it, since that changes what is read.
pub(crate) struct Rescan {
    pub(crate) paths: Paths,
    pub(crate) watch: Watch,
    pub(crate) trash_page: bool,
}

impl Hash for Rescan {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.trash_page.hash(state);
    }
}

pub(crate) fn read(paths: &Paths, trash_page: bool) -> Scan {
    Scan {
        library: Some(load_library(paths).map_err(|error| error.to_string())),
        trash: Some(if trash_page {
            Trash::Page(crate::trash_work::read(paths))
        } else {
            Trash::Badge(TrashBadge::read(paths))
        }),
    }
}

/// Sleeps first: a new stream starts at boot, whose load is already running,
/// and on refocus, which reloads explicitly.
pub(crate) fn stream(rescan: &Rescan) -> impl Stream<Item = Message> + use<> {
    let paths = rescan.paths.clone();
    let watch = rescan.watch.clone();
    let trash_page = rescan.trash_page;
    iced::stream::channel(1, async move |mut output| {
        use iced::futures::SinkExt;
        loop {
            tokio::time::sleep(INTERVAL).await;
            let paths = paths.clone();
            let Ok(scan) = tokio::task::spawn_blocking(move || read(&paths, trash_page)).await
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
mod tests {
    use std::fs;

    use iced::futures::StreamExt;
    use wineo_core::runtime::State;
    use wineo_core::trash::{self, PendingEntry};

    use super::*;
    use crate::App;
    use crate::sidebar::Section;
    use crate::tests::card;

    fn fixture() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                .unwrap();
        (root, App::boot(paths).0)
    }

    fn native(paths: &Paths, name: &str) {
        let config = paths.environment_config(name);
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
    }

    fn library(paths: &Paths) -> Result<Vec<Card>, String> {
        load_library(paths).map_err(|error| error.to_string())
    }

    /// The app after its first library and badge loads.
    fn shown(names: &[&str]) -> (tempfile::TempDir, App) {
        let (root, mut app) = fixture();
        for name in names {
            native(&app.paths, name);
        }
        let _ = app.update(Message::Loaded(library(&app.paths)));
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        (root, app)
    }

    fn now() -> jiff::Timestamp {
        jiff::Timestamp::now()
    }

    fn names(app: &App) -> Vec<&str> {
        app.cards.iter().map(|card| card.name.as_str()).collect()
    }

    #[test]
    fn nothing_is_reported_before_the_first_load_is_shown() {
        let (_root, app) = fixture();
        native(&app.paths, "game");
        assert!(app.shown.changes(read(&app.paths, false), now()).is_none());
        assert!(app.shown.changes(read(&app.paths, true), now()).is_none());
    }

    #[test]
    fn an_unchanged_rescan_reports_nothing_and_a_change_is_applied() {
        let (_root, mut app) = shown(&["game", "old"]);
        assert!(app.shown.changes(read(&app.paths, false), now()).is_none());
        trash::remove(&app.paths, "old", false).unwrap();
        let scan = app.shown.changes(read(&app.paths, false), now()).unwrap();
        assert!(scan.library.is_some());
        assert!(matches!(scan.trash, Some(Trash::Badge(_))));
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        assert_eq!(names(&app), ["game"]);
        assert_eq!(app.trash_badge.count, Some(1));
        assert!(app.shown.changes(read(&app.paths, false), now()).is_none());
    }

    #[test]
    fn only_the_changed_part_is_reported() {
        let (_root, app) = shown(&["game"]);
        native(&app.paths, "new");
        let scan = app.shown.changes(read(&app.paths, false), now()).unwrap();
        assert!(scan.library.is_some());
        assert!(scan.trash.is_none());
    }

    #[test]
    fn every_card_field_counts_as_a_change() {
        let watch = Watch::default();
        let base = card("game", State::Inactive, 0);
        watch.show_library(Ok(vec![base.clone()]));
        let scan = |cards: Vec<Card>| Scan {
            library: Some(Ok(cards)),
            trash: None,
        };
        assert!(watch.changes(scan(vec![base.clone()]), now()).is_none());
        let mut running = base.clone();
        running.state = State::Running;
        let mut hook = base.clone();
        hook.busy_holder = Some(wineo_core::operation::Operation::Hook(
            wineo_core::hooks::Phase::After,
        ));
        let mut art = base.clone();
        art.art = Some(crate::tests::art_file("/tmp/grid.png", 20));
        let mut renamed = base.clone();
        renamed.display_title = "Game".into();
        for changed in [running, hook, art, renamed] {
            assert!(watch.changes(scan(vec![changed]), now()).is_some());
        }
        assert!(
            watch
                .changes(
                    Scan {
                        library: Some(Err("unreadable".into())),
                        trash: None,
                    },
                    now(),
                )
                .is_some()
        );
    }

    #[test]
    fn a_shown_error_is_replaced_once_the_library_reads_again() {
        let (_root, mut app) = shown(&["game"]);
        let _ = app.update(Message::Loaded(Err("broken".into())));
        let scan = app.shown.changes(read(&app.paths, false), now()).unwrap();
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        assert!(app.error.is_none());
    }

    #[test]
    fn the_trash_page_compares_its_entries_and_their_age_labels() {
        let (_root, mut app) = shown(&["a", "b"]);
        trash::remove(&app.paths, "a", false).unwrap();
        let _ = app.update(Message::Section(Section::Trash));
        let snapshot = crate::trash_work::read(&app.paths);
        let _ = app.update(Message::Trash(crate::trash_page::Event::Loaded(
            app.trash_generation,
            snapshot.clone(),
        )));
        let page = |snapshot: Result<Snapshot, String>| Scan {
            library: None,
            trash: Some(Trash::Page(snapshot)),
        };
        let shown_at = now();
        assert!(
            app.shown
                .changes(page(snapshot.clone()), shown_at)
                .is_none()
        );
        let later = shown_at + jiff::SignedDuration::from_secs(120);
        assert!(app.shown.changes(page(snapshot.clone()), later).is_some());

        let pending = |ids: &[&str]| {
            let mut snapshot = snapshot.clone().unwrap();
            snapshot.pending = ids
                .iter()
                .map(|id| PendingEntry {
                    id: (*id).into(),
                    name: (*id).into(),
                    finished: false,
                })
                .collect();
            Ok(snapshot)
        };
        app.shown.show_trash(pending(&["x", "y"]), shown_at);
        assert!(
            app.shown
                .changes(page(pending(&["y", "x"])), shown_at)
                .is_none()
        );
        assert!(app.shown.changes(page(pending(&["x"])), shown_at).is_some());

        trash::remove(&app.paths, "b", false).unwrap();
        let scan = app.shown.changes(read(&app.paths, true), now()).unwrap();
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        let Some(Ok(snapshot)) = &app.trash_page.snapshot else {
            panic!("the page shows its snapshot");
        };
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(app.trash_badge.count, Some(2));
    }

    #[test]
    fn an_explicit_reload_in_flight_wins_and_the_change_is_not_lost() {
        let (_root, mut app) = shown(&["game"]);
        native(&app.paths, "new");
        let scan = app.shown.changes(read(&app.paths, false), now()).unwrap();
        let _ = app.update(Message::Refresh);
        let generation = app.library_generation;
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        assert!(app.refreshing, "the explicit reload is still awaited");
        assert_eq!(app.library_generation, generation);
        assert_eq!(names(&app), ["game"]);
        // The dropped result still differs from the screen, so it comes again.
        assert!(app.shown.changes(read(&app.paths, false), now()).is_some());
        let _ = app.update(Message::LibraryReloaded(generation, library(&app.paths)));
        assert_eq!(names(&app), ["game", "new"]);
        assert!(app.shown.changes(read(&app.paths, false), now()).is_none());
    }

    #[test]
    fn unfocused_pauses_the_rescan_and_focus_reloads_at_once() {
        let (_root, mut app) = shown(&["game"]);
        assert!(app.focused, "focused until the window says otherwise");
        let _ = app.update(Message::Window(iced::window::Event::Focused));
        assert!(
            !app.refreshing && !app.trash_refreshing,
            "no reload without a change of focus"
        );
        let _ = app.update(Message::Window(iced::window::Event::Unfocused));
        assert!(!app.focused);
        assert!(!app.refreshing && !app.trash_refreshing);
        let _ = app.update(Message::Window(iced::window::Event::Focused));
        assert!(app.focused);
        assert!(app.refreshing && app.trash_refreshing);
    }

    fn paused<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn the_stream_stays_silent_until_something_changes() {
        let (_root, app) = shown(&["game"]);
        let rescan = Rescan {
            paths: app.paths.clone(),
            watch: app.shown.clone(),
            trash_page: false,
        };
        paused(async {
            let mut stream = Box::pin(stream(&rescan));
            let quiet = tokio::time::timeout(INTERVAL * 5, stream.next()).await;
            assert!(quiet.is_err(), "unchanged rescans send nothing");
            native(&app.paths, "new");
            let message = tokio::time::timeout(INTERVAL * 2, stream.next())
                .await
                .expect("a change arrives within one interval")
                .unwrap();
            let Message::Rescanned(scan) = message else {
                panic!("unexpected message");
            };
            assert!(scan.library.is_some() && scan.trash.is_none());
        });
    }
}
