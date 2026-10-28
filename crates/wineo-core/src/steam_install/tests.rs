use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::launch::Overrides;
use crate::runtime::{RunMode, RunState};

fn paths(root: &Path) -> Paths {
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(&home)),
        None,
    )
    .unwrap()
}

fn steam_root(paths: &Paths) -> PathBuf {
    let root = paths.home().join(".local/share/Steam");
    fs::create_dir_all(root.join("steamapps")).unwrap();
    root
}

fn write_manifest(root: &Path, flags: u32, to_download: u64, downloaded: u64) {
    fs::write(
            root.join("steamapps/appmanifest_42.acf"),
            format!(
                "\"AppState\" {{ \"appid\" \"42\" \"name\" \"Real Title\" \"StateFlags\" \"{flags}\" \"BytesToDownload\" \"{to_download}\" \"BytesDownloaded\" \"{downloaded}\" }}"
            ),
        )
        .unwrap();
}

fn game(paths: &Paths, name: &str) {
    let path = paths.environment_config(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"42\"\ntitle = \"Config Title\"\n",
    )
    .unwrap();
}

/// What the fake Steam was asked for, as the program and arguments the
/// handoff would have run.
type Recorded = RefCell<Vec<(String, Vec<String>)>>;

fn ask(
    paths: &Paths,
    name: &str,
    action: Action,
    recorded: &Recorded,
    steam_runs_it: bool,
) -> anyhow::Result<Outcome> {
    request_with(
        paths,
        &GlobalConfig::default(),
        name,
        action,
        &mut |launch| {
            let command = launch.command();
            recorded.borrow_mut().push((
                command.get_program().to_string_lossy().into_owned(),
                command
                    .get_args()
                    .map(OsStr::to_string_lossy)
                    .map(String::from)
                    .collect(),
            ));
            Ok(())
        },
        &|_, _| Ok(steam_runs_it),
    )
}

fn url(recorded: &Recorded) -> Vec<String> {
    recorded
        .borrow()
        .iter()
        .flat_map(|(_, args)| args.clone())
        .collect()
}

#[test]
fn install_admission_follows_the_state() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let recorded = Recorded::default();
    let cases: [(Option<u32>, &str); 7] = [
        (Some(4), "Already installed."),
        (Some(4 | 1024), "Already installed."),
        (Some(1026), "Steam is already installing witcher."),
        (None, "opened"),
        (Some(1), "opened"),
        (Some(2), "opened"),
        (Some(2048), "refused: Steam is uninstalling witcher."),
    ];
    for (flags, expected) in cases {
        match flags {
            Some(flags) => write_manifest(&root, flags, 0, 0),
            None => {
                let _ = fs::remove_file(root.join("steamapps/appmanifest_42.acf"));
            }
        }
        recorded.borrow_mut().clear();
        let outcome = ask(&paths, "witcher", Action::Install, &recorded, false);
        match (expected, outcome) {
            ("opened", Ok(Outcome::Opened)) => {
                assert_eq!(
                    recorded.borrow().as_slice(),
                    [("steam".to_owned(), vec!["steam://install/42".to_owned()])],
                    "{flags:?}"
                );
            }
            (note, Ok(outcome)) => {
                assert_eq!(outcome.note().as_deref(), Some(note), "{flags:?}");
                assert!(recorded.borrow().is_empty(), "{flags:?}");
            }
            (reason, Err(error)) => {
                assert_eq!(format!("refused: {error}"), reason, "{flags:?}");
                assert!(recorded.borrow().is_empty(), "{flags:?}");
            }
        }
    }
}

#[test]
fn install_refuses_an_unreadable_library_or_unknown_state_with_the_reason() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    game(&paths, "witcher");
    let recorded = Recorded::default();
    assert_eq!(
        ask(&paths, "witcher", Action::Install, &recorded, false)
            .unwrap_err()
            .to_string(),
        "Steam isn't found."
    );
    let root = steam_root(&paths);
    let gone = temp.path().join("unmounted");
    fs::write(
        root.join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{ \"1\" {{ \"path\" \"{}\" }} }}",
            gone.display()
        ),
    )
    .unwrap();
    for action in [Action::Install, Action::Uninstall] {
        assert_eq!(
            ask(&paths, "witcher", action, &recorded, false)
                .unwrap_err()
                .to_string(),
            format!("Steam's library at {} isn't available.", gone.display())
        );
    }
    fs::remove_file(root.join("steamapps/libraryfolders.vdf")).unwrap();
    fs::write(
        root.join("steamapps/appmanifest_42.acf"),
        "\"AppState\" { \"appid\" \"42\" }",
    )
    .unwrap();
    for action in [Action::Install, Action::Uninstall] {
        let error = ask(&paths, "witcher", action, &recorded, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("has no StateFlags"), "{error}");
    }
    assert!(recorded.borrow().is_empty());
}

