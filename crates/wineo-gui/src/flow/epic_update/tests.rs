use std::fs;

use wineo_core::epic::install;

use super::*;
use crate::epic_fake::Epic;
use crate::epic_legendary::Event as OfferEvent;
use crate::flow::epic_install::tests::{install_review, offline, world};

/// A logged-in host with the offline game installed at 1.0, while Epic
/// lists 0.24, so an update waits.
pub(crate) fn installed() -> Epic {
    let epic = world();
    let plan = install_review(&epic, &offline());
    crate::flow::epic_install::install_with(
        &epic.paths,
        &plan,
        install::STALL,
        &Token::new(),
        |_| {},
        |_, _, _, _| (Vec::new(), Vec::new()),
    )
    .unwrap();
    // The next run of the fake records the listed version.
    fs::write(epic.file("version-AppOffline"), "0.24").unwrap();
    // So waiting for the next run to start waits for that run.
    fs::remove_file(epic.file("installing-AppOffline")).unwrap();
    epic
}

fn ready(epic: &Epic) -> Plan {
    match plan_with(&epic.paths, "offline-game", &Token::new()).unwrap() {
        Planning::Ready(plan) => *plan,
        other => panic!("{other:?}"),
    }
}

fn texts(page: &Page) -> Vec<String> {
    crate::layout_probe::texts(page.view(Palette::DARK), iced::Size::new(800.0, 1000.0))
}

fn page_with(epic: &Epic, planning: Planning) -> Page {
    let (mut page, _) = Page::new(epic.paths.clone(), "offline-game".to_owned());
    let generation = page.generation;
    let _ = page.update(Event::Planned(generation, Ok(planning)));
    page
}

#[test]
fn an_update_is_reviewed_applied_and_reported_like_the_terminal() {
    let epic = installed();
    let plan = ready(&epic);
    assert_eq!(button_label(&plan), "Update");
    let rows = review_rows(&epic.paths, &plan);
    assert_eq!(
        rows[1],
        ("Offline Game".to_owned(), "1.0 -> 0.24".to_owned())
    );
    let page = page_with(&epic, Planning::Ready(Box::new(plan.clone())));
    let shown = texts(&page);
    for line in [
        "Review Epic update",
        "Offline Game",
        "1.0 -> 0.24",
        "Update",
        "Back",
    ] {
        assert!(shown.contains(&line.to_owned()), "{line}: {shown:?}");
    }
    let mut said = Vec::new();
    let done = update_with(&epic.paths, &plan, install::STALL, &Token::new(), |line| {
        said.push(format!("{line:?}"))
    })
    .unwrap();
    assert_eq!(done.lines[0], "Updated offline-game.");
    assert!(
        done.lines.contains(&"Offline Game: 1.0 -> 0.24".to_owned()),
        "{done:?}"
    );
    assert!(
        said.iter()
            .any(|line| line.contains("Downloading: Offline Game")),
        "{said:?}"
    );
    assert!(matches!(
        plan_with(&epic.paths, "offline-game", &Token::new()).unwrap(),
        Planning::UpToDate
    ));
    assert!(epic.sweep().is_empty());
}

#[test]
fn an_interrupted_update_says_which_button_finishes_it_and_finish_does() {
    let epic = installed();
    let plan = ready(&epic);
    epic.set_mode("AppOffline", "hang");
    let token = Token::new();
    let (paths, worker_token, worker_plan) = (epic.paths.clone(), token.clone(), plan.clone());
    let worker = std::thread::spawn(move || {
        update_with(&paths, &worker_plan, install::STALL, &worker_token, |_| {})
    });
    epic.wait_started("AppOffline");
    token.cancel();
    assert_eq!(
        worker.join().unwrap().unwrap_err(),
        "Update interrupted. Use Finish update on offline-game to finish it."
    );
    let canceled = Token::new();
    canceled.cancel();
    assert_eq!(
        update_with(&epic.paths, &plan, install::STALL, &canceled, |_| {}).unwrap_err(),
        "Update canceled."
    );
    fs::remove_file(epic.file("mode-AppOffline")).unwrap();
    let plan = ready(&epic);
    assert!(plan.resumes());
    assert_eq!(button_label(&plan), "Finish");
    let done = update_with(&epic.paths, &plan, install::STALL, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        done.lines[0],
        "Finished the interrupted update of offline-game."
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn the_page_works_cancels_and_tells_the_library_to_check_again() {
    let epic = installed();
    let mut page = page_with(&epic, Planning::Ready(Box::new(ready(&epic))));
    let _ = page.update(Event::Update);
    assert_eq!(page.stage, Stage::Working);
    assert!(page.busy());
    let generation = page.generation;
    let _ = page.update(Event::Progress(generation, "Downloading: 37%".to_owned()));
    let _ = page.update(Event::Stalled(generation, 5));
    let shown = texts(&page);
    assert!(shown.contains(&"Downloading: 37%".to_owned()), "{shown:?}");
    assert!(shown.contains(&crate::flow::epic_install::stalled_line(5)));
    // Back waits while it works.
    let _ = page.back();
    assert!(page.take_action().is_none());
    let _ = page.update(Event::Cancel);
    assert!(texts(&page).contains(&"Force stop".to_owned()));
    let _ = page.update(Event::Finished(
        generation,
        Err("Update interrupted. Use Finish update on offline-game to finish it.".to_owned()),
    ));
    assert_eq!(page.take_updated().as_deref(), Some("offline-game"));
    assert_eq!(page.take_updated(), None, "once");
    let shown = texts(&page);
    assert!(shown.contains(&"Epic update finished".to_owned()));
    assert!(shown.contains(&"Back to library".to_owned()));
    let _ = page.update(Event::BackToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { name: Some(name), launch: false }) if name == "offline-game"
    ));
    assert!(epic.sweep().is_empty());
}

#[test]
fn up_to_date_says_so_and_no_legendary_asks_first() {
    let epic = world();
    let page = page_with(&epic, Planning::UpToDate);
    assert!(texts(&page).contains(&"Up to date.".to_owned()));
    let missing = Missing {
        older_on_path: None,
    };
    let mut page = page_with(&epic, Planning::Offer(missing));
    assert!(texts(&page).contains(
        &"Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?".to_owned()
    ));
    let _ = page.update(Event::Offer(OfferEvent::Decline));
    assert!(texts(&page).contains(&"Update canceled.".to_owned()));
}
