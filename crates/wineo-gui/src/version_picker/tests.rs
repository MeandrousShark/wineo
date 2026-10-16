//! The Version picker against a fake GOG with a content system.

use std::ffi::OsString;

use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::cancel::Token;
use wineo_core::gog::update::galaxy::update as galaxy_update;
use wineo_core::gog::update::{Status, UpdateState};
use wineo_core::runtime::State;

use super::*;
use crate::flow;
use crate::galaxy_fake::{GAME, Galaxy, NAME};
use crate::gog_library::GogCard;
use crate::settings::steam_key::tests::TestKeyring;
use crate::tests::{app, card};

const PASSWORD: &str = "hunter2-private";

/// The game installed at 1.2.76 (build 100) with 1.4.11 (build 200)
/// listed ahead of it.
fn world() -> Galaxy {
    let world = Galaxy::new();
    world.install();
    world.newer("200", "1.4.11");
    world
}

fn load(world: &Galaxy, entered: Option<(&str, &Secret)>) -> Result<Loaded, String> {
    load_with(
        &world.paths,
        &world.keyring,
        world.server.client(),
        NAME,
        GAME,
        entered,
    )
}

fn listed(world: &Galaxy, entered: Option<(&str, &Secret)>) -> Listing {
    let Ok(Loaded::Galaxy(listing)) = load(world, entered) else {
        panic!("expected a listing");
    };
    *listing
}

fn row<'a>(listing: &'a Listing, build: &str) -> &'a Listed {
    listing
        .builds
        .iter()
        .find(|row| row.build == build)
        .unwrap()
}

/// Pins the game to `build` the way `wineo update --build` does.
fn pin(world: &Galaxy, build: &str) {
    let mut session = Session::open(&world.paths, &world.keyring, world.server.client())
        .unwrap()
        .unwrap();
    let Ok(core_galaxy::PlannedUpdate::Galaxy(galaxy_update::Planned::Follow(plan))) =
        core_galaxy::plan_update(
            &world.paths,
            Some(&mut session),
            NAME,
            Options {
                choice: Choice::Build(build.to_owned()),
                ..Options::default()
            },
            &Token::new(),
        )
    else {
        panic!("expected a change of pin");
    };
    core_galaxy::switch::apply(&world.paths, &plan, &Token::new()).unwrap();
}

fn game_card(unfinished: bool) -> Card {
    let mut game = card(NAME, State::Inactive, 0);
    game.kind = Some(CardKind::Wine);
    game.gog = Some(GogCard {
        gog_id: Some(GAME.to_string()),
        unfinished,
        blocked_by: None,
        install: false,
    });
    game
}

/// The app with the game's panel showing the picker, before anything
/// loaded.
fn opened(unfinished: bool) -> App {
    let mut app = app();
    app.cards = vec![game_card(unfinished)];
    let _ = app.update(Message::Version(Event::Open(NAME.to_owned())));
    app
}

/// `opened`, with `loaded` landed for the request it made.
fn showing(loaded: Loaded) -> App {
    let mut app = opened(false);
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(loaded),
    )));
    app
}

fn picker(app: &App) -> &Picker {
    app.version.as_ref().unwrap()
}

/// The options the update page was opened with, if it was.
fn reviewed(app: &App) -> Option<Options> {
    match &app.flow {
        Some(flow::Page::GogUpdate(page)) => Some(page.options().clone()),
        _ => None,
    }
}

fn fits(app: &App) {
    let mut view = app.version_content();
    let size = iced::Size::new(388.0, 2000.0);
    let probe = crate::layout_probe::probe(&mut view, size);
    let frame = iced::Rectangle::new(iced::Point::ORIGIN, size);
    assert!(probe.outside(frame).is_empty());
}

