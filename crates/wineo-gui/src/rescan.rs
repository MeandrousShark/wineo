//! The periodic library, trash, and Art page rescan. Every message rebuilds
//! and redraws the whole window, so a rescan only reaches `update` when its
//! result differs from what the app shows.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iced::futures::Stream;
use wineo_core::paths::Paths;

use crate::Message;
use crate::art_page::{self, EnvironmentIdentity};
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
        (library.is_some() || trash.is_some() || art.is_some()).then_some(Scan {
            revisions: scan.revisions,
            library,
            trash,
            art,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Scan {
    /// What was shown when the read started.
    pub(crate) revisions: Revisions,
    pub(crate) library: Option<Result<Vec<Card>, String>>,
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

pub(crate) fn read(paths: &Paths, watch: &Watch, trash_page: bool, art_page: Option<&str>) -> Scan {
    let revisions = watch.revisions();
    Scan {
        revisions,
        library: Some(load_library(paths).map_err(|error| error.to_string())),
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

    fn rescan(app: &App, trash_page: bool) -> Scan {
        read(&app.paths, &app.shown, trash_page, app.art_page.as_deref())
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
        assert!(app.shown.changes(rescan(&app, false), now()).is_none());
        assert!(app.shown.changes(rescan(&app, true), now()).is_none());
    }

    #[test]
    fn an_unchanged_rescan_reports_nothing_and_a_change_is_applied() {
        let (_root, mut app) = shown(&["game", "old"]);
        assert!(app.shown.changes(rescan(&app, false), now()).is_none());
        trash::remove(&app.paths, "old", false).unwrap();
        let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
        assert!(scan.library.is_some());
        assert!(matches!(scan.trash, Some(Trash::Badge(_))));
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        assert_eq!(names(&app), ["game"]);
        assert_eq!(app.trash_badge.count, Some(1));
        assert!(app.shown.changes(rescan(&app, false), now()).is_none());
    }

    #[test]
    fn only_the_changed_part_is_reported() {
        let (_root, app) = shown(&["game"]);
        native(&app.paths, "new");
        let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
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
            ..Scan::default()
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
                        ..Scan::default()
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
        let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
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
            trash: Some(Trash::Page(snapshot)),
            ..Scan::default()
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
        let scan = app.shown.changes(rescan(&app, true), now()).unwrap();
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
        let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
        let _ = app.update(Message::Refresh);
        let generation = app.library_generation;
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        assert!(app.refreshing, "the explicit reload is still awaited");
        assert_eq!(app.library_generation, generation);
        assert_eq!(names(&app), ["game"]);
        // The dropped result still differs from the screen, so it comes again.
        assert!(app.shown.changes(rescan(&app, false), now()).is_some());
        let _ = app.update(Message::LibraryReloaded(generation, library(&app.paths)));
        assert_eq!(names(&app), ["game", "new"]);
        assert!(app.shown.changes(rescan(&app, false), now()).is_none());
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

    #[test]
    fn a_scan_read_before_an_explicit_reload_finished_is_dropped() {
        let (_root, mut app) = shown(&["game"]);
        let stale = rescan(&app, false);
        native(&app.paths, "new");
        let _ = app.update(Message::Refresh);
        let _ = app.update(Message::LibraryReloaded(
            app.library_generation,
            library(&app.paths),
        ));
        assert_eq!(names(&app), ["game", "new"]);
        let stale = app.shown.changes(stale, now()).unwrap();
        assert!(!app.refreshing);
        let _ = app.update(Message::Rescanned(Box::new(stale)));
        assert_eq!(names(&app), ["game", "new"], "the newer reload stays");
        // A change after the dropped scan still arrives with the next one.
        native(&app.paths, "later");
        let next = app.shown.changes(rescan(&app, false), now()).unwrap();
        let _ = app.update(Message::Rescanned(Box::new(next)));
        assert_eq!(names(&app), ["game", "later", "new"]);
    }

    #[test]
    fn any_explicit_reload_makes_earlier_scans_stale_when_it_starts() {
        let (_root, mut app) = shown(&["game"]);
        native(&app.paths, "new");
        let stale = app.shown.changes(rescan(&app, false), now()).unwrap();
        // Like an Art save, which reloads without marking `refreshing`.
        let _ = app.reload_library();
        assert!(!app.refreshing);
        let _ = app.update(Message::Rescanned(Box::new(stale)));
        assert_eq!(names(&app), ["game"], "the scan predates the reload");

        let (_root, mut app) = shown(&["game", "old"]);
        trash::remove(&app.paths, "old", false).unwrap();
        let stale = app.shown.changes(rescan(&app, false), now()).unwrap();
        let _ = app.reload_badge();
        app.trash_refreshing = false;
        let _ = app.update(Message::Rescanned(Box::new(stale)));
        assert_eq!(app.trash_badge.count, Some(0));
        let stale = app.shown.changes(rescan(&app, false), now()).unwrap();
        let _ = app.reload_trash_page();
        app.trash_refreshing = false;
        let _ = app.update(Message::Rescanned(Box::new(stale)));
        assert_eq!(app.trash_badge.count, Some(0));
    }

    #[test]
    fn only_one_scan_reads_at_a_time_and_a_busy_tick_skips() {
        let (_root, app) = shown(&["game"]);
        let guard = app.shown.begin_scan().unwrap();
        assert!(app.shown.begin_scan().is_none());
        let rescan = Rescan {
            paths: app.paths.clone(),
            watch: app.shown.clone(),
            trash_page: false,
            art_page: None,
        };
        paused(async {
            let mut stream = Box::pin(stream(&rescan));
            native(&app.paths, "new");
            let skipped = tokio::time::timeout(INTERVAL * 3, stream.next()).await;
            assert!(skipped.is_err(), "ticks skip while another read runs");
            // The read finishing elsewhere, as a dropped stream's would.
            std::thread::spawn(move || drop(guard)).join().unwrap();
            let message = tokio::time::timeout(INTERVAL * 2, stream.next())
                .await
                .expect("the next tick scans again")
                .unwrap();
            assert!(matches!(message, Message::Rescanned(_)));
        });
        assert!(app.shown.begin_scan().is_some(), "released after the read");
    }

    fn art_app() -> (tempfile::TempDir, App) {
        let (root, mut app) = fixture();
        wineo_core::environment::Environment::create_native(
            &app.paths,
            &wineo_core::config::GlobalConfig::default(),
            "game",
            None,
            false,
        )
        .unwrap();
        let _ = app.update(Message::Loaded(library(&app.paths)));
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        app.selected = Some("game".into());
        let _ = app.update(Message::OpenArt);
        assert_eq!(app.art_page.as_deref(), Some("game"));
        load_art(&mut app);
        (root, app)
    }

    /// Delivers the Art page's load, as its task would.
    fn load_art(app: &mut App) {
        let page = &app.art_pages["game"];
        let event = art_page::Event::Loaded(
            page.load_sequence,
            art_page::load_snapshot(&app.paths, "game"),
        );
        let _ = app.update(Message::ArtPage("game".into(), page.generation, event));
    }

    #[test]
    fn an_open_art_page_sees_icon_and_game_only_changes() {
        let (_root, mut app) = art_app();
        assert!(app.shown.changes(rescan(&app, false), now()).is_none());
        fs::create_dir_all(app.paths.art_dir("game")).unwrap();
        let icon = app.paths.art_dir("game").join("icon.png");
        let edits: [&dyn Fn(&App); 3] = [
            &|_| fs::write(&icon, b"one").unwrap(),
            &|_| fs::write(&icon, b"other").unwrap(),
            &|app| wineo_core::art::save_game(&app.paths, "game", 42).unwrap(),
        ];
        for edit in edits {
            edit(&app);
            let changed = app.shown.changes(rescan(&app, false), now()).unwrap();
            assert!(changed.library.is_none(), "only the Art page changed");
            assert!(changed.art.is_some());
            let sequence = app.art_pages["game"].load_sequence;
            let _ = app.update(Message::Rescanned(Box::new(changed)));
            assert_ne!(app.art_pages["game"].load_sequence, sequence, "it reloads");
            load_art(&mut app);
            assert!(app.shown.changes(rescan(&app, false), now()).is_none());
        }
    }

    #[test]
    fn a_recreated_environment_closes_its_art_page() {
        let (_root, mut app) = art_app();
        let config = app.paths.environment_config("game");
        let text = fs::read(&config).unwrap();
        fs::remove_file(&config).unwrap();
        fs::write(&config, text).unwrap();
        let changed = app.shown.changes(rescan(&app, false), now()).unwrap();
        let _ = app.update(Message::Rescanned(Box::new(changed)));
        assert!(app.art_page.is_none());
    }

    #[test]
    fn input_counts_as_focus_when_no_focused_event_comes() {
        use iced::window::Event::{Focused, Unfocused};
        let press =
            || iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left));
        let key = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
            modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        });
        let moved = iced::Event::Mouse(iced::mouse::Event::CursorMoved {
            position: iced::Point::ORIGIN,
        });
        assert!(
            crate::window_or_press(moved).is_none(),
            "only presses count"
        );
        assert!(matches!(
            crate::window_or_press(key),
            Some(Message::Pressed)
        ));
        // Events reach update in the order they happened, through one mapping.
        let run = |events: Vec<iced::Event>| {
            let (_root, mut app) = shown(&["game"]);
            for event in events {
                let _ = app.update(crate::window_or_press(event).unwrap());
            }
            (app.focused, app.refreshing && app.trash_refreshing)
        };
        let window = iced::Event::Window;
        assert_eq!(
            run(vec![window(Focused), window(Unfocused), press()]),
            (true, true),
            "a press after losing focus counts as focus"
        );
        assert_eq!(
            run(vec![window(Focused), press(), window(Unfocused)]),
            (false, false),
            "a press before losing focus doesn't"
        );
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
            art_page: None,
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
