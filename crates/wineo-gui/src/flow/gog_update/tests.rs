use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::time::Instant;

use wineo_core::gog::folder::{Product, ProductStatus};
use wineo_core::gog::update::{Progress, SavedAside};

use super::*;
use crate::gog_fake::{
    native_game, native_game_with_dlc, native_product, native_product_with_dlc, product_server,
    save_login,
};
use crate::settings::steam_key::tests::TestKeyring;
use crate::size::format_bytes;

const RIMWORLD: u64 = 1094900565;

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap();
    (root, paths)
}

fn keyring() -> TestKeyring {
    TestKeyring(RefCell::new(None))
}

/// RimWorld at 1.0 with GOG offering 1.1, and its planned update.
fn planned() -> (tempfile::TempDir, Paths, Box<Pending>) {
    let (root, paths) = setup();
    native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
    let keyring = keyring();
    save_login(&paths, &keyring);
    let server = product_server(vec![native_product(RIMWORLD, "RimWorld", "1.1")]);
    let planned = plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap();
    let Planned::Update(pending) = planned else {
        panic!("expected an update");
    };
    (root, paths, pending)
}

#[test]
fn the_review_shows_versions_sizes_and_the_estimate() {
    let (_root, _paths, pending) = planned();
    assert!(!pending.is_recovery());
    let rows = review_rows(&pending);
    assert_eq!(rows[0].0, "Folder");
    assert!(rows[0].1.ends_with("Games/rimworld"), "{}", rows[0].1);
    assert_eq!(rows[1], ("RimWorld".to_owned(), "1.0 -> 1.1".to_owned()));
    assert_eq!(rows[2], ("Download".to_owned(), format_bytes(7340032)));
    assert_eq!(
        rows[3],
        (
            "Unpacked".to_owned(),
            format!("about {} (an estimate)", format_bytes(7340032))
        )
    );
    assert_eq!(pending.action(), "Update");
}

const ROYALTY: u64 = 1233017772;

/// RimWorld with Royalty installed at 1.0 and GOG offering 1.1 of both,
/// Royalty titled `gog_title`, and `info` as its `goggame-*.info` name.
fn planned_with_dlc(gog_title: &str, info: Option<&str>) -> Vec<(String, String)> {
    let (root, paths) = setup();
    let folder = native_game_with_dlc(
        &paths,
        root.path(),
        "rimworld",
        RIMWORLD,
        "1.0",
        &[ROYALTY],
        false,
    );
    if let Some(name) = info {
        fs::create_dir_all(folder.join("game")).unwrap();
        fs::write(
            folder.join(format!("game/goggame-{ROYALTY}.info")),
            serde_json::json!({"gameId": ROYALTY.to_string(), "name": name}).to_string(),
        )
        .unwrap();
    }
    let keyring = keyring();
    save_login(&paths, &keyring);
    let server = product_server(vec![native_product_with_dlc(
        RIMWORLD,
        "RimWorld",
        "1.1",
        &[(ROYALTY, gog_title)],
    )]);
    let planned = plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap();
    let Planned::Update(pending) = planned else {
        panic!("expected an update");
    };
    let rows = review_rows(&pending);
    // Skip the folder row and keep the product rows.
    rows[1..3].to_vec()
}

#[test]
fn a_dlc_row_is_its_gog_title_and_id() {
    let rows = planned_with_dlc("RimWorld - Royalty", Some("Something else"));
    assert_eq!(rows[0].0, "RimWorld");
    assert_eq!(
        rows[1],
        ("Royalty (1233017772)".to_owned(), "1.0 -> 1.1".to_owned())
    );
}

#[test]
fn a_dlc_gog_gives_no_title_takes_the_folder_s_name_or_shows_its_id() {
    let rows = planned_with_dlc("", Some("RimWorld - Royalty"));
    assert_eq!(rows[1].0, "Royalty (1233017772)");
    let rows = planned_with_dlc("", None);
    assert_eq!(rows[1].0, "DLC (1233017772)");
}

