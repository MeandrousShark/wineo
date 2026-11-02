use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;
use crate::environment;
use crate::library::{Recorded, load};

struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
    global: GlobalConfig,
}

impl Fixture {
    fn new() -> Self {
        Self::with(false)
    }

    fn with(portable: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(&home)),
            portable.then(|| root.path().join("portable")).as_deref(),
        )
        .unwrap();
        fs::create_dir_all(steam_root(&paths).join("steamapps")).unwrap();
        Self {
            _root: root,
            paths,
            global: GlobalConfig::default(),
        }
    }

    fn on() -> Self {
        let fixture = Self::new();
        library::set_auto_import(&fixture.paths, true).unwrap();
        fixture
    }

    fn game(&self, id: &str, title: &str) -> PendingGame {
        fs::write(
            steam_root(&self.paths).join(format!("steamapps/appmanifest_{id}.acf")),
            format!(r#""AppState" {{ "appid" "{id}" "name" "{title}" "StateFlags" "4" }}"#),
        )
        .unwrap();
        PendingGame {
            app_id: id.to_owned(),
            title: title.to_owned(),
        }
    }

    fn run(&self, games: &[PendingGame]) -> Report {
        run(&self.paths, &self.global, games, &Token::new(), |_| {})
    }

    fn reason(&self, id: &str) -> Option<Recorded> {
        load(&self.paths).steam.games.get(id).copied()
    }

    fn environments(&self) -> Vec<String> {
        environment::names(&self.paths).unwrap()
    }

    /// An environment already holding `name`, so importing onto it fails.
    fn taken(&self, name: &str) {
        let config = self.paths.environment_config(name);
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
    }

    /// Runs `body` with `library.toml`'s folder read-only, so a write fails
    /// and a read doesn't.
    fn read_only<T>(&self, body: impl FnOnce() -> T) -> T {
        let dir = self.paths.library_file().parent().unwrap().to_owned();
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let result = body();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        result
    }
}

fn steam_root(paths: &Paths) -> PathBuf {
    paths.home().join(".local/share/Steam")
}

fn config_text(paths: &Paths, name: &str) -> String {
    fs::read_to_string(paths.environment_config(name)).unwrap()
}

#[test]
fn pending_games_become_environments_and_the_record_says_imported() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One"), fixture.game("20", "Two")];
    let report = fixture.run(&games);
    assert_eq!(report.imported, ["10", "20"]);
    assert!(report.failures.is_empty() && report.stopped.is_none() && !report.canceled);
    assert_eq!(fixture.reason("10"), Some(Recorded::Imported));
    assert_eq!(fixture.reason("20"), Some(Recorded::Imported));
    let names = fixture.environments();
    assert_eq!(names.len(), 2, "{names:?}");
    let text = config_text(&fixture.paths, &names[0]);
    assert!(text.contains("kind = \"proxied\""), "{text}");
    assert!(text.contains("desktop = false"), "{text}");
}

#[test]
fn off_writes_nothing_at_all() {
    let fixture = Fixture::new();
    fixture.taken("placeholder");
    let games = [fixture.game("10", "One")];
    assert!(!fixture.paths.library_file().exists());
    assert_eq!(fixture.run(&games), Report::default());
    assert!(!fixture.paths.library_file().exists());

    library::set_auto_import(&fixture.paths, false).unwrap();
    library::mark_baselined(&fixture.paths, "/somewhere").unwrap();
    let before = fs::read(fixture.paths.library_file()).unwrap();
    assert_eq!(fixture.run(&games), Report::default());
    assert_eq!(fs::read(fixture.paths.library_file()).unwrap(), before);
}

#[test]
fn a_dismissal_queued_before_the_claim_wins() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One"), fixture.game("20", "Two")];
    // The scan's list is out of date: another process dismissed this after.
    library::dismiss_game(&fixture.paths, "10").unwrap();
    let report = fixture.run(&games);
    assert_eq!(report.imported, ["20"]);
    assert_eq!(fixture.reason("10"), Some(Recorded::Dismissed));
    assert_eq!(fixture.environments().len(), 1);
}

#[test]
fn a_game_claimed_elsewhere_is_left_to_that_import() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One")];
    library::record_games(&fixture.paths, &["10".to_owned()], Recorded::Claimed).unwrap();
    assert_eq!(fixture.run(&games), Report::default());
    assert_eq!(fixture.reason("10"), Some(Recorded::Claimed));
    assert!(fixture.environments().is_empty());
}

