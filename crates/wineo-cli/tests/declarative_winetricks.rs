mod common;

use std::fs;

use common::*;

const APP: &str = "winetricks = ['win10', 'corefonts']\n[targets.app]\nexe = 'exit-0'\n";
const EXIT: &str = "winetricks = ['win10', 'corefonts']\n[targets.app]\nexe = 'exit-0'\n";

fn fake_winetricks(f: &Fixture) {
    let script = f.root.path().join("bin/winetricks");
    write_script(
        &script,
        "#!/bin/sh\nprintf 'winetricks: %s FAKE_VAR=%s\\n' \"$*\" \"${FAKE_VAR-}\"\nprintf '%s\\n' \"$*\" >>\"$WINEPREFIX/winetricks-calls\"\n[ \"$1\" = -q ] || exit 0\nshift\nfor verb in \"$@\"; do\n    [ \"$verb\" = \"${WINEO_TEST_FAIL_VERB-}\" ] && exit 1\n    printf '%s\\n' \"$verb\" >>\"$WINEPREFIX/winetricks.log\"\ndone\n",
    );
}

#[test]
fn wine_installs_once_before_detaching_and_reports_status() {
    let f = Fixture::new();
    f.environment("app", APP);
    fake_winetricks(&f);
    let prefix = f.root.path().join("prefixes/app");
    fs::write(prefix.join("winetricks.log"), "win10\n").unwrap();

    let status = f.run(&["status", "app"]);
    assert_success(&status);
    assert!(stdout(&status).contains("winetricks  win10, corefonts (missing)"));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&f.run(&["status", "app", "--json"]))).unwrap();
    assert_eq!(
        json["winetricks"],
        serde_json::json!(["win10", "corefonts"])
    );
    assert_eq!(json["winetricks_missing"], serde_json::json!(["corefonts"]));

    let started = f.run(&["start", "app", "-e", "FAKE_VAR=hello"]);
    assert_success(&started);
    assert!(stderr(&started).contains("Installing corefonts with winetricks"));
    assert!(stdout(&started).contains("winetricks: -q corefonts FAKE_VAR=hello"));
    assert!(stdout(&started).contains("Started app"));
    assert_eq!(
        fs::read_to_string(prefix.join("winetricks-calls")).unwrap(),
        "-q corefonts\n"
    );

    wait_for("app to exit", || {
        f.state("app").contains("status = \"exited\"")
    });
    let again = f.run(&["start", "app"]);
    assert_success(&again);
    assert!(!stdout(&again).contains("winetricks:"));
    wait_for("app to exit again", || {
        f.state("app").contains("status = \"exited\"")
    });
    let launched = f.run(&["start", "app"]);
    assert_success(&launched);
    assert_eq!(
        fs::read_to_string(prefix.join("winetricks-calls")).unwrap(),
        "-q corefonts\n"
    );
    wait_for("app to exit a third time", || {
        f.state("app").contains("status = \"exited\"")
            && !f.runtime("app").is_running().unwrap_or(true)
    });
}

#[test]
fn preinstalled_verbs_need_no_winetricks_program() {
    let f = Fixture::new();
    f.environment("app", EXIT);
    fs::write(
        f.root.path().join("prefixes/app/winetricks.log"),
        "win10\ncorefonts\n",
    )
    .unwrap();
    let ran = f.run_sandboxed(&["run", "app"]);
    assert_success(&ran);
    assert!(!stdout(&ran).contains("winetricks"));
}

#[test]
fn wine_failure_stops_launch_and_retries_remaining_verbs() {
    let f = Fixture::new();
    f.environment("app", EXIT);
    fake_winetricks(&f);
    let failed = f
        .wineo()
        .env("WINEO_TEST_FAIL_VERB", "corefonts")
        .args(["run", "app"])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(stderr(&failed).contains("winetricks failed installing corefonts for app"));
    assert!(!stdout(&failed).contains("fake wine: exit-0"));
    let prefix = f.root.path().join("prefixes/app");
    assert_eq!(
        fs::read_to_string(prefix.join("winetricks.log")).unwrap(),
        "win10\n"
    );

    let retried = f.run(&["start", "app"]);
    assert_success(&retried);
    assert!(stdout(&retried).contains("winetricks: -q corefonts"));
    assert_eq!(
        fs::read_to_string(prefix.join("winetricks-calls")).unwrap(),
        "-q win10 corefonts\n-q corefonts\n"
    );
    wait_for("app to exit", || {
        f.state("app").contains("status = \"exited\"")
    });
}

