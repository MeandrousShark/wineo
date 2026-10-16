//! GOG games in the library: what the disk says about each card (an
//! interrupted update to finish, or a clone it blocks), and what the
//! background check says GOG offers.

use wineo_core::config::Kind;
use wineo_core::environment::Environment;
use wineo_core::gog::folder::{self, FolderState, JournalKind};
use wineo_core::gog::update::{self, Reason, Status, UpdateState};
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
    /// The installed version as its own folder records it, for the Version
    /// row before GOG has been asked.
    pub(crate) version: Option<String>,
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
        let version = gog_id.as_deref().and_then(|id| {
            let state = self.states.iter().find(|state| {
                state.owner.as_deref() == Some(env.name.as_str()) && state.is_game(id)
            })?;
            local_version(paths, state)
        });
        (gog_id.is_some() || blocked_by.is_some()).then_some(GogCard {
            gog_id,
            unfinished,
            blocked_by,
            install,
            version,
        })
    }
}

/// What a game's own folder says is installed: its Galaxy build with its pin
/// or branch, or the base game's installer version.
fn local_version(paths: &Paths, state: &FolderState) -> Option<String> {
    if let Some(local) = wineo_core::gog::update::galaxy::local_of(paths, state) {
        return Some(crate::version_picker::described(
            wineo_core::gog::update::galaxy::present::version(
                local.version.as_deref(),
                &local.build,
            ),
            local.follow.branch.as_deref(),
            local.follow.pinned.is_some(),
        ));
    }
    let committed = state.committed.as_ref()?;
    committed
        .products
        .iter()
        .find(|product| product.id == committed.gog_id)?
        .version
        .clone()
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

/// Whether `name`'s interrupted operation is a Galaxy install, which the
/// install page finishes: `wineo update` can't carry one on.
pub(crate) fn galaxy_install(paths: &Paths, name: &str) -> bool {
    update::journal_of(paths, name).is_some_and(|journal| {
        journal.kind == JournalKind::Install && journal.target.track == folder::Track::Galaxy
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

/// How a game's update status reads in the panel: a pinned game with a
/// newer build waiting says so instead of offering it.
pub(crate) fn status_label(status: &Status) -> &'static str {
    match status.reason {
        Some(Reason::Pinned { .. }) => "Pinned",
        _ => state_label(status.state),
    }
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
/// didn't ask for beyond what `check_all` records itself. `asked` is each
/// game's re-check generation when it starts, so what lands after it can't
/// replace newer state.
pub(crate) fn check_task(
    paths: Paths,
    asked: std::collections::HashMap<String, u64>,
) -> iced::Task<crate::Message> {
    if paths.portable_root().is_some() {
        return iced::Task::none();
    }
    crate::task::blocking(
        move || {
            let keyring = wineo_core::secret_service::SecretServiceKeyring::gog();
            let client = wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
            check_with(&paths, &keyring, client)
        },
        move |result| crate::Message::GogChecked(asked, result),
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

/// What the app knows from GOG about each game: the checks' statuses and
/// the Version row's value from a listing. Every request that can write
/// either for a game captures the game's generation when it is asked
/// (`asked`), and its reply counts only while that generation still holds.
/// Accepting a reply, or a change to the game (`changed`), moves the
/// generation on, so whatever was asked before is dropped. The fields are
/// private, so no writer can skip that.
#[derive(Debug, Default)]
pub(crate) struct GogState {
    statuses: std::collections::HashMap<String, Status>,
    versions: std::collections::HashMap<String, String>,
    generations: std::collections::HashMap<String, u64>,
}

impl GogState {
    pub(crate) fn status(&self, name: &str) -> Option<&Status> {
        self.statuses.get(name)
    }

    /// The Version row's value from the last listing accepted.
    pub(crate) fn listed_version(&self, name: &str) -> Option<&str> {
        self.versions.get(name).map(String::as_str)
    }

    /// What a request about `name` captures when it is asked.
    pub(crate) fn asked(&self, name: &str) -> u64 {
        self.generations.get(name).copied().unwrap_or_default()
    }

    /// What a request about every game captures when it is asked.
    pub(crate) fn asked_all(&self) -> std::collections::HashMap<String, u64> {
        self.generations.clone()
    }

    /// Whether a reply asked at `asked` is still the newest about `name`;
    /// if so it takes the generation, so anything asked before is dropped.
    fn accept(&mut self, name: &str, asked: u64) -> bool {
        if self.asked(name) != asked {
            return false;
        }
        self.generations
            .insert(name.to_owned(), asked.wrapping_add(1));
        true
    }

    /// `name`'s game changed, so nothing known or asked about it before
    /// counts.
    pub(crate) fn changed(&mut self, name: &str) {
        let generation = self.asked(name).wrapping_add(1);
        self.generations.insert(name.to_owned(), generation);
        self.statuses.remove(name);
        self.versions.remove(name);
    }

    /// A check of one game, asked at `asked`. Returns whether it counted.
    pub(crate) fn accept_status(&mut self, name: &str, asked: u64, status: Status) -> bool {
        if !self.accept(name, asked) {
            return false;
        }
        self.versions.remove(name);
        self.statuses.insert(name.to_owned(), status);
        true
    }

    /// A listing's Version row value, asked at `asked`. Returns whether it
    /// counted.
    pub(crate) fn accept_listing(&mut self, name: &str, asked: u64, value: Option<String>) -> bool {
        if !self.accept(name, asked) {
            return false;
        }
        match value {
            Some(value) => self.versions.insert(name.to_owned(), value),
            None => self.versions.remove(name),
        };
        true
    }

    /// A check of every game, asked when the generations were `asked`. Each
    /// game counts on its own: one reported takes the new status, one no
    /// longer reported loses its old one, and one whose generation moved on
    /// keeps what it has.
    pub(crate) fn accept_batch(
        &mut self,
        asked: &std::collections::HashMap<String, u64>,
        statuses: Vec<Status>,
    ) {
        let asked_for = |name: &str| asked.get(name).copied().unwrap_or_default();
        let reported: std::collections::HashSet<String> = statuses
            .iter()
            .map(|status| status.environment.clone())
            .collect();
        let gone: Vec<String> = self
            .statuses
            .keys()
            .filter(|name| !reported.contains(*name))
            .cloned()
            .collect();
        for name in gone {
            if self.accept(&name, asked_for(&name)) {
                self.statuses.remove(&name);
            }
        }
        for status in statuses {
            let name = status.environment.clone();
            if self.accept(&name, asked_for(&name)) {
                self.versions.remove(&name);
                self.statuses.insert(name, status);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.statuses.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, name: &str) -> bool {
        self.statuses.contains_key(name)
    }

    /// Seeds a status for a test, as an accepted check would.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, name: String, status: Status) {
        let asked = self.asked(&name);
        assert!(self.accept_status(&name, asked, status));
    }

    /// Seeds a listing's value for a test, as an accepted listing would.
    #[cfg(test)]
    pub(crate) fn insert_version(&mut self, name: &str, value: &str) {
        let asked = self.asked(name);
        assert!(self.accept_listing(name, asked, Some(value.to_owned())));
    }
}

#[cfg(test)]
impl std::ops::Index<&str> for GogState {
    type Output = Status;

    fn index(&self, name: &str) -> &Status {
        &self.statuses[name]
    }
}

/// The games whose own folder says something new about them between the
/// library's `old` cards and `new` ones, and whether each can be checked
/// with GOG. These are the facts a check could contradict: the game, its
/// recorded version, pin, and branch, and an unfinished operation. A card
/// seen for the first time has nothing to compare, so ordinary rescans ask
/// GOG nothing.
pub(crate) fn local_changes(
    old: &[crate::library::Card],
    new: &[crate::library::Card],
) -> Vec<(String, bool)> {
    let facts = |gog: Option<&GogCard>| {
        gog.map(|gog| {
            (
                gog.gog_id.clone(),
                gog.version.clone(),
                gog.unfinished,
                gog.install,
            )
        })
    };
    new.iter()
        .filter_map(|card| {
            let before = old.iter().find(|old| old.name == card.name)?;
            (facts(before.gog.as_ref()) != facts(card.gog.as_ref())).then(|| {
                let tracked = card.gog.as_ref().is_some_and(|gog| gog.gog_id.is_some());
                (card.name.clone(), tracked)
            })
        })
        .collect()
}

/// Checks `name` again once an update changed it. Logged out, or in a
/// portable session, there is nothing to check.
pub(crate) fn recheck(app: &mut crate::App, name: String) -> iced::Task<crate::Message> {
    let generation = app.gog_updates.asked(&name);
    let paths = app.paths.clone();
    if paths.portable_root().is_some() {
        return iced::Task::none();
    }
    let checked = name.clone();
    crate::task::blocking(
        move || {
            let keyring = wineo_core::secret_service::SecretServiceKeyring::gog();
            let client = wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
            recheck_with(&paths, &keyring, client, &checked)
        },
        move |result| crate::Message::GogRechecked(name, generation, result),
    )
}

/// What checking `name` found, or None when logged out.
pub(crate) fn recheck_with(
    paths: &Paths,
    keyring: &dyn wineo_core::key_store::Keyring,
    client: wineo_core::gog::api::Client,
    name: &str,
) -> Result<Option<Box<Status>>, String> {
    let Some(mut session) = wineo_core::gog::account::Session::open(paths, keyring, client)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    update::check(&mut session, name, false)
        .map(|status| Some(Box::new(status)))
        .map_err(|error| format!("{error:#}"))
}

/// Lands a check of one game asked at `generation`, like the background
/// check does.
pub(crate) fn rechecked(
    app: &mut crate::App,
    name: String,
    generation: u64,
    result: Result<Option<Box<Status>>, String>,
) {
    match result {
        Ok(Some(status)) => {
            app.gog_updates.accept_status(&name, generation, *status);
        }
        Ok(None) => {}
        Err(error) => eprintln!("wineo-gui: couldn't check {name} for GOG updates: {error}"),
    }
}

/// Lands the background check: statuses mark cards, and problems are only
/// logged, since nobody asked for the check.
#[cfg(test)]
pub(crate) fn checked(
    app: &mut crate::App,
    result: Result<Option<wineo_core::gog::update::Batch>, String>,
) {
    let asked = app.gog_updates.asked_all();
    checked_since(app, &asked, result);
}

/// `checked`, for a check asked when the games' generations were `asked`.
pub(crate) fn checked_since(
    app: &mut crate::App,
    asked: &std::collections::HashMap<String, u64>,
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
            app.gog_updates.accept_batch(asked, batch.statuses);
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
    use wineo_core::gog::update::galaxy::present::Wording;
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
            branch: None,
            pinned: None,
            pinned_name: None,
            state,
            applicable,
            reason: (!applicable).then(|| Reason::Galaxy {
                name: name.to_owned(),
                branch: None,
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
            version: None,
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
                name: "disco".to_owned(),
                branch: None,
            }
            .note(),
            "GOG has no Galaxy build of this game that wineo can update. Move it to offline installers with: wineo update disco --reinstall --track installer"
        );
    }

    #[test]
    fn a_startup_check_landing_after_a_newer_change_keeps_it() {
        let mut app = app();
        // The startup check starts, then papers changes and is checked again
        // before it answers.
        let asked = app.gog_updates.asked_all();
        app.gog_updates.changed("papers");
        let generation = app.gog_updates.asked("papers");
        let newer = pinned_status("papers");
        rechecked(
            &mut app,
            "papers".to_owned(),
            generation,
            Ok(Some(Box::new(newer.clone()))),
        );
        let batch = wineo_core::gog::update::Batch {
            statuses: vec![
                status("papers", UpdateState::Available, true),
                status("rimworld", UpdateState::Available, true),
            ],
            errors: Vec::new(),
        };
        checked_since(&mut app, &asked, Ok(Some(batch)));
        assert_eq!(app.gog_updates["papers"], newer);
        assert_eq!(app.gog_updates["rimworld"].state, UpdateState::Available);

        // A check that started after the change counts for it.
        let asked = app.gog_updates.asked_all();
        let batch = wineo_core::gog::update::Batch {
            statuses: vec![status("papers", UpdateState::UpToDate, true)],
            errors: Vec::new(),
        };
        checked_since(&mut app, &asked, Ok(Some(batch)));
        assert_eq!(app.gog_updates["papers"].state, UpdateState::UpToDate);
        // As before, a game the check no longer reports is dropped.
        assert!(!app.gog_updates.contains_key("rimworld"));
    }

    /// Every way to write what the app knows from GOG about a game refuses
    /// a reply asked before something newer about it landed.
    #[test]
    fn every_writer_drops_a_reply_asked_before_something_newer() {
        let fresh = || status("papers", UpdateState::UpToDate, true);
        let stale = || status("papers", UpdateState::Available, true);
        // Each writer, landing at the generation it was asked at.
        type Writer = fn(&mut GogState, u64, Status) -> bool;
        let writers: [(&str, Writer); 3] = [
            ("status", |state, asked, status| {
                state.accept_status("papers", asked, status)
            }),
            ("listing", |state, asked, _| {
                state.accept_listing("papers", asked, Some("listed".to_owned()))
            }),
            ("batch", |state, asked, status| {
                let before = state.asked("papers");
                let asked = std::collections::HashMap::from([("papers".to_owned(), asked)]);
                state.accept_batch(&asked, vec![status]);
                state.asked("papers") != before
            }),
        ];
        for (name, write) in writers {
            for (newer, landed) in writers {
                // Two requests asked together; the other one lands first.
                let mut state = GogState::default();
                let asked = state.asked("papers");
                assert!(landed(&mut state, asked, fresh()), "{newer} first");
                let known = (
                    state.status("papers").cloned(),
                    state.listed_version("papers").map(str::to_owned),
                );
                assert!(!write(&mut state, asked, stale()), "{name} after {newer}");
                assert_eq!(
                    (
                        state.status("papers").cloned(),
                        state.listed_version("papers").map(str::to_owned)
                    ),
                    known,
                    "{name} after {newer}"
                );
            }
            // And after the game changed.
            let mut state = GogState::default();
            let asked = state.asked("papers");
            state.changed("papers");
            assert!(!write(&mut state, asked, stale()), "{name} after a change");
            assert!(state.status("papers").is_none() && state.listed_version("papers").is_none());
        }
    }

    fn pinned_status(name: &str) -> Status {
        let mut pinned = status(name, UpdateState::Available, false);
        pinned.platform = Platform::Windows;
        pinned.track = Track::Galaxy;
        pinned.pinned = Some("100".to_owned());
        pinned.pinned_name = Some("1.2.76".to_owned());
        pinned.reason = Some(Reason::Pinned {
            name: name.to_owned(),
            build: "100".to_owned(),
            version: Some("1.2.76".to_owned()),
        });
        pinned
    }

    #[test]
    fn a_pinned_game_has_no_update_badge_or_action_and_says_why() {
        let pinned = pinned_status("papers");
        assert!(!updatable(&pinned));
        let plain = gog_card(false, None);
        assert_eq!(badge(plain.as_ref(), Some(&pinned)), None);
        // An interrupted update is still finished, whatever the pin says.
        let finish = gog_card(true, None);
        assert_eq!(
            badge(finish.as_ref(), Some(&pinned)),
            Some(Badge::FinishUpdate)
        );
        assert_eq!(status_label(&pinned), "Pinned");
        assert_eq!(
            pinned.reason.as_ref().unwrap().note_for(Wording::App),
            "Pinned to build 100 (1.2.76). Use Change to follow its branch again."
        );
        assert_eq!(pinned.reason.as_ref().unwrap().as_str(), "pinned");

        let mut app = app();
        let mut game = card("papers", State::Inactive, 0);
        game.gog = gog_card(false, None);
        app.cards = vec![game];
        app.gog_updates.insert("papers".to_owned(), pinned);
        assert!(!app.gog_update_action("papers"));

        // With nothing newer, the pin is still told, and nothing is offered.
        let mut level = pinned_status("papers");
        level.state = UpdateState::UpToDate;
        level.reason = None;
        assert!(!updatable(&level));
        assert_eq!(status_label(&level), "Up to date");
        assert_eq!(
            level.pin_note_for(Wording::App).as_deref(),
            Some("Pinned to build 100 (1.2.76). Use Change to follow its branch again.")
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