#[test]
fn uninstall_admission_follows_the_state() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let recorded = Recorded::default();
    let cases: [(Option<u32>, bool); 8] = [
        (Some(4), true),
        (Some(4 | 1024), true),
        // Partial installs are allowed, since Steam's dialog cancels them.
        (Some(2), true),
        (Some(1026), true),
        (Some(2048), true),
        (Some(1), false),
        (Some(0), false),
        (None, false),
    ];
    for (flags, opens) in cases {
        match flags {
            Some(flags) => write_manifest(&root, flags, 0, 0),
            None => {
                let _ = fs::remove_file(root.join("steamapps/appmanifest_42.acf"));
            }
        }
        recorded.borrow_mut().clear();
        let outcome = ask(&paths, "witcher", Action::Uninstall, &recorded, false).unwrap();
        if opens {
            assert_eq!(outcome, Outcome::Opened, "{flags:?}");
            assert_eq!(url(&recorded), ["steam://uninstall/42"], "{flags:?}");
        } else {
            assert_eq!(outcome, Outcome::NotInstalled, "{flags:?}");
            assert_eq!(outcome.note().as_deref(), Some("Not installed."));
            assert!(recorded.borrow().is_empty(), "{flags:?}");
        }
    }
}

#[test]
fn uninstall_is_refused_while_the_game_is_starting_or_running() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    write_manifest(&root, 4, 0, 0);
    game(&paths, "witcher");
    let recorded = Recorded::default();
    let refusal = "Stop witcher before uninstalling it.";

    // Started from the Steam client, which only the process scan sees.
    assert_eq!(
        ask(&paths, "witcher", Action::Uninstall, &recorded, true)
            .unwrap_err()
            .to_string(),
        refusal
    );

    // Started by wineo, before and after Steam's reaper shows up.
    let runtime = Runtime::new(&paths, "witcher");
    let lock = runtime.try_lock().unwrap().unwrap();
    for starting in [true, false] {
        let mut run = RunState::running(
            RunMode::Detached,
            "steam".to_owned(),
            1,
            None,
            &Overrides::default(),
        );
        if starting {
            run.starting();
        }
        runtime.write_state(&run).unwrap();
        assert_eq!(
            ask(&paths, "witcher", Action::Uninstall, &recorded, false)
                .unwrap_err()
                .to_string(),
            refusal
        );
    }
    assert!(recorded.borrow().is_empty());

    // Installing something that's running is a no-op, not a refusal.
    assert_eq!(
        ask(&paths, "witcher", Action::Install, &recorded, true).unwrap(),
        Outcome::AlreadyInstalled
    );
    drop(lock);
    runtime
        .write_state(&RunState::running(
            RunMode::Detached,
            "steam".to_owned(),
            1,
            None,
            &Overrides::default(),
        ))
        .unwrap();
    // The state file alone, with nobody holding the lock, isn't a run.
    assert_eq!(
        ask(&paths, "witcher", Action::Uninstall, &recorded, false).unwrap(),
        Outcome::Opened
    );
}

#[test]
fn only_steam_games_can_be_installed_or_uninstalled() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    steam_root(&paths);
    let path = paths.environment_config("dusk");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "kind = \"native\"\n").unwrap();
    let recorded = Recorded::default();
    for (action, message) in [
        (Action::Install, "Only Steam games can be installed."),
        (Action::Uninstall, "Only Steam games can be uninstalled."),
    ] {
        assert_eq!(
            ask(&paths, "dusk", action, &recorded, false)
                .unwrap_err()
                .to_string(),
            message
        );
    }
    assert!(recorded.borrow().is_empty());
}

#[test]
fn a_flatpak_install_asks_flatpak_to_open_the_url() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = paths
        .home()
        .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
    fs::create_dir_all(root.join("steamapps")).unwrap();
    game(&paths, "witcher");
    let recorded = Recorded::default();
    ask(&paths, "witcher", Action::Install, &recorded, false).unwrap();
    assert_eq!(
        recorded.borrow().as_slice(),
        [(
            "flatpak".to_owned(),
            ["run", "com.valvesoftware.Steam", "steam://install/42"]
                .map(String::from)
                .to_vec()
        )]
    );
}