#[test]
fn up_to_date_and_logged_out_say_so() {
    let (root, paths) = setup();
    native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.1", false);
    let keyring = keyring();
    let server = product_server(vec![native_product(RIMWORLD, "RimWorld", "1.1")]);
    let error =
        plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap_err();
    assert_eq!(error, LOGGED_OUT);
    save_login(&paths, &keyring);
    assert!(matches!(
        plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()),
        Ok(Planned::UpToDate)
    ));
}

fn page_with(pending: Box<Pending>, paths: Paths) -> Page {
    let (mut page, _) = Page::new(paths, "rimworld".to_owned());
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Ok(Planned::Update(pending))));
    page
}

fn updated() -> Updated {
    Updated {
        environment: "rimworld".to_owned(),
        folder: "/games/rimworld".into(),
        kind: JournalKind::Update,
        recovered: false,
        claimed: true,
        products: vec![Product {
            reported_language: None,
            id: RIMWORLD,
            version: Some("1.1".to_owned()),
            build: None,
            language: None,
            status: ProductStatus::Installed,
        }],
        saved: vec![SavedAside {
            path: "Data/Core/About.xml".to_owned(),
            saved_as: "Data/Core/About.xml.wineo-saved".to_owned(),
        }],
        kept: vec!["Mods/old.dll".to_owned()],
        overwritten: vec!["RimWorldLinux".to_owned()],
        warnings: vec!["Kept the downloaded GOG packages".to_owned()],
        needs_restart: Vec::new(),
    }
}

#[test]
fn an_update_shows_what_changed_and_your_files() {
    let (_root, paths, pending) = planned();
    let mut page = page_with(pending.clone(), paths);
    assert_eq!(page.step, Step::Review);
    let _ = page.update(Event::Update);
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    assert_eq!(page.status, "Update rimworld...");
    let generation = page.generation;
    let _ = page.update(Event::Progress(
        generation,
        "Downloading RimWorld: 1.0 MiB / 7.0 MiB".to_owned(),
    ));
    assert_eq!(page.status, "Downloading RimWorld: 1.0 MiB / 7.0 MiB");
    let _ = page.update(Event::Finished(generation, Ok(Box::new(updated()))));
    assert!(!page.busy());
    assert_eq!(page.step, Step::Finished);
    assert_eq!(page.take_updated().as_deref(), Some("rimworld"));
    assert_eq!(page.take_updated(), None);
    let updated = page.updated.as_ref().unwrap();
    assert_eq!(
        result_lines(&pending, updated),
        [
            "Updated rimworld.",
            "RimWorld: 1.0 -> 1.1",
            "rimworld now owns /games/rimworld.",
        ]
    );
    let lists = result_lists(updated);
    assert_eq!(
        lists,
        [
            (
                "Moved your files aside where the new version has files:",
                vec!["Data/Core/About.xml -> Data/Core/About.xml.wineo-saved".to_owned()]
            ),
            (
                "Kept these old files you changed, which the new version no longer ships:",
                vec!["Mods/old.dll".to_owned()]
            ),
            (
                "Overwrote these files that differed from the new version:",
                vec!["RimWorldLinux".to_owned()]
            ),
        ]
    );
    for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view(Palette::DARK);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        assert!(probe.outside(inset).is_empty(), "{width}x{height}");
    }
    let _ = page.update(Event::BackToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { name: Some(name), launch: false }) if name == "rimworld"
    ));
}

#[test]
fn canceling_says_whether_there_is_an_update_to_finish() {
    let (_root, paths, pending) = planned();
    for (journal, message) in [
        (
            true,
            "Update interrupted. Use Finish update on rimworld to finish it.",
        ),
        (false, "Update canceled."),
    ] {
        let mut page = page_with(pending.clone(), paths.clone());
        let _ = page.update(Event::Update);
        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Working, "Back waits for the work");
        let _ = page.update(Event::Cancel);
        assert!(page.token.as_ref().unwrap().is_canceled());
        assert_eq!(page.status, "Canceling...");
        let generation = page.generation;
        let _ = page.update(Event::Finished(
            generation,
            Err(Failure::Canceled { journal }),
        ));
        assert_eq!(page.error.as_deref(), Some(message));
        assert_eq!(page.take_updated(), None);
    }
}

