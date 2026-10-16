//! The update page on the Galaxy track, against a fake GOG with a content
//! system.

use std::fs;

use wineo_core::gog::account::Session;
use wineo_core::gog::update::galaxy::{self as core_galaxy, update as galaxy_update};

use super::super::*;
use crate::galaxy_fake::{Galaxy, NAME};

fn plan(world: &Galaxy, options: Options) -> Result<Planned, String> {
    plan_with_options(
        &world.paths,
        &world.keyring,
        world.server.client(),
        NAME,
        Options {
            refresh: true,
            ..options
        },
        &Token::new(),
    )
}

/// The game installed at 1.2.76 with 1.4.11 listed ahead of it, which
/// brings another runtime.
fn world() -> Galaxy {
    let world = Galaxy::new();
    world.install();
    world.newer("200", "1.4.11");
    world.runtime(b"vc 2", "0");
    world
}

fn planned_by(world: &Galaxy) -> Box<galaxy_update::Plan> {
    let Ok(Planned::Galaxy(plan)) = plan(world, Options::default()) else {
        panic!("expected a Galaxy update");
    };
    plan
}

fn planned() -> (Galaxy, Box<galaxy_update::Plan>) {
    let world = world();
    let plan = planned_by(&world);
    (world, plan)
}

fn apply(
    world: &Galaxy,
    plan: &galaxy_update::Plan,
    token: &Token,
    say: impl FnMut(String),
) -> Result<Updated, Failure> {
    apply_galaxy_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        plan,
        token,
        say,
    )
}

fn fits(page: &Page) {
    for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view(Palette::DARK);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
        assert!(probe.outside(inset).is_empty(), "{width}x{height}");
    }
}

#[test]
fn a_galaxy_update_reviews_its_build_files_and_notes() {
    let (world, plan) = planned();
    assert_eq!(plan.action(), "Update");
    let rows = galaxy_review_rows(&world.paths, &plan);
    assert_eq!(rows[0].0, "Folder");
    assert!(rows[0].1.ends_with("Games/Papers"), "{}", rows[0].1);
    assert_eq!(rows[1], ("Track".to_owned(), "Galaxy".to_owned()));
    assert_eq!(
        rows[2],
        ("Papers, Please".to_owned(), "1.2.76 -> 1.4.11".to_owned())
    );
    let last = rows.last().unwrap();
    assert_eq!(last.0, "Download");
    assert!(last.1.ends_with("B"), "{}", last.1);
    assert_eq!(
        rows[rows.len() - 2],
        ("Setup".to_owned(), "MSVC2017".to_owned())
    );
    assert_eq!(
        plan.note_lines(),
        ["Setup's own writes in the prefix aren't counted in the space check."]
    );

    let mut page = Page::new(world.paths.clone(), NAME.to_owned()).0;
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Ok(Planned::Galaxy(plan))));
    assert_eq!(page.step, Step::Review);
    fits(&page);
}

#[test]
fn a_galaxy_update_reports_progress_changes_and_files_moved_aside() {
    let (world, plan) = planned();
    // A file the player changed that the new build replaces is moved aside.
    fs::write(world.file("data/levels.dat"), "my own levels").unwrap();
    let mut said = Vec::new();
    let updated = apply(&world, &plan, &Token::new(), |line| said.push(line)).unwrap();
    assert!(
        said.iter().any(|line| line.starts_with("Downloading: ")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|line| line.starts_with("Running setup 1 of")),
        "{said:?}"
    );
    assert_eq!(
        fs::read_to_string(world.file("Papers.exe")).unwrap(),
        "MZ 1.4.11"
    );

    let lines = galaxy_result_lines(&world.paths, &plan, &updated);
    assert_eq!(
        lines,
        [
            format!("Updated {NAME}."),
            "Papers, Please: 1.2.76 -> 1.4.11".to_owned()
        ]
    );
    let lists = ready_lists(&Ready::Galaxy(plan.clone()), &updated);
    assert_eq!(
        lists[0].0,
        "Moved your files aside where the new version has files:"
    );
    assert_eq!(lists[0].1.len(), 1, "{lists:?}");
    assert!(
        lists[0].1[0].starts_with("data/levels.dat -> "),
        "{lists:?}"
    );

    let mut page = Page::new(world.paths.clone(), NAME.to_owned()).0;
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Ok(Planned::Galaxy(plan))));
    let _ = page.update(Event::Update);
    let generation = page.generation;
    let _ = page.update(Event::Finished(generation, Ok(Box::new(updated))));
    assert_eq!(page.step, Step::Finished);
    assert_eq!(page.take_updated().as_deref(), Some(NAME));
    fits(&page);
}

