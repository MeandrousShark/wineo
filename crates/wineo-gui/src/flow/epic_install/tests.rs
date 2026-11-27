use std::fs;
use std::path::PathBuf;

use wineo_core::environment::Environment;
use wineo_core::epic::Version;

use super::*;
use crate::epic_fake::Epic;
use crate::epic_legendary::{Event as OfferEvent, Failure};

pub(in crate::flow) fn offline() -> Game {
    Game {
        app: "AppOffline".to_owned(),
        title: "Offline Game".to_owned(),
        unfinished: false,
    }
}

fn no_extras(_: &Paths, _: &Plan, _: &str, _: &Token) -> (Vec<String>, Vec<String>) {
    (vec!["Added Epic art: poster.".to_owned()], Vec::new())
}

/// A logged-in host with a runner a new environment can use.
pub(in crate::flow) fn world() -> Epic {
    let epic = Epic::new();
    epic.log_in();
    crate::gog_fake::inno_runner(&epic.paths, &epic.file("inno-runs"));
    epic
}

fn choices() -> Choices {
    Choices {
        runner: Some("inno".to_owned()),
        ..Choices::default()
    }
}

fn review(epic: &Epic, game: &Game, choices: &Choices) -> Review {
    match prepare_with(&epic.paths, game, choices, &Token::new()).unwrap() {
        Prepared::Review(review) => *review,
        Prepared::Offer(_) => panic!("no offer with the fake set"),
    }
}

fn texts(page: &Install) -> Vec<String> {
    let (body, footer) = page.view(iced::widget::Column::new(), Palette::DARK);
    let mut texts = crate::layout_probe::texts(body.into(), iced::Size::new(800.0, 1200.0));
    if let Some(footer) = footer {
        texts.extend(crate::layout_probe::texts(
            footer,
            iced::Size::new(800.0, 100.0),
        ));
    }
    texts
}

fn page_with(epic: &Epic, game: Game, review: Review) -> Install {
    let (mut page, _) = Install::new(epic.paths.clone(), game);
    page.choices = review.choices.clone();
    let generation = page.generation;
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Prepared::Review(Box::new(review))),
    ));
    page
}

#[test]
fn the_review_shows_what_the_terminal_prints() {
    let epic = world();
    let game = Game {
        app: "Dlc Game".to_owned(),
        title: "Dlc Game".to_owned(),
        unfinished: false,
    };
    let review = review(&epic, &game, &choices());
    let plan = review.plan.as_ref().unwrap();
    assert_eq!(plan.environment(), "dlc-game");
    assert_eq!(plan.folder(), epic.games().join("Dlc Game"));
    let rows = plan_rows(plan);
    assert_eq!(rows[0].0, "Version");
    assert_eq!(rows[1].0, "DLC");
    assert_eq!(rows[2].0, "", "the second DLC sits under the first");
    assert!(rows[3].1.ends_with(" plus 2 DLC"), "{rows:?}");
    assert_eq!(rows[3].0, "Download");
    assert_eq!(rows[4].0, "Installed");
    let page = page_with(&epic, game.clone(), review.clone());
    let mut typed = page_with(&epic, game.clone(), review.clone());
    let _ = typed.update(Event::Folder("/elsewhere".to_owned()));
    assert!(!typed.ready(), "a typed folder counts once it's planned");
    assert!(texts(&typed).contains(&"Press Enter to check the new folder.".to_owned()));
    let shown = texts(&page);
    for line in [
        "Review Epic install",
        "Environment",
        "dlc-game (new Windows environment)",
        "Folder",
        "Browse...",
        "Runner",
        "Arch",
        "Install",
    ] {
        assert!(shown.contains(&line.to_owned()), "{line}: {shown:?}");
    }
    assert!(epic.sweep().is_empty());
}

#[test]
fn a_game_installs_end_to_end_and_returns_to_its_card() {
    let epic = world();
    let review = review(&epic, &offline(), &choices());
    let plan = review.plan.clone().unwrap();
    let mut said = Vec::new();
    let done = install_with(
        &epic.paths,
        &plan,
        install::STALL,
        &Token::new(),
        |line| said.push(format!("{line:?}")),
        no_extras,
    )
    .unwrap();
    assert_eq!(
        done.lines,
        [
            "Installed Offline Game as offline-game.",
            "Added Epic art: poster."
        ]
    );
    assert!(
        said.iter()
            .any(|line| line.contains("Downloading: Offline Game")),
        "{said:?}"
    );
    let environment = Environment::load(&epic.paths, "offline-game").unwrap();
    assert_eq!(environment.config.epic_id.as_deref(), Some("AppOffline"));

    let mut page = page_with(&epic, offline(), review);
    let generation = page.generation;
    page.stage = Stage::Working;
    let _ = page.update(Event::Finished(generation, Ok(Box::new(done))));
    let shown = texts(&page);
    assert!(shown.contains(&"Epic install finished".to_owned()));
    assert!(shown.contains(&"Installed Offline Game as offline-game.".to_owned()));
    let _ = page.update(Event::BackToLibrary);
    assert!(matches!(
        page.take_leave(),
        Some(Leave::Library(Some(name))) if name == "offline-game"
    ));
    assert!(epic.sweep().is_empty());
}