#[test]
fn closing_the_window_while_updating_asks_first_then_cancels() {
    let (_root, paths, pending) = planned();
    let mut page = page_with(pending, paths.clone());
    let _ = page.update(Event::Update);
    let mut app = crate::App::boot(paths).0;
    app.flow = Some(super::super::Page::GogUpdate(Box::new(page)));
    let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt);
    let _ = app.update(crate::Message::CancelAndClose);
    assert!(app.closing);
    let Some(super::super::Page::GogUpdate(page)) = &app.flow else {
        panic!("the page stays until its work ends");
    };
    assert!(page.token.as_ref().unwrap().is_canceled());
}

#[test]
fn canceling_while_gog_answers_drops_the_plan() {
    let (root, paths) = setup();
    native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
    let keyring = keyring();
    save_login(&paths, &keyring);
    let server = crate::gog_fake::product_server_slow(
        vec![native_product(RIMWORLD, "RimWorld", "1.1")],
        std::time::Duration::from_millis(300),
    );
    let token = Token::new();
    let result = std::thread::scope(|scope| {
        let (paths, server, token) = (&paths, &server, &token);
        let worker =
            scope.spawn(move || plan_with(paths, &keyring, server.client(), "rimworld", token));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !server
            .requests()
            .iter()
            .any(|line| line.contains("/products/"))
        {
            assert!(Instant::now() < deadline, "GOG was never asked");
            std::thread::sleep(Duration::from_millis(5));
        }
        token.cancel();
        worker.join().unwrap()
    });
    assert_eq!(result.unwrap_err(), "Update canceled.");
}

#[test]
fn planning_is_work_for_back_cancel_and_close() {
    let (_root, paths) = setup();
    let (mut page, _) = Page::new(paths.clone(), "rimworld".to_owned());
    assert!(page.busy());
    let _ = page.update(Event::Back);
    assert!(page.take_action().is_none(), "Back waits for planning");
    let _ = page.update(Event::Cancel);
    assert!(page.token.as_ref().unwrap().is_canceled());
    assert_eq!(page.status, "Canceling...");
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Err(CANCELED.to_owned())));
    assert!(!page.busy());
    assert_eq!(page.error.as_deref(), Some("Update canceled."));
    let _ = page.update(Event::Back);
    assert!(matches!(page.take_action(), Some(Action::Library { .. })));

    let (page, _) = Page::new(paths.clone(), "rimworld".to_owned());
    let mut app = crate::App::boot(paths).0;
    app.flow = Some(super::super::Page::GogUpdate(Box::new(page)));
    let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt, "closing while planning asks first");
    let _ = app.update(crate::Message::CancelAndClose);
    let Some(super::super::Page::GogUpdate(page)) = &app.flow else {
        panic!("the page stays until planning ends");
    };
    assert!(page.token.as_ref().unwrap().is_canceled());
}

/// Cancels once bytes have arrived and while more are still coming,
/// from a fake GOG that sends the installer slowly.
#[test]
fn canceling_during_a_download_changes_nothing() {
    let (root, paths) = setup();
    let folder = native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
    let keyring = keyring();
    save_login(&paths, &keyring);
    let server = crate::gog_fake::product_server_streaming(vec![native_product(
        RIMWORLD, "RimWorld", "1.1",
    )]);
    let Planned::Update(pending) =
        plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap()
    else {
        panic!("expected an update");
    };
    let before = wineo_core::gog::folder::peek(&paths, &folder).unwrap();
    let token = Token::new();
    let total = format_bytes(crate::gog_fake::FILE_SIZE);
    let started = format!("Downloading RimWorld: {} / {total}", format_bytes(0));
    let finished = format!("Downloading RimWorld: {total} / {total}");
    let mut said = Vec::new();
    let failure = apply_with(
        &paths,
        &keyring,
        server.client(),
        &pending,
        &token,
        |line| {
            // Some bytes are in, and the rest aren't yet.
            if line.starts_with("Downloading") && line != started && line != finished {
                token.cancel();
            }
            said.push(line);
        },
    )
    .unwrap_err();
    assert_eq!(failure, Failure::Canceled { journal: false }, "{said:?}");
    assert_eq!(failure.message("rimworld"), "Update canceled.");
    assert_eq!(said[0], started);
    assert!(!said.contains(&finished), "{said:?}");
    assert!(
        !said.iter().any(|line| line.starts_with("Verifying")),
        "{said:?}"
    );
    assert_eq!(
        wineo_core::gog::folder::peek(&paths, &folder).unwrap(),
        before
    );
}

