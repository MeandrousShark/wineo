use crate::test_script::write_script;
use std::ffi::OsString;
use std::fs;

use std::path::PathBuf;

use wineo_core::art::Source;
use wineo_core::gog::extras::{Filled, Playtime};
use wineo_core::gog::folder::Track;
use wineo_core::heroic::HeroicOrigin;

use super::*;

const RIMWORLD: &str = "1094900565";
const STARDEW: &str = "1453375253";
const DISCO: &str = "1771589310";

struct Fixture {
    root: tempfile::TempDir,
    paths: Paths,
    source: HeroicSource,
    installed: Vec<serde_json::Value>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                .unwrap();
        let heroic = root.path().join("heroic");
        for dir in ["GamesConfig", "gog_store", "store_cache"] {
            fs::create_dir_all(heroic.join(dir)).unwrap();
        }
        fs::write(
            heroic.join("config.json"),
            r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
        )
        .unwrap();
        fs::write(
            heroic.join("store_cache/gog_library.json"),
            format!(
                r#"{{"games":[{{"runner":"gog","app_name":"{RIMWORLD}","title":"RimWorld"}},{{"runner":"gog","app_name":"{STARDEW}","title":"Stardew Valley"}},{{"runner":"gog","app_name":"{DISCO}","title":"Disco Elysium"}}],"__timestamp":1}}"#
            ),
        )
        .unwrap();
        let source = HeroicSource::from_config_dir_with_home(heroic, root.path().to_owned());
        let fixture = Self {
            root,
            paths,
            source,
            installed: Vec::new(),
        };
        fixture.write_installed();
        fixture
    }

    fn write_installed(&self) {
        fs::write(
            self.source.config_dir().join("gog_store/installed.json"),
            serde_json::json!({ "installed": self.installed }).to_string(),
        )
        .unwrap();
    }

    fn install(&mut self, entry: serde_json::Value) {
        self.installed.push(entry);
        self.write_installed();
    }

    /// A native game, its `gameinfo` naming `id`.
    fn native(&mut self, id: &str, title: &str, dlc: &[u64]) -> PathBuf {
        let dir = self.root.path().join("Games").join(title);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("gameinfo"),
            format!("{title}\n1.6.4871 rev588\n93132\nen-US\n{id}\n{id}\n59819264299049740\n"),
        )
        .unwrap();
        let start = dir.join("start.sh");
        write_script(&start, "#!/bin/sh\n");
        self.install(serde_json::json!({
            "platform": "linux", "executable": "", "install_path": dir,
            "install_size": "5.03 GiB", "is_dlc": false, "version": "1.6.4871 rev588",
            "appName": id, "installedDLCs": dlc,
            "language": "en", "versionEtag": "", "buildId": "", "pinnedVersion": false,
        }));
        dir
    }

    fn disco(&mut self) {
        let dir = self.root.path().join("Games/Disco Elysium");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("Disco Elysium.exe"), "game").unwrap();
        fs::write(
            dir.join(format!("goggame-{DISCO}.info")),
            serde_json::json!({
                "gameId": DISCO, "rootGameId": DISCO, "buildId": "59781086834418085",
                "playTasks": [{
                    "isPrimary": true, "type": "FileTask", "path": "Disco Elysium.exe",
                    "workingDir": "bin", "category": "game",
                }],
            })
            .to_string(),
        )
        .unwrap();
        let prefix = self.root.path().join("Prefixes/Disco");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(
            self.source
                .config_dir()
                .join(format!("GamesConfig/{DISCO}.json")),
            serde_json::json!({
                DISCO: {
                    "winePrefix": prefix,
                    "wineVersion": {"type": "wine", "name": "System", "bin": "wine"},
                }
            })
            .to_string(),
        )
        .unwrap();
        self.install(serde_json::json!({
            "platform": "windows", "executable": "", "install_path": dir,
            "install_size": "9.57 GiB", "is_dlc": false, "version": "",
            "appName": DISCO, "installedDLCs": [], "language": "en-US",
            "versionEtag": "", "buildId": "59781086834418085", "pinnedVersion": false,
        }));
    }

    fn listing(&self) -> Listing {
        listing_from(&self.paths, std::slice::from_ref(&self.source))
    }

    fn prepared(&self, id: &str) -> Prepared {
        let candidate = self
            .listing()
            .games
            .into_iter()
            .find(|candidate| candidate.game.id == id)
            .unwrap();
        let Standing::Ready(migration) = candidate.standing else {
            panic!("{id} isn't ready");
        };
        prepare(
            &self.paths,
            candidate.source,
            candidate.heroic,
            migration,
            None,
        )
        .unwrap()
    }

    fn page(&self) -> Page {
        let (mut page, _) = Page::new(self.paths.clone());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Ok(self.listing())));
        page
    }
}

fn no_extras(_: &Paths, _: &GlobalConfig, _: &str, _: u64, _: &str, _: &Token) -> Extras {
    Extras::default()
}

fn titles(listing: &Listing) -> Vec<&str> {
    listing
        .games
        .iter()
        .map(|candidate| candidate.game.title.as_str())
        .collect()
}

#[test]
fn lists_heroic_games_with_migrated_ones_marked_and_bad_entries_as_issues() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[1233017772]);
    fx.native(STARDEW, "Stardew Valley", &[]);
    fx.install(serde_json::json!({"platform": "osx", "install_path": "/y", "appName": "5"}));
    let stardew = fx.prepared(STARDEW);
    let report = run(&fx.paths, &stardew, &Token::new(), |_| {}, no_extras).unwrap();
    assert_eq!(report.environment.as_deref(), Some("stardew-valley"));

    let listing = fx.listing();
    assert_eq!(titles(&listing), ["RimWorld", "Stardew Valley"]);
    assert!(matches!(listing.games[0].standing, Standing::Ready(_)));
    assert!(matches!(
        &listing.games[1].standing,
        Standing::Already { environment } if environment == "stardew-valley"
    ));
    assert_eq!(listing.issues, ["GOG game 5: osx games can't be migrated"]);

    let mut page = fx.page();
    // A migrated game can't be picked again.
    let _ = page.update(Event::Pick(1));
    assert_eq!(page.picked, None);
    let _ = page.update(Event::Pick(0));
    assert_eq!(page.picked, Some(0));
    assert_eq!(page.status, "Checking RimWorld...");
    crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
}