#[test]
fn canceling_a_galaxy_download_leaves_an_update_to_finish() {
    let (world, plan) = planned();
    let token = Token::new();
    let failure = apply(&world, &plan, &token, |line| {
        if line.starts_with("Downloading: ") {
            token.cancel();
        }
    })
    .unwrap_err();
    assert_eq!(failure, Failure::Canceled { journal: true });
    assert_eq!(
        failure.message(NAME),
        format!("Update interrupted. Use Finish update on {NAME} to finish it.")
    );
    assert!(update::journal_of(&world.paths, NAME).is_some());

    // The page plans the rest as an interrupted update and finishes it.
    let Ok(Planned::Galaxy(again)) = plan_with_options(
        &world.paths,
        &world.keyring,
        world.server.client(),
        NAME,
        Options::default(),
        &Token::new(),
    ) else {
        panic!("expected the update to finish");
    };
    assert_eq!(again.action(), "Finish the interrupted update of");
    let updated = apply(&world, &again, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        galaxy_result_lines(&world.paths, &again, &updated)[0],
        format!("Finished the interrupted update of {NAME}.")
    );
    assert!(update::journal_of(&world.paths, NAME).is_none());
}

#[test]
fn a_failed_setup_step_says_how_to_finish_once() {
    let world = world();
    world.runtime(b"vc 2", "1");
    let plan = planned_by(&world);
    let failure = apply(&world, &plan, &Token::new(), |_| {}).unwrap_err();
    let hint = format!("Use Finish update on {NAME} to finish it.");
    let Failure::Failed(text) = &failure else {
        panic!("{failure:?}");
    };
    assert!(
        text.starts_with("Setting up Papers, Please stopped: MSVC2017 failed"),
        "{text}"
    );
    assert!(text.ends_with(&hint), "{text}");
    assert_eq!(text.matches(&hint).count(), 1, "{text}");
    assert!(update::journal_of(&world.paths, NAME).is_some());
}

