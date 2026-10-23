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
        version: None,
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
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76");
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
    // Finish update stays in the panel, and can start.
    assert!(app.version_visible());
    let ids = panel_controls(&app);
    assert!(
        ids.contains(&iced::widget::Id::new(crate::panel::PANEL_PLAY_ID)),
        "{ids:?}"
    );
    // A game with nothing unfinished shows only the view.
    let app = opened(false);
    let ids = panel_controls(&app);
    assert!(
        !ids.contains(&iced::widget::Id::new(crate::panel::PANEL_PLAY_ID)),
        "{ids:?}"
    );
}

/// The ids of the enabled controls the panel draws.
fn panel_controls(app: &App) -> Vec<iced::widget::Id> {
    use std::sync::{Arc, Mutex};
    struct Ids(Arc<Mutex<Vec<iced::widget::Id>>>);
    impl iced::advanced::widget::Operation for Ids {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }

        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            _bounds: iced::Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            self.0.lock().unwrap().extend(id.cloned());
        }
    }
    let ids = Arc::new(Mutex::new(Vec::new()));
    let mut laid =
        crate::layout_probe::lay_out(app.panel(&app.cards[0]), iced::Size::new(420.0, 1600.0));
    laid.run(Box::new(Ids(Arc::clone(&ids))));
    ids.lock().unwrap().clone()
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
    assert_eq!(app.version_value(&app.cards[0]), "Unknown");
    app.gog_updates
        .insert(NAME.to_owned(), status(Track::Galaxy));
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76");
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
        version: None,
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
    app.gog_updates.insert_version(NAME, "stale (pinned)");
    app.gog_updates.changed(NAME);
    let generation = app.gog_updates.asked(NAME);
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        generation,
        Ok(Some(status)),
    ));
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76");
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

    // A check asked with the listing answers after it.
    let mut app = opened(false);
    let late = app.gog_updates.asked(NAME);
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
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76 (pinned)");

    // Nor after a check asked with it that answered first.
    let older = app.gog_updates.asked(NAME);
    let mut newer = before.clone();
    newer.state = UpdateState::UpToDate;
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        older,
        Ok(Some(newer)),
    ));
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        older,
        Ok(Some(before)),
    ));
    assert_eq!(app.gog_updates[NAME].state, UpdateState::UpToDate);
}

/// The game's card as the library reads it from disk.
fn card_from_disk(world: &Galaxy) -> GogCard {
    let env = wineo_core::environment::Environment::load(&world.paths, NAME).unwrap();
    crate::gog_library::Folders::read(&world.paths)
        .card(&world.paths, &env)
        .unwrap()
}

#[test]
fn before_any_check_the_version_row_reads_the_games_own_record() {
    let world = world();
    assert_eq!(card_from_disk(&world).version.as_deref(), Some("1.2.76"));
    pin(&world, "100");
    let local = card_from_disk(&world);
    assert_eq!(local.version.as_deref(), Some("1.2.76 (pinned)"));

    let mut app = app();
    let mut game = game_card(false);
    game.gog = Some(local);
    app.cards = vec![game];
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76 (pinned)");
    // A check, once it has run, says what GOG lists.
    app.gog_updates
        .insert(NAME.to_owned(), status(Track::Galaxy));
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76");

    // Without its record the build id is all there is, and its follow is
    // the default branch's.
    world.lose_record();
    assert_eq!(card_from_disk(&world).version.as_deref(), Some("100"));

    // An installer-track game shows its installer's version.
    let world = Galaxy::new();
    world.install();
    world.on_installer_track();
    assert_eq!(card_from_disk(&world).version.as_deref(), Some("1.2.76"));
}

/// A status GOG's check could give for the game, distinct from a listing.
fn checked_status() -> Status {
    let mut checked = status(Track::Galaxy);
    checked.state = UpdateState::UpToDate;
    checked
}

#[test]
fn a_listing_answering_after_a_newer_recheck_only_shows_in_the_picker() {
    let world = world();
    pin(&world, "100");
    // The recheck and the listing are both asked, and the recheck lands first.
    let mut app = opened(false);
    let recheck = app.gog_updates.asked(NAME);
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        recheck,
        Ok(Some(Box::new(checked_status()))),
    ));
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Galaxy(Box::new(listed(&world, None)))),
    )));
    // The panel keeps the recheck's answer; the picker still shows the builds.
    assert_eq!(app.gog_updates.listed_version(NAME), None);
    assert_eq!(app.gog_updates[NAME].state, UpdateState::UpToDate);
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76");
    assert_eq!(picker(&app).rows().len(), 2);
}