/// Copies the fixture's Heroic config to where native and Flatpak
/// Heroic keep theirs, with `flatpak` edited first, and finds both.
fn two_heroics(fx: &Fixture, flatpak: impl FnOnce(&std::path::Path)) -> Vec<HeroicSource> {
    let home = fx.root.path();
    let native = home.join(".config/heroic");
    let other = home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic");
    for dir in [&native, &other] {
        for sub in ["gog_store", "store_cache", "GamesConfig"] {
            fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in [
            "config.json",
            "gog_store/installed.json",
            "store_cache/gog_library.json",
        ] {
            fs::copy(fx.source.config_dir().join(file), dir.join(file)).unwrap();
        }
    }
    flatpak(&other);
    HeroicSource::discover(home, None, None).unwrap()
}

#[test]
fn a_game_both_heroics_list_at_one_folder_shows_once_and_says_which() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let sources = two_heroics(&fx, |_| {});
    assert_eq!(sources.len(), 2);
    let listing = listing_from(&fx.paths, &sources);
    assert_eq!(titles(&listing), ["RimWorld"]);
    assert!(listing.issues.is_empty(), "{:?}", listing.issues);
    let candidate = listing.games.into_iter().next().unwrap();
    assert_eq!(candidate.source.origin(), HeroicOrigin::Native);
    let Standing::Ready(migration) = candidate.standing else {
        panic!("RimWorld isn't ready");
    };
    let prepared = prepare(
        &fx.paths,
        candidate.source,
        candidate.heroic,
        migration,
        None,
    )
    .unwrap();
    let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
    let native = fx.root.path().join(".config/heroic");
    assert_eq!(
        rows[2],
        (
            "Heroic",
            format!(
                "native ({}), also listed by Flatpak Heroic",
                native.display()
            )
        )
    );
    // One config, as before: no Heroic row.
    let single = fx.prepared(RIMWORLD);
    assert!(
        !review_rows(&single.migration, single.heroic.as_deref())
            .iter()
            .any(|(label, _)| *label == "Heroic")
    );
}

#[test]
fn a_game_the_heroics_put_in_two_folders_is_an_issue() {
    let mut fx = Fixture::new();
    let folder = fx.native(RIMWORLD, "RimWorld", &[]);
    let elsewhere = fx.root.path().join("Elsewhere/RimWorld");
    let sources = two_heroics(&fx, |flatpak| {
        let installed = flatpak.join("gog_store/installed.json");
        let text = fs::read_to_string(&installed).unwrap();
        let moved = text.replace(
            &serde_json::to_string(&folder).unwrap(),
            &serde_json::to_string(&elsewhere).unwrap(),
        );
        assert_ne!(text, moved);
        fs::write(installed, moved).unwrap();
    });
    let listing = listing_from(&fx.paths, &sources);
    assert!(listing.games.is_empty());
    assert_eq!(
        listing.issues,
        [format!(
            "GOG game {RIMWORLD} is installed in two places: {} (native Heroic) and {} (Flatpak Heroic).",
            folder.display(),
            elsewhere.display()
        )]
    );
    let mut page = fx.page();
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Ok(listing)));
    crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
}

#[test]
fn the_review_says_what_migrating_does() {
    let mut fx = Fixture::new();
    let folder = fx.native(RIMWORLD, "RimWorld", &[1233017772, 1827857764]);
    let prepared = fx.prepared(RIMWORLD);
    assert!(prepared.ready(), "{:?}", prepared.blocking);
    let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
    let canonical = folder.canonicalize().unwrap();
    assert_eq!(
        rows,
        [
            (
                "Environment",
                "rimworld (new native environment)".to_owned()
            ),
            ("Folder", canonical.display().to_string()),
            ("Track", "installer".to_owned()),
            ("Version", "1.6.4871 rev588".to_owned()),
            ("Language", "en".to_owned()),
            ("DLC", "DLC (1233017772)".to_owned()),
            ("", "DLC (1827857764)".to_owned()),
        ]
    );
}

#[test]
fn the_review_lists_each_dlc_by_name_and_id() {
    let mut fx = Fixture::new();
    let folder = fx.native(RIMWORLD, "RimWorld", &[1233017772, 1827857764, 1865538500]);
    let game = folder.join("game");
    fs::create_dir_all(&game).unwrap();
    for (id, name) in [
        (1233017772_u64, "RimWorld - Royalty"),
        (1827857764, "RimWorld - Ideology"),
    ] {
        fs::write(
            game.join(format!("goggame-{id}.info")),
            serde_json::json!({"gameId": id.to_string(), "name": name}).to_string(),
        )
        .unwrap();
    }
    let prepared = fx.prepared(RIMWORLD);
    let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
    let dlc: Vec<_> = rows
        .iter()
        .skip_while(|(label, _)| *label != "DLC")
        .cloned()
        .collect();
    assert_eq!(
        dlc,
        [
            ("DLC", "Royalty (1233017772)".to_owned()),
            ("", "Ideology (1827857764)".to_owned()),
            ("", "DLC (1865538500)".to_owned()),
        ]
    );
    // The rows draw, the unlabeled ones included.
    let mut page = fx.page();
    page.picked = Some(0);
    page.prepared = Some(prepared);
    page.step = Step::Review;
    let mut view = page.view(Palette::DARK);
    let probe = crate::layout_probe::probe(&mut view, iced::Size::new(680.0, 560.0));
    let inset = iced::Rectangle::new(iced::Point::new(24.0, 24.0), iced::Size::new(632.0, 512.0));
    assert!(probe.outside(inset).is_empty());
}

#[test]
fn a_windows_game_reviews_with_a_runner_choice() {
    let mut fx = Fixture::new();
    fx.disco();
    let prepared = fx.prepared(DISCO);
    assert!(matches!(
        prepared.migration.target(),
        Target::Windows { .. }
    ));
    assert!(prepared.suggestion.is_some());
    let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
    assert_eq!(rows[2], ("Track", "galaxy".to_owned()));
    assert_eq!(rows[3], ("Version", "build 59781086834418085".to_owned()));
    // Without a runner the game can't be migrated yet.
    if prepared.runner.is_empty() {
        assert!(!prepared.ready());
    }
    let mut page = fx.page();
    page.picked = Some(0);
    page.prepared = Some(prepared);
    page.step = Step::Review;
    for (width, height) in [(280.0, 240.0), (680.0, 560.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view(Palette::DARK);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        let spilled = probe.outside(inset);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
    }
}

#[test]
fn migrating_reports_like_the_cli_and_adds_the_extras() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let prepared = fx.prepared(RIMWORLD);
    let mut said = Vec::new();
    let report = run(
        &fx.paths,
        &prepared,
        &Token::new(),
        |line| said.push(line),
        |_, _, name, id, title, _| {
            assert_eq!((name, id, title), ("rimworld", 1094900565, "RimWorld"));
            Extras {
                lines: vec!["Imported 41 h of GOG playtime.".to_owned()],
                notes: vec!["Skipped SteamGridDB art: no key".to_owned()],
                icon: false,
            }
        },
    )
    .unwrap();
    assert_eq!(said, ["Adding GOG playtime and art for RimWorld..."]);
    assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
    assert!(
        report
            .lines
            .contains(&"Imported 41 h of GOG playtime.".to_owned())
    );
    assert_eq!(report.lines.last().unwrap(), HEROIC_NOTE);
    assert_eq!(report.warnings, ["Skipped SteamGridDB art: no key"]);
    let config = fs::read_to_string(fx.paths.environment_config("rimworld")).unwrap();
    assert!(config.contains(RIMWORLD), "{config}");
}