#[test]
fn the_listing_loads_and_a_stale_reply_is_dropped() {
    let world = world();
    let listing = listed(&world, None);
    assert_eq!(listing.builds.len(), 2);

    let mut app = opened(false);
    assert!(app.version_visible());
    assert!(picker(&app).busy);
    let generation = app.version_generation;
    // An answer for an earlier request changes nothing.
    let _ = app.update(Message::Version(Event::Loaded(
        generation.wrapping_sub(1),
        None,
        Ok(Loaded::Galaxy(Box::new(listing.clone()))),
    )));
    assert!(picker(&app).loaded.is_none() && picker(&app).busy);
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Galaxy(Box::new(listing))),
    )));
    let shown = picker(&app);
    assert!(!shown.busy);
    assert_eq!(shown.branch, None);
    let rows: Vec<&str> = shown.rows().iter().map(|row| row.build.as_str()).collect();
    assert_eq!(rows, ["200", "100"]);
    // The row in the details now says what the listing found.
    assert_eq!(app.version_value(NAME), "1.2.76");
    fits(&app);

    // Closing drops a reply still on its way.
    let _ = app.update(Message::Version(Event::Back));
    assert!(app.version.is_none() && app.panel_open);
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Err("late".to_owned()),
    )));
    assert!(app.version.is_none());
}

#[test]
fn the_newest_build_follows_its_branch_and_an_older_one_pins() {
    let world = world();
    let listing = listed(&world, None);
    let (latest, older) = (row(&listing, "200"), row(&listing, "100"));
    assert!(latest.latest && !older.latest && older.installed);
    assert_eq!(choice_for(&listing, latest), Choice::DefaultBranch);
    assert_eq!(choice_for(&listing, older), Choice::Build("100".to_owned()));
    assert!(!is_current(&listing, latest) && !is_current(&listing, older));

    // Pinned to the build it has: that row is what it has now, and the
    // newest one unpins.
    pin(&world, "100");
    let listing = listed(&world, None);
    assert!(is_current(&listing, row(&listing, "100")));
    assert_eq!(choice_for(&listing, row(&listing, "200")), Choice::Latest);

    // A private branch's newest build follows it.
    world.private("beta", PASSWORD, "300", "1.3.0-beta");
    let listing = listed(&world, Some(("beta", &Secret::new(PASSWORD))));
    assert_eq!(
        choice_for(&listing, row(&listing, "300")),
        Choice::Branch("beta".to_owned())
    );
}

#[test]
fn the_newest_build_installed_and_followed_offers_no_review() {
    let world = Galaxy::new();
    world.install();
    let listing = listed(&world, None);
    let only = row(&listing, "100");
    assert!(only.latest && only.installed);
    assert!(is_current(&listing, only));

    let mut app = showing(Loaded::Galaxy(Box::new(listing)));
    let _ = app.update(Message::Version(Event::Pick("100".to_owned())));
    assert_eq!(picker(&app).picked.as_deref(), Some("100"));
    assert_eq!(picker(&app).choice(), None);
    let _ = app.update(Message::Version(Event::Review));
    assert!(app.flow.is_none());
}

#[test]
fn each_action_opens_the_update_page_with_its_options() {
    let world = world();
    let listing = listed(&world, None);
    let review = |event: Event, prepare: &dyn Fn(&mut App)| {
        let mut app = showing(Loaded::Galaxy(Box::new(listing.clone())));
        prepare(&mut app);
        let _ = app.update(Message::Version(event));
        reviewed(&app)
    };
    let pick = |build: &'static str| {
        move |app: &mut App| {
            let _ = app.update(Message::Version(Event::Pick(build.to_owned())));
        }
    };
    let none = |_: &mut App| {};

    let older = review(Event::Review, &pick("100")).unwrap();
    assert_eq!(older.choice, Choice::Build("100".to_owned()));
    let newest = review(Event::Review, &pick("200")).unwrap();
    assert_eq!(newest.choice, Choice::DefaultBranch);
    // Nothing picked, nothing to review.
    assert_eq!(review(Event::Review, &none), None);

    let repair = review(Event::Repair, &none).unwrap();
    assert!(repair.repair && !repair.reinstall && repair.track.is_none());
    let installers = review(Event::MoveToInstallers, &none).unwrap();
    assert!(installers.reinstall && installers.track == Some(Track::Installer));
    for options in [&older, &newest, &repair, &installers] {
        assert!(options.check().is_ok());
    }
    // Move to Galaxy is only for a game on the installer track.
    assert_eq!(review(Event::MoveToGalaxy, &none), None);
}