#[test]
fn an_installer_track_answer_drops_checks_asked_before_it() {
    let mut app = opened(false);
    let recheck = app.gog_updates.asked(NAME);
    let startup = app.gog_updates.asked_all();
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Installer { galaxy: true }),
    )));
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        recheck,
        Ok(Some(Box::new(checked_status()))),
    ));
    let _ = app.update(Message::GogChecked(
        startup,
        Ok(Some(wineo_core::gog::update::Batch {
            statuses: vec![checked_status()],
            errors: Vec::new(),
        })),
    ));
    assert!(app.gog_updates.status(NAME).is_none());
}

#[test]
fn a_listing_answering_after_a_newer_startup_check_only_shows_in_the_picker() {
    let world = world();
    pin(&world, "100");
    let mut app = app();
    app.cards = vec![game_card(false)];
    let startup = app.gog_updates.asked_all();
    let _ = app.update(Message::Version(Event::Open(NAME.to_owned())));
    let _ = app.update(Message::GogChecked(
        startup,
        Ok(Some(wineo_core::gog::update::Batch {
            statuses: vec![checked_status()],
            errors: Vec::new(),
        })),
    ));
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Galaxy(Box::new(listed(&world, None)))),
    )));
    assert_eq!(app.gog_updates.listed_version(NAME), None);
    assert_eq!(app.gog_updates[NAME].state, UpdateState::UpToDate);
    assert_eq!(picker(&app).rows().len(), 2);

    // Opened again after it, the listing counts.
    let _ = app.update(Message::Version(Event::Open(NAME.to_owned())));
    let generation = app.version_generation;
    let _ = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(Loaded::Galaxy(Box::new(listed(&world, None)))),
    )));
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76 (pinned)");
}

/// The game's card with `version` read from its folder.
fn recorded(version: &str) -> Card {
    let mut game = game_card(false);
    game.gog.as_mut().unwrap().version = Some(version.to_owned());
    game
}

#[test]
fn a_rescan_with_newer_local_facts_refuses_an_older_check_and_checks_again() {
    let mut app = app();
    app.merge(vec![recorded("1.2.76")]);
    // A check is asked, then the CLI pins the game and a rescan sees it.
    let asked = app.gog_updates.asked(NAME);
    assert_eq!(
        crate::gog_library::local_changes(&app.cards, &[recorded("1.2.76 (pinned)")]),
        [(NAME.to_owned(), true)]
    );
    app.merge(vec![recorded("1.2.76 (pinned)")]);
    let rechecked = app.gog_updates.asked(NAME);
    assert_ne!(rechecked, asked);
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        asked,
        Ok(Some(Box::new(status(Track::Galaxy)))),
    ));
    assert!(app.gog_updates.status(NAME).is_none());
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76 (pinned)");
    // The check the rescan scheduled is the one that counts.
    let mut pinned = status(Track::Galaxy);
    pinned.pinned = Some("100".to_owned());
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        rechecked,
        Ok(Some(Box::new(pinned))),
    ));
    assert_eq!(app.version_value(&app.cards[0]), "1.2.76 (pinned)");
    assert!(app.gog_updates.status(NAME).is_some());
}

#[test]
fn after_an_app_update_the_reload_checks_again_and_the_panel_keeps_a_status() {
    let mut app = app();
    app.merge(vec![recorded("1.2.76")]);
    // The update page finishes, which checks the game again.
    let _ = app.open_gog_update(NAME.to_owned(), None);
    let generation = match &app.flow {
        Some(flow::Page::GogUpdate(page)) => page.generation(),
        _ => panic!("expected the update page"),
    };
    let _ = app.update(Message::Flow(
        app.flow_generation,
        flow::Event::update_message_planned_up_to_date(generation),
    ));
    let from_update = app.gog_updates.asked(NAME);
    // Its library reload sees the new version, and checks again itself.
    app.merge(vec![recorded("1.4.11")]);
    let from_reload = app.gog_updates.asked(NAME);
    assert_ne!(from_reload, from_update);
    let mut updated = status(Track::Galaxy);
    updated.installed_name = Some("1.4.11".to_owned());
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        from_update,
        Ok(Some(Box::new(updated.clone()))),
    ));
    assert!(app.gog_updates.status(NAME).is_none());
    let _ = app.update(Message::GogRechecked(
        NAME.to_owned(),
        from_reload,
        Ok(Some(Box::new(updated))),
    ));
    assert!(app.gog_updates.status(NAME).is_some());
    assert_eq!(app.version_value(&app.cards[0]), "1.4.11");
}

