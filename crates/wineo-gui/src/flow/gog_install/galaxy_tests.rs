//! The install page on the Galaxy track, against a fake GOG with a content
//! system.

use wineo_core::gog::update::windows::set_graphical_session;

use super::*;
use crate::galaxy_fake::{GAME, Galaxy, NAME};

fn offer() -> Offer {
    Offer {
        gog_id: GAME,
        title: "Papers, Please".to_owned(),
        linux: false,
        windows: true,
        unfinished: None,
    }
}

fn choices(world: &Galaxy) -> Choices {
    Choices {
        runner: Some("galaxy".to_owned()),
        dir: world.folder().display().to_string(),
        ..Choices::default()
    }
}

fn prepared(world: &Galaxy, choices: &Choices, known: Option<bool>) -> Prepared {
    set_graphical_session(Some(true));
    prepare_known(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &offer(),
        choices,
        false,
        known,
        &Token::new(),
    )
    .unwrap()
}

fn builds_requests(world: &Galaxy) -> usize {
    world
        .server
        .requests()
        .iter()
        .filter(|request| request.path().ends_with("/os/windows/builds"))
        .count()
}

fn no_extras(_: &Paths, _: &GlobalConfig, _: &str, _: u64, _: &str, _: &Token) -> Extras {
    Extras::default()
}

fn install(
    world: &Galaxy,
    plan: &Plan,
    token: &Token,
    say: impl FnMut(String),
) -> Result<Done, Failure> {
    install_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        plan,
        plan,
        token,
        say,
        no_extras,
    )
}

fn fits(page: &Install) {
    for (width, height) in [(280.0, 240.0), (360.0, 480.0), (900.0, 720.0)] {
        let (body, footer) = page.view(iced::widget::column![], Palette::DARK);
        let mut view = iced::widget::column![super::heroic::scroll_body(body, None, Palette::DARK)]
            .height(Fill);
        if let Some(footer) = footer {
            view = view.push(footer);
        }
        let mut view: Element<'_, Event> = view.into();
        let inset = iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(width, height));
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        assert!(probe.outside(inset).is_empty(), "{width}x{height}");
    }
}

fn page(world: &Galaxy, prepared: Prepared) -> Install {
    let (mut page, _) = Install::new(world.paths.clone(), offer());
    let generation = page.generation;
    page.choices = prepared.choices.clone();
    let _ = page.update(Event::Prepared(generation, Ok(Box::new(prepared))));
    page
}

#[test]
fn the_review_lets_you_pick_the_track_when_both_are_possible() {
    let world = Galaxy::new();
    let first = prepared(&world, &choices(&world), None);
    assert!(first.both_tracks);
    let Ok(plan) = &first.plan else {
        panic!("{:?}", first.plan);
    };
    assert_eq!(plan.track(), Track::Galaxy);
    assert_eq!(plan.platform(), Platform::Windows);
    let Plan::Galaxy(galaxy) = plan.as_ref() else {
        panic!("expected Galaxy");
    };
    assert_eq!(
        galaxy.review_rows(&world.paths),
        [
            (GalaxyLabel::Version, "1.2.76".to_owned()),
            (GalaxyLabel::Setup, "MSVC2017".to_owned()),
            (GalaxyLabel::Download, format_bytes(galaxy.installed_size())),
        ]
    );

    let mut page = page(&world, first);
    assert_eq!(page.stage, Stage::Review);
    fits(&page);
    // Offline installer is picked: planned with that track, without asking
    // GOG for its builds again, and the picker stays to switch back.
    let before = builds_requests(&world);
    let _ = page.update(Event::Track(Track::Installer));
    assert_eq!(page.choices.track, Some(Track::Installer));
    let again = prepared(
        &world,
        &Choices {
            track: Some(Track::Installer),
            ..choices(&world)
        },
        page.both_tracks,
    );
    assert_eq!(builds_requests(&world), before);
    assert!(again.both_tracks);
    assert!(again.plan.is_err(), "the fake offers no installers");
    let generation = page.generation;
    let _ = page.update(Event::Prepared(generation, Ok(Box::new(again))));
    assert_eq!(page.both_tracks, Some(true));
    fits(&page);
    // Another platform plans again with no track and learns it afresh.
    let _ = page.update(Event::Platform(Platform::Windows));
    assert_eq!(page.choices.track, None);
    assert_eq!(page.both_tracks, None);
}