#[test]
fn a_request_survives_the_handoff_and_an_old_one_is_a_launch() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    write_manifest(&root, 4, 0, 0);
    let install = steam::active_for(&paths, &GlobalConfig::default()).unwrap();
    let launch = Launch::steam_request(&paths, install, "42", SteamRequest::Install);
    let text = toml::to_string(&launch).unwrap();
    assert!(text.contains("request = \"install\""), "{text}");
    let read: Launch = toml::from_str(&text).unwrap();
    assert_eq!(
        read.command().get_args().collect::<Vec<_>>(),
        ["steam://install/42"]
    );

    let old = text.replace("request = \"install\"\n", "");
    let read: Launch = toml::from_str(&old).unwrap();
    assert_eq!(
        read.command().get_args().collect::<Vec<_>>(),
        ["-silent", "-applaunch", "42"]
    );
}

fn follower(paths: &Paths, action: Action) -> Follow {
    Follow::new(paths, &GlobalConfig::default(), "witcher", action).unwrap()
}

fn remove_manifest(root: &Path) {
    fs::remove_file(root.join("steamapps/appmanifest_42.acf")).unwrap();
}

#[test]
fn following_an_install_to_the_end() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let mut follow = follower(&paths, Action::Install);
    assert_eq!(follow.title(), "Config Title");

    // The dialog is open and Steam hasn't written anything.
    assert_eq!(follow.poll(), Update::Waiting);
    assert_eq!(follow.poll(), Update::Waiting);
    write_manifest(&root, 2, 0, 0);
    assert_eq!(follow.poll(), Update::Queued);
    assert_eq!(follow.title(), "Real Title");
    write_manifest(&root, 1026, 0, 0);
    assert_eq!(
        follow.poll(),
        Update::Installing {
            percent: None,
            done: 0,
            total: 0
        }
    );
    write_manifest(&root, 1026, 400, 100);
    assert_eq!(
        follow.poll(),
        Update::Installing {
            percent: Some(25),
            done: 100,
            total: 400
        }
    );
    // A pause drops the counts to zero and keeps following.
    write_manifest(&root, 1026, 0, 0);
    assert_eq!(
        follow.poll(),
        Update::Installing {
            percent: None,
            done: 0,
            total: 0
        }
    );
    write_manifest(&root, 1026, 400, 400);
    assert!(matches!(
        follow.poll(),
        Update::Installing {
            percent: Some(100),
            ..
        }
    ));
    // An unreadable manifest mid-write doesn't end it.
    fs::write(root.join("steamapps/appmanifest_42.acf"), "\"AppState\" {").unwrap();
    assert!(matches!(follow.poll(), Update::Unreadable(_)));
    write_manifest(&root, 4, 0, 0);
    let end = follow.poll();
    assert_eq!(end, Update::Finished);
    assert!(end.is_final());
}

#[test]
fn the_bytes_match_the_phase_the_percent_comes_from() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let mut follow = follower(&paths, Action::Install);
    let staging = |downloaded: u64, staged: u64| {
        fs::write(
                root.join("steamapps/appmanifest_42.acf"),
                format!(
                    "\"AppState\" {{ \"appid\" \"42\" \"StateFlags\" \"1026\" \"BytesToDownload\" \"400\" \"BytesDownloaded\" \"{downloaded}\" \"BytesToStage\" \"800\" \"BytesStaged\" \"{staged}\" }}"
                ),
            )
            .unwrap();
    };
    // Downloading: the download bytes.
    staging(100, 100);
    assert_eq!(
        follow.poll(),
        Update::Installing {
            percent: Some(25),
            done: 100,
            total: 400
        }
    );
    // Download complete, staging partial: the staged bytes.
    staging(400, 200);
    assert_eq!(
        follow.poll(),
        Update::Installing {
            percent: Some(25),
            done: 200,
            total: 800
        }
    );
}

#[test]
fn following_an_install_that_steam_cancels() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let mut follow = follower(&paths, Action::Install);
    write_manifest(&root, 1026, 400, 100);
    assert!(matches!(follow.poll(), Update::Installing { .. }));
    remove_manifest(&root);
    let end = follow.poll();
    assert_eq!(end, Update::SteamCanceled);
    assert!(end.is_final());
}