#[test]
fn migrating_with_a_broken_library_keeps_its_warning_in_the_report() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let prepared = fx.prepared(RIMWORLD);
    let broken = "[environments.old]\nfavorite = true\n[broken\n";
    fs::create_dir_all(fx.paths.library_file().parent().unwrap()).unwrap();
    fs::write(fx.paths.library_file(), broken).unwrap();

    let report = run(&fx.paths, &prepared, &Token::new(), |_| {}, no_extras).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].starts_with("Can't update library.toml: it can't be read: "),
        "{:?}",
        report.warnings
    );
    assert_eq!(fs::read_to_string(fx.paths.library_file()).unwrap(), broken);
}

/// A cancel that comes after the migration committed, while GOG's
/// playtime and art are being fetched, reaches that work through the
/// page's token: it stops, and the migration stays done.
#[test]
fn a_cancel_during_the_extras_stops_them_and_keeps_the_migration() {
    use std::sync::mpsc;
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let prepared = fx.prepared(RIMWORLD);
    let token = Token::new();
    let (started, extras_started) = mpsc::channel();
    let report = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            run(
                &fx.paths,
                &prepared,
                &token,
                |_| {},
                |paths, _, name, _, _, extras_token| {
                    // The migration is committed before the extras start.
                    assert!(paths.environment_config(name).exists());
                    started.send(()).unwrap();
                    // Stands in for a slow GOG request that checks the
                    // token, as `extras::import` does between steps.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    while !extras_token.is_canceled() {
                        assert!(std::time::Instant::now() < deadline, "never canceled");
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Extras {
                        notes: vec!["Skipped GOG artwork: canceled.".to_owned()],
                        ..Extras::default()
                    }
                },
            )
        });
        extras_started.recv().unwrap();
        token.cancel();
        worker.join().unwrap()
    })
    .unwrap();
    assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
    assert_eq!(report.warnings, ["Skipped GOG artwork: canceled."]);
    assert!(fx.paths.environment_config("rimworld").exists());
}

/// Core decides whether a cancel before the migration comes in time. A
/// native game has nothing to copy, so it finishes and stays done, and
/// the extras are skipped with a note.
#[test]
fn a_cancel_before_a_native_migration_keeps_it_and_skips_the_extras() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let prepared = fx.prepared(RIMWORLD);
    let token = Token::new();
    token.cancel();
    let report = run(
        &fx.paths,
        &prepared,
        &token,
        |_| {},
        |_, _, _, _, _, _| panic!("no extras after a cancel"),
    )
    .unwrap();
    assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
    assert_eq!(report.warnings, ["Skipped GOG playtime and art: canceled."]);
    assert!(fx.paths.environment_config("rimworld").exists());
}

#[test]
fn working_cancels_and_a_failed_migration_returns_to_the_review() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let generation = page.generation;
    let _ = page.update(Event::Pick(0));
    let generation = page.generation.max(generation);
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Box::new(fx.prepared(RIMWORLD))),
    ));
    assert_eq!(page.step, Step::Review);
    let _ = page.update(Event::Migrate);
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    let _ = page.update(Event::Cancel);
    assert!(page.token.as_ref().unwrap().is_canceled());
    assert_eq!(page.status, "Canceling...");
    // Back does nothing while it works.
    let _ = page.update(Event::Back);
    assert_eq!(page.step, Step::Working);
    let generation = page.generation;
    let _ = page.update(Event::Finished(
        generation,
        Err("Import canceled.".to_owned()),
    ));
    assert!(!page.busy());
    assert_eq!(page.step, Step::Review);
    assert_eq!(page.error.as_deref(), Some("Import canceled."));
}

#[test]
fn closing_the_window_while_migrating_asks_first_then_cancels() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let _ = page.update(Event::Pick(0));
    let generation = page.generation;
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Box::new(fx.prepared(RIMWORLD))),
    ));
    let _ = page.update(Event::Migrate);
    let mut app = crate::App::boot(fx.paths.clone()).0;
    app.flow = Some(super::super::Page::Gog(Box::new(page)));
    let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt);
    let _ = app.update(crate::Message::CancelAndClose);
    assert!(app.closing);
    let Some(super::super::Page::Gog(page)) = &app.flow else {
        panic!("the page stays until its work ends");
    };
    assert!(page.token.as_ref().unwrap().is_canceled());
}

/// Like Heroic import: reading and checking games is dropped, not
/// waited on, when the page moves on; copying a runner or downloading a
/// build is work, which Cancel stops and closing the window asks about.
#[test]
fn checking_is_dropped_on_back_and_prerequisites_cancel_like_heroic() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let _ = page.update(Event::Pick(0));
    assert!(
        page.waiting() && !page.busy(),
        "checking isn't work to wait for"
    );
    let generation = page.generation;
    let _ = page.update(Event::Back);
    assert!(page.status.is_empty());
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Box::new(fx.prepared(RIMWORLD))),
    ));
    assert_eq!(page.step, Step::Games, "a dropped check doesn't reopen");
    assert!(page.prepared.is_none());

    let _ = page.update(Event::KeepEditing);
    let _ = page.update(Event::Pick(0));
    let generation = page.generation;
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Box::new(fx.prepared(RIMWORLD))),
    ));
    let _ = page.update(Event::DownloadRunner);
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    let mut app = crate::App::boot(fx.paths.clone()).0;
    app.flow = Some(super::super::Page::Gog(Box::new(page)));
    let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt);
    let _ = app.update(crate::Message::CancelAndClose);
    let Some(super::super::Page::Gog(page)) = &mut app.flow else {
        panic!("the page stays until the download stops");
    };
    assert!(page.token.as_ref().unwrap().is_canceled());
    let generation = page.generation;
    let _ = page.update(Event::PrerequisiteFinished(
        generation,
        Err("Canceled".to_owned()),
    ));
    assert!(!page.busy());
    assert_eq!(page.step, Step::Review);
}