#[test]
fn canceling_leaves_the_install_to_finish_and_finishing_says_so() {
    let epic = world();
    let review = review(&epic, &offline(), &choices());
    let plan = review.plan.unwrap();
    epic.set_mode("AppOffline", "hang");
    let token = Token::new();
    let (paths, worker_token) = (epic.paths.clone(), token.clone());
    let worker = std::thread::spawn(move || {
        install_with(
            &paths,
            &plan,
            install::STALL,
            &worker_token,
            |_| {},
            no_extras,
        )
    });
    epic.wait_started("AppOffline");
    token.cancel();
    assert_eq!(
        worker.join().unwrap().unwrap_err(),
        "Install interrupted. Install it again here to finish it."
    );
    fs::remove_file(epic.file("mode-AppOffline")).unwrap();
    let unfinished = Game {
        unfinished: true,
        ..offline()
    };
    let review = review_of(&epic, &unfinished);
    assert!(review.plan.as_ref().unwrap().resumes());
    let page = page_with(&epic, unfinished, review);
    let shown = texts(&page);
    assert!(shown.contains(&"Finish install".to_owned()), "{shown:?}");
    // A resumed install goes on as it began.
    assert!(!shown.contains(&"Browse...".to_owned()));
    assert!(epic.sweep().is_empty());
}

fn review_of(epic: &Epic, game: &Game) -> Review {
    review(epic, game, &Choices::default())
}

/// A plan for `game` with the test runner, ready to install.
pub(in crate::flow) fn install_review(epic: &Epic, game: &Game) -> Plan {
    *review(epic, game, &choices()).plan.unwrap()
}

#[test]
fn working_shows_progress_a_stall_and_cancel() {
    let epic = world();
    let review = review(&epic, &offline(), &choices());
    let mut page = page_with(&epic, offline(), review);
    let _ = page.update(Event::Install);
    assert_eq!(page.stage, Stage::Working);
    assert!(page.busy());
    let generation = page.generation;
    let _ = page.update(Event::Progress(
        generation,
        "Downloading 2 of 3: 37%".to_owned(),
    ));
    let _ = page.update(Event::Stalled(generation, 5));
    let shown = texts(&page);
    assert!(
        shown.contains(&"Downloading 2 of 3: 37%".to_owned()),
        "{shown:?}"
    );
    assert!(shown.contains(&stalled_line(5)));
    assert!(shown.contains(&"Cancel".to_owned()));
    // Progress clears the stall.
    let _ = page.update(Event::Progress(
        generation,
        "Downloading 2 of 3: 38%".to_owned(),
    ));
    assert!(!texts(&page).contains(&stalled_line(5)));
    // Back waits while it works.
    let _ = page.back();
    assert!(page.take_leave().is_none());
    let _ = page.update(Event::Cancel);
    let shown = texts(&page);
    assert!(shown.contains(&"Canceling...".to_owned()));
    assert!(shown.contains(&"Force stop".to_owned()));
    let _ = page.update(Event::Progress(
        generation,
        "Downloading 2 of 3: 39%".to_owned(),
    ));
    assert_eq!(
        page.status, "Canceling...",
        "a canceled run's progress is dropped"
    );
    let _ = page.update(Event::Finished(
        generation,
        Err("Install interrupted. Install it again here to finish it.".to_owned()),
    ));
    assert!(!page.busy());
    let shown = texts(&page);
    assert!(shown.contains(&"Install interrupted. Install it again here to finish it.".to_owned()));
    assert!(shown.contains(&"Back to library".to_owned()));
}

