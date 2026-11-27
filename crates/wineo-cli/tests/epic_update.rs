//! `wineo update` for Epic games against the fake Legendary: checking both
//! stores together, updating with DLC bought since or failed before,
//! repairing, a missing record, cancel and resume, a running game, a new
//! main program, and the GOG flags an Epic game refuses.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Output, Stdio};

use common::epic::{Epic, Fake};
use common::gog_api::{CODE as GOG_CODE, GogApi, installer, owned_game, product, success_address};
use common::gog_world::{RIMWORLD, RIMWORLD_DLC};
use common::pty::Pty;
use common::{KillOnDrop, stderr, stdout};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use serde_json::{Value, json};
use wineo_core::epic::game::{self, Record, Work};

const APP: &str = "AppOffline";
const DLC_GAME: &str = "Dlc Game";
const DLC_ONE: &str = "0000000000000000000000000000000c";
const DLC_TWO: &str = "0000000000000000000000000000000e";
const DLC_NEW: &str = "0000000000000000000000000000000f";
const BUILD: &str = "++Dlc Game+Release-43.11-CL-60803671-Windows";

/// Logged in, with the fake on `PATH`.
fn epic() -> Epic {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    epic
}

#[track_caller]
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

#[track_caller]
fn assert_code(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

/// What the fake records for `app`'s next install or update.
fn set_version(epic: &Epic, app: &str, version: &str) {
    fs::write(epic.file(&format!("version-{app}")), version).unwrap();
}

/// Offline Game installed as `offline-game`, at the version Epic lists.
fn offline_game(epic: &Epic) {
    set_version(epic, APP, "0.24");
    assert_ok(&epic.install(&["import", "epic", "--game", APP, "--yes"]));
}

/// The owned list with `edit` applied, and the saved copy dropped so the
/// next command fetches it.
fn list_with(epic: &Epic, edit: impl FnOnce(&mut Value)) {
    let list = epic.edited_list(edit);
    epic.on_path(&Fake {
        list,
        ..Fake::default()
    });
    let _ = fs::remove_file(epic.cache());
}

fn listed<'a>(list: &'a mut Value, app: &str) -> &'a mut Value {
    list.as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|game| game["app_name"] == app)
        .unwrap()
}

/// Epic lists `version` for Offline Game now.
fn offer(epic: &Epic, version: &str) {
    list_with(epic, |list| {
        listed(list, APP)["asset_infos"]["Windows"]["build_version"] = json!(version);
    });
}

fn record(epic: &Epic, app: &str) -> Option<Record> {
    game::peek(&epic.f.paths(), app).unwrap()
}

fn mark(epic: &Epic, app: &str) -> Option<Work> {
    record(epic, app)
        .unwrap()
        .in_flight
        .map(|in_flight| in_flight.work)
}

fn config(epic: &Epic, name: &str) -> String {
    fs::read_to_string(epic.f.config_dir().join(format!("environments/{name}.toml"))).unwrap()
}

fn folder(epic: &Epic) -> PathBuf {
    epic.games().join("Offline Game")
}

fn check_json(epic: &Epic) -> Value {
    let output = epic.run(&["update", "--check", "--json"]);
    serde_json::from_str(&stdout(&output)).unwrap()
}

fn row<'a>(json: &'a Value, name: &str) -> &'a Value {
    json["environments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("no row for {name}: {json}"))
}

fn update_call(epic: &Epic, app: &str, folder: &str, extra: &str) -> String {
    format!(
        "-y install {app} --base-path {} --game-folder {folder} --skip-dlcs --disable-sdl{extra}",
        epic.games().display()
    )
}