#[test]
fn leaving_after_picking_a_game_asks_first() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let _ = page.update(Event::Back);
    assert!(matches!(page.take_action(), Some(Action::Choose)));

    let mut page = fx.page();
    let _ = page.update(Event::Pick(0));
    let generation = page.generation;
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Box::new(fx.prepared(RIMWORLD))),
    ));
    let _ = page.update(Event::Back);
    assert_eq!(page.step, Step::Games);
    assert!(page.take_action().is_none());
    let _ = page.update(Event::Back);
    assert!(page.leave_prompt);
    assert!(page.take_action().is_none());
    let _ = page.update(Event::KeepEditing);
    assert!(!page.leave_prompt);
    let _ = page.update(Event::Back);
    let _ = page.update(Event::Discard);
    assert!(matches!(page.take_action(), Some(Action::Choose)));
}

#[test]
fn a_portable_session_says_where_gog_games_live() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(&home)),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    let error = load_listing(&paths).unwrap_err();
    assert_eq!(
        error,
        "GOG games live in the host library. Run this without --portable."
    );
    let (mut page, _) = Page::new(paths);
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Err(error.clone())));
    assert_eq!(page.error.as_deref(), Some(error.as_str()));
    assert!(page.listing.is_none());
}

#[test]
fn extras_read_like_import_gog() {
    let report = extras::Report {
        playtime: Some(Playtime {
            gog_seconds: 41 * 3600 + 600,
            before_seconds: 0,
            after_seconds: 41 * 3600 + 600,
        }),
        art: vec![
            Filled {
                kind: Kind::Icon,
                source: Source::Gog(1),
            },
            Filled {
                kind: Kind::Grid,
                source: Source::Gog(1),
            },
            Filled {
                kind: Kind::Logo,
                source: Source::SteamGridDb(9),
            },
        ],
        notes: Vec::new(),
    };
    assert_eq!(
        report.lines(),
        [
            "Imported 41 h of GOG playtime.",
            "Added GOG art: poster, icon.",
            "Added SteamGridDB art: logo.",
        ]
    );
}

const CELESTE: u64 = 1452425640;
const HADES: u64 = 1477079530;

/// Writes an install of `id` that stopped before `owner`'s config was
/// written.
fn configless(paths: &Paths, folder: &std::path::Path, owner: &str, id: u64) {
    use wineo_core::gog::folder::{FolderLock, FolderState, Journal, Platform, Target};
    fs::create_dir_all(folder).unwrap();
    let folder = fs::canonicalize(folder).unwrap();
    let state = FolderState {
        journal: Some(Journal {
            operation: "op".to_owned(),
            kind: JournalKind::Install,
            target: Target {
                gog_id: id,
                platform: Platform::Linux,
                language: "en".to_owned(),
                track: Track::Installer,
                products: Vec::new(),
            },
            packages: Vec::new(),
            attempted: Vec::new(),
        }),
        ..FolderState::new(folder.clone(), Some(owner.to_owned()))
    };
    FolderLock::exclusive(paths, &folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
}

#[test]
fn the_grid_lists_every_owned_game_by_what_wineo_has_of_it() {
    use super::super::gog_install::tests::{
        DISCO, RIMWORLD as RIMWORLD_ID, World, disco, inno, rimworld,
    };
    use crate::gog_fake::{native_game, owned_game};
    use wineo_core::gog::update::windows::set_graphical_session;

    let world = World::new();
    set_graphical_session(Some(true));
    world.installed(&rimworld(), &Default::default()).unwrap();
    // Disco's installer stops partway, which leaves its install to finish.
    world.disco("1", "exit 4", true);
    let failure = world.installed(&disco(), &inno()).unwrap_err();
    assert!(
        format!("{failure:?}").contains("stopped partway"),
        "{failure:?}"
    );
    set_graphical_session(None);
    native_game(
        &world.paths,
        world.root.path(),
        "stardew",
        STARDEW.parse().unwrap(),
        "1.0",
        false,
    );
    configless(
        &world.paths,
        &world.root.path().join("Games/Hades"),
        "hades",
        HADES,
    );
    world.library.lock().unwrap().owned.extend([
        owned_game(STARDEW.parse().unwrap(), "Stardew Valley", true, true),
        owned_game(CELESTE, "Celeste", true, false),
        owned_game(HADES, "Hades", true, true),
    ]);

    let mut listing = Listing::default();
    add_owned(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &mut listing,
    );
    assert_eq!(listing.owned_note, None);
    let states: Vec<(&str, &OwnedState, Option<&str>)> = listing
        .owned
        .iter()
        .map(|owned| {
            (
                owned.offer.title.as_str(),
                &owned.state,
                owned.offer.unfinished.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        states,
        [
            ("Celeste", &OwnedState::NotInstalled, None),
            (
                "Disco Elysium",
                &OwnedState::Unfinished,
                Some("disco-elysium")
            ),
            ("Hades", &OwnedState::Unfinished, Some("hades")),
            (
                "RimWorld",
                &OwnedState::Installed("rimworld".to_owned()),
                None
            ),
            (
                "Stardew Valley",
                &OwnedState::Imported("stardew".to_owned()),
                None
            ),
        ]
    );
    assert_eq!(listing.owned[0].offer.gog_id, CELESTE);
    assert!(listing.owned[0].offer.linux && !listing.owned[0].offer.windows);
    assert_eq!(listing.owned[3].offer.gog_id, RIMWORLD_ID);
    assert_eq!(listing.owned[1].offer.gog_id, DISCO);

    let (mut page, _) = Page::new(world.paths.clone());
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Ok(listing)));
    // Narrow and short dialogs keep the grid scrolling inside them.
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (680.0, 560.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view_at(Palette::DARK, width);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        assert!(probe.outside(inset).is_empty(), "{width}x{height}");
    }
    // Installed and migrated games can't be opened.
    let _ = page.update(Event::Install(3));
    let _ = page.update(Event::Install(4));
    assert_eq!(page.step, Step::Games);
    // The keyboard moves to Celeste first, and Enter opens its install.
    let task = page.key(
        &Key::Named(Named::ArrowRight),
        keyboard::Modifiers::empty(),
        680.0,
    );
    assert!(task.is_some());
    assert_eq!(page.grid.focus, Some(0));
    let _ = page.key(
        &Key::Named(Named::Enter),
        keyboard::Modifiers::empty(),
        680.0,
    );
    assert_eq!(page.step, Step::Install);
    assert!(page.waiting(), "it plans the install");
    {
        let mut view = page.view_at(Palette::DARK, 680.0);
        let _ = crate::layout_probe::layout(&mut view, iced::Size::new(680.0, 560.0));
    }
    // Back from the review returns to the grid and reads it again.
    let _ = page.back();
    assert_eq!(page.step, Step::Games);
    assert!(page.install.is_none());
}

#[test]
fn heroic_games_show_once_and_logged_out_still_migrates() {
    use crate::gog_fake::{Library, library_server, owned_game, save_login};
    use crate::settings::steam_key::tests::TestKeyring;

    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let library = std::sync::Arc::new(std::sync::Mutex::new(Library {
        owned: vec![
            owned_game(RIMWORLD.parse().unwrap(), "RimWorld", true, true),
            owned_game(CELESTE, "Celeste", true, false),
        ],
        ..Library::default()
    }));
    let server = library_server(library);
    let keyring = TestKeyring(std::cell::RefCell::new(None));

    let mut listing = fx.listing();
    add_owned(&fx.paths, &keyring, server.client(), &mut listing);
    assert_eq!(
        listing.owned_note.as_deref(),
        Some("Log in to GOG in Settings to install the games you own.")
    );
    assert!(listing.owned.is_empty());
    assert_eq!(titles(&listing), ["RimWorld"]);

    save_login(&fx.paths, &keyring);
    let mut listing = fx.listing();
    add_owned(&fx.paths, &keyring, server.client(), &mut listing);
    let owned: Vec<&str> = listing
        .owned
        .iter()
        .map(|owned| owned.offer.title.as_str())
        .collect();
    assert_eq!(owned, ["Celeste"], "Heroic's RimWorld shows once");
    assert_eq!(Page::entries(&listing), [Entry::Owned(0), Entry::Heroic(0)]);
}

#[test]
fn the_install_pages_stay_inside_narrow_and_short_dialogs() {
    use super::super::gog_install::tests::{World, inno, rimworld};
    use wineo_core::gog::update::windows::set_graphical_session;

    let world = World::new();
    set_graphical_session(Some(true));
    let native = world.prepare(&rimworld(), &Default::default(), false);
    let windows = world.prepare(
        &rimworld(),
        &super::super::gog_install::Choices {
            platform: Some(wineo_core::gog::folder::Platform::Windows),
            ..inno()
        },
        false,
    );
    set_graphical_session(None);
    for prepared in [native, windows] {
        check_install_layout(&world, prepared);
    }
}

fn check_install_layout(
    world: &super::super::gog_install::tests::World,
    prepared: super::super::gog_install::Prepared,
) {
    let (mut page, _) = Page::new(world.paths.clone());
    let generation = page.generation;
    let mut listing = Listing::default();
    add_owned(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &mut listing,
    );
    let _ = page.update(Event::Listed(generation, Ok(listing)));
    let index = page
        .listing
        .as_ref()
        .unwrap()
        .owned
        .iter()
        .position(|owned| owned.offer.title == "RimWorld")
        .unwrap();
    let _ = page.update(Event::Install(index));
    let generation = page.install.as_ref().unwrap().generation();
    let instance = page.install_instance;
    let _ = page.update(Event::Installing(
        instance,
        Box::new(gog_install::Event::Prepared(
            generation,
            Ok(Box::new(prepared)),
        )),
    ));
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view_at(Palette::DARK, width);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        let spilled = probe.outside(inset);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
    }
}