#[test]
fn a_dismissal_after_the_claim_lets_the_import_finish_and_stands() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One")];
    library::claim_game(&fixture.paths, "10", |_| true).unwrap();
    library::dismiss_game(&fixture.paths, "10").unwrap();
    // What the import does once it holds a claim, with the dismissal between.
    let mut report = Report::default();
    import_claimed(
        &fixture.paths,
        &fixture.global,
        &games[0],
        &Token::new(),
        &mut report,
    );
    assert_eq!(report.imported, ["10"]);
    assert_eq!(fixture.environments().len(), 1);
    assert_eq!(fixture.reason("10"), Some(Recorded::Dismissed));
}

#[test]
fn a_failed_import_goes_back_to_pending_and_a_dismissal_survives_its_cleanup() {
    let fixture = Fixture::on();
    let game = fixture.game("10", "One");
    fixture.taken(&default_name("One", None));
    let report = fixture.run(std::slice::from_ref(&game));
    assert!(report.imported.is_empty());
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].app_id, "10");
    assert_eq!(fixture.reason("10"), None, "the claim was taken back");

    library::claim_game(&fixture.paths, "10", |_| true).unwrap();
    library::dismiss_game(&fixture.paths, "10").unwrap();
    let mut report = Report::default();
    import_claimed(
        &fixture.paths,
        &fixture.global,
        &game,
        &Token::new(),
        &mut report,
    );
    assert_eq!(report.failures.len(), 1);
    assert_eq!(fixture.reason("10"), Some(Recorded::Dismissed));
}

#[test]
fn a_record_that_cant_be_written_imports_nothing_and_stops_at_once() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One"), fixture.game("20", "Two")];
    let mut seen = 0;
    let report = fixture.read_only(|| {
        run(
            &fixture.paths,
            &fixture.global,
            &games,
            &Token::new(),
            |_| seen += 1,
        )
    });
    assert!(report.stopped.is_some());
    assert!(!report.claimed());
    assert_eq!(seen, 0, "the first failure ends the pass");
    assert!(fixture.environments().is_empty());
    assert_eq!(load(&fixture.paths).steam.games.len(), 0);
}

#[test]
fn an_unusable_library_file_imports_nothing() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One")];
    fs::write(fixture.paths.library_file(), "[steam\nbroken").unwrap();
    let report = fixture.run(&games);
    assert!(report.stopped.is_some());
    assert!(fixture.environments().is_empty());
}

#[test]
fn turning_it_off_during_a_pass_stops_at_the_next_game() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One"), fixture.game("20", "Two")];
    let report = run(
        &fixture.paths,
        &fixture.global,
        &games,
        &Token::new(),
        |id| {
            if id == "10" {
                library::set_auto_import(&fixture.paths, false).unwrap();
            }
        },
    );
    assert_eq!(report.imported, ["10"]);
    assert_eq!(fixture.reason("20"), None, "no claim was written");
    assert_eq!(fixture.environments().len(), 1);
}

#[test]
fn cancelling_stops_at_the_next_game_and_leaves_no_claim() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "One"), fixture.game("20", "Two")];
    let token = Token::new();
    let report = run(&fixture.paths, &fixture.global, &games, &token, |_| {
        token.cancel()
    });
    assert_eq!(report.imported, ["10"]);
    assert!(report.canceled);
    assert_eq!(fixture.reason("20"), None);
}

#[test]
fn a_cancel_landing_after_the_claim_takes_it_back_without_blaming_the_game() {
    let fixture = Fixture::on();
    let game = fixture.game("10", "One");
    library::claim_game(&fixture.paths, "10", |_| true).unwrap();
    let token = Token::new();
    token.cancel();
    let mut report = Report::default();
    import_claimed(&fixture.paths, &fixture.global, &game, &token, &mut report);
    assert!(report.canceled);
    assert!(report.failures.is_empty());
    assert_eq!(fixture.reason("10"), None);
}

#[test]
fn a_game_imported_meanwhile_counts_and_finishes_its_claim() {
    let fixture = Fixture::on();
    let game = fixture.game("10", "One");
    let PlanOutcome::Ready(plan) = plan(
        &fixture.paths,
        &fixture.global,
        OwnedGames::Unloaded,
        "10",
        Some("mine"),
    )
    .unwrap() else {
        panic!("nothing is imported yet");
    };
    apply(
        &fixture.paths,
        &fixture.global,
        OwnedGames::Unloaded,
        &plan,
        &Token::new(),
    )
    .unwrap();
    // A claim on a game with an environment, as a crash between the two
    // would leave.
    let text = fs::read_to_string(fixture.paths.library_file()).unwrap();
    fs::write(
        fixture.paths.library_file(),
        text.replace("\"imported\"", "\"claimed\""),
    )
    .unwrap();
    assert_eq!(fixture.reason("10"), Some(Recorded::Claimed));

    let mut report = Report::default();
    import_claimed(
        &fixture.paths,
        &fixture.global,
        &game,
        &Token::new(),
        &mut report,
    );
    assert_eq!(report.imported, ["10"]);
    assert_eq!(fixture.reason("10"), Some(Recorded::Imported));
    assert_eq!(fixture.environments(), ["mine"]);
}