#[test]
fn progress_lines_read_like_the_cli_and_are_paced() {
    let (_root, _paths, pending) = planned();
    let mut reporter = Reporter::new(pending.product_labels(), PROGRESS_INTERVAL);
    let download = |done| Progress::Download {
        product: RIMWORLD,
        file_id: "f".to_owned(),
        done,
        total: 7340032,
    };
    assert_eq!(
        reporter.line(&download(0)),
        Some(format!(
            "Downloading RimWorld: {} / {}",
            format_bytes(0),
            format_bytes(7340032)
        ))
    );
    // Too soon after the last one.
    assert_eq!(reporter.line(&download(1024)), None);
    assert!(reporter.line(&download(7340032)).is_some());
    assert_eq!(
        reporter
            .line(&Progress::Verify { product: RIMWORLD })
            .as_deref(),
        Some("Verifying RimWorld")
    );
    assert_eq!(
        reporter
            .line(&Progress::Stage {
                product: RIMWORLD,
                done: 0,
                total: 9
            })
            .as_deref(),
        Some("Unpacking RimWorld")
    );
    assert_eq!(
        reporter
            .line(&Progress::Publish { done: 0, total: 12 })
            .as_deref(),
        Some("Publishing 12 files")
    );
    assert_eq!(
        reporter
            .line(&Progress::Installing {
                product: RIMWORLD,
                interactive: false
            })
            .as_deref(),
        Some("Running RimWorld's installer")
    );
}

#[test]
fn a_failed_plan_shows_its_error() {
    let (_root, paths) = setup();
    let (mut page, _) = Page::new(paths, "rimworld".to_owned());
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Err(LOGGED_OUT.to_owned())));
    assert_eq!(page.error.as_deref(), Some(LOGGED_OUT));
    assert_eq!(page.step, Step::Planning);
    let _ = page.update(Event::Back);
    assert!(matches!(page.take_action(), Some(Action::Library { .. })));
}