#[test]
fn a_reply_to_a_review_that_was_left_never_reaches_the_next_one() {
    use super::super::gog_install::tests::{World, rimworld};

    let world = World::new();
    let stale = world.prepare(&rimworld(), &Default::default(), false);
    let mut listing = Listing::default();
    add_owned(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &mut listing,
    );
    let titles: Vec<String> = listing
        .owned
        .iter()
        .map(|owned| owned.offer.title.clone())
        .collect();
    let rimworld_at = titles.iter().position(|title| title == "RimWorld").unwrap();
    let disco_at = titles
        .iter()
        .position(|title| title == "Disco Elysium")
        .unwrap();
    let (mut page, _) = Page::new(world.paths.clone());
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Ok(listing.clone())));
    // Open RimWorld, leave while it plans, then open Disco Elysium.
    let _ = page.update(Event::Install(rimworld_at));
    let first = page.install_instance;
    let first_generation = page.install.as_ref().unwrap().generation();
    let _ = page.back();
    assert!(page.install.is_none());
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Ok(listing)));
    let _ = page.update(Event::Install(disco_at));
    assert_eq!(
        page.install.as_ref().unwrap().generation(),
        first_generation,
        "both start at the same generation"
    );
    // RimWorld's plan arrives late and is dropped.
    let _ = page.update(Event::Installing(
        first,
        Box::new(gog_install::Event::Prepared(
            first_generation,
            Ok(Box::new(stale)),
        )),
    ));
    let install = page.install.as_ref().unwrap();
    assert_eq!(install.planned(), None);
    assert!(install.waiting(), "Disco Elysium is still being planned");
}

// -- posters --

fn png_bytes() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::RgbaImage::from_pixel(8, 12, ::image::Rgba([200, 30, 30, 255]))
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn handle() -> image::Handle {
    image::Handle::from_rgba(1, 1, vec![0; 4])
}

/// Serves one body for every cover URL, and records the requests.
struct Covers {
    status: u16,
    body: Vec<u8>,
    seen: std::sync::Mutex<Vec<String>>,
}

impl Covers {
    fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl wineo_core::download::Transport for Covers {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        anyhow::bail!("unexpected plain request to {url}")
    }

    fn request(
        &self,
        url: &str,
        _bearer: Option<&str>,
    ) -> anyhow::Result<wineo_core::download::Response> {
        self.seen.lock().unwrap().push(url.to_owned());
        Ok(wineo_core::download::Response {
            status: self.status,
            body: Box::new(std::io::Cursor::new(self.body.clone())),
        })
    }
}

/// A GOG whose art answer names a cover for whichever game is asked
/// about, or fails with 500.
fn art_server(works: bool) -> crate::gog_fake::Server {
    crate::gog_fake::Server::start(move |line| {
        let path = line.split(' ').nth(1).unwrap_or_default();
        let path = path.split('?').next().unwrap_or_default();
        match path.strip_prefix("/v2/games/") {
            Some(id) if works => crate::gog_fake::reply(
                200,
                &serde_json::json!({
                    "_links": {"boxArtImage": {"href": format!("https://images.example/{id}.png")}},
                    "_embedded": {"product": {"id": id.parse::<u64>().unwrap(), "title": "X"}},
                })
                .to_string(),
            ),
            _ => crate::gog_fake::reply(500, "{}"),
        }
    })
}

fn owned_game_in(id: u64, title: &str, state: OwnedState) -> Owned {
    Owned {
        offer: Offer {
            gog_id: id,
            title: title.to_owned(),
            linux: true,
            windows: false,
            unfinished: None,
        },
        state,
    }
}

fn owned_listing(owned: Vec<Owned>) -> Listing {
    Listing {
        owned,
        ..Listing::default()
    }
}

