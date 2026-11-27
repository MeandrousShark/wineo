use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::epic::fake::{Fake, Setup};
use crate::epic::owned::Game;
use crate::test_script::write_script;

const NEVER: &dyn Fn() -> bool = &|| false;

/// A drop-in runner named `fake` whose wine does nothing.
fn runner(setup: &Setup) {
    let bin = GlobalConfig::default()
        .runner_root(&setup.paths)
        .unwrap()
        .join("fake/bin");
    fs::create_dir_all(&bin).unwrap();
    for program in ["wine", "wineserver"] {
        write_script(bin.join(program), "#!/bin/sh\nexit 0\n");
    }
}

/// A logged-in host with the fake on `PATH` and a runner to create with.
fn host() -> (Setup, Legendary) {
    let setup = Setup::new();
    runner(&setup);
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    (setup, legendary)
}

fn owned(app: &str) -> Game {
    let list = fs::read_to_string(crate::epic::fake::fixture("list.json")).unwrap();
    owned::parse_list(&list)
        .unwrap()
        .into_iter()
        .find(|game| game.app_name == app)
        .unwrap()
}

fn request(app: &str) -> Request {
    Request {
        app_name: app.to_owned(),
        runner: Some("fake".to_owned()),
        desktop: false,
        ..Request::default()
    }
}

fn plan_for(legendary: &Legendary, app: &str) -> Plan {
    let found = existing(legendary.paths(), app).unwrap();
    plan(legendary, &request(app), &owned(app), &found, &Token::new()).unwrap()
}

/// Builds the prefix as wineboot would, which the fake wine doesn't, and
/// notes the rest.
fn on_event(event: Event, seen: &mut Vec<String>) {
    match event {
        Event::Create(CreateEvent::BuildingPrefix { dir, .. }) => {
            fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        }
        Event::Started {
            index,
            total,
            title,
        } => seen.push(format!("start {index}/{total} {title}")),
        Event::Progress {
            index,
            total,
            percent,
        } => seen.push(format!("progress {index}/{total} {percent}")),
        Event::Stalled { index, .. } => seen.push(format!("stalled {index}")),
        Event::DlcFailed { title, .. } => seen.push(format!("dlc failed {title}")),
        Event::Working { .. } | Event::Create(_) => {}
    }
}

fn install(legendary: &Legendary, plan: &Plan, token: &Token) -> (Result<Installed>, Vec<String>) {
    let mut seen = Vec::new();
    let result = apply(legendary, plan, STALL, token, |event| {
        on_event(event, &mut seen)
    });
    (result, seen)
}

fn set_mode(setup: &Setup, app: &str, mode: &str) {
    fs::write(setup.root.path().join(format!("mode-{app}")), mode).unwrap();
}

fn record(setup: &Setup, app: &str) -> Option<Record> {
    game::peek(&setup.paths, app).unwrap()
}

fn games(setup: &Setup) -> PathBuf {
    fs::canonicalize(setup.root.path()).unwrap().join("Games")
}

fn installs(setup: &Setup) -> Vec<String> {
    setup
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("-y install"))
        .collect()
}

