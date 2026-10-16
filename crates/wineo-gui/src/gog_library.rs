//! GOG games in the library: what the disk says about each card (an
//! interrupted update to finish, or a clone it blocks), and what the
//! background check says GOG offers.

use wineo_core::config::Kind;
use wineo_core::environment::Environment;
use wineo_core::gog::folder::{self, FolderState, JournalKind};
use wineo_core::gog::update::{self, Status, UpdateState};
use wineo_core::paths::Paths;

/// What a card knows about GOG without asking GOG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GogCard {
    /// Its `gog_id`, when it tracks a GOG game.
    pub(crate) gog_id: Option<String>,
    /// It owns its game's folder, and an update there was interrupted.
    pub(crate) unfinished: bool,
    /// Another environment owns a game folder this one launches from, and
    /// its update was interrupted. Holds that owner.
    pub(crate) blocked_by: Option<String>,
    /// What was interrupted, its own or the blocking owner's, is an install
    /// rather than an update.
    pub(crate) install: bool,
}

impl GogCard {
    /// Why Play is off for a clone, naming the game to finish.
    pub(crate) fn blocked_note(&self) -> Option<String> {
        self.blocked_by.as_deref().map(|owner| {
            if self.install {
                format!("{owner}'s game folder is mid-install. Use Finish install on {owner}.")
            } else {
                blocked_note(owner)
            }
        })
    }

    /// The label of the action that finishes what was interrupted.
    pub(crate) fn finish_label(&self) -> &'static str {
        if self.install {
            "Finish install"
        } else {
            "Finish update"
        }
    }
}

pub(crate) fn blocked_note(owner: &str) -> String {
    format!("{owner}'s game folder is mid-update. Use Finish update on {owner}.")
}

/// The GOG folder states, read once per library load.
pub(crate) struct Folders {
    states: Vec<FolderState>,
    /// Whether any has a journal, since only then can a clone be blocked.
    journal: bool,
}

impl Folders {
    pub(crate) fn read(paths: &Paths) -> Self {
        // GOG games live in the host library only.
        let states: Vec<FolderState> =
            if paths.portable_root().is_none() && paths.gog_folders_dir().exists() {
                folder::list(paths)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(_, state)| state.ok())
                    .collect()
            } else {
                Vec::new()
            };
        let journal = states.iter().any(|state| state.journal.is_some());
        Self { states, journal }
    }

    /// What `env`'s card shows about GOG, None for one that has nothing to do
    /// with it.
    pub(crate) fn card(&self, paths: &Paths, env: &Environment) -> Option<GogCard> {
        if matches!(env.config.kind, Kind::Proxied(_)) || paths.portable_root().is_some() {
            return None;
        }
        let gog_id = env.config.gog_id.clone();
        let own = gog_id.as_deref().and_then(|id| {
            self.states.iter().find_map(|state| {
                (state.owner.as_deref() == Some(env.name.as_str()) && state.is_game(id))
                    .then_some(state.journal.as_ref()?.kind)
            })
        });
        let unfinished = own.is_some();
        let blocking = (self.journal && !unfinished)
            .then(|| blocking_owner(paths, env))
            .flatten();
        let install =
            own.or(blocking.as_ref().map(|(_, kind)| *kind)) == Some(JournalKind::Install);
        let blocked_by = blocking.map(|(owner, _)| owner);
        (gog_id.is_some() || blocked_by.is_some()).then_some(GogCard {
            gog_id,
            unfinished,
            blocked_by,
            install,
        })
    }
}

/// The owner whose interrupted update stops `env` from launching: the owner
/// of a folder with a journal that one of its targets runs from. Read
/// without locks, unlike a launch's check, since this runs every refresh and
/// a held in-use lock, even briefly, could make an update refuse. Anything
/// else a launch would refuse is left for Play to report.
fn blocking_owner(paths: &Paths, env: &Environment) -> Option<(String, JournalKind)> {
    update::named_paths(paths, &env.name)
        .iter()
        .filter_map(|path| folder::folder_holding(paths, path).ok().flatten())
        .find_map(|state| {
            let kind = state.journal.as_ref()?.kind;
            state
                .owner
                .filter(|owner| *owner != env.name)
                .map(|owner| (owner, kind))
        })
}