#[test]
fn two_passes_racing_import_a_game_once() {
    let fixture = Arc::new(Fixture::on());
    let games: Vec<_> = (1..=8)
        .map(|id| fixture.game(&id.to_string(), &format!("Game {id}")))
        .collect();
    let games = Arc::new(games);
    let barrier = Arc::new(Barrier::new(2));
    let racers: Vec<_> = (0..2)
        .map(|_| {
            let (fixture, games, barrier) = (fixture.clone(), games.clone(), barrier.clone());
            thread::spawn(move || {
                barrier.wait();
                fixture.run(&games)
            })
        })
        .collect();
    let reports: Vec<Report> = racers.into_iter().map(|it| it.join().unwrap()).collect();
    let mut imported: Vec<_> = reports.iter().flat_map(|it| it.imported.clone()).collect();
    imported.sort();
    let mut wanted: Vec<_> = games.iter().map(|game| game.app_id.clone()).collect();
    wanted.sort();
    assert_eq!(imported, wanted, "each game by exactly one pass");
    assert_eq!(fixture.environments().len(), 8);
    assert!(reports.iter().all(|it| it.failures.is_empty()));
}

#[test]
fn a_name_collision_resolves_as_a_bulk_import_does() {
    let fixture = Fixture::on();
    let games = [fixture.game("10", "Same"), fixture.game("20", "Same")];
    let report = fixture.run(&games);
    assert_eq!(report.imported, ["10"]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].app_id, "20");
    assert_eq!(fixture.reason("20"), None);

    // What `--all` does with the same two games.
    let bulk = Fixture::new();
    let wanted = [bulk.game("10", "Same"), bulk.game("20", "Same")]
        .into_iter()
        .map(|game| super::super::Wanted {
            app_id: game.app_id,
            title: Some(game.title),
            imported_as: None,
        });
    let batch = super::super::plan_batch(
        &bulk.paths,
        &bulk.global,
        OwnedGames::Unloaded,
        wanted,
        None,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(batch.ready.len(), 1);
    assert_eq!(batch.ready[0].app_id, "10");
    assert_eq!(batch.failures[0].name, report.failures[0].name);
    assert_eq!(batch.failures[0].reason, report.failures[0].reason);
}

#[test]
fn cached_art_comes_along_and_no_network_is_needed() {
    let fixture = Fixture::on();
    let game = fixture.game("10", "One");
    let cache = steam_root(&fixture.paths).join("appcache/librarycache/10");
    fs::create_dir_all(&cache).unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(2, 3)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    fs::write(cache.join("logo.png"), bytes.into_inner()).unwrap();
    assert_eq!(fixture.run(&[game]).imported, ["10"]);
    let name = &fixture.environments()[0];
    assert!(fs::read_dir(fixture.paths.art_dir(name)).unwrap().count() > 0);
}

#[test]
fn a_portable_folder_never_tries() {
    let fixture = Fixture::with(true);
    let games = [fixture.game("10", "One")];
    library::set_auto_import(&fixture.paths, true).unwrap();
    let before = fs::read(fixture.paths.library_file()).unwrap();
    assert_eq!(fixture.run(&games), Report::default());
    assert_eq!(fs::read(fixture.paths.library_file()).unwrap(), before);
}

#[test]
fn no_steam_install_fails_the_game_and_leaves_no_claim() {
    let fixture = Fixture::on();
    let game = fixture.game("10", "One");
    fs::remove_dir_all(steam_root(&fixture.paths)).unwrap();
    let report = fixture.run(&[game]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(fixture.reason("10"), None);
}

#[test]
fn a_claim_that_cant_be_taken_back_is_reported() {
    let fixture = Fixture::on();
    let clash = fixture.game("10", "One");
    let waiting = fixture.game("20", "Two");
    fixture.taken(&default_name("One", None));
    library::claim_game(&fixture.paths, "10", |_| true).unwrap();
    library::claim_game(&fixture.paths, "20", |_| true).unwrap();
    let canceled = Token::new();
    canceled.cancel();

    let report = fixture.read_only(|| {
        let mut report = Report::default();
        let fine = Token::new();
        import_claimed(&fixture.paths, &fixture.global, &clash, &fine, &mut report);
        import_claimed(
            &fixture.paths,
            &fixture.global,
            &waiting,
            &canceled,
            &mut report,
        );
        report
    });
    assert_eq!(report.unreleased, ["10", "20"]);
    assert_eq!(report.failures.len(), 1, "only a real failure is one");
    assert!(report.canceled);
    assert_eq!(fixture.reason("10"), Some(Recorded::Claimed));
    assert_eq!(fixture.reason("20"), Some(Recorded::Claimed));
}