fn store_grid(paths: &Paths, environment: &str) {
    let dir = paths.art_dir(environment);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("grid.png"), png_bytes()).unwrap();
}

/// Runs the worker on `jobs` and collects what it sent.
fn load(
    paths: &Paths,
    jobs: Vec<Job>,
    token: &Token,
    server: Option<&crate::gog_fake::Server>,
    covers: &'static Covers,
) -> Vec<(String, Result<image::Handle, String>)> {
    let mut sent = Vec::new();
    gog_posters::run(
        paths,
        jobs,
        token,
        || {
            let server = server.ok_or_else(|| "no network in this test".to_owned())?;
            Ok(Network {
                client: server.client(),
                transport: Box::new(CoversRef(covers)),
            })
        },
        |id, poster| sent.push((id, poster)),
    );
    sent
}

struct CoversRef(&'static Covers);

impl wineo_core::download::Transport for CoversRef {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        self.0.get(url)
    }

    fn request(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> anyhow::Result<wineo_core::download::Response> {
        self.0.request(url, bearer)
    }
}

fn leaked(covers: Covers) -> &'static Covers {
    Box::leak(Box::new(covers))
}

fn covers_dir_is_empty(paths: &Paths) -> bool {
    fs::read_dir(paths.gog_covers_dir()).map_or(true, |mut entries| entries.next().is_none())
}

fn page_with(paths: &Paths, listing: Listing) -> Page {
    let (mut page, _) = Page::new(paths.clone());
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Ok(listing)));
    page
}

#[test]
fn a_stale_poster_event_is_ignored() {
    let fx = Fixture::new();
    let mut page = fx.page();
    let generation = page.grid.poster_generation;
    let _ = page.update(Event::Poster(
        generation.wrapping_sub(1),
        RIMWORLD.to_owned(),
        Ok(handle()),
    ));
    assert!(page.poster(RIMWORLD).is_none());
    let _ = page.update(Event::Poster(generation, RIMWORLD.to_owned(), Ok(handle())));
    assert!(page.poster(RIMWORLD).is_some());

    // A relist starts over: the old load's reply lands nowhere.
    let _ = page.list();
    assert_ne!(page.grid.poster_generation, generation);
    let _ = page.update(Event::Poster(generation, STARDEW.to_owned(), Ok(handle())));
    assert!(page.posters.is_empty());

    // A page opened later never shares a load number with this one.
    let (next, _) = Page::new(fx.paths.clone());
    assert_ne!(next.grid.poster_generation, page.grid.poster_generation);
    assert_ne!(next.grid.poster_generation, generation);
}

#[test]
fn art_progress_follows_the_worker_and_ends_with_it() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let total = page.poster_jobs().len();
    assert!(total >= 1);
    assert_eq!(page.grid.progress.shown(), None, "not before the delay");
    assert!(page.grid.progress.pending_wakeup().is_some());
    let _ = page.update(Event::ProgressDue(page.grid.progress.wakeup_generation()));
    assert_eq!(page.grid.progress.shown(), Some((0, total)));
    crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));

    // A reply from an earlier load moves nothing.
    let generation = page.grid.poster_generation;
    let _ = page.update(Event::Poster(
        generation.wrapping_sub(1),
        RIMWORLD.to_owned(),
        Ok(handle()),
    ));
    let _ = page.update(Event::PostersEnded(generation.wrapping_sub(1)));
    assert_eq!(page.grid.progress.shown(), Some((0, total)));

    // Failures count, and a game that got no poster still ends with the
    // worker.
    let _ = page.update(Event::Poster(
        generation,
        RIMWORLD.to_owned(),
        Err("x".into()),
    ));
    assert_eq!(
        page.grid.progress.shown(),
        (total > 1).then_some((1, total))
    );
    let _ = page.update(Event::PostersEnded(generation));
    assert_eq!(page.grid.progress.shown(), None);
    assert_eq!(page.grid.progress.pending_wakeup(), None);
}

#[test]
fn art_progress_never_shows_when_the_worker_ends_before_the_delay() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let late = page.grid.progress.wakeup_generation();
    let generation = page.grid.poster_generation;
    let _ = page.update(Event::PostersEnded(generation));
    assert_eq!(page.grid.progress.pending_wakeup(), None);
    let _ = page.update(Event::ProgressDue(late));
    assert_eq!(page.grid.progress.shown(), None);
}

#[test]
fn leaving_the_grid_clears_the_art_progress() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let _ = page.update(Event::ProgressDue(page.grid.progress.wakeup_generation()));
    assert!(page.grid.progress.shown().is_some());
    let _ = page.back();
    assert_eq!(page.grid.progress.shown(), None);
    assert_eq!(page.grid.progress.pending_wakeup(), None);
}

#[test]
fn leaving_the_grid_stops_and_invalidates_the_poster_load() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let token = page.poster_token.clone().expect("a load started");
    let generation = page.grid.poster_generation;
    let _ = page.back();
    assert!(matches!(page.take_action(), Some(Action::Choose)));
    assert!(token.is_canceled());
    assert_ne!(page.grid.poster_generation, generation);
    let _ = page.update(Event::Poster(generation, RIMWORLD.to_owned(), Ok(handle())));
    assert!(page.poster(RIMWORLD).is_none());

    // Dropping the page stops the worker too.
    let mut page = fx.page();
    let token = page.poster_token.take().unwrap();
    page.poster_token = Some(token.clone());
    drop(page);
    assert!(token.is_canceled());
}

#[test]
fn going_back_from_a_review_restarts_the_unfinished_posters_under_a_new_load() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let mut page = fx.page();
    let old = page.poster_token.clone().unwrap();
    let generation = page.grid.poster_generation;
    page.step = Step::Review;
    let _ = page.back();
    assert_eq!(page.step, Step::Games);
    assert!(old.is_canceled());
    let token = page.poster_token.as_ref().expect("loading again");
    assert!(!token.is_canceled());
    assert_ne!(page.grid.poster_generation, generation);
    // The old load's reply is dropped; the new one's is kept.
    let _ = page.update(Event::Poster(generation, RIMWORLD.to_owned(), Ok(handle())));
    assert!(page.poster(RIMWORLD).is_none());
    let current = page.grid.poster_generation;
    let _ = page.update(Event::Poster(current, RIMWORLD.to_owned(), Ok(handle())));
    assert!(page.poster(RIMWORLD).is_some());
}

#[test]
fn a_failed_listing_loads_no_posters() {
    let fx = Fixture::new();
    let (mut page, _) = Page::new(fx.paths.clone());
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, Err("nope".to_owned())));
    assert!(page.poster_token.is_none());
    assert!(page.poster_jobs().is_empty());
}