/// The badge on a card's poster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Badge {
    UpdateAvailable,
    FinishUpdate,
    FinishInstall,
}

impl Badge {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UpdateAvailable => "Update available",
            Self::FinishUpdate => "Finish update",
            Self::FinishInstall => "Finish install",
        }
    }
}

pub(crate) fn badge(gog: Option<&GogCard>, status: Option<&Status>) -> Option<Badge> {
    if let Some(gog) = gog.filter(|gog| gog.unfinished) {
        Some(if gog.install {
            Badge::FinishInstall
        } else {
            Badge::FinishUpdate
        })
    } else if status.is_some_and(updatable) {
        Some(Badge::UpdateAvailable)
    } else {
        None
    }
}

/// Whether this wineo can update the game now: a game on either track with
/// an update it can apply.
pub(crate) fn updatable(status: &Status) -> bool {
    status.applicable && status.state == UpdateState::Available
}

/// How a game's update state reads in the panel.
pub(crate) fn state_label(state: UpdateState) -> &'static str {
    match state {
        UpdateState::UpToDate => "Up to date",
        UpdateState::Available => "Update available",
        UpdateState::OnGog => "Update on GOG",
        UpdateState::CantCompare => "Can't compare",
    }
}

/// Checks every GOG game for updates, once after start. Logged out, or in a
/// portable session, there is nothing to check. Changes nothing a user
/// didn't ask for beyond what `check_all` records itself.
pub(crate) fn check_task(paths: Paths) -> iced::Task<crate::Message> {
    if paths.portable_root().is_some() {
        return iced::Task::none();
    }
    crate::task::blocking(
        move || {
            let keyring = wineo_core::secret_service::SecretServiceKeyring::gog();
            let client = wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
            check_with(&paths, &keyring, client)
        },
        crate::Message::GogChecked,
    )
}

/// What the background check found, or None when logged out.
pub(crate) fn check_with(
    paths: &Paths,
    keyring: &dyn wineo_core::key_store::Keyring,
    client: wineo_core::gog::api::Client,
) -> Result<Option<wineo_core::gog::update::Batch>, String> {
    let Some(mut session) = wineo_core::gog::account::Session::open(paths, keyring, client)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    wineo_core::gog::update::check_all(&mut session, false)
        .map(Some)
        .map_err(|error| format!("{error:#}"))
}

