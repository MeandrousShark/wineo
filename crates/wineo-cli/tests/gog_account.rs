//! `wineo import gog`: logging in and out, listing the owned games, and
//! forgetting a game folder, against a fake GOG.

mod common;

use std::fs;
use std::process::Stdio;

use common::gog_api::{CODE, GogApi, USER, installer, owned_game, product, success_address};
use common::gog_world::{DISCO, RIMWORLD, RIMWORLD_DLC};
use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout};

fn login(f: &Fixture, api: &GogApi, address: &str) -> (std::process::ExitStatus, String) {
    let mut command = f.gog_command(api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(address);
    pty.finish()
}

/// Logs in with the right code.
fn logged_in(f: &Fixture, api: &GogApi) {
    let (status, output) = login(f, api, &success_address(CODE));
    assert!(status.success(), "{output}");
}

/// No secret may show up in anything wineo prints.
#[track_caller]
fn assert_no_secrets(text: &str) {
    for secret in ["SECRET", "GoodCode"] {
        assert!(!text.contains(secret), "{secret} in:\n{text}");
    }
}

fn library() -> GogApi {
    GogApi::with_library(
        vec![
            owned_game(RIMWORLD, "RimWorld", true, false),
            owned_game(DISCO, "Disco Elysium", false, true),
            owned_game(777, "Hades", true, true),
        ],
        vec![
            product(
                RIMWORLD,
                "RimWorld",
                &[installer("linux", "en", "1.6.4900")],
                &[product(
                    RIMWORLD_DLC[0],
                    "Royalty",
                    &[installer("linux", "en", "1.6.4900")],
                    &[],
                )],
            ),
            product(777, "Hades", &[installer("linux", "en", "1")], &[]),
        ],
    )
}

#[test]
fn login_saves_the_login_and_prints_no_secret() {
    let f = Fixture::new();
    let api = GogApi::start();
    let (status, output) = login(&f, &api, &success_address(CODE));
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Log in to GOG in your browser."),
        "{output}"
    );
    assert!(
        output.contains(&format!("{}/auth?client_id=", api.base)),
        "{output}"
    );
    assert!(
        output.contains(&format!("Logged in to GOG as account {USER}.")),
        "{output}"
    );
    // The address was typed at a prompt that doesn't echo, and nothing
    // printed it back.
    assert_no_secrets(&output);

    // The saved login works for the next command, which renews it.
    api.with(|config| config.owned = vec![owned_game(777, "Hades", true, true)]);
    let listed = f.gog(&api, &["import", "gog", "--list"]);
    assert_success(&listed);
    assert!(stdout(&listed).contains("Hades"), "{}", stdout(&listed));
    assert_no_secrets(&stdout(&listed));
    assert_no_secrets(&stderr(&listed));
}

#[test]
fn a_bare_code_is_accepted_too() {
    let f = Fixture::new();
    let api = GogApi::start();
    let (status, output) = login(&f, &api, CODE);
    assert!(status.success(), "{output}");
    assert_no_secrets(&output);
}

#[test]
fn a_bad_address_is_refused_without_echoing_it() {
    let f = Fixture::new();
    let api = GogApi::start();
    for pasted in [
        "https://evil.example/on_login_success?code=SECRETCODE1234",
        "https://embed.gog.com/on_login_success?origin=client",
        "SECRET words",
    ] {
        let (status, output) = login(&f, &api, pasted);
        assert_eq!(status.code(), Some(1), "{output}");
        assert!(output.contains("error: "), "{output}");
        assert_no_secrets(&output);
        assert!(!output.contains("evil"), "{output}");
    }
    // Nothing reached GOG, and nothing was saved.
    assert!(api.requests().is_empty(), "{:?}", api.requests());
    let listed = f.gog(&api, &["import", "gog", "--list"]);
    assert_eq!(listed.status.code(), Some(1));
    assert!(stderr(&listed).contains("Log in to GOG first: wineo import gog --login."));
}

#[test]
fn a_login_gog_refuses_is_not_saved() {
    let f = Fixture::new();
    let api = GogApi::start();
    let (status, output) = login(&f, &api, &success_address("WrongCode-SECRET-9999"));
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(
        output.contains("GOG didn't accept that login. Log in again and paste the new address."),
        "{output}"
    );
    assert_no_secrets(&output);
    let listed = f.gog(&api, &["import", "gog", "--list"]);
    assert!(
        stderr(&listed).contains("Log in to GOG first"),
        "{}",
        stderr(&listed)
    );

    // A GOG that is down keeps the failure temporary and still prints no
    // secret.
    api.with(|config| config.token_status = Some(503));
    let (status, output) = login(&f, &api, &success_address(CODE));
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(output.contains("GOG answered with status 503"), "{output}");
    assert_no_secrets(&output);
}