#[test]
fn games_wineo_has_use_their_stored_grid_art_with_no_network() {
    let mut fx = Fixture::new();
    fx.native(STARDEW, "Stardew Valley", &[]);
    let stardew = fx.prepared(STARDEW);
    run(&fx.paths, &stardew, &Token::new(), |_| {}, no_extras).unwrap();
    store_grid(&fx.paths, "stardew-valley");
    store_grid(&fx.paths, "rimworld");
    store_grid(&fx.paths, "stardew");
    let mut listing = fx.listing();
    assert!(matches!(
        &listing.games[0].standing,
        Standing::Already { environment } if environment == "stardew-valley"
    ));
    listing.owned = vec![
        owned_game_in(1, "Installed", OwnedState::Installed("rimworld".to_owned())),
        owned_game_in(2, "Imported", OwnedState::Imported("stardew".to_owned())),
        owned_game_in(3, "Bare", OwnedState::Installed("absent".to_owned())),
    ];
    let mut page = page_with(&fx.paths, listing);
    let jobs: Vec<(String, PosterSource)> = page
        .poster_jobs()
        .into_iter()
        .map(|job| (job.id, job.source))
        .collect();
    // In grid order, which sorts by title.
    assert_eq!(
        jobs,
        [
            ("3".to_owned(), PosterSource::Stored("absent".to_owned())),
            ("2".to_owned(), PosterSource::Stored("stardew".to_owned())),
            ("1".to_owned(), PosterSource::Stored("rimworld".to_owned())),
            (
                STARDEW.to_owned(),
                PosterSource::Stored("stardew-valley".to_owned())
            ),
        ]
    );

    // The worker never asks for a connection, and a game without stored
    // art simply has no poster.
    let covers = leaked(Covers::new(500, Vec::new()));
    let generation = page.grid.poster_generation;
    let sent = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
    assert_eq!(covers.count(), 0);
    assert!(covers_dir_is_empty(&fx.paths));
    assert_eq!(
        sent.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["2", "1", STARDEW]
    );
    for (id, poster) in sent {
        let _ = page.update(Event::Poster(generation, id, poster));
    }
    assert!(page.poster("1").is_some());
    assert!(page.poster("2").is_some());
    assert!(page.poster(STARDEW).is_some());
    assert!(page.poster("3").is_none());
    // What has a poster isn't loaded again.
    assert_eq!(page.poster_jobs().len(), 1);
    crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
}

#[test]
fn a_game_wineo_lacks_loads_its_cover_once_and_then_from_the_cache() {
    let fx = Fixture::new();
    let listing = owned_listing(vec![
        owned_game_in(7, "Celeste", OwnedState::NotInstalled),
        owned_game_in(8, "Hades", OwnedState::Unfinished),
    ]);
    let mut page = page_with(&fx.paths, listing);
    let server = art_server(true);
    let covers = leaked(Covers::new(200, png_bytes()));

    let sent = load(
        &fx.paths,
        page.poster_jobs(),
        &Token::new(),
        Some(&server),
        covers,
    );
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|(_, poster)| poster.is_ok()));
    assert_eq!(server.requests().len(), 2);
    assert_eq!(covers.count(), 2);
    assert!(fx.paths.gog_covers_dir().join("7.png").is_file());
    assert!(!fx.paths.art_cache_dir().exists());

    // A second page finds both in the cache and never builds a client.
    let again = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
    assert_eq!(again.len(), 2);
    assert!(again.iter().all(|(_, poster)| poster.is_ok()));
    assert_eq!(server.requests().len(), 2);
    assert_eq!(covers.count(), 2);

    let generation = page.grid.poster_generation;
    for (id, poster) in again {
        let _ = page.update(Event::Poster(generation, id, poster));
    }
    assert!(page.poster("7").is_some() && page.poster("8").is_some());
}

#[test]
fn a_cover_that_cant_be_fetched_or_isnt_an_image_leaves_no_file_and_no_poster() {
    for (server, covers) in [
        (art_server(false), Covers::new(200, png_bytes())),
        (art_server(true), Covers::new(404, png_bytes())),
        (
            art_server(true),
            Covers::new(200, b"<html>no</html>".to_vec()),
        ),
    ] {
        let fx = Fixture::new();
        let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
        let mut page = page_with(&fx.paths, listing);
        let covers = leaked(covers);
        let sent = load(
            &fx.paths,
            page.poster_jobs(),
            &Token::new(),
            Some(&server),
            covers,
        );
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.is_err());
        assert!(covers_dir_is_empty(&fx.paths));
        let generation = page.grid.poster_generation;
        for (id, poster) in sent {
            let _ = page.update(Event::Poster(generation, id, poster));
        }
        assert!(page.poster("7").is_none());
        // The tile keeps its title and still lays out.
        crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
    }
}

#[test]
fn a_canceled_load_fetches_nothing_more() {
    let fx = Fixture::new();
    let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
    let page = page_with(&fx.paths, listing);
    let server = art_server(true);
    let covers = leaked(Covers::new(200, png_bytes()));
    let token = Token::new();
    token.cancel();
    let sent = load(&fx.paths, page.poster_jobs(), &token, Some(&server), covers);
    assert!(sent.is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn a_cached_cover_that_wont_decode_is_removed_and_fetched_again_next_load() {
    let fx = Fixture::new();
    let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
    let page = page_with(&fx.paths, listing);
    // A truncated download that passed the signature check.
    fs::create_dir_all(fx.paths.gog_covers_dir()).unwrap();
    let cached = fx.paths.gog_covers_dir().join("7.png");
    fs::write(&cached, &png_bytes()[..12]).unwrap();
    let server = art_server(true);
    let covers = leaked(Covers::new(200, png_bytes()));

    let first = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
    assert_eq!(first.len(), 1);
    assert!(first[0].1.is_err());
    assert!(!cached.exists(), "it can't poison the cache");
    assert_eq!(server.requests().len(), 0);

    let second = load(
        &fx.paths,
        page.poster_jobs(),
        &Token::new(),
        Some(&server),
        covers,
    );
    assert!(second[0].1.is_ok());
    assert_eq!(server.requests().len(), 1);
    assert!(cached.is_file());
}

#[test]
fn the_worker_decodes_only_when_a_shared_permit_is_free_and_stops_when_canceled() {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    let fx = Fixture::new();
    let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
    let page = page_with(&fx.paths, listing);
    fs::create_dir_all(fx.paths.gog_covers_dir()).unwrap();
    fs::write(fx.paths.gog_covers_dir().join("7.png"), png_bytes()).unwrap();
    let covers = leaked(Covers::new(200, png_bytes()));
    let slots = crate::flow::posters::import_decode_slots();
    let all = crate::flow::posters::import_limit() as u32;
    // Dropped before any assert can fail, since the scope below joins a
    // worker that is blocked until the permits come back.
    let mut held =
        Some(iced::futures::executor::block_on(slots.clone().acquire_many_owned(all)).unwrap());
    let token = Token::new();

    std::thread::scope(|scope| {
        let (sender, receiver) = channel();
        let (done, paths, jobs) = (&token, &fx.paths, page.poster_jobs());
        scope.spawn(move || {
            let _ = sender.send(load(paths, jobs, done, None, covers));
        });
        // Both permits are out, so the cover waits instead of decoding.
        assert!(receiver.recv_timeout(Duration::from_millis(200)).is_err());
        token.cancel();
        let sent = receiver.recv_timeout(Duration::from_secs(5));
        drop(held.take_if(|_| sent.is_err()));
        assert!(sent.unwrap().is_empty());
    });

    std::thread::scope(|scope| {
        let (sender, receiver) = channel();
        let (paths, jobs) = (&fx.paths, page.poster_jobs());
        scope.spawn(move || {
            let _ = sender.send(load(paths, jobs, &Token::new(), None, covers));
        });
        let early = receiver.recv_timeout(Duration::from_millis(200));
        drop(held.take());
        assert!(early.is_err());
        let sent = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.is_ok());
    });
}

#[test]
fn stored_art_that_wont_decode_is_never_removed() {
    let fx = Fixture::new();
    let dir = fx.paths.art_dir("rimworld");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("grid.png"), &png_bytes()[..12]).unwrap();
    let listing = owned_listing(vec![owned_game_in(
        1,
        "RimWorld",
        OwnedState::Installed("rimworld".to_owned()),
    )]);
    let page = page_with(&fx.paths, listing);
    let covers = leaked(Covers::new(200, Vec::new()));
    let sent = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
    assert!(sent[0].1.is_err());
    assert!(dir.join("grid.png").exists());
}