#[test]
fn a_linux_install_offers_no_track_picker() {
    let world = Galaxy::new();
    // Galaxy builds are only for Windows, so Linux has the installers alone.
    let linux = prepared(
        &world,
        &Choices {
            platform: Some(Platform::Linux),
            ..choices(&world)
        },
        None,
    );
    assert!(!linux.both_tracks);
    assert!(
        linux
            .plan
            .as_ref()
            .err()
            .is_some_and(|error| error.contains("installer")),
        "{:?}",
        linux.plan
    );
}

#[test]
fn a_galaxy_install_reports_progress_and_finishes_like_an_installer() {
    let world = Galaxy::new();
    let prepared = prepared(&world, &choices(&world), None);
    let plan = prepared.plan.clone().unwrap();
    let mut said = Vec::new();
    let done = install(&world, &plan, &Token::new(), |line| said.push(line)).unwrap();
    assert!(
        said.iter().any(|line| line.starts_with("Downloading: ")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|line| line.starts_with("Running setup 1 of 1")),
        "{said:?}"
    );
    assert!(
        said.contains(&"Adding GOG playtime and art for Papers, Please...".to_owned()),
        "{said:?}"
    );
    assert_eq!(done.environment, NAME);
    assert_eq!(
        done.lines[0],
        format!("Installed Papers, Please as {NAME}.")
    );
    assert!(done.programs.is_none(), "its info file names the program");
    assert_eq!(
        std::fs::read_to_string(world.file("Papers.exe")).unwrap(),
        "MZ 1.2.76"
    );
    let mut page = page(&world, prepared);
    let _ = page.update(Event::Install);
    assert_eq!(page.stage, Stage::Working);
    let generation = page.generation;
    let _ = page.update(Event::Finished(generation, Ok(Box::new(done))));
    assert_eq!(page.stage, Stage::Finished);
    fits(&page);
}

#[test]
fn a_canceled_galaxy_install_says_how_to_finish_and_the_library_finishes_it_here() {
    use crate::gog_library::{Badge, Folders, badge};
    use wineo_core::environment::Environment;

    let world = Galaxy::new();
    let prepared = prepared(&world, &choices(&world), None);
    let plan = prepared.plan.clone().unwrap();
    let token = Token::new();
    let failure = install(&world, &plan, &token, |line| {
        if line.starts_with("Downloading: ") {
            token.cancel();
        }
    })
    .unwrap_err();
    assert_eq!(
        failure.message,
        format!("Install interrupted. Use Finish install on {NAME} in the library.")
    );

    // The library's card offers Finish install, and it opens the install
    // page for this game rather than the update page.
    let env = Environment::load(&world.paths, NAME).unwrap();
    let gog = Folders::read(&world.paths)
        .card(&world.paths, &env)
        .unwrap();
    assert!(gog.unfinished && gog.install);
    assert_eq!(badge(Some(&gog), None), Some(Badge::FinishInstall));
    assert!(crate::gog_library::galaxy_install(&world.paths, NAME));
    let mut app = crate::App::boot(world.paths.clone()).0;
    let mut card = crate::tests::card(NAME, wineo_core::runtime::State::Inactive, 0);
    card.gog = Some(gog);
    app.cards = vec![card];
    let _ = app.update(crate::Message::GogUpdate(NAME.to_owned()));
    let Some(crate::flow::Page::Gog(gog_page)) = &app.flow else {
        panic!("expected the install page");
    };
    assert_eq!(gog_page.finishing_name(), Some(NAME));
    // Back leaves for the library, not for a grid it never listed.
    let Some(crate::flow::Page::Gog(gog_page)) = &mut app.flow else {
        unreachable!();
    };
    let _ = gog_page.back();
    assert!(matches!(
        gog_page.take_action(),
        Some(crate::flow::Action::Library { name: Some(name), launch: false }) if name == NAME
    ));

    // Finishing carries on the install where it stopped.
    set_graphical_session(Some(true));
    let resumed = prepare_known(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &Offer {
            unfinished: Some(NAME.to_owned()),
            ..offer()
        },
        &Choices::default(),
        false,
        None,
        &Token::new(),
    )
    .unwrap();
    let plan = resumed.plan.clone().unwrap();
    assert!(plan.resumes());
    assert_eq!(plan.track(), Track::Galaxy);
    let done = install(&world, &plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        done.lines[0],
        format!("Finished the interrupted install of {NAME}.")
    );
    let env = Environment::load(&world.paths, NAME).unwrap();
    let gog = Folders::read(&world.paths)
        .card(&world.paths, &env)
        .unwrap();
    assert!(!gog.unfinished);
    let page = page(&world, resumed);
    assert!(page.plan().is_some_and(Plan::resumes));
    fits(&page);
}