#[test]
fn an_installer_track_game_offers_move_to_galaxy_only_when_gog_has_a_build() {
    let world = Galaxy::new();
    world.install();
    world.on_installer_track();
    assert!(matches!(
        load(&world, None),
        Ok(Loaded::Installer { galaxy: true })
    ));
    let mut app = showing(Loaded::Installer { galaxy: true });
    fits(&app);
    let _ = app.update(Message::Version(Event::MoveToGalaxy));
    let options = reviewed(&app).unwrap();
    assert!(options.reinstall && options.track == Some(Track::Galaxy));
    assert!(options.check().is_ok());
    // Nothing on Galaxy's side is offered.
    let mut app = showing(Loaded::Installer { galaxy: true });
    let _ = app.update(Message::Version(Event::Repair));
    let _ = app.update(Message::Version(Event::MoveToInstallers));
    assert!(app.flow.is_none());

    world.hide("100");
    assert!(matches!(
        load(&world, None),
        Ok(Loaded::Installer { galaxy: false })
    ));
    let mut app = showing(Loaded::Installer { galaxy: false });
    let _ = app.update(Message::Version(Event::MoveToGalaxy));
    assert!(app.flow.is_none());
}

#[test]
fn a_private_branch_entered_with_its_password_is_listed_and_saved() {
    let world = world();
    world.private("beta", PASSWORD, "300", "1.3.0-beta");
    let listing = listed(&world, None);
    assert!(listing.private_branches);
    let mut app = showing(Loaded::Galaxy(Box::new(listing)));
    let _ = app.update(Message::Version(Event::EnterPrivate));
    assert!(picker(&app).private_open);
    let _ = app.update(Message::Version(Event::PrivateBranch(" beta ".to_owned())));
    let _ = app.update(Message::Version(Event::PrivatePassword(Secret::new(
        PASSWORD,
    ))));
    fits(&app);
    let _ = app.update(Message::Version(Event::ListBuilds));
    // The password left with the request, and the field is empty again.
    assert_eq!(picker(&app).private_password.expose(), "");
    assert!(picker(&app).busy);
    // The form waits for the answer: nothing typed meanwhile is kept.
    let _ = app.update(Message::Version(Event::PrivatePassword(Secret::new(
        "typed-meanwhile",
    ))));
    let _ = app.update(Message::Version(Event::PrivateBranch("other".to_owned())));
    assert_eq!(picker(&app).private_password.expose(), "");
    assert_eq!(picker(&app).private_branch, " beta ");
    fits(&app);

    let listing = listed(&world, Some(("beta", &Secret::new(PASSWORD))));
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        Some("beta".to_owned()),
        Ok(Loaded::Galaxy(Box::new(listing))),
    )));
    let shown = picker(&app);
    assert_eq!(shown.branch.as_deref(), Some("beta"));
    assert!(!shown.private_open && shown.private_error.is_none());
    assert!(shown.private_branch.is_empty());
    assert_eq!(
        branches(shown.listing().unwrap())
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["Default", "beta (private)"]
    );
    assert_eq!(shown.private_password.expose(), "");
    // Closing the view drops the form with it.
    let _ = app.update(Message::Version(Event::EnterPrivate));
    let _ = app.update(Message::Version(Event::PrivatePassword(Secret::new(
        PASSWORD,
    ))));
    let _ = app.update(Message::Version(Event::Back));
    assert!(app.version.is_none());
    // Saved, so the next listing has it without being asked.
    let again = listed(&world, None);
    assert!(again.builds.iter().any(|build| build.build == "300"));
}