#[test]
fn proton_uses_umu_and_its_pfx_log() {
    let f = Fixture::new();
    f.install_proton("proton-test");
    f.proton_environment("app", "proton-test", EXIT);
    let first = f.run(&["start", "app", "-e", "FAKE_VAR=proton"]);
    assert_success(&first);
    assert!(stdout(&first).contains("umu-run: winetricks -q win10 corefonts"));
    assert!(stdout(&first).contains("umu-run: FAKE_VAR=proton"));
    let log = f.root.path().join("prefixes/app/pfx/winetricks.log");
    assert_eq!(fs::read_to_string(log).unwrap(), "win10\ncorefonts\n");
    wait_for("app to exit", || {
        f.state("app").contains("status = \"exited\"")
    });
    let second = f.run(&["start", "app"]);
    assert_success(&second);
    assert!(!stdout(&second).contains("umu-run: winetricks"));
}

#[test]
fn running_start_skips_missing_verbs() {
    let f = Fixture::new();
    f.environment("app", "[targets.app]\nexe = 'app'\n");
    assert_success(&f.run(&["start", "app"]));
    wait_for("app to run", || f.fake_program_running("app"));
    assert_success(&f.run(&["set", "app", "winetricks=['win10']"]));
    let again = f.run_sandboxed(&["start", "app"]);
    assert_success(&again);
    assert!(stderr(&again).contains("already running"));
    assert!(!stdout(&again).contains("winetricks"));
    assert_success(&f.run(&["stop", "app"]));
}

#[test]
fn set_and_unset_do_not_touch_prefix() {
    let f = Fixture::new();
    f.environment("app", "[targets.app]\nexe = 'exit-0'\n");
    let prefix = f.root.path().join("prefixes/app");
    let set = f.run(&["set", "app", "winetricks=['win10']"]);
    assert_success(&set);
    assert!(f.created_config("app").contains("winetricks = ['win10']"));
    assert!(!prefix.join("winetricks.log").exists());
    let unset = f.run(&["set", "app", "--unset", "winetricks"]);
    assert_success(&unset);
    assert!(!f.created_config("app").contains("winetricks ="));
    assert!(!prefix.join("winetricks.log").exists());
}

#[test]
fn native_set_refuses_winetricks() {
    let f = Fixture::new();
    let dir = f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("app.toml"), "kind = 'native'\n").unwrap();
    let out = f.run(&["set", "app", "winetricks=['win10']"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("native environment app can't set winetricks"));
}

#[test]
fn unreadable_log_warns_without_crashing_status() {
    let f = Fixture::new();
    f.environment("app", EXIT);
    fs::create_dir(f.root.path().join("prefixes/app/winetricks.log")).unwrap();
    let status = f.run(&["status", "app", "--json"]);
    assert_success(&status);
    assert!(stderr(&status).contains("warning: cannot read winetricks log"));
    let json: serde_json::Value = serde_json::from_str(&stdout(&status)).unwrap();
    assert_eq!(
        json["winetricks_missing"],
        serde_json::json!(["win10", "corefonts"])
    );
}

#[test]
fn status_marks_only_missing_verbs_and_json_has_both_arrays() {
    let f = Fixture::new();
    f.environment("app", EXIT);
    fs::write(f.root.path().join("prefixes/app/winetricks.log"), "win10\n").unwrap();
    let text = f.run(&["status", "app"]);
    assert_success(&text);
    assert!(stdout(&text).contains("winetricks  win10, corefonts (missing)"));
    let json = f.run(&["status", "app", "--json"]);
    assert_success(&json);
    let json: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(
        json["winetricks"],
        serde_json::json!(["win10", "corefonts"])
    );
    assert_eq!(json["winetricks_missing"], serde_json::json!(["corefonts"]));
}

#[test]
fn validate_reports_codes_and_fixes_missing_verbs() {
    let f = Fixture::new();
    f.environment("app", EXIT);
    let checked = f.run_sandboxed(&["validate", "app", "--json"]);
    let report: serde_json::Value = serde_json::from_str(&stdout(&checked))
        .unwrap_or_else(|err| panic!("{err}: {} {}", stdout(&checked), stderr(&checked)));
    let codes: Vec<_> = report["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"winetricks_missing"), "{report}");
    assert!(codes.contains(&"winetricks_unavailable"), "{report}");
    fake_winetricks(&f);
    let fixed = f.run(&["validate", "app", "--fix", "--json"]);
    assert_success(&fixed);
    let fixed: serde_json::Value = serde_json::from_str(&stdout(&fixed)).unwrap();
    assert_eq!(fixed["problems"], serde_json::json!([]));
    assert_eq!(
        fs::read_to_string(f.root.path().join("prefixes/app/winetricks.log")).unwrap(),
        "win10\ncorefonts\n"
    );
}