#[test]
fn logging_in_needs_a_terminal() {
    let f = Fixture::new();
    let api = GogApi::start();
    let output = f
        .gog_command(&api)
        .args(["import", "gog", "--login"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("Logging in to GOG needs a terminal."));
    assert!(api.requests().is_empty());
}

#[test]
fn logout_removes_the_login_and_the_owned_list() {
    let f = Fixture::new();
    let api = library();
    logged_in(&f, &api);
    assert_success(&f.gog(&api, &["import", "gog", "--list"]));
    let cache = f
        .root
        .path()
        .join(format!("cache/wineo/gog-owned-{USER}.json"));
    assert!(cache.exists(), "the owned list is cached");

    let out = f.gog(&api, &["import", "gog", "--logout"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("Removed the GOG login."), "{text}");
    assert!(
        text.contains(&format!("Removed the owned-games list for account {USER}.")),
        "{text}"
    );
    assert!(!cache.exists());

    let listed = f.gog(&api, &["import", "gog", "--list"]);
    assert_eq!(listed.status.code(), Some(1));
    assert!(stderr(&listed).contains("error: Log in to GOG first: wineo import gog --login."));

    let again = f.gog(&api, &["import", "gog", "--logout"]);
    assert_success(&again);
    assert!(stdout(&again).contains("No GOG login was saved."));
}

#[test]
fn list_shows_the_three_states() {
    let f = Fixture::new();
    f.heroic_gog();
    f.heroic_disco(serde_json::json!({"type": "wine", "name": "System", "bin": "wine"}));
    let rimworld = f.heroic_rimworld();
    f.gog_environment(
        "rimworld",
        &rimworld,
        RIMWORLD,
        "1.6.4871 rev588",
        &[(RIMWORLD_DLC[0], None)],
    );
    let api = library();
    logged_in(&f, &api);

    let out = f.gog(&api, &["import", "gog", "--list"]);
    assert_success(&out);
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("ID"), "{text}");
    let row = |title: &str| lines.iter().find(|line| line.contains(title)).unwrap();
    assert!(row("Disco Elysium").contains("in Heroic"), "{text}");
    assert!(row("Hades").contains("not installed"), "{text}");
    let rimworld_row = row("RimWorld");
    for part in [
        "imported",
        "rimworld",
        "installer",
        "1.6.4871 rev588",
        "update available",
    ] {
        assert!(rimworld_row.contains(part), "{rimworld_row}");
    }
}

#[test]
fn list_json_has_the_plans_shape() {
    let f = Fixture::new();
    f.heroic_gog();
    f.heroic_disco(serde_json::json!({"type": "wine", "name": "System", "bin": "wine"}));
    let rimworld = f.heroic_rimworld();
    f.gog_environment(
        "rimworld",
        &rimworld,
        RIMWORLD,
        "1.6.4871 rev588",
        &[(RIMWORLD_DLC[0], None)],
    );
    let api = library();
    logged_in(&f, &api);

    let out = f.gog(&api, &["import", "gog", "--list", "--json"]);
    assert_success(&out);
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["errors"], serde_json::json!([]));
    let games = json["games"].as_array().unwrap();
    let titles: Vec<_> = games
        .iter()
        .map(|game| game["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Disco Elysium", "Hades", "RimWorld"]);

    let [disco, hades, rimworld] = games.as_slice() else {
        panic!("three games");
    };
    assert_eq!(
        disco,
        &serde_json::json!({
            "id": DISCO.to_string(), "title": "Disco Elysium", "platforms": ["windows"],
            "state": "heroic", "environment": null, "track": null, "version": null,
            "update": null, "applicable": false, "reason": null, "dlc": [],
        })
    );
    assert_eq!(hades["state"], "not_installed");
    assert_eq!(hades["platforms"], serde_json::json!(["windows", "linux"]));
    assert_eq!(
        rimworld,
        &serde_json::json!({
            "id": RIMWORLD.to_string(), "title": "RimWorld", "platforms": ["linux"],
            "state": "imported", "environment": "rimworld", "track": "installer",
            "version": "1.6.4871 rev588", "update": "available", "applicable": true,
            "reason": null,
            "dlc": [{"id": RIMWORLD_DLC[0].to_string(), "title": "Royalty", "state": "unknown"}],
        })
    );
}