#[test]
fn a_private_branch_gog_doesnt_list_says_so_and_saves_nothing() {
    let world = world();
    world.private("beta", PASSWORD, "300", "1.3.0-beta");
    let mut app = showing(Loaded::Galaxy(Box::new(listed(&world, None))));
    let _ = app.update(Message::Version(Event::EnterPrivate));
    let _ = app.update(Message::Version(Event::PrivateBranch("beta".to_owned())));
    let _ = app.update(Message::Version(Event::PrivatePassword(Secret::new(
        "wrong",
    ))));
    let _ = app.update(Message::Version(Event::ListBuilds));

    let listing = listed(&world, Some(("beta", &Secret::new("wrong"))));
    assert!(!listing.builds.iter().any(|build| build.build == "300"));
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        Some("beta".to_owned()),
        Ok(Loaded::Galaxy(Box::new(listing))),
    )));
    let shown = picker(&app);
    let miss = "GOG lists no branch beta for Papers, Please, or refused its password.";
    assert_eq!(shown.private_error.as_deref(), Some(miss));
    assert!(shown.private_open);
    assert_eq!(shown.private_branch, "beta");
    assert_eq!(shown.branch, None);
    fits(&app);
    assert!(!world.paths.host_data().join("gog-branches.key").exists());
}

#[test]
fn logged_out_or_portable_the_view_says_why() {
    let world = world();
    let keyring = TestKeyring(std::cell::RefCell::new(None));
    let logged_out = load_with(
        &world.paths,
        &keyring,
        world.server.client(),
        NAME,
        GAME,
        None,
    );
    assert_eq!(logged_out.unwrap_err(), crate::flow::LOGGED_OUT);

    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = Paths::select(
        |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    let error = load_with(
        &portable,
        &world.keyring,
        world.server.client(),
        NAME,
        GAME,
        None,
    )
    .unwrap_err();
    assert_eq!(error, MigrateError::Portable.to_string());

    let mut app = opened(false);
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Err(crate::flow::LOGGED_OUT.to_owned()),
    )));
    assert_eq!(picker(&app).error.as_deref(), Some(crate::flow::LOGGED_OUT));
    fits(&app);
}

#[test]
fn an_unfinished_operation_says_why_and_loads_nothing() {
    let app = opened(true);
    let shown = picker(&app);
    assert!(!shown.busy);
    assert_eq!(
        shown.note.as_deref(),
        Some(
            format!(
                "{NAME} has an unfinished GOG operation, so it can't change its branch or build yet."
            )
            .as_str()
        )
    );
    // Finish update keeps its place.
    assert!(app.cards[0].finish_update());
}

#[test]
fn an_open_overlay_stops_the_pickers_input_but_not_its_answers() {
    let world = world();
    let mut app = opened(false);
    let generation = app.version_generation;
    app.flow = Some(flow::Page::choose(app.paths.clone()));
    let _ = app.update(Message::Version(Event::Back));
    assert!(app.version.is_some());
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Galaxy(Box::new(listed(&world, None)))),
    )));
    assert!(picker(&app).loaded.is_some());
    let _ = app.update(Message::Version(Event::Pick("100".to_owned())));
    assert_eq!(picker(&app).picked, None);

    // Nor does it open behind one.
    let mut app = crate::tests::app();
    app.cards = vec![game_card(false)];
    app.flow = Some(flow::Page::choose(app.paths.clone()));
    let _ = app.update(Message::Version(Event::Open(NAME.to_owned())));
    assert!(app.version.is_none());
}

#[test]
fn escape_closes_the_picker_before_the_panel() {
    let mut app = opened(false);
    let escape = || Key::Named(Named::Escape);
    let _ = app.handle_key(escape(), Modifiers::empty(), false, false);
    assert!(app.version.is_none() && app.panel_open);
    let _ = app.handle_key(escape(), Modifiers::empty(), false, false);
    assert!(!app.panel_open);
}

#[test]
fn opening_the_log_or_another_game_closes_the_picker() {
    let mut app = opened(false);
    let mut other = game_card(false);
    other.name = "other".to_owned();
    app.cards.push(other);
    let _ = app.update(Message::Select("other".to_owned()));
    assert!(app.version.is_none());
    let _ = app.update(Message::Version(Event::Open("other".to_owned())));
    assert!(app.version_visible());
    let _ = app.update(Message::CloseLog);
    assert!(app.version.is_none());
}