/// `wineo update ARGS` started in the background, for a cancel.
fn spawn(epic: &Epic, args: &[&str]) -> KillOnDrop {
    KillOnDrop::new(
        epic.install_command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

fn interrupt(child: &KillOnDrop) {
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    let pidfd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
    pidfd_send_signal(&pidfd, Signal::INT).unwrap();
}

#[test]
fn an_update_downloads_the_game_and_refreshes_its_record_and_main_program() {
    let epic = epic();
    offline_game(&epic);
    let output = epic.run(&["update", "--check", "offline-game"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "NAME          INSTALLED  AVAILABLE  STATE\n\
         offline-game  0.24       0.24       up to date\n"
    );
    let output = epic.run(&["update", "offline-game"]);
    assert_ok(&output);
    assert_eq!(stdout(&output), "Up to date.\n");

    offer(&epic, "0.25");
    set_version(&epic, APP, "0.25");
    fs::write(epic.file("exe-AppOffline"), "Bin/New.exe").unwrap();
    let json = check_json(&epic);
    assert_eq!(
        json,
        json!({
            "environments": [{
                "name": "offline-game", "id": APP, "store": "epic", "track": null,
                "installed": "0.24", "available": "0.25", "branch": null, "pinned": null,
                "update": "available", "applicable": true, "reason": null,
                "unfinished": false, "dlc": [],
            }],
            "errors": [],
            "warnings": [],
        })
    );
    let output = epic.run(&["update", "--check", "offline-game", "--json"]);
    assert_ok(&output);
    let one: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(one["environments"], json["environments"]);

    let output = epic.install(&["update", "offline-game", "--yes"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "Update offline-game\n\
         \x20 folder        ~/Games/Offline Game\n\
         \x20 Offline Game  0.24 -> 0.25\n\
         Updated offline-game.\n\
         \x20 Offline Game  0.24 -> 0.25\n\
         offline-game's main program is now ~/Games/Offline Game/Bin/New.exe.\n"
    );
    assert!(
        stderr(&output).contains("Downloading: Offline Game\nDownloading: 50%\nDownloading: 100%\n"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        epic.install_calls().last().unwrap(),
        &update_call(&epic, APP, "Offline Game", " --update-only")
    );
    let after = record(&epic, APP).unwrap();
    assert_eq!(after.installed_version.as_deref(), Some("0.25"));
    assert_eq!(after.in_flight, None);
    assert_eq!(after.offline.unwrap().executable, "Bin/New.exe");
    let text = config(&epic, "offline-game");
    let folder = folder(&epic);
    assert!(
        text.contains(&format!("exe = \"{}\"", folder.join("Bin/New.exe").display())),
        "{text}"
    );
    // wineo wrote the old working folder, so it follows the program.
    assert!(
        text.contains(&format!("cwd = \"{}\"", folder.join("Bin").display())),
        "{text}"
    );

    // The way past the guard is the update's alone.
    let output = epic.run(&["set", "offline-game", "targets.game.exe=/elsewhere/Other.exe"]);
    assert_code(&output, 1);
    assert!(
        stderr(&output)
            .contains("offline-game's main program comes from Epic and can't be changed."),
        "{}",
        stderr(&output)
    );
    assert_eq!(config(&epic, "offline-game"), text);
    assert_eq!(row(&check_json(&epic), "offline-game")["update"], "none");
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

/// Dlc Game installed as `dlc-game` at the build Epic lists, with its
/// second DLC failing to install.
fn dlc_game_missing_one(epic: &Epic) {
    for app in [DLC_GAME, DLC_ONE, DLC_TWO, DLC_NEW] {
        set_version(epic, app, BUILD);
    }
    epic.set_mode(DLC_TWO, "unrecorded");
    let output = epic.install(&["import", "epic", "--game", DLC_GAME, "--yes"]);
    assert_ok(&output);
    assert!(stderr(&output).contains("warning: Couldn't install Extra Two Dlc Game Content"));
}

#[test]
fn a_dlc_that_failed_and_one_bought_since_install_with_the_next_update() {
    let epic = epic();
    dlc_game_missing_one(&epic);
    let json = check_json(&epic);
    let dlc_game = row(&json, "dlc-game");
    assert_eq!(dlc_game["update"], "available");
    assert_eq!(dlc_game["installed"], BUILD);
    assert_eq!(dlc_game["available"], BUILD);
    assert_eq!(
        dlc_game["dlc"],
        json!([
            {"id": DLC_ONE, "title": "Dlc Game Extra One Content", "state": "none"},
            {"id": DLC_TWO, "title": "Extra Two Dlc Game Content", "state": "not_installed"},
        ])
    );
    let table = stdout(&epic.run(&["update", "--check"]));
    assert!(
        table.ends_with("  DLC Extra Two Dlc Game Content: not installed\n"),
        "{table}"
    );

    // It fails again: a warning, and the game stays as it was.
    let calls = epic.install_calls().len();
    let output = epic.install(&["update", "dlc-game", "--yes"]);
    assert_ok(&output);
    assert!(
        stdout(&output).contains("  Extra Two Dlc Game Content  not installed -> ++Dlc Game"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains(
            "warning: Couldn't install Extra Two Dlc Game Content: Legendary didn't install Extra Two Dlc Game Content: it isn't in Legendary's records afterwards. The game was updated without it."
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(epic.install_calls().len(), calls + 1);
    assert_eq!(record(&epic, DLC_GAME).unwrap().dlc.len(), 1);
    assert_eq!(mark(&epic, DLC_GAME), None);

    // Fixed, and another DLC bought meanwhile: both install.
    epic.set_mode(DLC_TWO, "");
    list_with(&epic, |list| {
        let dlcs = listed(list, DLC_GAME)["dlcs"].as_array_mut().unwrap();
        let mut bought = dlcs[0].clone();
        bought["app_name"] = json!(DLC_NEW);
        bought["app_title"] = json!("Bought Since Content");
        dlcs.push(bought);
    });
    assert_eq!(
        row(&check_json(&epic), "dlc-game")["dlc"][2],
        json!({"id": DLC_NEW, "title": "Bought Since Content", "state": "not_installed"})
    );
    let output = epic.install(&["update", "dlc-game", "--yes"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        format!(
            "Update dlc-game\n\
             \x20 folder                      ~/Games/Dlc Game\n\
             \x20 Extra Two Dlc Game Content  not installed -> {BUILD}\n\
             \x20 Bought Since Content        not installed -> {BUILD}\n\
             Updated dlc-game.\n\
             \x20 Extra Two Dlc Game Content  not installed -> {BUILD}\n\
             \x20 Bought Since Content        not installed -> {BUILD}\n"
        )
    );
    let err = stderr(&output);
    assert!(
        err.contains("Downloading 1 of 2: Extra Two Dlc Game Content\n")
            && err.contains("Downloading 2 of 2: Bought Since Content\n"),
        "{err}"
    );
    let calls = epic.install_calls();
    assert_eq!(
        calls[calls.len() - 2..],
        [
            update_call(&epic, DLC_TWO, "Dlc Game", ""),
            update_call(&epic, DLC_NEW, "Dlc Game", ""),
        ]
    );
    let dlc: Vec<String> = record(&epic, DLC_GAME)
        .unwrap()
        .dlc
        .into_iter()
        .map(|dlc| dlc.app_name)
        .collect();
    assert_eq!(dlc, [DLC_ONE, DLC_TWO, DLC_NEW]);
    let json = check_json(&epic);
    assert_eq!(row(&json, "dlc-game")["update"], "none");
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

fn gog_log_in(epic: &Epic, api: &GogApi) {
    let mut command = epic.command();
    command
        .env("WINEO_TEST_GOG_API", &api.base)
        .args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(GOG_CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

/// RimWorld from GOG beside Epic's Offline Game. `latest` is what GOG
/// offers for RimWorld and both its DLC.
fn both_stores(epic: &Epic, latest: &str) -> GogApi {
    let f = &epic.f;
    f.heroic_gog();
    let rimworld = f.heroic_rimworld();
    f.gog_environment(
        "rimworld",
        &rimworld,
        RIMWORLD,
        "1.6.4871 rev588",
        &[
            (RIMWORLD_DLC[0], Some("1.6.4871 rev588")),
            (RIMWORLD_DLC[1], Some("1.6.4871 rev588")),
        ],
    );
    GogApi::with_library(
        vec![owned_game(RIMWORLD, "RimWorld", true, false)],
        vec![product(
            RIMWORLD,
            "RimWorld",
            &[installer("linux", "en", latest)],
            &[
                product(
                    RIMWORLD_DLC[0],
                    "Royalty",
                    &[installer("linux", "en", latest)],
                    &[],
                ),
                product(
                    RIMWORLD_DLC[1],
                    "Ideology",
                    &[installer("linux", "en", latest)],
                    &[],
                ),
            ],
        )],
    )
}

fn both(epic: &Epic, api: &GogApi, args: &[&str]) -> Output {
    epic.install_command()
        .env("WINEO_TEST_GOG_API", &api.base)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn check_lists_both_stores_with_gogs_rows_as_before() {
    let epic = epic();
    offline_game(&epic);
    offer(&epic, "0.25");
    let api = both_stores(&epic, "1.6.4900 rev600");
    gog_log_in(&epic, &api);

    let output = both(&epic, &api, &["update", "--check"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "NAME          INSTALLED        AVAILABLE        STATE\n\
         offline-game  0.24             0.25             update available\n\
         rimworld      1.6.4871 rev588  1.6.4900 rev600  update available\n\
         \x20 DLC Royalty (1233017772): 1.6.4871 rev588 -> 1.6.4900 rev600\n\
         \x20 DLC Ideology (1827857764): 1.6.4871 rev588 -> 1.6.4900 rev600\n"
    );
    let output = both(&epic, &api, &["update", "--check", "--json"]);
    assert_ok(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        json["environments"],
        json!([
            {
                "name": "offline-game", "id": APP, "store": "epic", "track": null,
                "installed": "0.24", "available": "0.25", "branch": null, "pinned": null,
                "update": "available", "applicable": true, "reason": null,
                "unfinished": false, "dlc": [],
            },
            {
                "name": "rimworld", "id": RIMWORLD.to_string(), "track": "installer",
                "installed": "1.6.4871 rev588", "available": "1.6.4900 rev600",
                "branch": null, "pinned": null, "update": "available", "applicable": true, "reason": null,
                "unfinished": false,
                "dlc": [
                    {"id": RIMWORLD_DLC[0].to_string(), "title": "Royalty", "state": "available"},
                    {"id": RIMWORLD_DLC[1].to_string(), "title": "Ideology", "state": "available"},
                ],
            },
        ])
    );
    assert_eq!(json["errors"], json!([]));

    // Logged out of Epic: its game is an error, and GOG's is still checked.
    fs::remove_file(epic.state().join("user.json")).unwrap();
    let output = both(&epic, &api, &["update", "--check", "--json"]);
    assert_code(&output, 1);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["environments"][0]["name"], "rimworld");
    assert_eq!(
        json["errors"],
        json!([{
            "name": "offline-game", "id": APP,
            "message": "Not logged in to Epic. Log in with wineo import epic --login.",
        }])
    );

    // Logged out of GOG instead: the other way round.
    epic.log_in();
    assert_ok(&both(&epic, &api, &["import", "gog", "--logout"]));
    let output = both(&epic, &api, &["update", "--check"]);
    assert_code(&output, 1);
    assert_eq!(
        stdout(&output),
        "NAME          INSTALLED  AVAILABLE  STATE\n\
         offline-game  0.24       0.25       update available\n"
    );
    assert_eq!(
        stderr(&output),
        "Failed rimworld: Log in to GOG first: wineo import gog --login.\n"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn all_updates_epic_games_beside_gogs_and_reports_a_logged_out_store() {
    let epic = epic();
    offline_game(&epic);
    offer(&epic, "0.25");
    set_version(&epic, APP, "0.25");
    let api = both_stores(&epic, "1.6.4871 rev588");
    gog_log_in(&epic, &api);
    // A dry run uses the saved list.
    assert_ok(&both(&epic, &api, &["update", "--check"]));

    let output = both(&epic, &api, &["update", "--all", "--dry-run"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "Update offline-game\n\
         \x20 folder        ~/Games/Offline Game\n\
         \x20 Offline Game  0.24 -> 0.25\n\
         Dry run, nothing was updated.\n"
    );
    let calls = epic.install_calls().len();
    assert_eq!(epic.install_calls().len(), calls);

    let output = both(&epic, &api, &["update", "--all", "--yes"]);
    assert_ok(&output);
    let out = stdout(&output);
    assert!(
        out.ends_with("Updating offline-game\nUpdated offline-game.\n  Offline Game  0.24 -> 0.25\nUpdated 1 of 1 game.\n"),
        "{out}"
    );
    assert_eq!(
        record(&epic, APP).unwrap().installed_version.as_deref(),
        Some("0.25")
    );

    // Logged out of GOG, its game fails and Epic's still updates.
    offer(&epic, "0.26");
    set_version(&epic, APP, "0.26");
    assert_ok(&both(&epic, &api, &["import", "gog", "--logout"]));
    let output = both(&epic, &api, &["update", "--all", "--yes"]);
    assert_code(&output, 1);
    assert!(
        stdout(&output).contains("Updated offline-game.\n  Offline Game  0.25 -> 0.26\n"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("Failed rimworld: Log in to GOG first: wineo import gog --login.\n"),
        "{}",
        stderr(&output)
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn gogs_flags_on_an_epic_game_are_refused() {
    let epic = epic();
    let dir = epic.f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("fowl.toml"),
        "epic_id = \"AppOffline\"\nrunner = \"wine\"\nprefix = \"/p\"\n",
    )
    .unwrap();
    for (args, flag) in [
        (&["--reinstall"][..], "--reinstall"),
        (&["--reinstall", "--track", "galaxy"], "--reinstall"),
        (&["--branch", "beta"], "--branch"),
        (&["--branch", "beta", "--password-stdin"], "--branch"),
        (&["--default-branch"], "--default-branch"),
        (&["--build", "1"], "--build"),
        (&["--latest"], "--latest"),
        (&["--builds"], "--builds"),
    ] {
        let mut command = vec!["update", "fowl"];
        command.extend_from_slice(args);
        let output = epic.run(&command);
        assert_code(&output, 1);
        assert_eq!(
            stderr(&output),
            format!("error: {flag} is for GOG games.\n"),
            "{args:?}"
        );
    }
    assert!(epic.calls().is_empty(), "{:?}", epic.calls());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_repair_checks_the_game_and_a_failed_one_is_finished_later() {
    let epic = epic();
    offline_game(&epic);
    epic.set_mode(APP, "damaged");
    let output = epic.install(&["update", "offline-game", "--repair", "--yes"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "Repair offline-game\n\
         \x20 folder        ~/Games/Offline Game\n\
         \x20 Offline Game  0.24\n\
         \x20 check         every file\n\
         \x20 download      only what's missing or damaged\n\
         Repaired offline-game.\n\
         \x20 Offline Game  0.24\n"
    );
    let err = stderr(&output);
    assert!(err.contains("Repairing: Offline Game\n"), "{err}");
    assert!(!err.contains('%'), "a repair shows no percentage: {err}");
    assert_eq!(
        epic.install_calls().last().unwrap(),
        &update_call(&epic, APP, "Offline Game", " --repair")
    );
    assert_eq!(fs::read_to_string(epic.file("repaired")).unwrap(), "AppOffline\n");
    assert_eq!(mark(&epic, APP), None);

    for (mode, why) in [
        (
            "failed-write",
            "it logged an error (Writing file failed: [Errno 28] No space left on device)",
        ),
        ("kept", "it stopped before fetching what its check found"),
    ] {
        epic.set_mode(APP, mode);
        let output = epic.install(&["update", "offline-game", "--repair", "--yes"]);
        assert_code(&output, 1);
        assert_eq!(
            stderr(&output).lines().last().unwrap(),
            format!(
                "error: Legendary didn't repair Offline Game: {why}. Finish it with: wineo update offline-game --repair"
            )
        );
        assert_eq!(mark(&epic, APP), Some(Work::Repair));
        let _ = fs::remove_file(epic.state().join("tmp/AppOffline.repair"));
    }
    let start = epic.install(&["start", "offline-game"]);
    assert_code(&start, 1);
    assert!(
        stderr(&start).contains(
            "offline-game's game folder is mid-repair. Finish it with wineo update offline-game --repair."
        ),
        "{}",
        stderr(&start)
    );

    // Plain update finishes a repair as a repair.
    epic.set_mode(APP, "");
    let output = epic.install(&["update", "offline-game", "--yes"]);
    assert_ok(&output);
    let out = stdout(&output);
    assert!(
        out.starts_with("Finish the interrupted repair of offline-game\n"),
        "{out}"
    );
    assert!(
        out.contains("Finished the interrupted repair of offline-game.\n"),
        "{out}"
    );
    assert_eq!(mark(&epic, APP), None);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_repair_rebuilds_a_missing_record_from_legendarys() {
    let epic = epic();
    offline_game(&epic);
    let before = record(&epic, APP).unwrap();
    let path = game::record_path(&epic.f.paths(), APP);
    fs::remove_file(&path).unwrap();

    let output = epic.install(&["update", "offline-game", "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: offline-game's Epic record is missing. Repair it with wineo update offline-game --repair.\n"
    );
    assert!(!epic.calls().iter().any(|call| call.contains("--update-only")));

    let output = epic.install(&["update", "offline-game", "--repair", "--yes"]);
    assert_ok(&output);
    assert!(
        stdout(&output).starts_with(
            "Repair offline-game\n\
             \x20 folder        ~/Games/Offline Game\n\
             \x20 record        missing, rebuilt from Legendary's\n"
        ),
        "{}",
        stdout(&output)
    );
    assert_eq!(record(&epic, APP).unwrap(), before);

    // Legendary has lost it too.
    fs::remove_file(&path).unwrap();
    fs::remove_dir_all(epic.state().join("fake-installed")).unwrap();
    fs::write(epic.state().join("installed.json"), "{}").unwrap();
    let output = epic.install(&["update", "offline-game", "--repair", "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: Legendary has no record of offline-game's Epic game AppOffline either, so its record can't be rebuilt.\n"
    );
    assert!(!path.exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn ctrl_c_interrupts_an_update_keeps_the_mark_and_the_same_command_finishes_it() {
    let epic = epic();
    offline_game(&epic);
    offer(&epic, "0.25");
    set_version(&epic, APP, "0.25");
    epic.set_mode(APP, "hang");
    fs::remove_file(epic.file("installing-AppOffline")).unwrap();
    let child = spawn(&epic, &["update", "offline-game", "--yes"]);
    epic.wait_started(APP);
    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_code(&output, 130);
    assert!(
        stderr(&output).ends_with(
            "Update interrupted.\nFinish the interrupted update with: wineo update offline-game\n"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fs::read_to_string(epic.file("got-int")).unwrap(),
        "AppOffline\n"
    );
    assert_eq!(mark(&epic, APP), Some(Work::Update));
    assert_eq!(epic.sweep(), Vec::<i32>::new());

    let json = check_json(&epic);
    assert_eq!(row(&json, "offline-game")["unfinished"], true);
    let table = stdout(&epic.run(&["update", "--check"]));
    assert!(
        table.ends_with("  Finish the interrupted update with: wineo update offline-game\n"),
        "{table}"
    );
    let start = epic.install(&["start", "offline-game"]);
    assert_code(&start, 1);
    assert!(
        stderr(&start).contains(
            "offline-game's game folder is mid-update. Finish it with wineo update offline-game."
        ),
        "{}",
        stderr(&start)
    );
    // A repair waits for the update.
    let output = epic.install(&["update", "offline-game", "--repair", "--yes"]);
    assert_code(&output, 1);
    assert!(
        stderr(&output).contains("Finish it with wineo update offline-game."),
        "{}",
        stderr(&output)
    );

    epic.set_mode(APP, "");
    let output = epic.install(&["update", "offline-game", "--yes"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "Finish the interrupted update of offline-game\n\
         \x20 folder        ~/Games/Offline Game\n\
         \x20 Offline Game  0.24 -> 0.25\n\
         Finished the interrupted update of offline-game.\n\
         \x20 Offline Game  0.24 -> 0.25\n"
    );
    let calls = epic.install_calls();
    assert_eq!(calls[calls.len() - 1], calls[calls.len() - 2], "the same run");
    assert_eq!(mark(&epic, APP), None);
    assert_eq!(
        record(&epic, APP).unwrap().installed_version.as_deref(),
        Some("0.25")
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn ctrl_c_interrupts_a_repair_and_repair_finishes_it() {
    let epic = epic();
    offline_game(&epic);
    epic.set_mode(APP, "hang");
    fs::remove_file(epic.file("installing-AppOffline")).unwrap();
    let child = spawn(&epic, &["update", "offline-game", "--repair", "--yes"]);
    epic.wait_started(APP);
    // Past the check, so the repair file is written.
    common::wait_for("the repair file", || {
        epic.state().join("tmp/AppOffline.repair").exists()
    });
    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_code(&output, 130);
    assert!(
        stderr(&output).ends_with(
            "Repair interrupted.\nFinish the interrupted repair with: wineo update offline-game --repair\n"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(mark(&epic, APP), Some(Work::Repair));
    assert!(epic.state().join("tmp/AppOffline.repair").exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());

    epic.set_mode(APP, "");
    let output = epic.install(&["update", "offline-game", "--repair", "--yes"]);
    assert_ok(&output);
    assert!(
        stdout(&output).contains("Finished the interrupted repair of offline-game.\n"),
        "{}",
        stdout(&output)
    );
    assert!(!epic.state().join("tmp/AppOffline.repair").exists());
    assert_eq!(mark(&epic, APP), None);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_running_game_refuses_an_update_and_a_repair() {
    let epic = epic();
    offline_game(&epic);
    offer(&epic, "0.25");
    set_version(&epic, APP, "0.25");
    let folder = fs::canonicalize(folder(&epic)).unwrap();
    let lock = game::in_use_lock(&epic.f.paths(), &folder).unwrap();
    let running = lock.try_shared("offline-game").unwrap().unwrap();
    let calls = epic.install_calls().len();
    for (args, doing) in [
        (&["update", "offline-game", "--yes"][..], "updating"),
        (&["update", "offline-game", "--repair", "--yes"], "repairing"),
    ] {
        let output = epic.install(args);
        assert_code(&output, 1);
        assert_eq!(
            stderr(&output),
            format!(
                "error: offline-game is running from this game folder. Stop it before {doing} offline-game.\n"
            )
        );
    }
    assert_eq!(epic.install_calls().len(), calls);
    assert_eq!(mark(&epic, APP), None);
    drop(running);
    assert_ok(&epic.install(&["update", "offline-game", "--yes"]));
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn an_unfinished_install_is_finished_by_installing_again() {
    let epic = epic();
    offline_game(&epic);
    let paths = epic.f.paths();
    let lock = game::RecordLock::exclusive(&paths, APP, &|| false).unwrap();
    let mut record = lock.read(&paths).unwrap().unwrap();
    lock.mark(&paths, &mut record, Work::Install, APP).unwrap();
    drop(lock);
    let output = epic.install(&["update", "offline-game", "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: offline-game's Epic install didn't finish. Finish it with wineo import epic --game AppOffline.\n"
    );
    offer(&epic, "0.25");
    let output = epic.install(&["update", "--all", "--yes"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "Skipped offline-game: Finish the interrupted install with: wineo import epic --game AppOffline\nNothing to update.\n"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}
