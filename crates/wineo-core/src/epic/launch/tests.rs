use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::epic::fake::{Fake, Setup};
use crate::epic::game::OfflineStart;
use crate::launch::{Host, Launch, Wayland, Wrappers};

const APP: &str = "AppOffline";
const MARK: &str = "EPICSECRET";

/// A game installed in Legendary's eyes, as the fake reads it, with wineo's
/// record of it.
fn installed(setup: &Setup, can_run_offline: bool) -> Record {
    let folder = setup.root.path().join("Games/Offline Game");
    fs::create_dir_all(folder.join("Binaries/Win64")).unwrap();
    let state = crate::epic::state_dir(&setup.paths).join("fake-installed");
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join(format!("{APP}.launch")),
        format!("{}\nBinaries/Win64/Game.exe\n", folder.display()),
    )
    .unwrap();
    Record {
        installed_version: Some("1.0".to_owned()),
        can_run_offline,
        offline: Some(OfflineStart {
            executable: "Binaries/Win64/Game.exe".to_owned(),
            arguments: vec!["-fromrecord".to_owned()],
            working_directory: Some("Binaries/Win64".to_owned()),
        }),
        ..Record::new(APP, folder)
    }
}

fn mode(setup: &Setup, mode: &str) {
    fs::write(setup.root.path().join("launch-mode"), mode).unwrap();
}

fn usable(setup: &Setup) -> Result<Found, Error> {
    Ok(Found::Usable(Box::new(setup.legendary(&Fake::default()))))
}

fn ask(setup: &Setup, record: &Record) -> Result<Start> {
    ask_with(
        &setup.paths,
        "fowl",
        record,
        || usable(setup),
        &Token::new(),
    )
}

fn launches(setup: &Setup) -> Vec<String> {
    setup
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("launch") || call.starts_with("status"))
        .collect()
}

fn program(record: &Record) -> Option<PathBuf> {
    Some(record.folder.join("Binaries/Win64/Game.exe"))
}

fn epic(start: &Start) -> Vec<String> {
    start.epic.reveal().to_vec()
}

const ONLINE: &str = "launch AppOffline --json --no-wine --skip-version-check";
const OFFLINE: &str = "launch AppOffline --json --no-wine --offline";
const STATUS: &str = "status --offline --json";

