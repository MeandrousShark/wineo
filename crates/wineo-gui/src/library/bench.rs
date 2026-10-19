//! Timings for the library read the 2 s rescan runs, and for rebuilding the
//! order `filtered()` reads.
//! Ignored by default: they need a generated library and an optimized build.
//!
//! Generate a library (see `scripts/generate-bench-library.sh`), then:
//!
//!   WINEO_BENCH_ROOT=/path/to/root \
//!   cargo test -p wineo-gui --profile release-test library::bench \
//!     -- --ignored --nocapture --test-threads=1
//!
//! Run one measurement by naming it instead of `library::bench`:
//! `rescan_quiet_and_busy`, `rescan_parts`, `stats_overview_cache`,
//! `filtered_search_change`, `sort_change`, or `filter_change`.
//! `WINEO_BENCH_ITERS` sets the iterations per measurement (default 100).
//!
//! The profile is release with debug assertions on for iced_core only, which
//! is what builds iced's null renderer; the numbers match `--release`. These
//! tests refuse to run in a build with debug assertions, since that would
//! time unoptimized code. Only `WINEO_BENCH_ROOT` is read, never the real
//! HOME, so a typo can't time (or append to) a real library.
//!
//! The busy case appends a session to every stats log before each timed
//! rescan and cuts the logs back afterward, so the library ends as it began.
//! Plan 0054 adds a library.toml edit touching every entry to it once that
//! file exists.

use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::stats::{self, End, Session};
use wineo_core::trash::PendingScan;

use super::{Card, load_library};
use crate::rescan::{self, Watch};

fn iterations() -> usize {
    std::env::var("WINEO_BENCH_ITERS")
        .ok()
        .and_then(|text| text.parse().ok())
        .filter(|&count| count > 0)
        .unwrap_or(100)
}

fn bench_root() -> PathBuf {
    if cfg!(debug_assertions) {
        panic!("build with --profile release-test, a debug build's timings mean nothing");
    }
    let root = PathBuf::from(
        std::env::var_os("WINEO_BENCH_ROOT").expect("set WINEO_BENCH_ROOT to a generated root"),
    );
    let root = fs::canonicalize(root).expect("WINEO_BENCH_ROOT exists");
    assert!(
        root.join(".bench-library").is_file(),
        "{} was not made by generate-bench-library.sh",
        root.display()
    );
    root
}

fn paths() -> Paths {
    let root = bench_root();
    Paths::from_lookup(|key| (key == "HOME").then(|| root.clone().into_os_string())).unwrap()
}

fn library(paths: &Paths) -> Vec<Card> {
    let cards = load_library(paths).unwrap();
    let broken = cards.iter().filter(|card| card.error.is_some()).count();
    let arts = cards.iter().filter(|card| card.art.is_some()).count();
    println!(
        "library: {} environments, {broken} with errors, {arts} with art",
        cards.len()
    );
    assert_eq!(broken, 0, "the generated library should read cleanly");
    cards
}

fn header(what: &str, iterations: usize) {
    println!(
        "\n{what}: {iterations} iterations, {} threads, optimized={}",
        std::thread::available_parallelism().map_or(0, usize::from),
        !cfg!(debug_assertions)
    );
}

/// Min, median, p90, and max of `samples`, in the unit `scale` converts a
/// duration to.
fn report(label: &str, mut samples: Vec<Duration>, unit: &str, scale: f64) {
    samples.sort();
    let at = |fraction: f64| {
        let index = ((samples.len() as f64 * fraction) as usize).min(samples.len() - 1);
        samples[index].as_secs_f64() * scale
    };
    println!(
        "{label:<34} min {:>9.3}  median {:>9.3}  p90 {:>9.3}  max {:>9.3}  {unit}",
        samples[0].as_secs_f64() * scale,
        at(0.5),
        at(0.9),
        samples[samples.len() - 1].as_secs_f64() * scale,
    );
}

fn millis(label: &str, samples: Vec<Duration>) {
    report(label, samples, "ms", 1e3);
}

fn time<T>(iterations: usize, mut work: impl FnMut() -> T) -> Vec<Duration> {
    // The first reads fill the page cache and size the allocator, and the
    // app's rescans are never the first read.
    for _ in 0..5 {
        black_box(work());
    }
    (0..iterations)
        .map(|_| {
            let start = Instant::now();
            black_box(work());
            start.elapsed()
        })
        .collect()
}