#[test]
fn a_failed_galaxy_setup_step_says_how_to_finish_once() {
    let world = Galaxy::new();
    world.runtime_exits("1");
    let prepared = prepared(&world, &choices(&world), None);
    let plan = prepared.plan.clone().unwrap();
    let failure = install(&world, &plan, &Token::new(), |_| {}).unwrap_err();
    let hint = format!("Use Finish install on {NAME} in the library.");
    assert!(
        failure
            .message
            .starts_with("Setting up Papers, Please stopped: MSVC2017 failed"),
        "{}",
        failure.message
    );
    assert!(failure.message.ends_with(&hint), "{}", failure.message);
    assert_eq!(failure.message.matches(&hint).count(), 1);
}

/// An install of the game canceled once it is downloading, which leaves its
/// journal.
fn interrupted(world: &Galaxy) {
    let prepared = prepared(world, &choices(world), None);
    let plan = prepared.plan.unwrap();
    let token = Token::new();
    let failure = install(world, &plan, &token, |line| {
        if line.starts_with("Downloading: ") {
            token.cancel();
        }
    })
    .unwrap_err();
    assert!(failure.message.starts_with("Install interrupted."));
}

fn resumed(world: &Galaxy, token: &Token) -> Result<Prepared, String> {
    prepare_known(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &Offer {
            unfinished: Some(NAME.to_owned()),
            ..offer()
        },
        &Choices::default(),
        false,
        None,
        token,
    )
}

#[test]
fn a_galaxy_plan_that_fails_keeps_the_picker_to_fall_back_to_the_installers() {
    let world = Galaxy::new();
    world.fail_manifests(true);
    let first = prepared(&world, &choices(&world), None);
    // GOG lists a build, but its file list can't be read.
    assert!(first.both_tracks);
    let galaxy_error = first.plan.as_ref().unwrap_err().clone();
    let mut page = page(&world, first);
    fits(&page);
    let before = builds_requests(&world);
    let _ = page.update(Event::Track(Track::Installer));
    let installers = prepared(
        &world,
        &Choices {
            track: Some(Track::Installer),
            ..choices(&world)
        },
        page.both_tracks,
    );
    assert_eq!(builds_requests(&world), before, "no second builds fetch");
    assert!(installers.both_tracks);
    // It planned on the installers, whose failure is its own.
    assert_ne!(installers.plan.unwrap_err(), galaxy_error);
}

#[test]
fn a_planning_failure_ends_with_how_to_finish_once_only_with_a_journal() {
    let world = Galaxy::new();
    world.fail_manifests(true);
    let fresh = prepared(&world, &choices(&world), None);
    let error = fresh.plan.unwrap_err();
    assert!(!error.contains("Finish install"), "{error}");
    world.fail_manifests(false);

    interrupted(&world);
    world.lose_record();
    world.fail_builds(true);
    let hint = format!("Use Finish install on {NAME} in the library.");
    let error = resumed(&world, &Token::new()).unwrap().plan.unwrap_err();
    assert!(error.ends_with(&hint), "{error}");
    assert_eq!(error.matches(&hint).count(), 1, "{error}");
}

#[test]
fn canceling_while_planning_says_whether_an_install_is_left_to_finish() {
    let world = Galaxy::new();
    let canceled = Token::new();
    canceled.cancel();
    let error = prepare_known(
        &world.paths,
        &world.keyring,
        world.server.client(),
        &offer(),
        &choices(&world),
        false,
        None,
        &canceled,
    )
    .unwrap_err();
    assert_eq!(error, "Install canceled.");

    interrupted(&world);
    let error = resumed(&world, &canceled).unwrap_err();
    assert_eq!(
        error,
        format!("Install interrupted. Use Finish install on {NAME} in the library.")
    );
}
