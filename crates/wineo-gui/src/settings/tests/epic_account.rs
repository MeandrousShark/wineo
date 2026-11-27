//! The Epic section's login, logout, and Legendary download outlive the
//! page that started them, like GOG's: the result shows when the page
//! opens again, and nothing else starts meanwhile.

use super::*;
use crate::epic_legendary::{Event as OfferEvent, Failure, Offer};
use crate::settings::epic as section;
use crate::sidebar::Section;

fn epic_texts(app: &App) -> Vec<String> {
    crate::layout_probe::texts(
        view::epic_row(
            &app.settings.as_ref().unwrap().epic,
            false,
            theme::Palette::DARK,
        ),
        iced::Size::new(700.0, 600.0),
    )
}

#[test]
fn an_epic_logout_lands_after_leaving_and_returning() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::Epic(section::Event::Logout)));
    let page = app.settings.as_ref().unwrap();
    assert_eq!(page.epic.busy, Some(section::Busy::LoggingOut));
    assert!(app.epic_work.pending());
    let (generation, sequence) = (page.generation, page.epic.busy_sequence.unwrap());
    let _ = app.update(Message::Section(Section::Trash));
    assert!(app.settings.is_none());
    reopen_global(&mut app, &paths);
    assert_eq!(
        app.settings.as_ref().unwrap().epic.busy,
        Some(section::Busy::LoggingOut)
    );
    let _ = app.update(Message::Settings(Event::Epic(section::Event::LoginOpen)));
    assert_eq!(
        app.settings.as_ref().unwrap().epic.busy,
        Some(section::Busy::LoggingOut)
    );
    let _ = app.update(Message::Section(Section::Trash));
    let _ = app.update(Message::Settings(Event::Epic(section::Event::LoggedOut(
        generation,
        sequence,
        None,
        Ok("Removed the Epic login.".to_owned()),
    ))));
    assert!(!app.epic_work.pending());
    reopen_global(&mut app, &paths);
    assert_eq!(app.settings.as_ref().unwrap().epic.busy, None);
    assert!(epic_texts(&app).contains(&"Removed the Epic login.".to_owned()));
}

#[test]
fn a_legendary_download_keeps_going_when_the_page_is_left() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let missing = wineo_core::epic::legendary::Missing {
        older_on_path: None,
    };
    app.settings.as_mut().unwrap().epic.offer = Some(Offer::new(&missing));
    let _ = app.update(Message::Settings(Event::Epic(section::Event::Offer(
        0,
        OfferEvent::Download,
    ))));
    let page = app.settings.as_ref().unwrap();
    assert_eq!(page.epic.busy, Some(section::Busy::Downloading));
    let sequence = page.epic.busy_sequence.unwrap();
    assert!(app.epic_work.pending());
    // Closing the window doesn't wait for it, as for a login.
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(!app.close_prompt);

    let _ = app.update(Message::Section(Section::Trash));
    // Its progress has no page to go to.
    let _ = app.update(Message::Settings(Event::Epic(section::Event::Offer(
        sequence,
        OfferEvent::Progress(1, 5, 10),
    ))));
    reopen_global(&mut app, &paths);
    assert!(epic_texts(&app).contains(&"Downloading Legendary 0.21.1...".to_owned()));
    let _ = app.update(Message::Section(Section::Trash));
    let _ = app.update(Message::Settings(Event::Epic(section::Event::Offer(
        sequence,
        OfferEvent::Done(
            1,
            Err(Failure::Failed(
                "the download is no longer there".to_owned(),
            )),
        ),
    ))));
    assert!(!app.epic_work.pending());
    reopen_global(&mut app, &paths);
    let shown = epic_texts(&app);
    assert!(
        shown.contains(&"the download is no longer there".to_owned()),
        "{shown:?}"
    );
    assert_eq!(app.settings.as_ref().unwrap().epic.busy, None);
}