/// Cuts every stats log back to its length when this was made, even if a
/// timed block panics, so a failed run doesn't leave a longer library.
struct Logs(Vec<(PathBuf, u64)>);

impl Logs {
    /// Refuses logs that could reach outside the bench root, since this
    /// appends to and truncates them. Every log must already exist: creating
    /// one would skip these checks for its directory and leave a file behind.
    fn new(paths: &Paths, cards: &[Card]) -> Self {
        let root = bench_root();
        for card in cards {
            let log = paths.stats_log(&card.name);
            let dir = log.parent().expect("a log has a directory");
            for path in [dir, log.as_path()] {
                let meta = fs::symlink_metadata(path).unwrap_or_else(|error| {
                    panic!(
                        "can't stat {}: {error}; regenerate the bench library with \
                         scripts/generate-bench-library.sh",
                        path.display()
                    )
                });
                assert!(
                    !meta.file_type().is_symlink(),
                    "{} is a symlink, refusing to write through it",
                    path.display()
                );
                let real = fs::canonicalize(path).unwrap();
                assert!(
                    real.starts_with(&root),
                    "{} resolves to {}, outside the bench root {}",
                    path.display(),
                    real.display(),
                    root.display()
                );
            }
        }
        Self(
            cards
                .iter()
                .map(|card| paths.stats_log(&card.name))
                .map(|path| {
                    let len = fs::metadata(&path).unwrap().len();
                    (path, len)
                })
                .collect(),
        )
    }

    /// A new session in every log, so every card's playtime and session
    /// count really differ from the last read.
    fn append_session(&self, count: u64) {
        let start = jiff::Timestamp::now();
        let end = start
            .checked_add(jiff::SignedDuration::from_secs(540 + count as i64))
            .unwrap();
        for (path, _) in &self.0 {
            let session = Session::new("game".to_owned(), start, end, End::Exited);
            stats::append(path, &session).unwrap();
        }
    }
}

impl Drop for Logs {
    fn drop(&mut self) {
        for (path, len) in &self.0 {
            if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
                let _ = file.set_len(*len);
            }
        }
    }
}

/// The same read the 2 s tick does (`rescan::read`), plus the comparison
/// against what is on screen that decides whether to send a message.
#[test]
#[ignore = "needs WINEO_BENCH_ROOT and a release-test build"]
fn rescan_quiet_and_busy() {
    let paths = paths();
    let iterations = iterations();
    let cards = library(&paths);
    let watch = Watch::default();
    watch.show_library(Ok(cards.clone()));
    let now = jiff::Timestamp::now;

    header("rescan, quiet (nothing changed)", iterations);
    let quiet = time(iterations, || {
        let scan = rescan::read(&paths, &watch, false, None);
        let changes = watch.changes(scan, now());
        assert!(changes.is_none(), "a quiet rescan found a difference");
    });
    millis("quiet rescan", quiet);

    header("rescan, busy (new session in every stats log)", iterations);
    let logs = Logs::new(&paths, &cards);
    let mut busy = Vec::new();
    for round in 0..iterations + 5 {
        logs.append_session(round as u64);
        let start = Instant::now();
        let scan = rescan::read(&paths, &watch, false, None);
        let changes = watch.changes(scan, now());
        let elapsed = start.elapsed();
        let changes = changes.expect("a busy rescan should differ from the screen");
        // The app applies the change, so the next round compares against it.
        watch.show_library(changes.library.expect("the library differs"));
        if round >= 5 {
            busy.push(elapsed);
        }
    }
    millis("busy rescan", busy);
}