#[test]
fn logged_out_a_galaxy_update_asks_for_the_login() {
    let world = world();
    let keyring = crate::settings::steam_key::tests::TestKeyring(std::cell::RefCell::new(None));
    let error = plan_with_options(
        &world.paths,
        &keyring,
        world.server.client(),
        NAME,
        Options::default(),
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(error, LOGGED_OUT);
}

#[test]
fn a_repair_the_cli_left_is_finished_here_like_any_update() {
    let world = Galaxy::new();
    world.install();
    fs::write(world.file("Papers.exe"), "damaged").unwrap();
    // `wineo update --repair`, stopped by Ctrl+C once it starts fetching.
    let mut session = Session::open(&world.paths, &world.keyring, world.server.client())
        .unwrap()
        .unwrap();
    let Ok(galaxy_update::Planned::Update(repair)) = galaxy_update::plan(
        &world.paths,
        &mut session,
        NAME,
        Options {
            repair: true,
            refresh: true,
            ..Options::default()
        },
        &Token::new(),
    ) else {
        panic!("expected a repair");
    };
    assert_eq!(repair.action(), "Repair");
    let token = Token::new();
    let error = galaxy_update::apply(&world.paths, &mut session, &repair, &token, |step| {
        if matches!(step, core_galaxy::Progress::Download { .. }) {
            token.cancel();
        }
    })
    .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<UpdateError>(),
            Some(UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    drop(session);

    let Ok(Planned::Galaxy(plan)) = plan(&world, Options::default()) else {
        panic!("expected the repair to finish");
    };
    assert_eq!(plan.kind(), JournalKind::Repair);
    assert_eq!(plan.action(), "Finish the interrupted repair of");
    let updated = apply(&world, &plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        fs::read_to_string(world.file("Papers.exe")).unwrap(),
        "MZ 1.2.76"
    );
    assert_eq!(
        galaxy_result_lines(&world.paths, &plan, &updated)[0],
        format!("Finished the interrupted repair of {NAME}.")
    );
    assert!(update::journal_of(&world.paths, NAME).is_none());
}

#[test]
fn the_background_check_offers_a_galaxy_update_like_any_other() {
    use wineo_core::gog::update::Check;

    let world = world();
    let mut session = Session::open(&world.paths, &world.keyring, world.server.client())
        .unwrap()
        .unwrap();
    let batch = update::check_all_with(
        &mut session,
        Check {
            refresh: true,
            read_only: false,
        },
    )
    .unwrap();
    let status = batch
        .statuses
        .iter()
        .find(|status| status.environment == NAME)
        .unwrap();
    assert!(crate::gog_library::updatable(status), "{status:?}");
    assert_eq!(
        crate::gog_library::badge(None, Some(status)),
        Some(crate::gog_library::Badge::UpdateAvailable)
    );
}

/// The update to 1.4.11 canceled once it is downloading, which leaves its
/// journal.
fn interrupted(world: &Galaxy) {
    let plan = planned_by(world);
    let token = Token::new();
    let failure = apply(world, &plan, &token, |line| {
        if line.starts_with("Downloading: ") {
            token.cancel();
        }
    })
    .unwrap_err();
    assert_eq!(failure, Failure::Canceled { journal: true });
}

fn replan(world: &Galaxy, token: &Token) -> Result<Planned, String> {
    plan_with_options(
        &world.paths,
        &world.keyring,
        world.server.client(),
        NAME,
        Options::default(),
        token,
    )
}

#[test]
fn a_planning_failure_ends_with_how_to_finish_once_only_with_a_journal() {
    let world = world();
    world.fail_builds(true);
    let error = plan(&world, Options::default()).unwrap_err();
    assert!(!error.contains("Finish update"), "no journal: {error}");
    world.fail_builds(false);

    interrupted(&world);
    world.lose_record();
    world.fail_builds(true);
    let hint = format!("Use Finish update on {NAME} to finish it.");
    let error = replan(&world, &Token::new()).unwrap_err();
    assert!(error.ends_with(&hint), "{error}");
    assert_eq!(error.matches(&hint).count(), 1, "{error}");
}

#[test]
fn canceling_while_planning_says_whether_an_update_is_left_to_finish() {
    let world = world();
    let canceled = Token::new();
    canceled.cancel();
    assert_eq!(replan(&world, &canceled).unwrap_err(), "Update canceled.");
    interrupted(&world);
    assert_eq!(
        replan(&world, &canceled).unwrap_err(),
        format!("Update interrupted. Use Finish update on {NAME} to finish it.")
    );
}

#[test]
fn a_galaxy_install_journal_reaching_the_update_page_points_to_the_library() {
    let world = Galaxy::new();
    let token = Token::new();
    wineo_core::gog::update::windows::set_graphical_session(Some(true));
    let mut session = Session::open(&world.paths, &world.keyring, world.server.client())
        .unwrap()
        .unwrap();
    let plan =
        core_galaxy::install::plan(&world.paths, &mut session, &world.request(), &token).unwrap();
    core_galaxy::install::apply(&world.paths, &mut session, &plan, &token, |step| {
        if matches!(step, core_galaxy::Progress::Download { .. }) {
            token.cancel();
        }
    })
    .unwrap_err();
    drop(session);
    assert_eq!(
        replan(&world, &Token::new()).unwrap_err(),
        format!("{NAME}'s install didn't finish. Use Finish install on {NAME} in the library.")
    );
}

#[test]
fn a_retargeted_update_shows_the_warning_and_updates_when_asked() {
    let world = world();
    interrupted(&world);
    // The build it was updating to is gone from GOG's lists, with another
    // ahead of it.
    world.lose_record();
    world.hide("200");
    world.newer("300", "1.5.0");
    let Ok(Planned::Galaxy(plan)) = replan(&world, &Token::new()) else {
        panic!("expected the update to finish");
    };
    assert!(plan.retarget().is_some(), "{:?}", plan.warnings());
    assert!(
        plan.warnings()
            .iter()
            .any(|warning| warning.contains("300")),
        "{:?}",
        plan.warnings()
    );
    let mut page = Page::new(world.paths.clone(), NAME.to_owned()).0;
    let generation = page.generation;
    let _ = page.update(Event::Planned(
        generation,
        Ok(Planned::Galaxy(plan.clone())),
    ));
    assert_eq!(page.step, Step::Review);
    fits(&page);
    // The review's Update button is the acceptance.
    let _ = page.update(Event::Update);
    assert_eq!(page.step, Step::Working);
    let updated = apply(&world, &plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        fs::read_to_string(world.file("Papers.exe")).unwrap(),
        "MZ 1.5.0"
    );
    assert_eq!(
        galaxy_result_lines(&world.paths, &plan, &updated)[0],
        format!("Finished the interrupted update of {NAME}.")
    );
}

#[test]
fn a_pinned_game_is_told_it_is_pinned_instead_of_failing() {
    let world = world();
    // `wineo update --build 100`: the build it has, so only the pin changes.
    let mut session = Session::open(&world.paths, &world.keyring, world.server.client())
        .unwrap()
        .unwrap();
    let Ok(core_galaxy::PlannedUpdate::Galaxy(galaxy_update::Planned::Follow(pin))) =
        core_galaxy::plan_update(
            &world.paths,
            Some(&mut session),
            NAME,
            Options {
                choice: core_galaxy::Choice::Build("100".to_owned()),
                refresh: true,
                ..Options::default()
            },
            &Token::new(),
        )
    else {
        panic!("expected a change of pin");
    };
    core_galaxy::switch::apply(&world.paths, &pin, &Token::new()).unwrap();

    let planned = plan(&world, Options::default()).unwrap();
    let Planned::Pinned(message) = planned else {
        panic!("expected the pinned message");
    };
    assert_eq!(
        message,
        format!(
            "{NAME} is pinned to build 100 (1.2.76). Run wineo update {NAME} --latest to follow its branch again."
        )
    );
    let mut page = Page::new(world.paths.clone(), NAME.to_owned()).0;
    let generation = page.generation;
    let _ = page.update(Event::Planned(
        generation,
        Ok(Planned::Pinned(message.clone())),
    ));
    assert_eq!(page.step, Step::Finished);
    assert_eq!(page.note.as_deref(), Some(message.as_str()));
    assert_eq!(page.error, None);
    fits(&page);
}