#[test]
fn parameters_split_as_pythons_shlex_does_without_posix() {
    let split = |text: &str| split_parameters(text);
    let words = |words: &[&str]| Some(words.iter().map(|&word| word.to_owned()).collect());
    // Each answer is what Python 3.14's `shlex.split(text, posix=False)`
    // gave for the same text.
    assert_eq!(
        split("-skipintro -lang=en"),
        words(&["-skipintro", "-lang=en"])
    );
    assert_eq!(split(r"-dir=C:\Games\Foo"), words(&[r"-dir=C:\Games\Foo"]));
    assert_eq!(
        split(r#""C:\Program Files\x" -a"#),
        words(&[r#""C:\Program Files\x""#, "-a"])
    );
    assert_eq!(
        split(r#"-path="C:\Program Files\x" -a"#),
        words(&[r#"-path="C:\Program"#, r#"Files\x""#, "-a"])
    );
    assert_eq!(split("'a b'c d"), words(&["'a b'", "c", "d"]));
    assert_eq!(split("x\"y z\"w"), words(&["x\"y", "z\"w"]));
    assert_eq!(split("\"\" x"), words(&["\"\"", "x"]));
    assert_eq!(split("a\tb\nc\rd"), words(&["a", "b", "c", "d"]));
    assert_eq!(split("  \t "), words(&[]));
    assert_eq!(split("\"open -a"), None);
}

#[test]
fn an_online_start_asks_once_and_keeps_epics_arguments_secret() {
    let setup = Setup::new();
    setup.log_in();
    let record = installed(&setup, true);
    let start = ask(&setup, &record).unwrap();
    assert_eq!(start.program, program(&record));
    assert_eq!(start.parameters, ["-skipintro", "-lang=en"]);
    assert_eq!(start.notice, None);
    assert_eq!(launches(&setup), [ONLINE]);
    let epic = epic(&start);
    assert_eq!(epic[0], "-AUTH_LOGIN=unused");
    assert_eq!(epic[1], "-AUTH_PASSWORD=EPICSECRETcode");
    assert!(epic.contains(&"-epicusername=EPICSECRETuser".to_owned()));
    assert!(!format!("{start:?}").contains(MARK), "{start:?}");
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_game_with_an_update_waiting_still_starts_online() {
    let setup = Setup::new();
    setup.log_in();
    mode(&setup, "outdated");
    let record = installed(&setup, false);
    let start = ask(&setup, &record).unwrap();
    assert_eq!(start.notice, None);
    assert_eq!(epic(&start)[1], "-AUTH_PASSWORD=EPICSECRETcode");
    assert_eq!(launches(&setup), [ONLINE]);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn when_epic_doesnt_answer_a_game_marked_offline_starts_through_legendarys_offline_answer() {
    let setup = Setup::new();
    setup.log_in();
    mode(&setup, "unreachable");
    let record = installed(&setup, true);
    let start = ask(&setup, &record).unwrap();
    assert_eq!(
        start.notice.as_deref(),
        Some("Epic didn't answer, so fowl starts offline. Online features may not work.")
    );
    assert_eq!(start.program, program(&record));
    assert_eq!(start.parameters, ["-skipintro", "-lang=en"]);
    let epic = epic(&start);
    // Offline, no code, but still the account.
    assert_eq!(epic[1], "-AUTH_PASSWORD=");
    assert!(epic.contains(&"-epicuserid=EPICSECRETid".to_owned()));
    assert_eq!(launches(&setup), [ONLINE, STATUS, OFFLINE]);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn when_epic_doesnt_answer_a_game_not_marked_offline_refuses() {
    let setup = Setup::new();
    setup.log_in();
    mode(&setup, "unreachable");
    let record = installed(&setup, false);
    let error = ask(&setup, &record).unwrap_err();
    assert_eq!(
        error.to_string(),
        "fowl isn't marked to run offline, and Epic didn't answer: `legendary launch AppOffline --json --no-wine --skip-version-check` failed: exit code 1: [cli] ERROR: Login failed, cannot continue!"
    );
    assert_eq!(launches(&setup), [ONLINE, STATUS]);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn logged_out_a_game_marked_offline_starts_from_wineos_record() {
    let setup = Setup::new();
    let record = installed(&setup, true);
    let start = ask(&setup, &record).unwrap();
    assert_eq!(
        start,
        Start {
            program: program(&record),
            parameters: vec!["-fromrecord".to_owned()],
            environment: BTreeMap::new(),
            epic: Secret::default(),
            notice: Some(
                "Not logged in to Epic, so fowl starts offline. Online features may not work."
                    .to_owned()
            ),
        }
    );
    assert_eq!(launches(&setup), [ONLINE, STATUS]);

    let record = Record {
        can_run_offline: false,
        ..record
    };
    assert_eq!(
        ask(&setup, &record).unwrap_err().to_string(),
        "fowl isn't marked to run offline, so it needs an Epic login. Log in with wineo import epic --login."
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_login_epic_no_longer_takes_counts_as_logged_out() {
    let setup = Setup::new();
    setup.log_in();
    mode(&setup, "expired");
    let record = installed(&setup, true);
    let start = ask(&setup, &record).unwrap();
    assert_eq!(start.parameters, ["-fromrecord"]);
    assert!(start.epic.is_empty());
    assert!(
        start
            .notice
            .unwrap()
            .starts_with("Not logged in to Epic, so fowl starts offline.")
    );
    assert_eq!(launches(&setup), [ONLINE, STATUS]);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

fn missing() -> Result<Found, Error> {
    Ok(Found::Missing(legendary::Missing {
        older_on_path: None,
    }))
}

fn no_python() -> Result<Found, Error> {
    Err(Error::Python)
}

fn broken_setting() -> Result<Found, Error> {
    Err(Error::Setting {
        shown: "~/nowhere".to_owned(),
        reason: "there is nothing there".to_owned(),
    })
}

#[test]
fn without_legendary_a_game_marked_offline_starts_from_wineos_record() {
    let setup = Setup::new();
    let record = installed(&setup, true);
    let unmarked = Record {
        can_run_offline: false,
        ..record.clone()
    };
    let cases: [(fn() -> Result<Found, Error>, &str, &str); 3] = [
        (
            missing,
            "Legendary isn't installed, so fowl starts offline. Online features may not work.",
            "fowl isn't marked to run offline, so it needs Legendary to start. Get it with wineo import epic --login.",
        ),
        (
            no_python,
            "Legendary needs Python 3.10 or newer, so fowl starts offline. Online features may not work.",
            "fowl isn't marked to run offline, and Epic needs Python 3.10 or newer to run Legendary.",
        ),
        (
            broken_setting,
            "Legendary can't be used, so fowl starts offline. Online features may not work.",
            "fowl isn't marked to run offline, and Legendary can't be used: [epic] legendary is set to ~/nowhere, which can't be used: there is nothing there.",
        ),
    ];
    for (found, notice, refusal) in cases {
        let start = ask_with(&setup.paths, "fowl", &record, found, &Token::new()).unwrap();
        assert_eq!(start.notice.as_deref(), Some(notice));
        assert_eq!(start.program, program(&record));
        assert_eq!(start.parameters, ["-fromrecord"]);
        assert!(start.epic.is_empty());
        let error = ask_with(&setup.paths, "fowl", &unmarked, found, &Token::new()).unwrap_err();
        assert_eq!(error.to_string(), refusal);
    }
    assert!(setup.calls().is_empty(), "{:?}", setup.calls());
}

#[test]
fn a_record_with_no_offline_program_keeps_the_targets() {
    let setup = Setup::new();
    let record = Record {
        offline: None,
        ..installed(&setup, true)
    };
    let start = ask(&setup, &record).unwrap();
    assert_eq!(start.program, None);
    assert!(start.parameters.is_empty());
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn an_answer_for_another_folder_or_a_program_outside_it_is_refused() {
    let setup = Setup::new();
    setup.log_in();
    let record = installed(&setup, true);
    mode(&setup, "elsewhere");
    assert_eq!(
        ask(&setup, &record).unwrap_err().to_string(),
        format!(
            "Legendary puts fowl's game in /elsewhere, not {}. Repair it with wineo update fowl --repair.",
            record.folder.display()
        )
    );
    mode(&setup, "outside");
    assert_eq!(
        ask(&setup, &record).unwrap_err().to_string(),
        "Legendary names a program outside fowl's game folder."
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn an_answer_that_cant_be_read_is_epic_not_answering_and_never_quoted() {
    let setup = Setup::new();
    setup.log_in();
    mode(&setup, "garbled");
    let record = installed(&setup, true);
    let error = format!("{:#}", ask(&setup, &record).unwrap_err());
    assert_eq!(
        error,
        "Epic didn't answer, and fowl couldn't start offline: Couldn't read what `legendary launch AppOffline --json --no-wine --offline` printed: it isn't the launch answer wineo expects."
    );
    assert_eq!(launches(&setup), [ONLINE, STATUS, OFFLINE]);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn asking_waits_for_the_epic_lock() {
    let setup = Setup::new();
    setup.log_in();
    let record = installed(&setup, true);
    let lock = EpicLock::acquire(&setup.paths).unwrap();
    std::thread::scope(|scope| {
        let asking = scope.spawn(|| ask(&setup, &record));
        std::thread::sleep(Duration::from_millis(500));
        assert!(launches(&setup).is_empty(), "{:?}", setup.calls());
        assert!(!asking.is_finished());
        drop(lock);
        asking.join().unwrap().unwrap();
    });
    assert_eq!(launches(&setup), [ONLINE]);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_cancel_is_a_canceled_launch() {
    let setup = Setup::new();
    setup.log_in();
    let record = installed(&setup, true);
    let token = Token::new();
    token.cancel();
    let error = ask_with(&setup.paths, "fowl", &record, || usable(&setup), &token).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<crate::Error>(),
            Some(crate::Error::Canceled("launch"))
        ),
        "{error:#}"
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

fn native(args: &[&str], env: &[(&str, &str)]) -> Launch {
    Launch {
        host: Host::Native {
            exe: PathBuf::from("/bin/true"),
        },
        target: Some("game".to_owned()),
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        cwd: PathBuf::from("/target/cwd"),
        env: env
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        wrappers: Wrappers::default(),
        wayland: Wayland::Off,
        gameid: None,
        store: None,
        epic: Secret::default(),
        build_guards: Vec::new(),
    }
}

fn start(program: Option<&str>) -> Start {
    Start {
        program: program.map(PathBuf::from),
        parameters: vec!["-skipintro".to_owned(), "-lang=en".to_owned()],
        environment: [
            ("FAKE_VAR", "legendary"),
            ("EPIC_FAKE", "legendary"),
            ("WINEPREFIX", "/legendary"),
            ("PROTONPATH", "/legendary"),
            ("", "nameless"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect(),
        epic: Secret::new(&[
            "-AUTH_PASSWORD=EPICSECRETcode",
            "-epicusername=EPICSECRETuser",
        ]),
        notice: None,
    }
}

fn args(launch: &Launch) -> Vec<String> {
    launch
        .command()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_game_runs_with_its_parameters_then_the_targets_then_epics() {
    let mut launch = native(
        &["/game/Old.exe", "-target", "-override"],
        &[("FAKE_VAR", "environment")],
    );
    launch.run_epic(start(Some("/game/Binaries/Game.exe")), 2, false);
    assert_eq!(
        args(&launch),
        [
            "/game/Binaries/Game.exe",
            "-skipintro",
            "-lang=en",
            "-target",
            "-override",
            "-AUTH_PASSWORD=EPICSECRETcode",
            "-epicusername=EPICSECRETuser",
        ]
    );
    assert_eq!(launch.cwd, Path::new("/game/Binaries"));
    // Legendary's variables go under the environment's own, and never
    // replace what wineo sets itself.
    assert_eq!(
        launch.env,
        [("EPIC_FAKE", "legendary"), ("FAKE_VAR", "environment")]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect::<BTreeMap<_, _>>()
    );
    assert!(!format!("{launch:?}").contains(MARK), "{launch:?}");
    // The supervisor's pipe is the one place it is written out.
    let text = toml::to_string(&launch).unwrap();
    let back: Launch = toml::from_str(&text).unwrap();
    assert_eq!(back.epic, launch.epic);
}

#[test]
fn a_target_with_its_own_working_folder_keeps_it() {
    let mut launch = native(&["/game/Old.exe"], &[]);
    launch.run_epic(start(Some("/game/Binaries/Game.exe")), 0, true);
    assert_eq!(launch.cwd, Path::new("/target/cwd"));
    assert_eq!(
        args(&launch)[..3],
        ["/game/Binaries/Game.exe", "-skipintro", "-lang=en"]
    );
}

#[test]
fn a_start_without_a_program_keeps_the_targets() {
    let mut launch = native(&["/game/Old.exe", "-target"], &[]);
    launch.run_epic(start(None), 1, false);
    assert_eq!(launch.cwd, Path::new("/target/cwd"));
    assert_eq!(
        args(&launch)[..4],
        ["/game/Old.exe", "-skipintro", "-lang=en", "-target"]
    );
}

#[test]
fn a_launch_without_epics_arguments_writes_none() {
    let launch = native(&["/game/Old.exe"], &[]);
    assert!(!toml::to_string(&launch).unwrap().contains("epic"));
}