/// Where the read spends its time, to see whether the stats logs dominate.
/// The parts overlap what `load_library` does, so they don't sum to it
/// exactly: the overview and the card loop each load every config, and the
/// stats overview loads them again.
#[test]
#[ignore = "needs WINEO_BENCH_ROOT and a release-test build"]
fn rescan_parts() {
    let paths = paths();
    let iterations = iterations();
    let cards = library(&paths);
    let names: Vec<String> = cards.iter().map(|card| card.name.clone()).collect();

    header("rescan parts", iterations);
    millis(
        "load_library (whole library read)",
        time(iterations, || load_library(&paths).unwrap()),
    );
    millis(
        "rescan::read (adds trash badge)",
        time(iterations, || {
            rescan::read(&paths, &Watch::default(), false, None)
        }),
    );
    millis(
        "trash scan (PendingScan::new)",
        time(iterations, || PendingScan::new(&paths)),
    );
    let pending = PendingScan::new(&paths);
    millis(
        "report::overview_with_pending",
        time(iterations, || {
            report::overview_with_pending(&paths, OverviewFilter::default(), &pending).unwrap()
        }),
    );
    millis(
        "stats::overview (logs + configs)",
        time(iterations, || stats::overview(&paths).unwrap()),
    );
    let mut cache = stats::Cache::new();
    millis(
        "stats::overview_cached (warm)",
        time(iterations, || {
            stats::overview_cached(&paths, &mut cache).unwrap()
        }),
    );
    millis(
        "stats logs only (read + parse)",
        time(iterations, || {
            names
                .iter()
                .map(|name| stats::read(&paths.stats_log(name)).unwrap().sessions.len())
                .sum::<usize>()
        }),
    );
    millis(
        "stats logs only (raw file reads)",
        time(iterations, || {
            names
                .iter()
                .map(|name| fs::read(paths.stats_log(name)).unwrap().len())
                .sum::<usize>()
        }),
    );
    millis(
        "environment configs (load x all)",
        time(iterations, || {
            names
                .iter()
                .filter(|name| Environment::load_with_pending(&paths, name, &pending).is_ok())
                .count()
        }),
    );
    millis(
        "GOG folder states",
        time(iterations, || crate::gog_library::Folders::read(&paths)),
    );
    let bytes: u64 = names
        .iter()
        .map(|name| fs::metadata(paths.stats_log(name)).map_or(0, |meta| meta.len()))
        .sum();
    println!(
        "stats logs total {:.1} MiB",
        bytes as f64 / (1 << 20) as f64
    );
}

/// `stats::overview` against `stats::overview_cached`, the stats part of the
/// rescan. Quiet: nothing changed and the cache is warm. Busy: a new session
/// in every log before each call, so the cache re-parses all of them.
#[test]
#[ignore = "needs WINEO_BENCH_ROOT and a release-test build"]
fn stats_overview_cache() {
    let paths = paths();
    let iterations = iterations();
    let cards = library(&paths);

    header("stats overview, quiet (nothing changed)", iterations);
    millis(
        "stats::overview",
        time(iterations, || stats::overview(&paths).unwrap()),
    );
    let mut cache = stats::Cache::new();
    millis(
        "stats::overview_cached",
        time(iterations, || {
            stats::overview_cached(&paths, &mut cache).unwrap()
        }),
    );

    header(
        "stats overview, busy (new session in every log)",
        iterations,
    );
    let logs = Logs::new(&paths, &cards);
    let mut plain = Vec::new();
    let mut cached = Vec::new();
    let mut cache = stats::Cache::new();
    for round in 0..iterations + 5 {
        // One change both calls see, and the order alternates so neither
        // always gets the colder page cache.
        logs.append_session(round as u64);
        let (mut plain_elapsed, mut cached_elapsed) = (Duration::ZERO, Duration::ZERO);
        for step in 0..2 {
            if (step == 0) == (round % 2 == 0) {
                let start = Instant::now();
                black_box(stats::overview(&paths).unwrap());
                plain_elapsed = start.elapsed();
            } else {
                let start = Instant::now();
                black_box(stats::overview_cached(&paths, &mut cache).unwrap());
                cached_elapsed = start.elapsed();
            }
        }
        if round >= 5 {
            plain.push(plain_elapsed);
            cached.push(cached_elapsed);
        }
    }
    millis("stats::overview", plain);
    millis("stats::overview_cached", cached);
}

/// What a search change costs: rebuilding the cached order, which
/// `filtered()` then only reads. The cards are the real ones from the
/// generated library.
#[test]
#[ignore = "needs WINEO_BENCH_ROOT and a release-test build"]
fn filtered_search_change() {
    let paths = paths();
    let mut app = crate::tests::app();
    app.cards = library(&paths);
    // Mixes empty, broad, narrow, and no-match searches, since each lowercases
    // and scans every card.
    let searches = ["", "a", "ar", "arc", "iron", "harbor 3", "zzz", "gar", "e"];
    let iterations = iterations() * 20;

    header("order rebuild, one per search change", iterations);
    let mut samples = Vec::with_capacity(iterations);
    let mut matched = 0;
    for round in 0..iterations + 100 {
        app.search = searches[round % searches.len()].to_owned();
        let start = Instant::now();
        app.reorder();
        let elapsed = start.elapsed();
        matched += black_box(app.filtered()).len();
        if round >= 100 {
            samples.push(elapsed);
        }
    }
    black_box(matched);
    report("reorder() per search", samples, "us", 1e6);
}