#[test]
fn an_outage_stops_the_fetching_after_three_games_but_cached_covers_still_load() {
    let fx = Fixture::new();
    let listing = owned_listing(
        (1..=6)
            .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
            .collect(),
    );
    let mut page = page_with(&fx.paths, listing);
    // The last game in the grid was cached by an earlier load.
    fs::create_dir_all(fx.paths.gog_covers_dir()).unwrap();
    fs::write(fx.paths.gog_covers_dir().join("6.png"), png_bytes()).unwrap();
    let server = crate::gog_fake::Server::start(|_| crate::gog_fake::reply(503, "{}"));
    let covers = leaked(Covers::new(200, png_bytes()));
    let sent = load(
        &fx.paths,
        page.poster_jobs(),
        &Token::new(),
        Some(&server),
        covers,
    );
    let art_requests = server
        .requests()
        .iter()
        .filter(|line| line.contains("/v2/games/"))
        .count();
    assert_eq!(art_requests, 3);
    assert_eq!(covers.count(), 0);
    let loaded: Vec<&str> = sent
        .iter()
        .filter(|(_, poster)| poster.is_ok())
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(loaded, ["6"]);
    let generation = page.grid.poster_generation;
    for (id, poster) in sent {
        let _ = page.update(Event::Poster(generation, id, poster));
    }
    assert!(page.poster("6").is_some());
    assert!(page.poster("1").is_none());
    assert!(cached_only(&fx.paths, "6.png"), "nothing new was cached");
}

fn cached_only(paths: &Paths, name: &str) -> bool {
    fs::read_dir(paths.gog_covers_dir())
        .unwrap()
        .all(|entry| entry.unwrap().file_name() == name)
}

#[test]
fn a_game_without_a_cover_does_not_count_as_an_outage() {
    let fx = Fixture::new();
    let listing = owned_listing(
        (1..=6)
            .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
            .collect(),
    );
    let page = page_with(&fx.paths, listing);
    let server = crate::gog_fake::Server::start(|_| crate::gog_fake::reply(404, "{}"));
    let covers = leaked(Covers::new(200, png_bytes()));
    let sent = load(
        &fx.paths,
        page.poster_jobs(),
        &Token::new(),
        Some(&server),
        covers,
    );
    assert_eq!(sent.len(), 6);
    assert_eq!(server.requests().len(), 6);
}

#[test]
fn a_cover_that_isnt_an_outage_resets_the_count() {
    let fx = Fixture::new();
    let listing = owned_listing(
        (1..=7)
            .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
            .collect(),
    );
    let page = page_with(&fx.paths, listing);
    // Two outages, a game with no cover, then outages until the stop.
    let server = crate::gog_fake::Server::start(|line| {
        let status = if line.contains("/v2/games/3?") {
            404
        } else {
            503
        };
        crate::gog_fake::reply(status, "{}")
    });
    let covers = leaked(Covers::new(200, png_bytes()));
    load(
        &fx.paths,
        page.poster_jobs(),
        &Token::new(),
        Some(&server),
        covers,
    );
    let art_requests = server
        .requests()
        .iter()
        .filter(|line| line.contains("/v2/games/"))
        .count();
    assert_eq!(art_requests, 6);
}

#[test]
fn canceling_after_the_first_cover_arrives_requests_nothing_more() {
    let fx = Fixture::new();
    let listing = owned_listing(
        (1..=4)
            .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
            .collect(),
    );
    let page = page_with(&fx.paths, listing);
    let server = art_server(true);
    let covers = leaked(Covers::new(200, png_bytes()));
    let token = Token::new();
    let mut sent = Vec::new();
    gog_posters::run(
        &fx.paths,
        page.poster_jobs(),
        &token,
        || {
            Ok(Network {
                client: server.client(),
                transport: Box::new(CoversRef(covers)),
            })
        },
        |id, poster| {
            sent.push((id, poster));
            token.cancel();
        },
    );
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.is_ok());
    assert_eq!(server.requests().len(), 1);
    assert_eq!(covers.count(), 1);
}

#[test]
fn heroic_games_not_yet_migrated_take_the_cover_by_their_numeric_id() {
    let mut fx = Fixture::new();
    fx.native(RIMWORLD, "RimWorld", &[]);
    let page = fx.page();
    let jobs = page.poster_jobs();
    assert_eq!(
        jobs,
        [Job {
            id: RIMWORLD.to_owned(),
            source: PosterSource::Cover(RIMWORLD.parse().unwrap()),
        }]
    );
}

#[test]
fn a_reclaimed_folders_journal_decides_which_finish_action_is_named() {
    assert_eq!(
        interrupted_line(true, "papers-please"),
        "Its install was interrupted. Use Finish install on papers-please in the library."
    );
    assert_eq!(
        interrupted_line(false, "rimworld"),
        "Its last update was interrupted. Use Finish update on rimworld in the library."
    );
}