#[test]
fn unchanged_or_first_seen_cards_ask_gog_nothing() {
    let mut app = app();
    // First seen.
    assert!(crate::gog_library::local_changes(&app.cards, &[recorded("1.2.76")]).is_empty());
    app.merge(vec![recorded("1.2.76")]);
    let asked = app.gog_updates.asked(NAME);
    assert_eq!(asked, 0);
    // Unchanged, though another field of the card did change.
    let mut same = recorded("1.2.76");
    same.playtime = "1 hour".to_owned();
    assert!(crate::gog_library::local_changes(&app.cards, &[same.clone()]).is_empty());
    app.merge(vec![same]);
    assert_eq!(app.gog_updates.asked(NAME), asked);
    // A game that isn't GOG's never counts.
    let mut plain = card("notes", State::Inactive, 0);
    plain.kind = Some(CardKind::Wine);
    app.merge(vec![recorded("1.2.76"), plain.clone()]);
    plain.playtime = "2 hours".to_owned();
    assert!(crate::gog_library::local_changes(&app.cards, &[recorded("1.2.76"), plain]).is_empty());
}

fn build(branch: Option<&str>, id: &str, latest: bool) -> Listed {
    Listed {
        branch: branch.map(str::to_owned),
        build: id.to_owned(),
        version: Some(format!("1.{id}")),
        published: None,
        installed: id == "100",
        latest,
        pinned: false,
        followed: false,
        private: false,
    }
}

/// The default branch lists 300, 200, and 100, and beta lists 250 and 200.
fn branched() -> Loaded {
    Loaded::Galaxy(Box::new(Listing {
        environment: NAME.to_owned(),
        gog_id: GAME,
        title: "Papers, Please".to_owned(),
        follow: Follow::default(),
        installed: Some("100".to_owned()),
        private_branches: false,
        builds: vec![
            build(None, "300", true),
            build(None, "200", false),
            build(None, "100", false),
            build(Some("beta"), "250", true),
            build(Some("beta"), "200", false),
        ],
        warnings: Vec::new(),
    }))
}

fn build_ids(builds: &[&str]) -> Vec<String> {
    builds.iter().map(|id| build_id(id)).collect()
}

fn beta() -> Branch {
    Branch {
        name: Some("beta".to_owned()),
        private: false,
    }
}

/// The panel, where the picker shows, laid out over `tree` when given.
fn relaid(
    app: &App,
    tree: Option<iced::advanced::widget::Tree>,
) -> crate::layout_probe::Laid<'_, Message> {
    let panel = app.panel(&app.cards[0]);
    let size = iced::Size::new(420.0, 1600.0);
    match tree {
        Some(tree) => crate::layout_probe::relay_out(panel, tree, size),
        None => crate::layout_probe::lay_out(panel, size),
    }
}

/// The picker open on the game with its builds listed, and what landing
/// the listing did to focus.
fn listed_with_focus() -> (App, iced::advanced::widget::Tree) {
    let mut app = opened(false);
    let tree = relaid(&app, None).into_tree();
    let generation = app.version_generation;
    let task = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(branched()),
    )));
    let mut laid = relaid(&app, Some(tree));
    crate::focus_button::run_task(&mut laid, task);
    let tree = laid.into_tree();
    (app, tree)
}

#[test]
fn the_builds_take_focus_when_they_arrive_and_the_arrows_walk_them() {
    let (app, tree) = listed_with_focus();
    let mut laid = relaid(&app, Some(tree));
    let ids = build_ids(&["300", "200", "100"]);
    assert_eq!(laid.focused(), [iced::widget::Id::from(ids[0].clone())]);
    crate::focus_button::walk_rows(&mut laid, &ids);
}