/// What a sort change costs: the order rebuilt after a new key or
/// direction, with every key and both directions, favorites first off and
/// on.
#[test]
#[ignore = "needs WINEO_BENCH_ROOT and a release-test build"]
fn sort_change() {
    use wineo_core::library::SortKey;

    let paths = paths();
    let mut app = crate::tests::app();
    app.cards = library(&paths);
    app.search = String::new();
    let mut queries = Vec::new();
    for favorites_first in [false, true] {
        for sort in SortKey::ALL {
            for descending in [false, true] {
                queries.push(crate::order::Query {
                    sort,
                    descending,
                    favorites_first,
                });
            }
        }
    }
    let iterations = iterations() * 20;

    header("order rebuild, one per sort change", iterations);
    let mut samples = Vec::with_capacity(iterations);
    let mut first = 0;
    for round in 0..iterations + 100 {
        app.query = queries[round % queries.len()];
        let start = Instant::now();
        app.reorder();
        let elapsed = start.elapsed();
        first += black_box(app.filtered())
            .first()
            .map_or(0, |card| card.name.len());
        if round >= 100 {
            samples.push(elapsed);
        }
    }
    black_box(first);
    report("reorder() per sort", samples, "us", 1e6);
}

/// What a filter change costs: the order rebuilt after toggling a tag
/// filter, a platform filter, or Show hidden. Some cards are marked favorite
/// and hidden in memory so the Show hidden toggle has something to do; the
/// bench root isn't touched.
#[test]
#[ignore = "needs WINEO_BENCH_ROOT and a release-test build"]
fn filter_change() {
    use std::collections::BTreeMap;

    let paths = paths();
    let mut app = crate::tests::app();
    app.cards = library(&paths);
    app.search = String::new();
    for (index, card) in app.cards.iter_mut().enumerate() {
        card.hidden = index % 10 == 3;
        card.favorite = index % 7 == 0;
    }
    // The most common tag that doesn't cover every card, so the filter
    // really narrows.
    let mut tag_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for tag in app.cards.iter().flat_map(|card| &card.tags) {
        *tag_counts.entry(tag.as_str()).or_default() += 1;
    }
    let tag = tag_counts
        .iter()
        .filter(|&(_, &count)| count < app.cards.len())
        .max_by_key(|&(_, &count)| count)
        .map(|(tag, _)| (*tag).to_owned())
        .expect("the generated library has tags");
    // The generated library has no stores, so the platform filter is the
    // one that narrows.
    let platform = app.cards[0]
        .source
        .platform
        .map(wineo_core::source::Platform::label)
        .expect("the generated library has no Steam games");
    println!(
        "filters: tag {tag:?} on {} cards, platform {platform:?} on {} cards, {} hidden",
        tag_counts[tag.as_str()],
        app.cards
            .iter()
            .filter(|card| {
                card.source
                    .platform
                    .map(wineo_core::source::Platform::label)
                    == Some(platform)
            })
            .count(),
        app.cards.iter().filter(|card| card.hidden).count(),
    );
    let iterations = iterations() * 20;

    header("order rebuild, one per filter change", iterations);
    type Toggle = Box<dyn Fn(&mut crate::order::Filters)>;
    let toggles: [(&str, Toggle); 3] = [
        (
            "reorder() per tag filter",
            Box::new(move |filters| {
                if !filters.tags.remove(&tag) {
                    filters.tags.insert(tag.clone());
                }
            }),
        ),
        (
            "reorder() per platform filter",
            Box::new(move |filters| {
                if !filters.platforms.remove(platform) {
                    filters.platforms.insert(platform);
                }
            }),
        ),
        (
            "reorder() per Show hidden",
            Box::new(|filters| filters.show_hidden = !filters.show_hidden),
        ),
    ];
    let mut all = Vec::new();
    for (label, toggle) in &toggles {
        app.filters = crate::order::Filters::default();
        let mut samples = Vec::with_capacity(iterations);
        let mut shown = 0;
        for round in 0..iterations + 100 {
            toggle(&mut app.filters);
            let start = Instant::now();
            app.reorder();
            let elapsed = start.elapsed();
            shown += black_box(app.filtered()).len();
            if round >= 100 {
                samples.push(elapsed);
            }
        }
        black_box(shown);
        report(label, samples.clone(), "us", 1e6);
        all.extend(samples);
    }
    report("reorder() per filter, all", all, "us", 1e6);
}
