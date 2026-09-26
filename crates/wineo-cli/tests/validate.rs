//! `wineo validate` without a terminal: the report, `--fix`, and `--json`.

mod common;

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use common::*;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;

fn art_toml(f: &Fixture, name: &str) -> PathBuf {
    f.root
        .path()
        .join("data/wineo/art")
        .join(name)
        .join("art.toml")
}

/// An `art.toml` recording an icon whose file is gone.
fn missing_icon(f: &Fixture, name: &str) {
    let path = art_toml(f, name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "# picked by hand\nschema_version = 1\n\n[icon]\nfile = \"icon.png\"\nmanual = \"cover.png\"\n",
    )
    .unwrap();
}

/// The same locations the fixture's wineo uses, for taking its locks.
fn paths(f: &Fixture) -> Paths {
    let root = f.root.path().to_owned();
    Paths::from_lookup(move |key| {
        let path = match key {
            "HOME" => root.clone(),
            "XDG_CONFIG_HOME" => root.join("config"),
            "XDG_DATA_HOME" => root.join("data"),
            "XDG_STATE_HOME" => root.join("state"),
            _ => return None,
        };
        Some(OsString::from(path.as_os_str()))
    })
    .unwrap()
}

fn user_configs(f: &Fixture) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for dir in ["environments", "templates"] {
        for entry in fs::read_dir(f.config_dir().join(dir)).into_iter().flatten() {
            let path = entry.unwrap().path();
            files.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    let global = f.config_dir().join("config.toml");
    if let Ok(bytes) = fs::read(&global) {
        files.push((global, bytes));
    }
    files.sort();
    files
}

#[test]
fn a_clean_setup_prints_one_line() {
    let f = Fixture::new();
    f.environment("hades", "[targets.game]\nexe = \"notepad\"\n");
    f.environment("halo", "");
    let output = f.run(&["validate"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "No problems in 2 environments.\n");

    let output = f.run(&["validate", "hades"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "No problems in hades.\n");
}

#[test]
fn verbose_lists_what_was_checked() {
    let f = Fixture::new();
    f.environment("hades", "");
    f.write_template("mine", "gamemode = true\n");
    let output = f.run(&["validate", "-v"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("config.toml  ok\n"), "{text}");
    assert!(text.contains("templates/mine.toml  ok\n"), "{text}");
    assert!(
        text.ends_with("hades  ok\nNo problems in 1 environment.\n"),
        "{text}"
    );
}

#[test]
fn errors_exit_1() {
    let f = Fixture::new();
    f.environment("hades", "[targets.game]\nexe = \"/nonexistent/game\"\n");
    let output = f.run(&["validate"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "hades\n  error    target game: /nonexistent/game not found\n\n1 error.\n"
    );
}

#[test]
fn warnings_alone_exit_0_and_nothing_is_fixed_without_a_terminal() {
    let f = Fixture::new();
    let state = f.stray_state("oldgame");
    let output = f.run(&["validate"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.starts_with("oldgame\n  warning  state dir "), "{text}");
    assert!(text.ends_with("\n\n1 warning.\n"), "{text}");
    assert!(state.exists());
}

#[test]
fn fix_applies_everything_without_a_terminal_and_leaves_user_configs_alone() {
    let f = Fixture::new();
    f.global_config("# mine\nstop_timeout = 5 # seconds\n");
    f.write_template("mine", "# notes\ngamemode = false\n");
    f.environment(
        "hades",
        "# by hand\n[targets.game]\nexe = \"notepad\"   # spacing\n",
    );
    missing_icon(&f, "hades");
    let state = f.stray_state("oldgame");
    let entry = f.stray_entry("oldgame");
    let before = user_configs(&f);

    let output = f.run(&["validate", "--fix"]);
    assert_success(&output);
    assert!(
        stdout(&output).ends_with("\n3 warnings.\nFixed 3 problems.\n"),
        "{}",
        stdout(&output)
    );
    assert!(!state.exists());
    assert!(!entry.exists());
    assert_eq!(
        fs::read_to_string(art_toml(&f, "hades")).unwrap(),
        "# picked by hand\nschema_version = 1\n"
    );
    assert_eq!(user_configs(&f), before);
    assert_eq!(
        stdout(&f.run(&["validate"])),
        "No problems in 1 environment.\n"
    );
}

#[test]
fn errors_left_after_fixing_still_exit_1() {
    let f = Fixture::new();
    f.environment("hades", "[targets.game]\nexe = \"/nonexistent/game\"\n");
    f.stray_state("oldgame");
    let output = f.run(&["validate", "--fix"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).ends_with("Fixed 1 problem. 1 error left.\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_fix_for_a_busy_environment_fails_the_rest_go_on_and_it_exits_1() {
    let f = Fixture::new();
    let state = f.stray_state("oldgame");
    let entry = f.stray_entry("gone");
    let _busy = Locks::new(&paths(&f))
        .acquire(Operation::Snapshot, "oldgame", &[])
        .unwrap();

    let output = f.run(&["validate", "--fix"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(
            "error: Remove oldgame's leftover state failed: environment `oldgame` is busy: a snapshot operation is running"
        ),
        "{}",
        stderr(&output)
    );
    assert!(
        stdout(&output).ends_with("Fixed 1 problem. 1 warning left.\n"),
        "{}",
        stdout(&output)
    );
    assert!(state.exists());
    assert!(!entry.exists());
}

#[test]
fn json_reports_without_asking_and_fix_lists_what_it_fixed() {
    let f = Fixture::new();
    f.environment("hades", "[targets.game]\nexe = \"/nonexistent/game\"\n");
    missing_icon(&f, "hades");
    let before = fs::read(art_toml(&f, "hades")).unwrap();

    let output = f.run(&["validate", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "problems": [
                {
                    "environment": "hades",
                    "area": "target",
                    "level": "error",
                    "code": "exe_missing",
                    "message": "target game: /nonexistent/game not found",
                    "fix": null,
                },
                {
                    "environment": "hades",
                    "area": "art",
                    "level": "warning",
                    "code": "art_file_missing",
                    "message": "art: icon.png is recorded but missing",
                    "fix": "Drop hades's missing icon record",
                },
            ],
            "fixed": [],
            "errors": [],
        })
    );
    assert_eq!(fs::read(art_toml(&f, "hades")).unwrap(), before);

    let output = f.run(&["validate", "--json", "--fix"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["fixed"],
        serde_json::json!(["Drop hades's missing icon record"])
    );
    assert_eq!(json["problems"].as_array().unwrap().len(), 1);
    assert_eq!(json["problems"][0]["code"], "exe_missing");
}

#[test]
fn json_for_a_clean_setup_exits_0() {
    let f = Fixture::new();
    f.environment("hades", "");
    let output = f.run(&["validate", "--json"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "{\"problems\":[],\"fixed\":[],\"errors\":[]}\n"
    );
}

#[test]
fn a_name_checks_only_that_environment() {
    let f = Fixture::new();
    f.global_config("stop_timeuot = 3\n");
    f.environment("hades", "");
    f.environment("halo", "[targets.game]\nexe = \"/nonexistent/game\"\n");
    f.stray_state("oldgame");
    let output = f.run(&["validate", "hades"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "No problems in hades.\n");

    let output = f.run(&["validate", "nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("environment `nope` does not exist"));
}

#[test]
fn portable_mode_checks_only_the_portable_folder() {
    let f = Fixture::new();
    f.environment("hades", "[targets.game]\nexe = \"/nonexistent/game\"\n");
    let usb = f.root.path().join("usb");
    let config = usb.join("config/environments/game.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "prefix = 'prefixes/game'\nrunner = 'wine'\narch = 'win64'\n[targets.game]\nexe = 'notepad'\ndesktop = true\n",
    )
    .unwrap();
    let output = f.run(&["--portable", usb.to_str().unwrap(), "validate"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.starts_with("game\n"), "{text}");
    assert!(!text.contains("hades"), "{text}");
    assert!(
        text.contains("portable environments never get desktop entries"),
        "{text}"
    );
}

#[test]
fn json_fix_lists_failed_fixes_under_errors_and_exits_1() {
    let f = Fixture::new();
    f.stray_state("oldgame");
    let entry = f.stray_entry("gone");
    let _busy = Locks::new(&paths(&f))
        .acquire(Operation::Snapshot, "oldgame", &[])
        .unwrap();
    let output = f.run(&["validate", "--json", "--fix"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["fixed"],
        serde_json::json!(["Remove gone's desktop entry"])
    );
    assert_eq!(
        json["errors"],
        serde_json::json!([{
            "fix": "Remove oldgame's leftover state",
            "message": "environment `oldgame` is busy: a snapshot operation is running",
        }])
    );
    assert_eq!(json["problems"][0]["code"], "state_dir_stray");
    assert!(!entry.exists());
}