#[test]
fn with_no_legendary_it_asks_first_and_no_cancels_the_install() {
    let epic = world();
    let missing = Missing {
        older_on_path: Some((PathBuf::from("/usr/bin/legendary"), Version::new(0, 20, 34))),
    };
    let (mut page, _) = Install::new(epic.paths.clone(), offline());
    let generation = page.generation;
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Prepared::Offer(missing.clone())),
    ));
    assert_eq!(page.stage, Stage::Asking);
    let shown = texts(&page);
    assert!(shown.contains(
        &"Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?".to_owned()
    ));
    let _ = page.update(Event::Offer(OfferEvent::Decline));
    assert_eq!(page.stage, Stage::Finished);
    assert!(texts(&page).contains(&"Install canceled.".to_owned()));

    let (mut page, _) = Install::new(epic.paths.clone(), offline());
    let generation = page.generation;
    let _ = page.update(Event::Prepared(
        generation,
        Ok(Prepared::Offer(missing.clone())),
    ));
    let _ = page.update(Event::Offer(OfferEvent::Download));
    assert!(page.busy(), "the download keeps the page");
    let _ = page.back();
    assert!(page.take_leave().is_none());
    let _ = page.update(Event::Offer(OfferEvent::Done(
        1,
        Err(Failure::Failed(
            "the download is no longer there".to_owned(),
        )),
    )));
    assert_eq!(page.stage, Stage::Finished);
    assert!(texts(&page).contains(&"the download is no longer there".to_owned()));

    let (mut page, _) = Install::new(epic.paths.clone(), offline());
    let generation = page.generation;
    let _ = page.update(Event::Prepared(generation, Ok(Prepared::Offer(missing))));
    let _ = page.update(Event::Offer(OfferEvent::Download));
    let _ = page.update(Event::Offer(OfferEvent::Done(1, Ok(()))));
    assert_eq!(
        page.stage,
        Stage::Preparing,
        "it plans once Legendary is there"
    );
    assert!(page.waiting());
}

#[test]
fn an_installed_game_and_a_logged_out_one_explain_themselves() {
    let epic = world();
    let review = review(&epic, &offline(), &choices());
    install_with(
        &epic.paths,
        review.plan.as_ref().unwrap(),
        install::STALL,
        &Token::new(),
        |_| {},
        no_extras,
    )
    .unwrap();
    let error = match prepare_with(&epic.paths, &offline(), &choices(), &Token::new()) {
        Err(error) => error,
        Ok(prepared) => panic!("{prepared:?}"),
    };
    assert!(
        error.starts_with("Already installed. offline-game owns "),
        "{error}"
    );

    let epic = world();
    fs::remove_file(epic.state().join("user.json")).unwrap();
    let error = match prepare_with(&epic.paths, &offline(), &choices(), &Token::new()) {
        Err(error) => error,
        Ok(prepared) => panic!("{prepared:?}"),
    };
    assert_eq!(
        error,
        "Not logged in to Epic. Log in with wineo import epic --login."
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn lines_say_each_run_and_its_whole_percentage_once() {
    let mut lines = Lines::default();
    let said = |said: Option<Said>| said.map(|said| format!("{said:?}"));
    assert_eq!(
        said(lines.line(&InstallEvent::Started {
            index: 2,
            total: 3,
            title: "Expansion".to_owned()
        })),
        Some("Line(\"Downloading 2 of 3: Expansion\")".to_owned())
    );
    let progress = |percent| InstallEvent::Progress {
        index: 2,
        total: 3,
        percent,
    };
    assert_eq!(
        said(lines.line(&progress(37.2))),
        Some("Line(\"Downloading 2 of 3: 37%\")".to_owned())
    );
    assert_eq!(said(lines.line(&progress(37.9))), None);
    assert_eq!(
        said(lines.line(&InstallEvent::Started {
            index: 1,
            total: 1,
            title: "Game".to_owned()
        })),
        Some("Line(\"Downloading: Game\")".to_owned())
    );
    assert_eq!(
        said(lines.line(&InstallEvent::Working { index: 1, total: 1 })),
        None
    );
    assert_eq!(
        said(lines.line(&InstallEvent::Stalled {
            index: 1,
            total: 1,
            minutes: 5
        })),
        Some("Stalled(5)".to_owned())
    );
}

/// Runs `task` on a thread of its own, as the app's runtime would, and
/// returns what it sent.
pub(crate) fn run_in_background<M: Send + 'static>(
    task: Task<M>,
) -> std::thread::JoinHandle<Vec<M>> {
    std::thread::spawn(move || {
        let mut sent = Vec::new();
        crate::test_tasks::each_action(task, |action| {
            if let iced_runtime::Action::Output(message) = action {
                sent.push(message);
            }
        });
        sent
    })
}

#[test]
fn back_or_leaving_stops_the_reviews_legendary_call_so_nothing_waits_on_it() {
    use std::time::{Duration, Instant};
    for leave in ["back", "drop"] {
        let epic = world();
        let hanging = epic.hang("info");
        let (mut page, task) = Install::new(epic.paths.clone(), offline());
        assert!(page.busy(), "closing the window waits for it");
        assert!(!page.working(), "Back doesn't");
        let worker = run_in_background(task);
        epic.wait_for(&hanging);
        if leave == "back" {
            let _ = page.back();
            assert!(matches!(page.take_leave(), Some(Leave::Games)));
            assert!(page.job.all_canceled());
        } else {
            drop(page);
        }
        // The canceled `info` gives up the Epic lock a listing needs.
        fs::remove_file(epic.file("hang-info")).unwrap();
        let started = Instant::now();
        let listing = crate::flow::epic::load_listing(&epic.paths, &Token::new()).unwrap();
        assert!(!listing.games.is_empty());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{leave}: the listing waited {:?}",
            started.elapsed()
        );
        let _ = worker.join().unwrap();
        assert!(epic.sweep().is_empty(), "{leave}");
    }
}