/// Lands the background check: statuses mark cards, and problems are only
/// logged, since nobody asked for the check.
pub(crate) fn checked(
    app: &mut crate::App,
    result: Result<Option<wineo_core::gog::update::Batch>, String>,
) {
    match result {
        Ok(Some(batch)) => {
            for error in &batch.errors {
                eprintln!(
                    "wineo-gui: couldn't check {} for GOG updates: {}",
                    error.subject(),
                    error.message
                );
            }
            app.gog_updates = batch
                .statuses
                .into_iter()
                .map(|status| (status.environment.clone(), status))
                .collect();
        }
        Ok(None) => {}
        Err(error) => eprintln!("wineo-gui: couldn't check for GOG updates: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use wineo_core::gog::folder::{Platform, Track};
    use wineo_core::gog::update::Reason;
    use wineo_core::runtime::State;

    use super::*;
    use crate::Message;
    use crate::gog_fake::{native_environment, native_game};
    use crate::tests::{app, card};

    const RIMWORLD: u64 = 1094900565;

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                .unwrap();
        (root, paths)
    }

    fn card_of(paths: &Paths, name: &str) -> Option<GogCard> {
        let env = Environment::load(paths, name).unwrap();
        Folders::read(paths).card(paths, &env)
    }

    #[test]
    fn an_interrupted_update_marks_its_owner_and_names_it_for_clones() {
        let (root, paths) = setup();
        let folder = native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", true);
        native_environment(&paths, "rimworld-copy", &folder.join("start.sh"), None);
        let elsewhere = root.path().join("other.sh");
        std::fs::write(&elsewhere, "#!/bin/sh\n").unwrap();
        native_environment(&paths, "notes", &elsewhere, None);

        let owner = card_of(&paths, "rimworld").unwrap();
        assert!(owner.unfinished);
        assert_eq!(owner.blocked_by, None);
        let copy = card_of(&paths, "rimworld-copy").unwrap();
        assert!(!copy.unfinished);
        assert_eq!(copy.blocked_by.as_deref(), Some("rimworld"));
        assert_eq!(
            copy.blocked_note().unwrap(),
            "rimworld's game folder is mid-update. Use Finish update on rimworld."
        );
        assert_eq!(card_of(&paths, "notes"), None);
        // Reading cards takes no in-use lock and leaves no holder record, so
        // it can never make an update refuse.
        let key = folder::key(&folder);
        assert!(!paths.gog_folder_in_use_lock(&key).exists());
        assert!(!paths.gog_folder_holders(&key).exists());
    }

    #[test]
    fn a_finished_game_is_just_a_gog_game() {
        let (root, paths) = setup();
        let folder = native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
        native_environment(&paths, "rimworld-copy", &folder.join("start.sh"), None);
        let owner = card_of(&paths, "rimworld").unwrap();
        assert_eq!(owner.gog_id.as_deref(), Some("1094900565"));
        assert!(!owner.unfinished);
        assert_eq!(card_of(&paths, "rimworld-copy"), None);
    }

    #[test]
    fn a_portable_session_reads_no_gog_state() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let host = Paths::from_lookup({
            let home = home.clone();
            move |key| (key == "HOME").then(|| OsString::from(&home))
        })
        .unwrap();
        native_game(&host, root.path(), "rimworld", RIMWORLD, "1.0", true);
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(&home)),
            Some(&root.path().join("portable")),
        )
        .unwrap();
        assert!(Folders::read(&portable).states.is_empty());
    }

    pub(crate) fn status(name: &str, state: UpdateState, applicable: bool) -> Status {
        Status {
            environment: name.to_owned(),
            gog_id: RIMWORLD,
            title: "RimWorld".to_owned(),
            folder: "/games/rimworld".into(),
            platform: if applicable {
                Platform::Linux
            } else {
                Platform::Windows
            },
            track: Track::Installer,
            language: "en-US".to_owned(),
            installed: Some("1.0".to_owned()),
            available: Some("1.1".to_owned()),
            installed_name: None,
            available_name: None,
            state,
            applicable,
            reason: (!applicable).then(|| Reason::Galaxy {
                name: name.to_owned(),
            }),
            unfinished: false,
            galaxy_install: false,
            warnings: Vec::new(),
            dlc: Vec::new(),
        }
    }

    fn gog_card(unfinished: bool, blocked_by: Option<&str>) -> Option<GogCard> {
        Some(GogCard {
            gog_id: Some(RIMWORLD.to_string()),
            unfinished,
            blocked_by: blocked_by.map(str::to_owned),
            install: false,
        })
    }

    #[test]
    fn badges_say_update_available_or_finish_update() {
        let native = status("rimworld", UpdateState::Available, true);
        let windows = status("disco", UpdateState::Available, false);
        let finish = gog_card(true, None);
        let plain = gog_card(false, None);
        assert_eq!(
            badge(plain.as_ref(), Some(&native)),
            Some(Badge::UpdateAvailable)
        );
        assert_eq!(badge(plain.as_ref(), Some(&windows)), None);
        assert_eq!(badge(plain.as_ref(), None), None);
        assert_eq!(
            badge(finish.as_ref(), Some(&native)),
            Some(Badge::FinishUpdate)
        );
        assert_eq!(Badge::UpdateAvailable.label(), "Update available");
        assert_eq!(Badge::FinishUpdate.label(), "Finish update");
        assert_eq!(state_label(UpdateState::OnGog), "Update on GOG");
        assert_eq!(
            Reason::Galaxy {
                name: "disco".to_owned()
            }
            .note(),
            "GOG has no Galaxy build of this game that wineo can update. Move it to offline installers with: wineo update disco --reinstall --track installer"
        );
    }

    #[test]
    fn update_is_offered_for_native_updates_and_finish_update_replaces_play() {
        let mut app = app();
        let mut rimworld = card("rimworld", State::Inactive, 0);
        rimworld.gog = gog_card(false, None);
        let mut disco = card("disco", State::Inactive, 0);
        disco.gog = gog_card(false, None);
        let mut copy = card("rimworld-copy", State::Inactive, 0);
        copy.gog = gog_card(false, Some("rimworld"));
        let mut broken = card("broken", State::Inactive, 0);
        broken.gog = gog_card(true, None);
        app.cards = vec![rimworld, disco, copy, broken];
        assert!(!app.gog_update_action("rimworld"), "nothing checked yet");
        app.gog_updates.insert(
            "rimworld".to_owned(),
            status("rimworld", UpdateState::Available, true),
        );
        app.gog_updates.insert(
            "disco".to_owned(),
            status("disco", UpdateState::Available, false),
        );
        assert!(app.gog_update_action("rimworld"));
        assert!(
            !app.gog_update_action("disco"),
            "Windows games show status only"
        );
        assert!(!app.gog_update_action("broken"));

        let target = Some("game");
        assert!(crate::panel::play_enabled(&app.cards[0], false, target));
        assert!(!crate::panel::play_enabled(&app.cards[2], false, target));
        assert!(!crate::panel::play_enabled(&app.cards[3], false, target));
        assert!(app.cards[3].finish_update());

        // The context menu lays out with its Update row.
        app.action_menu = Some("rimworld".to_owned());
        let _ = crate::layout_probe::layout(&mut app.view(), iced::Size::new(1200.0, 800.0));

        // Update opens the review; a game without one can't.
        let _ = app.update(Message::GogUpdate("disco".to_owned()));
        assert!(app.flow.is_none());
        let _ = app.update(Message::GogUpdate("broken".to_owned()));
        assert!(matches!(app.flow, Some(crate::flow::Page::GogUpdate(_))));
    }

    #[test]
    fn the_panel_shows_finish_update_the_blocked_note_and_windows_status() {
        let mut app = app();
        let mut broken = card("broken", State::Inactive, 0);
        broken.gog = gog_card(true, None);
        let mut copy = card("rimworld-copy", State::Inactive, 0);
        copy.gog = gog_card(false, Some("rimworld"));
        let mut disco = card("disco", State::Inactive, 0);
        disco.gog = gog_card(false, None);
        app.gog_updates.insert(
            "disco".to_owned(),
            status("disco", UpdateState::Available, false),
        );
        app.cards = vec![broken, copy, disco];
        for card in &app.cards {
            let mut view = app.panel(card);
            let _ = crate::layout_probe::layout(&mut view, iced::Size::new(420.0, 800.0));
        }
    }

    #[test]
    fn the_background_check_marks_cards_and_stays_quiet_on_failure() {
        use crate::gog_fake::{Server, native_product, product_server, reply, save_login};
        use crate::settings::steam_key::tests::TestKeyring;

        let (root, paths) = setup();
        native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
        let keyring = TestKeyring(std::cell::RefCell::new(None));
        let server = product_server(vec![native_product(RIMWORLD, "RimWorld", "1.1")]);
        assert_eq!(check_with(&paths, &keyring, server.client()).unwrap(), None);
        assert!(server.requests().is_empty(), "logged out asks GOG nothing");

        save_login(&paths, &keyring);
        let batch = check_with(&paths, &keyring, server.client()).unwrap();
        let mut app = app();
        let mut rimworld = card("rimworld", State::Inactive, 0);
        rimworld.gog = gog_card(false, None);
        app.cards = vec![rimworld];
        checked(&mut app, Ok(batch));
        let status = &app.gog_updates["rimworld"];
        assert_eq!(status.state, UpdateState::Available);
        assert_eq!(
            badge(app.cards[0].gog.as_ref(), Some(status)),
            Some(Badge::UpdateAvailable)
        );
        assert!(app.gog_update_action("rimworld"));

        // GOG failing is a log line, never an error in the app. A fresh home,
        // since the details just fetched are cached for an hour.
        let (root, paths) = setup();
        native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
        save_login(&paths, &keyring);
        let down = Server::start(|_| reply(503, "{}"));
        let failed = check_with(&paths, &keyring, down.client());
        let mut app = crate::tests::app();
        match failed {
            Ok(batch) => {
                let batch = batch.unwrap();
                assert!(batch.statuses.is_empty());
                assert_eq!(batch.errors.len(), 1);
                checked(&mut app, Ok(Some(batch)));
            }
            Err(error) => checked(&mut app, Err(error)),
        }
        checked(&mut app, Err("no network".to_owned()));
        assert!(app.gog_updates.is_empty());
        assert!(app.error.is_none());
        assert!(app.panel_error.is_none());
        assert!(app.flow.is_none());
    }
}