#[test]
fn a_windows_update_shows_each_installer_and_offers_the_setup_window_once() {
    use super::super::gog_install::tests::{World, disco, inno};
    use wineo_core::gog::update::windows::set_graphical_session;

    let world = World::new();
    set_graphical_session(Some(true));
    world.installed(&disco(), &inno()).unwrap();
    world.disco("2", &world.fails_once(3), true);
    let name = "disco-elysium";
    // GOG's details are cached for an hour, so the newer ones are fetched.
    let refresh = Options {
        refresh: true,
        ..Options::default()
    };
    let pending = match plan_with_options(
        &world.paths,
        &world.keyring,
        world.server.client(),
        name,
        refresh.clone(),
        &Token::new(),
    )
    .unwrap()
    {
        Planned::Update(pending) => pending,
        Planned::UpToDate
        | Planned::Pinned(_)
        | Planned::Galaxy(_)
        | Planned::Follow(_)
        | Planned::NeedsPassword { .. }
        | Planned::Refused { .. } => {
            panic!("expected an update")
        }
    };
    let first = apply_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &pending,
        &Token::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        first,
        Failure::NotStarted {
            label: "Disco Elysium".to_owned(),
            message: "Disco Elysium's installer never started (exit code 3), so nothing changed."
                .to_owned(),
        }
    );
    let (mut page, _) = Page::new(world.paths.clone(), name.to_owned());
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Ok(Planned::Update(pending))));
    let _ = page.update(Event::Update);
    let generation = page.generation;
    let _ = page.update(Event::Finished(generation, Err(first)));
    assert_eq!(page.step, Step::Question);
    for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view(Palette::DARK);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        assert!(probe.outside(inset).is_empty(), "{width}x{height}");
    }
    let _ = page.update(Event::SetupWindow(true));
    assert_eq!(page.step, Step::Planning);
    assert!(page.wizard);
    let again = plan_with_options(
        &world.paths,
        &world.keyring,
        world.server.client(),
        name,
        Options {
            wizard: true,
            ..Options::default()
        },
        &Token::new(),
    )
    .unwrap();
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Ok(again)));
    assert_eq!(page.step, Step::Working, "the setup window runs right away");
    let Some(Ready::Installer(pending)) = page.ready.clone() else {
        panic!("expected an installer update");
    };
    assert!(pending.wizard());
    let mut lines = Vec::new();
    let updated = apply_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &pending,
        &Token::new(),
        |line| lines.push(line),
    )
    .unwrap();
    assert!(
        lines.contains(&"Opening Disco Elysium's setup window".to_owned()),
        "{lines:?}"
    );
    let generation = page.generation;
    let _ = page.update(Event::Finished(generation, Ok(Box::new(updated))));
    assert_eq!(page.step, Step::Finished);
    assert_eq!(page.take_updated().as_deref(), Some(name));

    // A plain update runs the installer silently and says so.
    world.disco("3", "", true);
    let Planned::Update(pending) = plan_with_options(
        &world.paths,
        &world.keyring,
        world.server.client(),
        name,
        refresh,
        &Token::new(),
    )
    .unwrap() else {
        panic!("expected an update");
    };
    let mut lines = Vec::new();
    apply_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &pending,
        &Token::new(),
        |line| lines.push(line),
    )
    .unwrap();
    assert!(
        lines.contains(&"Running Disco Elysium's installer".to_owned()),
        "{lines:?}"
    );
    set_graphical_session(None);
}

#[test]
fn an_interrupted_install_shows_finish_install_and_finishes_here() {
    use super::super::gog_install::tests::{World, disco, inno};
    use crate::gog_library::{Badge, Folders, badge};
    use wineo_core::environment::Environment;
    use wineo_core::gog::update::windows::set_graphical_session;

    let world = World::new();
    set_graphical_session(Some(true));
    world.disco("1", &world.fails_once(4), true);
    let failure = world.installed(&disco(), &inno()).unwrap_err();
    assert!(format!("{failure:?}").contains(
        "Disco Elysium's installer stopped partway (exit code 4). Use Finish install on disco-elysium in the library."
    ), "{failure:?}");
    let name = "disco-elysium";
    let env = Environment::load(&world.paths, name).unwrap();
    let card = Folders::read(&world.paths)
        .card(&world.paths, &env)
        .unwrap();
    assert!(card.unfinished && card.install);
    assert_eq!(card.finish_label(), "Finish install");
    assert_eq!(badge(Some(&card), None), Some(Badge::FinishInstall));
    assert_eq!(Badge::FinishInstall.label(), "Finish install");

    let Planned::Update(pending) = plan_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        name,
        &Token::new(),
    )
    .unwrap() else {
        panic!("expected the install to finish");
    };
    assert_eq!(pending.action(), "Finish the interrupted install of");
    let (mut page, _) = Page::new(world.paths.clone(), name.to_owned());
    let generation = page.generation;
    let _ = page.update(Event::Planned(
        generation,
        Ok(Planned::Update(pending.clone())),
    ));
    let _ = page.update(Event::Update);
    let generation = page.generation;
    let _ = page.update(Event::Finished(
        generation,
        Err(Failure::Canceled { journal: true }),
    ));
    assert_eq!(
        page.error.as_deref(),
        Some("Install interrupted. Use Finish install on disco-elysium to finish it.")
    );
    let updated = apply_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &pending,
        &Token::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        result_lines(&pending, &updated)[0],
        "Finished the interrupted install of disco-elysium."
    );
    let env = Environment::load(&world.paths, name).unwrap();
    let card = Folders::read(&world.paths)
        .card(&world.paths, &env)
        .unwrap();
    assert!(!card.unfinished);
    set_graphical_session(None);
}

mod galaxy;