#[test]
fn joining_an_install_steam_already_runs_ends_if_it_vanishes_first() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    // Installing when the request looked, canceled before the first poll.
    write_manifest(&root, 1026, 400, 100);
    let outcome = ask(
        &paths,
        "witcher",
        Action::Install,
        &Recorded::default(),
        true,
    )
    .unwrap();
    assert!(matches!(outcome, Outcome::AlreadyInstalling(_)));
    remove_manifest(&root);

    let mut follow = follower(&paths, Action::Install).already_installing();
    follow.interval = Duration::from_millis(10);
    assert_eq!(follow.wait(&Token::new(), |_, _| {}), End::SteamCanceled);

    // A plain install still waits for the dialog.
    assert_eq!(follower(&paths, Action::Install).poll(), Update::Waiting);
}

#[test]
fn wait_reports_the_title_each_look_left() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let mut follow = follower(&paths, Action::Install);
    follow.interval = Duration::from_millis(10);
    let token = Token::new();
    let mut titles = Vec::new();
    let end = follow.wait(&token, |update, title| {
        titles.push(title.to_owned());
        if *update == Update::Waiting {
            write_manifest(&root, 4, 0, 0);
        }
    });
    assert_eq!(end, End::Finished);
    assert_eq!(titles, ["Config Title", "Real Title"]);
}

#[test]
fn a_manifest_that_never_appeared_is_waited_for_without_a_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    let mut follow = follower(&paths, Action::Install);
    // A leftover "uninstalled" manifest is the same as none.
    write_manifest(&root, 1, 0, 0);
    for _ in 0..50 {
        assert_eq!(follow.poll(), Update::Waiting);
    }
}

#[test]
fn following_an_uninstall() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");

    // The dialog is open, so the game is still fully installed.
    write_manifest(&root, 4, 0, 0);
    let mut follow = follower(&paths, Action::Uninstall);
    assert_eq!(follow.poll(), Update::Uninstalling);
    assert_eq!(follow.poll(), Update::Uninstalling);
    // Step 1 saw the manifest simply vanish.
    remove_manifest(&root);
    assert_eq!(follow.poll(), Update::Finished);

    // It can also pass through 2048 first.
    write_manifest(&root, 4, 0, 0);
    let mut follow = follower(&paths, Action::Uninstall);
    write_manifest(&root, 4 | 2048, 0, 0);
    assert_eq!(follow.poll(), Update::Uninstalling);
    remove_manifest(&root);
    assert_eq!(follow.poll(), Update::Finished);
}

#[test]
fn an_uninstall_that_comes_back_fully_installed_was_canceled() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    write_manifest(&root, 4, 0, 0);
    let mut follow = follower(&paths, Action::Uninstall);
    write_manifest(&root, 4 | 2048, 0, 0);
    assert_eq!(follow.poll(), Update::Uninstalling);
    write_manifest(&root, 4, 0, 0);
    assert_eq!(follow.poll(), Update::SteamCanceled);
}

#[test]
fn an_uninstall_of_a_game_that_is_already_gone_finishes_at_once() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    steam_root(&paths);
    game(&paths, "witcher");
    assert_eq!(follower(&paths, Action::Uninstall).poll(), Update::Finished);
}

#[test]
fn wait_reports_every_look_and_stops_on_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let root = steam_root(&paths);
    game(&paths, "witcher");
    write_manifest(&root, 4, 0, 0);
    let mut follow = follower(&paths, Action::Install);
    let mut seen = Vec::new();
    let end = follow.wait(&Token::new(), |update, _| seen.push(update.clone()));
    assert_eq!(end, End::Finished);
    assert_eq!(seen, [Update::Finished]);

    // Nothing ever appears, so only the token ends the wait.
    remove_manifest(&root);
    let mut follow = follower(&paths, Action::Install);
    follow.interval = Duration::from_millis(10);
    let token = Token::new();
    let mut looks = 0;
    let end = follow.wait(&token, |_, _| {
        looks += 1;
        if looks == 3 {
            token.cancel();
        }
    });
    assert_eq!(end, End::Stopped);
    assert_eq!(looks, 3);

    let mut follow = follower(&paths, Action::Install);
    remove_manifest_if_any(&root);
    write_manifest(&root, 1026, 0, 0);
    let token = Token::new();
    token.cancel();
    assert_eq!(follow.wait(&token, |_, _| panic!("looked")), End::Stopped);
}

fn remove_manifest_if_any(root: &Path) {
    let _ = fs::remove_file(root.join("steamapps/appmanifest_42.acf"));
}

#[test]
fn following_a_non_steam_environment_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let path = paths.environment_config("witcher");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "kind = \"native\"\n").unwrap();
    let error = Follow::new(&paths, &GlobalConfig::default(), "witcher", Action::Install)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "Only Steam games can be installed.");
}