#[test]
fn a_review_that_no_choice_can_fix_shows_only_why() {
    let epic = world();
    // Only `info` says another launcher manages it.
    let game = Game {
        app: "AppThirdParty".to_owned(),
        title: "Third Party Game".to_owned(),
        unfinished: false,
    };
    let fatal = review(&epic, &game, &choices());
    assert!(fatal.fatal);
    let error = fatal.plan.as_ref().unwrap_err().clone();
    let shown = texts(&page_with(&epic, game, fatal));
    assert!(shown.contains(&error), "{shown:?}");
    for control in ["Browse...", "Runner", "Arch"] {
        assert!(!shown.contains(&control.to_owned()), "{control}: {shown:?}");
    }
    // A folder in the way is fixed by choosing another.
    let folder = epic.games().join("Offline Game");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("save.dat"), "x").unwrap();
    let blocked = review(&epic, &offline(), &choices());
    assert!(!blocked.fatal);
    assert!(blocked.plan.is_err());
    assert!(texts(&page_with(&epic, offline(), blocked)).contains(&"Browse...".to_owned()));
    assert!(epic.sweep().is_empty());
}

#[test]
fn a_stopped_install_whose_environment_exists_points_at_its_card() {
    let epic = world();
    assert_eq!(
        finish_hint(&epic.paths, "offline-game"),
        "Install it again here to finish it."
    );
    let config = epic.paths.environment_config("offline-game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "schema_version = 1\nkind = \"native\"\n").unwrap();
    assert_eq!(
        finish_hint(&epic.paths, "offline-game"),
        "Use Finish install on offline-game in the library."
    );
}

#[test]
fn closing_the_window_during_an_install_or_a_legendary_download_asks_then_cancels() {
    let epic = world();
    let review = review(&epic, &offline(), &choices());
    let mut installing = page_with(&epic, offline(), review);
    let _ = installing.update(Event::Install);
    let (mut downloading, _) = Install::new(epic.paths.clone(), offline());
    let generation = downloading.generation;
    let _ = downloading.update(Event::Prepared(
        generation,
        Ok(Prepared::Offer(Missing {
            older_on_path: None,
        })),
    ));
    let _ = downloading.update(Event::Offer(OfferEvent::Download));
    for page in [installing, downloading] {
        let mut app = crate::App::boot(epic.paths.clone()).0;
        let flow = crate::flow::epic::tests::page_holding(epic.paths.clone(), page);
        assert!(flow.busy());
        app.flow = Some(super::super::Page::Epic(Box::new(flow)));
        let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
        assert!(app.close_prompt);
        let _ = app.update(crate::Message::CancelAndClose);
        assert!(app.closing);
        let Some(super::super::Page::Epic(page)) = &app.flow else {
            panic!("the page stays until its work ends");
        };
        assert!(page.install_canceled());
    }
    assert!(epic.sweep().is_empty());
}

#[test]
fn a_game_already_on_disk_running_elsewhere_or_not_owned_is_unrecoverable() {
    use std::path::PathBuf;
    let fatal = [
        InstallError::Downloaded {
            app: "AppOffline".to_owned(),
            folder: PathBuf::from("/games/Offline Game"),
        },
        InstallError::Running {
            title: "Offline Game".to_owned(),
        },
        InstallError::NotOwned {
            app: "AppOffline".to_owned(),
        },
    ];
    for error in fatal {
        let shown = error.to_string();
        assert!(unrecoverable(&anyhow::Error::from(error)), "{shown}");
    }
    let fixable = InstallError::NotEmpty {
        folder: PathBuf::from("/games/Offline Game"),
    };
    assert!(!unrecoverable(&anyhow::Error::from(fixable)));
}