fn status(track: Track) -> Status {
    Status {
        environment: NAME.to_owned(),
        gog_id: GAME,
        title: "Papers, Please".to_owned(),
        folder: "/games/papers".into(),
        platform: Platform::Windows,
        track,
        language: "en-US".to_owned(),
        installed: Some("100".to_owned()),
        available: Some("200".to_owned()),
        installed_name: Some("1.2.76".to_owned()),
        available_name: Some("1.4.11".to_owned()),
        branch: None,
        pinned: None,
        pinned_name: None,
        state: UpdateState::Available,
        applicable: true,
        reason: None,
        unfinished: false,
        galaxy_install: false,
        warnings: Vec::new(),
        dlc: Vec::new(),
    }
}

#[test]
fn the_version_row_names_the_build_with_its_pin_or_branch() {
    let mut galaxy = status(Track::Galaxy);
    assert_eq!(status_value(&galaxy).as_deref(), Some("1.2.76"));
    galaxy.branch = Some("beta".to_owned());
    assert_eq!(
        status_value(&galaxy).as_deref(),
        Some("1.2.76 (branch beta)")
    );
    galaxy.pinned = Some("100".to_owned());
    assert_eq!(status_value(&galaxy).as_deref(), Some("1.2.76 (pinned)"));
    let mut installer = status(Track::Installer);
    installer.installed = Some("1.2.76".to_owned());
    installer.installed_name = None;
    assert_eq!(status_value(&installer).as_deref(), Some("1.2.76"));

    let world = world();
    pin(&world, "100");
    assert_eq!(
        listing_value(&listed(&world, None)).as_deref(),
        Some("1.2.76 (pinned)")
    );

    let mut app = app();
    app.cards = vec![game_card(false)];
    assert_eq!(app.version_value(NAME), "Unknown");
    app.gog_updates
        .insert(NAME.to_owned(), status(Track::Galaxy));
    assert_eq!(app.version_value(NAME), "1.2.76");
}

#[test]
fn only_gog_windows_games_get_a_version_row() {
    assert!(has_version(&game_card(false)));
    let mut native = game_card(false);
    native.kind = Some(CardKind::Native);
    assert!(!has_version(&native));
    let mut clone = game_card(false);
    clone.gog = Some(GogCard {
        gog_id: None,
        unfinished: false,
        blocked_by: Some("papers".to_owned()),
        install: false,
    });
    assert!(!has_version(&clone));
    let mut plain = game_card(false);
    plain.gog = None;
    assert!(!has_version(&plain));
}

#[test]
fn a_finished_change_is_checked_again_for_the_panel() {
    let world = world();
    let status =
        crate::gog_library::recheck_with(&world.paths, &world.keyring, world.server.client(), NAME)
            .unwrap()
            .unwrap();
    assert_eq!(status.installed.as_deref(), Some("100"));

    let mut app = app();
    app.cards = vec![game_card(false)];
    app.gog_versions
        .insert(NAME.to_owned(), "stale (pinned)".to_owned());
    let generation = crate::gog_library::supersede(&mut app, NAME);
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        generation,
        Ok(Some(status)),
    ));
    assert_eq!(app.version_value(NAME), "1.2.76");
    assert!(app.gog_updates.contains_key(NAME));
}

#[test]
fn a_recheck_answering_after_something_newer_is_dropped() {
    let world = world();
    let before =
        crate::gog_library::recheck_with(&world.paths, &world.keyring, world.server.client(), NAME)
            .unwrap()
            .unwrap();
    pin(&world, "100");

    // A check asked for before the pin answers after a listing of it.
    let mut app = opened(false);
    let late = crate::gog_library::supersede(&mut app, NAME);
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Galaxy(Box::new(listed(&world, None)))),
    )));
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        late,
        Ok(Some(before.clone())),
    ));
    assert!(!app.gog_updates.contains_key(NAME));
    assert_eq!(app.version_value(NAME), "1.2.76 (pinned)");

    // Nor after a newer check of its own.
    let older = crate::gog_library::supersede(&mut app, NAME);
    let _ = crate::gog_library::recheck(&mut app, NAME.to_owned());
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        older,
        Ok(Some(before)),
    ));
    assert!(!app.gog_updates.contains_key(NAME));
}