#[test]
fn list_marks_a_game_whose_details_cant_be_fetched() {
    let f = Fixture::new();
    f.heroic_gog();
    let rimworld = f.heroic_rimworld();
    f.gog_environment("rimworld", &rimworld, RIMWORLD, "1.6.4871 rev588", &[]);
    let api = library();
    logged_in(&f, &api);
    api.with(|config| {
        config.products.remove(&RIMWORLD);
    });

    let out = f.gog(&api, &["import", "gog", "--list", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let errors = json["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["name"], "rimworld");
    assert_eq!(errors[0]["id"], RIMWORLD.to_string());
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("GOG product request failed"),
        "{errors:?}"
    );
    // The games that were checked still print.
    assert_eq!(json["games"].as_array().unwrap().len(), 3);
}

#[test]
fn list_falls_back_to_the_saved_list_with_a_warning() {
    let f = Fixture::new();
    let api = library();
    logged_in(&f, &api);
    assert_success(&f.gog(&api, &["import", "gog", "--list"]));
    let requests = api.owned_requests();

    // Within the hour the saved list is used without asking GOG.
    let cached = f.gog(&api, &["import", "gog", "--list", "--json"]);
    assert_success(&cached);
    assert_eq!(api.owned_requests(), requests);
    assert!(stderr(&cached).is_empty());

    // --refresh asks again, and a failed answer falls back with a warning
    // that stays out of the JSON.
    api.with(|config| config.owned_status = Some(500));
    let stale = f.gog(&api, &["import", "gog", "--list", "--json", "--refresh"]);
    assert_success(&stale);
    assert!(
        stderr(&stale).contains("warning: Couldn't refresh owned GOG games: "),
        "{}",
        stderr(&stale)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout(&stale)).unwrap();
    assert_eq!(json["games"].as_array().unwrap().len(), 3);
    assert_eq!(json["errors"], serde_json::json!([]));
    assert_no_secrets(&stderr(&stale));
}

#[test]
fn list_works_in_a_portable_folder() {
    let f = Fixture::new();
    let api = library();
    logged_in(&f, &api);
    let portable = f.root.path().join("portable");
    let out = f
        .gog_command(&api)
        .args(["--portable"])
        .arg(&portable)
        .args(["import", "gog", "--list", "--json"])
        .output()
        .unwrap();
    assert_success(&out);
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["games"].as_array().unwrap().len(), 3);
    assert!(
        json["games"]
            .as_array()
            .unwrap()
            .iter()
            .all(|game| game["state"] != "imported")
    );
}

#[test]
fn logout_and_login_work_in_a_portable_folder() {
    let f = Fixture::new();
    let api = GogApi::start();
    logged_in(&f, &api);
    let portable = f.root.path().join("portable");
    let out = f
        .gog_command(&api)
        .arg("--portable")
        .arg(&portable)
        .args(["import", "gog", "--logout"])
        .output()
        .unwrap();
    assert_success(&out);
    assert!(stdout(&out).contains("Removed the GOG login."));
}

#[test]
fn forget_removes_an_unowned_folders_state() {
    let f = Fixture::new();
    let folder = f.root.path().join("Games/Gone");
    fs::create_dir_all(&folder).unwrap();
    f.gog_folder(&folder, None, RIMWORLD, false);
    assert!(f.gog_state(&folder).is_some());

    // The folder can be gone, which is why its state is forgotten.
    fs::remove_dir(&folder).unwrap();
    let out = f.run_at_home(&["import", "gog", "--forget", folder.to_str().unwrap()]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("Forgot the GOG state of"),
        "{}",
        stdout(&out)
    );
    assert!(
        f.gog_state(&folder.parent().unwrap().join("Gone"))
            .is_none()
    );

    let again = f.run_at_home(&["import", "gog", "--forget", folder.to_str().unwrap()]);
    assert_success(&again);
    assert!(stdout(&again).contains("No GOG state is saved for"));
}

#[test]
fn forget_is_refused_while_an_environment_owns_the_folder() {
    let f = Fixture::new();
    f.heroic_gog();
    let rimworld = f.heroic_rimworld();
    f.gog_environment("rimworld", &rimworld, RIMWORLD, "1", &[]);
    let out = f.run_at_home(&["import", "gog", "--forget", rimworld.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("belongs to rimworld."),
        "{}",
        stderr(&out)
    );
    assert!(f.gog_state(&rimworld).is_some());
}

#[test]
fn forget_refuses_in_a_portable_folder() {
    let f = Fixture::new();
    let portable = f.root.path().join("portable");
    let out = f
        .wineo_at_home()
        .arg("--portable")
        .arg(&portable)
        .args(["import", "gog", "--forget", "/somewhere"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("GOG games live in the host library. Run this without --portable.")
    );
}

#[test]
fn modes_that_dont_go_together_are_usage_errors() {
    let f = Fixture::new();
    for args in [
        &["import", "gog", "--login", "--logout"][..],
        &["import", "gog", "--list", "--game", "1"],
        &["import", "gog", "--game", "1", "--all"],
        &["import", "gog", "--refresh"],
        &["import", "gog", "--json"],
        &["import", "gog", "--list", "--yes"],
        &["import", "gog", "--forget", "x", "--dry-run"],
    ] {
        let out = f.run_at_home(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}