/// Waits for the fake to say it started installing `app`.
fn wait_started(setup: &Setup, app: &str) {
    let marker = setup.root.path().join(format!("installing-{app}"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the fake never started {app}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Runs `apply` on a thread and cancels it once the fake has started `app`.
fn install_canceled(
    setup: &Setup,
    legendary: &Legendary,
    plan: &Plan,
    app: &str,
) -> Result<Installed> {
    let token = Token::new();
    thread::scope(|scope| {
        let running = scope.spawn(|| install(legendary, plan, &token).0);
        wait_started(setup, app);
        token.cancel();
        running.join().unwrap()
    })
}

#[test]
fn progress_lines_read_as_their_percentage() {
    assert_eq!(
        progress(
            "[DLManager] INFO: = Progress: 6.38% (4506/70640), Running for 00:00:30, ETA: 00:07:20"
        ),
        Some(6.38)
    );
    assert_eq!(
        progress("[DLManager] INFO: = Progress: 100.00% (2/2)"),
        Some(100.0)
    );
    for line in [
        "[DLManager] INFO:  - Downloaded: 1.00 MiB, Written: 1.00 MiB",
        "[DLManager] INFO: = Progress: lots% (1/2)",
        "[DLManager] INFO: = Progress: 150.00% (3/2)",
        "",
    ] {
        assert_eq!(progress(line), None, "{line}");
    }
    let fixture =
        fs::read_to_string(crate::epic::fake::fixture("install-finished.stderr")).unwrap();
    let read: Vec<f64> = fixture.lines().filter_map(progress).collect();
    assert_eq!(read.first(), Some(&0.0));
    assert_eq!(read.last(), Some(&100.0));
}

#[test]
fn the_game_and_each_dlc_install_as_checked_runs_then_the_environment() {
    let (setup, legendary) = host();
    let app = "Dlc Game";
    let plan = plan_for(&legendary, app);
    let folder = games(&setup).join("Dlc Game");
    assert_eq!(plan.folder(), folder);
    assert_eq!(plan.environment(), "dlc-game");
    assert_eq!(plan.runs(), 3);
    assert_eq!(plan.download_size(), Some(156_518_026_736));
    assert_eq!(plan.install_size(), Some(163_315_444_480));
    assert!(!folder.exists(), "planning creates nothing");
    assert!(record(&setup, app).is_none());

    let (installed, seen) = install(&legendary, &plan, &Token::new());
    let installed = installed.unwrap();
    assert_eq!(installed.environment, "dlc-game");
    assert!(installed.created);
    assert_eq!(
        installed.dlc,
        ["Dlc Game Extra One Content", "Extra Two Dlc Game Content"]
    );
    assert!(installed.warnings.is_empty());

    // One run each, in order, each told the exact folder.
    let base = games(&setup).display().to_string();
    assert_eq!(
        installs(&setup),
        [
            app,
            "0000000000000000000000000000000c",
            "0000000000000000000000000000000e"
        ]
        .map(|name| format!(
            "-y install {name} --base-path {base} --game-folder Dlc Game --skip-dlcs --disable-sdl"
        ))
    );
    assert_eq!(
        seen,
        [
            "start 1/3 Dlc Game",
            "progress 1/3 50",
            "progress 1/3 100",
            "start 2/3 Dlc Game Extra One Content",
            "progress 2/3 50",
            "progress 2/3 100",
            "start 3/3 Extra Two Dlc Game Content",
            "progress 3/3 50",
            "progress 3/3 100",
        ]
    );

    let record = record(&setup, app).unwrap();
    assert_eq!(record.in_flight, None, "the mark is cleared");
    assert_eq!(record.folder, folder);
    assert_eq!(record.installed_version.as_deref(), Some("1.0"));
    assert!(record.can_run_offline);
    assert_eq!(
        record.offline,
        Some(OfflineStart {
            executable: "Binaries/Win64/Game.exe".to_owned(),
            arguments: vec!["-skipintro".to_owned(), "-lang=en".to_owned()],
            working_directory: Some("Binaries/Win64".to_owned()),
        })
    );
    assert_eq!(
        record
            .dlc
            .iter()
            .map(|dlc| dlc.app_name.as_str())
            .collect::<Vec<_>>(),
        [
            "0000000000000000000000000000000c",
            "0000000000000000000000000000000e"
        ]
    );

    let environment = Environment::load(&setup.paths, "dlc-game").unwrap();
    assert_eq!(environment.config.epic_id.as_deref(), Some(app));
    let target = &environment.config.targets[TARGET];
    assert_eq!(
        Path::new(&target.exe),
        folder.join("Binaries/Win64/Game.exe")
    );
    assert!(target.args.is_empty(), "Legendary gives them at each start");
    assert_eq!(
        target.cwd.as_deref().map(Path::new),
        Some(folder.join("Binaries/Win64").as_path())
    );
    // Wine reads no store.
    let text = fs::read_to_string(setup.paths.environment_config("dlc-game")).unwrap();
    assert!(!text.contains("store"), "{text}");
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_dlc_that_fails_after_the_game_is_a_warning() {
    let (setup, legendary) = host();
    let app = "Dlc Game";
    let dlc = "0000000000000000000000000000000c";
    set_mode(&setup, dlc, "unrecorded");
    let plan = plan_for(&legendary, app);
    let (installed, seen) = install(&legendary, &plan, &Token::new());
    let installed = installed.unwrap();
    assert_eq!(installed.dlc, ["Extra Two Dlc Game Content"]);
    assert_eq!(
        installed.warnings,
        [
            "Couldn't install Dlc Game Extra One Content: Legendary didn't install Dlc Game Extra One Content: it isn't in Legendary's records afterwards. The game installed without it."
        ]
    );
    assert!(seen.contains(&"dlc failed Dlc Game Extra One Content".to_owned()));
    let record = record(&setup, app).unwrap();
    assert_eq!(record.in_flight, None);
    assert_eq!(
        record
            .dlc
            .iter()
            .map(|dlc| dlc.app_name.as_str())
            .collect::<Vec<_>>(),
        ["0000000000000000000000000000000e"]
    );
    assert!(setup.paths.environment_config("dlc-game").exists());
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_game_run_that_doesnt_install_keeps_the_mark() {
    for (mode, detail) in [
        ("unrecorded", "it isn't in Legendary's records afterwards"),
        (
            "failed-write",
            "it logged an error (Writing file failed: [Errno 28] No space left on device)",
        ),
        (
            "elsewhere",
            "Legendary's records put it somewhere other than",
        ),
        ("exit1", "`legendary -y install AppOffline"),
    ] {
        let (setup, legendary) = host();
        let app = "AppOffline";
        set_mode(&setup, app, mode);
        let plan = plan_for(&legendary, app);
        let (installed, _) = install(&legendary, &plan, &Token::new());
        let error = format!("{:#}", installed.unwrap_err());
        assert!(error.contains(detail), "{mode}: {error}");
        let record = record(&setup, app).unwrap();
        assert_eq!(
            record.in_flight.map(|mark| mark.work),
            Some(Work::Install),
            "{mode}"
        );
        assert!(
            !setup.paths.environment_config(plan.environment()).exists(),
            "{mode}"
        );
        assert!(left_unfinished(&setup.paths, app));
        assert_eq!(setup.sweep(), Vec::<i32>::new(), "{mode}");
    }
}

#[test]
fn a_cancel_interrupts_the_group_keeps_the_mark_and_the_same_install_resumes() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    set_mode(&setup, app, "hang");
    let plan = plan_for(&legendary, app);
    let started = Instant::now();
    let error = install_canceled(&setup, &legendary, &plan, app).unwrap_err();
    assert!(is_cancel(&error), "{error:#}");
    assert!(matches!(
        error.downcast_ref(),
        Some(InstallError::Canceled { marked: true })
    ));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "an interrupt ended it, not the fake's own bound"
    );
    // The fake heeded the interrupt and left Legendary's resume file.
    assert_eq!(
        fs::read_to_string(setup.root.path().join("got-int")).unwrap(),
        "AppOffline\n"
    );
    let resume = crate::epic::state_dir(&setup.paths).join("tmp/AppOffline.resume");
    assert!(resume.exists());
    let record = record(&setup, app).unwrap();
    assert_eq!(record.in_flight.map(|mark| mark.work), Some(Work::Install));
    assert!(matches!(
        existing(&setup.paths, app).unwrap(),
        Existing::Unfinished { owner: None, .. }
    ));
    assert_eq!(setup.sweep(), Vec::<i32>::new());

    // The same request carries on in the same folder.
    set_mode(&setup, app, "");
    let again = plan_for(&legendary, app);
    assert!(again.resumes());
    assert_eq!(again.folder(), plan.folder());
    let (installed, _) = install(&legendary, &again, &Token::new());
    let installed = installed.unwrap();
    assert!(installed.resumed);
    assert_eq!(installed.folder, plan.folder());
    assert_eq!(installs(&setup).len(), 2);
    assert_eq!(installs(&setup)[0], installs(&setup)[1]);
    assert_eq!(
        game::peek(&setup.paths, app).unwrap().unwrap().in_flight,
        None
    );
    assert!(!resume.exists());
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

/// A cancel's group interrupt that lands after the fake's shell last looked
/// for traps but before it forks its sleep reaches the shell alone, and a
/// shell runs its trap only once its foreground child ends. Sent to the
/// waiting shell alone, the interrupt must still be heeded within wineo's
/// grace, or the cancel tests lose the resume file to the kill.
#[test]
fn the_fake_heeds_an_interrupt_its_sleep_never_got() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use rustix::process::{Pid, Signal, kill_process};

    use crate::test_script::KillOnDrop;

    let (setup, _legendary) = host();
    let app = "AppOffline";
    set_mode(&setup, app, "hang");
    let base = games(&setup).display().to_string();
    let mut command = Command::new(setup.bin.join("legendary"));
    command
        .args(["-y", "install", app, "--base-path", &base])
        .args(["--game-folder", "Offline Game"])
        .env_clear()
        .envs(setup.env().for_legendary(&setup.paths))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut fake = KillOnDrop::group(command.spawn().unwrap());
    wait_started(&setup, app);
    let pid = fake.id();
    let children = format!("/proc/{pid}/task/{pid}/children");
    let deadline = Instant::now() + Duration::from_secs(10);
    while fs::read_to_string(&children).unwrap().trim().is_empty() {
        assert!(Instant::now() < deadline, "the fake never slept");
        thread::sleep(Duration::from_millis(5));
    }
    kill_process(Pid::from_raw(pid as i32).unwrap(), Signal::INT).unwrap();
    // wineo's grace before it kills the group.
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = fake.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the interrupt waited on the sleep"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    assert_eq!(
        fs::read_to_string(setup.root.path().join("got-int")).unwrap(),
        "AppOffline\n"
    );
    assert!(
        crate::epic::state_dir(&setup.paths)
            .join("tmp/AppOffline.resume")
            .exists()
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_group_that_ignores_the_interrupt_is_killed_and_only_its_shared_memory_removed() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let tag = format!("wineotest{}x{}", std::process::id(), line!());
    fs::write(setup.root.path().join("shm-tag"), &tag).unwrap();
    let held = PathBuf::from(format!("/dev/shm/psm_{tag}_held"));
    let decoy = PathBuf::from(format!("/dev/shm/psm_{tag}_decoy"));
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }
    let _cleanup = Cleanup(vec![held.clone(), decoy.clone()]);
    // A segment by the same pattern that the group doesn't hold.
    fs::write(&decoy, "").unwrap();
    set_mode(&setup, app, "deaf");
    let plan = plan_for(&legendary, app);
    let token = Token::new();
    let started = Instant::now();
    let error = thread::scope(|scope| {
        let running = scope.spawn(|| install(&legendary, &plan, &token).0);
        wait_started(&setup, app);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !held.exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        token.cancel();
        running.join().unwrap()
    })
    .unwrap_err();
    assert!(is_cancel(&error), "{error:#}");
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(5),
        "waited out the grace: {took:?}"
    );
    assert!(took < Duration::from_secs(25), "killed, not left: {took:?}");
    assert!(!held.exists(), "the group's segment is removed");
    assert!(decoy.exists(), "a segment it didn't hold is left");
    assert!(!setup.root.path().join("got-int").exists());
    assert_eq!(
        record(&setup, app).unwrap().in_flight.map(|mark| mark.work),
        Some(Work::Install)
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn the_epic_lock_is_let_go_once_legendary_is_downloading() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    set_mode(&setup, app, "hang");
    let plan = plan_for(&legendary, app);
    let token = Token::new();
    thread::scope(|scope| {
        let running = scope.spawn(|| install(&legendary, &plan, &token).0);
        wait_started(&setup, app);
        let (sender, receiver) = mpsc::channel();
        let paths = setup.paths.clone();
        // A launch asking Legendary meanwhile gets the lock.
        let waiter = thread::spawn(move || {
            let _lock = EpicLock::acquire(&paths).unwrap();
            let _ = sender.send(());
        });
        let got = receiver.recv_timeout(Duration::from_secs(10));
        token.cancel();
        let _ = running.join().unwrap();
        waiter.join().unwrap();
        assert!(got.is_ok(), "the lock was still held during the download");
    });
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_download_with_no_progress_is_said_once_and_carries_on() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    set_mode(&setup, app, "quiet");
    let plan = plan_for(&legendary, app);
    let mut seen = Vec::new();
    apply(
        &legendary,
        &plan,
        Duration::from_secs(1),
        &Token::new(),
        |event| on_event(event, &mut seen),
    )
    .unwrap();
    assert_eq!(
        seen.iter()
            .filter(|line| line.starts_with("stalled"))
            .count(),
        1,
        "{seen:?}"
    );
    assert_eq!(record(&setup, app).unwrap().in_flight, None);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn the_folder_has_to_be_new_or_empty_and_apart_from_other_games() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let folder = games(&setup).join("Offline Game");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("mine.txt"), "").unwrap();
    let found = existing(&setup.paths, app).unwrap();
    let error = plan(
        &legendary,
        &request(app),
        &owned(app),
        &found,
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("{} already exists and isn't empty.", folder.display())
    );
    // Empty is fine.
    fs::remove_file(folder.join("mine.txt")).unwrap();
    plan(
        &legendary,
        &request(app),
        &owned(app),
        &found,
        &Token::new(),
    )
    .unwrap();

    // Inside another Epic game's folder is refused.
    let other = games(&setup).join("Other");
    let lock = RecordLock::exclusive(&setup.paths, "AppOther", NEVER).unwrap();
    lock.write(&setup.paths, &Record::new("AppOther", other.clone()))
        .unwrap();
    drop(lock);
    let inside = Request {
        dir: Some(other.join("sub")),
        ..request(app)
    };
    let error = plan(&legendary, &inside, &owned(app), &found, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "{} overlaps Epic game AppOther's folder {}.",
            other.join("sub").display(),
            other.display()
        )
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn unavailable_and_downloaded_games_are_refused_before_legendary_is_asked() {
    let (setup, legendary) = host();
    let mut third = owned("AppThirdParty");
    third.third_party = Some("Origin".to_owned());
    let error = plan(
        &legendary,
        &request("AppThirdParty"),
        &third,
        &Existing::None,
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Third Party Game can't be installed: another launcher manages it."
    );
    let mut no_windows = owned("AppOffline");
    no_windows.version = None;
    let error = plan(
        &legendary,
        &request("AppOffline"),
        &no_windows,
        &Existing::None,
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Offline Game can't be installed: Epic has no Windows build of it."
    );

    // A game needing another launcher's account, as the owned list says.
    let error = plan(
        &legendary,
        &request("AppThirdParty"),
        &owned("AppThirdParty"),
        &Existing::None,
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Third Party Game can't be installed: it needs an account at another launcher."
    );

    let folder = games(&setup).join("Offline Game");
    let lock = RecordLock::exclusive(&setup.paths, "AppOffline", NEVER).unwrap();
    lock.write(&setup.paths, &Record::new("AppOffline", folder.clone()))
        .unwrap();
    drop(lock);
    let found = existing(&setup.paths, "AppOffline").unwrap();
    assert!(matches!(found, Existing::Downloaded { .. }));
    let error = plan(
        &legendary,
        &request("AppOffline"),
        &owned("AppOffline"),
        &found,
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Epic game AppOffline is already downloaded in {}, and no environment owns it. Restore its environment with wineo restore, or forget it with wineo import epic --forget AppOffline.",
            folder.display()
        )
    );
    assert!(
        !setup.calls().iter().any(|call| call.starts_with("info")),
        "{:?}",
        setup.calls()
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn the_review_warns_about_what_wineo_doesnt_handle() {
    let (setup, legendary) = host();
    let prereq = plan_for(&legendary, "AppPrereq");
    assert_eq!(
        prereq.warnings(),
        [
            "Prereq Game\u{ae} uses Easy Anti-Cheat, which may keep it from running under Wine or Proton.",
            "Prereq Game\u{ae} ships a prerequisite installer that wineo doesn't run: Binaries/Win64/EasyAntiCheat/EasyAntiCheat_EOS_Setup.exe.",
        ]
    );
    let token = plan_for(&legendary, "AppToken");
    assert_eq!(
        token.warnings(),
        ["Token Game 2020 checks an Epic ownership token, which usually means Denuvo."]
    );
    let dlc = plan_for(&legendary, "Dlc Game");
    assert_eq!(
        dlc.warnings(),
        ["Dlc Game isn't marked to run offline, so it starts only while Epic answers."]
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn an_unfinished_install_finishes_only_in_its_own_folder() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let folder = games(&setup).join("Offline Game");
    fs::create_dir_all(&folder).unwrap();
    let lock = RecordLock::exclusive(&setup.paths, app, NEVER).unwrap();
    let mut unfinished = Record::new(app, folder.clone());
    lock.mark(&setup.paths, &mut unfinished, Work::Install, app)
        .unwrap();
    drop(lock);
    let found = existing(&setup.paths, app).unwrap();
    let elsewhere = Request {
        dir: Some(setup.root.path().join("Elsewhere")),
        ..request(app)
    };
    let error = plan(&legendary, &elsewhere, &owned(app), &found, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Offline Game's install didn't finish, so --game finishes it in {}. Leave out --dir.",
            folder.display()
        )
    );
    // What Legendary wrote doesn't make the folder unusable.
    fs::write(folder.join("partial.bin"), "").unwrap();
    let plan = plan(
        &legendary,
        &request(app),
        &owned(app),
        &found,
        &Token::new(),
    )
    .unwrap();
    assert!(plan.resumes());
    let (installed, _) = install(&legendary, &plan, &Token::new());
    installed.unwrap();
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn an_install_refuses_a_game_another_environment_took_meanwhile() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let plan = plan_for(&legendary, app);
    // Another environment claims the game after the review.
    let config = setup.paths.environment_config("other");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "prefix = \"/tmp/other\"\nrunner = \"fake\"\narch = \"win64\"\nepic_id = \"AppOffline\"\n",
    )
    .unwrap();
    let (installed, _) = install(&legendary, &plan, &Token::new());
    let error = installed.unwrap_err();
    assert_eq!(
        error.to_string(),
        "offline-game's Epic game AppOffline belongs to other now."
    );
    assert!(installs(&setup).is_empty(), "refused before downloading");
    assert!(record(&setup, app).is_none());
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn forget_drops_both_records_and_leaves_the_files() {
    let (setup, legendary) = host();
    let app = "Dlc Game";
    let plan = plan_for(&legendary, app);
    install(&legendary, &plan, &Token::new()).0.unwrap();
    let folder = plan.folder().to_owned();

    // Owned, it's refused.
    let error = forget(&setup.paths, Some(&legendary), app, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Epic game Dlc Game belongs to dlc-game, so it can't be forgotten."
    );
    fs::remove_file(setup.paths.environment_config("dlc-game")).unwrap();

    // Legendary is needed for its own record.
    let error = forget(&setup.paths, None, app, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Forgetting Epic game Dlc Game needs Legendary, to drop its own record of it."
    );
    assert!(record(&setup, app).is_some());

    let forgotten = forget(&setup.paths, Some(&legendary), app, &Token::new()).unwrap();
    assert_eq!(forgotten.folder, folder);
    assert!(record(&setup, app).is_none());
    assert!(game::legendary_installed(&setup.paths).unwrap().is_empty());
    let uninstalls: Vec<String> = setup
        .calls()
        .into_iter()
        .filter(|call| call.contains("uninstall"))
        .collect();
    assert_eq!(
        uninstalls,
        [
            "-y uninstall 0000000000000000000000000000000c --keep-files",
            "-y uninstall 0000000000000000000000000000000e --keep-files",
            "-y uninstall Dlc Game --keep-files",
        ]
    );
    assert!(
        folder.join("Binaries/Win64/Game.exe").exists(),
        "files stay"
    );

    let error = forget(&setup.paths, Some(&legendary), app, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "wineo has no record of Epic game Dlc Game."
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn forget_drops_an_unfinished_install_without_legendary() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    set_mode(&setup, app, "hang");
    let plan = plan_for(&legendary, app);
    install_canceled(&setup, &legendary, &plan, app).unwrap_err();
    let resume = crate::epic::state_dir(&setup.paths).join("tmp/AppOffline.resume");
    assert!(resume.exists());
    forget(&setup.paths, None, app, &Token::new()).unwrap();
    assert!(record(&setup, app).is_none());
    assert!(!resume.exists());
    assert!(plan.folder().exists(), "the folder stays");
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn local_states_come_from_the_records() {
    let (setup, legendary) = host();
    let plan = plan_for(&legendary, "Dlc Game");
    install(&legendary, &plan, &Token::new()).0.unwrap();
    let lock = RecordLock::exclusive(&setup.paths, "AppOffline", NEVER).unwrap();
    let mut unfinished = Record::new("AppOffline", games(&setup).join("Offline Game"));
    lock.mark(&setup.paths, &mut unfinished, Work::Install, "AppOffline")
        .unwrap();
    drop(lock);
    let lock = RecordLock::exclusive(&setup.paths, "AppToken", NEVER).unwrap();
    lock.write(
        &setup.paths,
        &Record::new("AppToken", games(&setup).join("Token Game 2020")),
    )
    .unwrap();
    drop(lock);
    let (states, errors) = local(&setup.paths).unwrap();
    assert!(errors.is_empty());
    let dlc = &states["Dlc Game"];
    assert_eq!(dlc.state, LocalState::Installed);
    assert_eq!(dlc.environment.as_deref(), Some("dlc-game"));
    assert_eq!(dlc.version.as_deref(), Some("1.0"));
    assert_eq!(dlc.dlc.len(), 2);
    assert_eq!(states["AppOffline"].state, LocalState::Unfinished);
    assert_eq!(states["AppToken"].state, LocalState::Downloaded);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn removing_an_environment_mid_work_is_refused_unless_forced() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let plan = plan_for(&legendary, app);
    install(&legendary, &plan, &Token::new()).0.unwrap();
    let name = plan.environment().to_owned();
    let lock = RecordLock::exclusive(&setup.paths, app, NEVER).unwrap();
    let mut record = lock.read(&setup.paths).unwrap().unwrap();
    lock.mark(&setup.paths, &mut record, Work::Install, app)
        .unwrap();
    drop(lock);
    let error = crate::trash::remove(&setup.paths, &name, false).unwrap_err();
    assert_eq!(
        error.to_string(),
        "offline-game's Epic install didn't finish. Finish it with wineo import epic --game AppOffline, or remove it anyway with --force."
    );
    let lock = RecordLock::exclusive(&setup.paths, app, NEVER).unwrap();
    lock.mark(&setup.paths, &mut record, Work::Update, app)
        .unwrap();
    drop(lock);
    let error = crate::trash::remove(&setup.paths, &name, false).unwrap_err();
    assert_eq!(
        error.to_string(),
        "offline-game is mid-update. Finish it with wineo update offline-game, or remove it anyway with --force."
    );
    crate::trash::remove_with_force(&setup.paths, &name, false, true, &Token::new(), |_| {})
        .unwrap();
    assert!(!setup.paths.environment_config(&name).exists());
    assert!(
        game::peek(&setup.paths, app)
            .unwrap()
            .unwrap()
            .in_flight
            .is_some(),
        "the mark stays for installing again"
    );
    assert_eq!(
        crate::trash::epic_owner(&setup.paths, app).unwrap(),
        Some(name)
    );
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn another_install_of_the_same_game_started_meanwhile_is_refused_by_the_record() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    // Another wineo wrote its record after this one looked for one.
    let folder = games(&setup).join("Offline Game");
    let lock = RecordLock::exclusive(&setup.paths, app, NEVER).unwrap();
    let mut theirs = Record::new(app, folder.clone());
    lock.mark(&setup.paths, &mut theirs, Work::Install, app)
        .unwrap();
    drop(lock);
    let plan = plan(
        &legendary,
        &request(app),
        &owned(app),
        &Existing::None,
        &Token::new(),
    )
    .unwrap();
    assert_eq!(plan.folder(), folder, "not an overlap with itself");
    let (installed, _) = install(&legendary, &plan, &Token::new());
    assert_eq!(
        installed.unwrap_err().to_string(),
        "Epic game AppOffline's record changed since the install was planned; plan it again."
    );
    assert!(installs(&setup).is_empty());
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn an_activation_only_info_names_is_refused_before_any_download() {
    let (setup, legendary) = host();
    // The owned list doesn't mark it; Epic's own answer about the game does.
    let mut unlinked = owned("AppThirdParty");
    unlinked.partner_link = None;
    let error = plan(
        &legendary,
        &request("AppThirdParty"),
        &unlinked,
        &Existing::None,
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Third Party Game can't be installed: it needs an account at another launcher."
    );
    assert!(
        setup
            .calls()
            .contains(&"info AppThirdParty --json".to_owned())
    );
    assert!(installs(&setup).is_empty());
    assert!(record(&setup, "AppThirdParty").is_none());
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn forgetting_waits_for_the_epic_lock_before_legendary_runs() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let plan = plan_for(&legendary, app);
    install(&legendary, &plan, &Token::new()).0.unwrap();
    fs::remove_file(setup.paths.environment_config(plan.environment())).unwrap();
    let held = EpicLock::acquire(&setup.paths).unwrap();
    let uninstalls = || {
        setup
            .calls()
            .iter()
            .filter(|call| call.contains("uninstall"))
            .count()
    };
    thread::scope(|scope| {
        let forgetting = scope.spawn(|| forget(&setup.paths, Some(&legendary), app, &Token::new()));
        thread::sleep(Duration::from_millis(500));
        assert_eq!(uninstalls(), 0, "Legendary ran while the lock was held");
        assert!(!forgetting.is_finished());
        drop(held);
        forgetting.join().unwrap().unwrap();
    });
    assert_eq!(uninstalls(), 1);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn a_name_that_cant_be_created_fails_before_the_download() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    let plan = plan_for(&legendary, app);
    // A snapshot restore of the name was interrupted after the review.
    let marker = setup.paths.restore_marker(plan.environment());
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "{}").unwrap();
    let (installed, _) = install(&legendary, &plan, &Token::new());
    let error = format!("{:#}", installed.unwrap_err());
    assert!(
        error.contains("an interrupted snapshot restore needs recovery"),
        "{error}"
    );
    assert!(installs(&setup).is_empty(), "refused before downloading");
    assert!(record(&setup, app).is_none(), "and before the mark");
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}

#[test]
fn programs_outside_the_game_folder_are_refused() {
    for (raw, want) in [
        ("Binaries/Win64/Game.exe", Some("Binaries/Win64/Game.exe")),
        ("\\Binaries\\Game.exe", Some("Binaries/Game.exe")),
        ("./Game.exe", Some("Game.exe")),
        ("/Game.exe", Some("Game.exe")),
        ("../Game.exe", None),
        ("Binaries/../../Game.exe", None),
        ("Binaries\\..\\..\\Game.exe", None),
        ("Binaries/../Game.exe", None),
        ("", None),
        (".", None),
    ] {
        let entry = Entry {
            executable: Some(raw.to_owned()),
            ..Entry::default()
        };
        assert_eq!(
            entry.offline_start().map(|start| start.executable),
            want.map(str::to_owned),
            "{raw}"
        );
    }
}

#[test]
fn an_install_whose_program_is_outside_its_folder_gets_no_target() {
    let (setup, legendary) = host();
    let app = "AppOffline";
    set_mode(&setup, app, "outside");
    let plan = plan_for(&legendary, app);
    let (installed, _) = install(&legendary, &plan, &Token::new());
    let installed = installed.unwrap();
    assert_eq!(
        installed.warnings,
        [format!(
            "Legendary names no program inside {} to start, so offline-game has none yet. Pick one with: wineo setup offline-game",
            plan.folder().display()
        )]
    );
    let environment = Environment::load(&setup.paths, "offline-game").unwrap();
    assert!(environment.config.targets.is_empty());
    let record = record(&setup, app).unwrap();
    assert_eq!(record.offline, None);
    assert_eq!(record.in_flight, None);
    assert_eq!(setup.sweep(), Vec::<i32>::new());
}