#[test]
fn up_and_down_on_a_build_leave_the_grid_selection_alone() {
    let (mut app, tree) = listed_with_focus();
    let mut other = game_card(false);
    other.name = "other".to_owned();
    app.cards.push(other);
    let mut tree = tree;
    for key in [Named::ArrowDown, Named::ArrowUp, Named::ArrowUp] {
        let mut laid = relaid(&app, Some(tree));
        let (messages, status) = crate::focus_button::press(&mut laid, key, Modifiers::empty());
        tree = laid.into_tree();
        assert!(messages.is_empty());
        assert_eq!(status, iced::event::Status::Captured);
        // What the library's keys get once the search box says it's not
        // focused.
        let _ = app.handle_key(Key::Named(key), Modifiers::empty(), false, true);
        assert_eq!(app.selected.as_deref(), Some(NAME));
        assert!(app.version_visible());
    }
}

#[test]
fn a_build_on_both_branches_keeps_focus_when_the_branch_changes() {
    let (mut app, tree) = listed_with_focus();
    let mut laid = relaid(&app, Some(tree));
    crate::focus_button::press(&mut laid, Named::ArrowDown, Modifiers::empty());
    assert_eq!(laid.focused(), [iced::widget::Id::from(build_id("200"))]);
    let tree = laid.into_tree();
    let _ = app.update(Message::Version(Event::Branch(beta())));
    let mut laid = relaid(&app, Some(tree));
    // 200 is beta's second row too, after 250.
    assert_eq!(laid.focused(), [iced::widget::Id::from(build_id("200"))]);
}

#[test]
fn a_build_the_new_branch_lacks_moves_focus_to_the_nearest_row() {
    let (mut app, tree) = listed_with_focus();
    let mut laid = relaid(&app, Some(tree));
    crate::focus_button::press(&mut laid, Named::ArrowDown, Modifiers::empty());
    crate::focus_button::press(&mut laid, Named::ArrowDown, Modifiers::empty());
    assert_eq!(laid.focused(), [iced::widget::Id::from(build_id("100"))]);
    let tree = laid.into_tree();
    let _ = app.update(Message::Version(Event::Branch(beta())));
    let mut laid = relaid(&app, Some(tree));
    // 100 was third; beta has two rows, so its last.
    assert_eq!(laid.focused(), [iced::widget::Id::from(build_id("200"))]);
    // And from the top row, the top row.
    crate::focus_button::press(&mut laid, Named::ArrowUp, Modifiers::empty());
    let tree = laid.into_tree();
    let _ = app.update(Message::Version(Event::Branch(Branch {
        name: None,
        private: false,
    })));
    let mut laid = relaid(&app, Some(tree));
    assert_eq!(laid.focused(), [iced::widget::Id::from(build_id("300"))]);
}

#[test]
fn builds_that_arrive_while_something_else_has_focus_leave_it() {
    let mut app = opened(false);
    let size = iced::Size::new(1200.0, 800.0);
    let mut laid = crate::layout_probe::lay_out(app.view(), size);
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus::<()>(iced::widget::Id::new(
            crate::SEARCH_ID,
        )),
    ));
    assert_eq!(laid.focused(), [iced::widget::Id::new(crate::SEARCH_ID)]);
    let tree = laid.into_tree();
    let generation = app.version_generation;
    let task = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(branched()),
    )));
    let mut laid = crate::layout_probe::relay_out(app.view(), tree, size);
    crate::focus_button::run_task(&mut laid, task);
    assert_eq!(laid.focused(), [iced::widget::Id::new(crate::SEARCH_ID)]);
}

#[test]
fn opening_the_picker_from_its_panel_row_by_keyboard_lands_on_the_first_build() {
    let mut app = app();
    app.cards = vec![game_card(false)];
    app.selected = Some(NAME.to_owned());
    app.panel_open = true;
    let mut laid = relaid(&app, None);
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus::<()>(iced::widget::Id::new(
            crate::panel::PANEL_VERSION_ID,
        )),
    ));
    assert_eq!(
        laid.focused(),
        [iced::widget::Id::new(crate::panel::PANEL_VERSION_ID)]
    );
    let (messages, _) = crate::focus_button::press(&mut laid, Named::Enter, Modifiers::empty());
    let tree = laid.into_tree();
    assert!(matches!(
        messages.as_slice(),
        [Message::Version(Event::Open(_))]
    ));
    for message in messages {
        let _ = app.update(message);
    }
    let tree = relaid(&app, Some(tree)).into_tree();
    let generation = app.version_generation;
    let task = app.update(Message::Version(Event::Loaded(
        generation,
        None,
        Ok(branched()),
    )));
    let mut laid = relaid(&app, Some(tree));
    crate::focus_button::run_task(&mut laid, task);
    assert_eq!(laid.focused(), [iced::widget::Id::from(build_id("300"))]);
}
