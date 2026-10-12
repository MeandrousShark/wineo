//! `wineo update --check` against a fake GOG, and the rest of the command
//! line it shares with applying updates.

mod common;

use common::gog_api::{self, CODE, GogApi, installer, owned_game, product, success_address};
use common::gog_world::{DISCO, RIMWORLD, RIMWORLD_DLC};
use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout};
use serde_json::{Value, json};

fn logged_in(f: &Fixture, api: &GogApi) {
    let mut command = f.gog_command(api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

/// RimWorld, with an update for the game and one of its two DLC, owned and
/// imported, and a Windows game from Galaxy at its latest build.
fn world() -> (Fixture, GogApi) {
    let f = Fixture::new();
    f.heroic_gog();
    let rimworld = f.heroic_rimworld();
    f.gog_environment(
        "rimworld",
        &rimworld,
        RIMWORLD,
        "1.6.4871 rev588",
        &[
            (RIMWORLD_DLC[0], Some("1.6.4871 rev588")),
            (RIMWORLD_DLC[1], None),
        ],
    );
    let api = GogApi::with_library(
        vec![owned_game(RIMWORLD, "RimWorld", true, false)],
        vec![product(
            RIMWORLD,
            "RimWorld",
            &[installer("linux", "en", "1.6.4900 rev600")],
            &[
                product(
                    RIMWORLD_DLC[0],
                    "Royalty",
                    &[installer("linux", "en", "1.6.4900 rev600")],
                    &[],
                ),
                product(
                    RIMWORLD_DLC[1],
                    "Ideology",
                    &[installer("linux", "en", "1.6.4900 rev600")],
                    &[],
                ),
            ],
        )],
    );
    logged_in(&f, &api);
    (f, api)
}

/// Adds Disco Elysium as a Windows game from Galaxy that is up to date.
fn disco(f: &Fixture, api: &GogApi) {
    let folder = f.heroic_disco(json!({"type": "wine", "name": "System", "bin": "wine"}));
    let config = f.config_dir().join("environments/disco.toml");
    std::fs::write(
        config,
        format!(
            "gog_id = \"{DISCO}\"\nrunner = \"wine\"\nprefix = \"{}\"\n\n[targets.game]\nexe = \"{}\"\n",
            f.root.path().join("Prefixes/Disco").display(),
            folder.join("Disco Elysium.exe").display()
        ),
    )
    .unwrap();
    use wineo_core::gog::folder::{
        Committed, FolderLock, FolderState, Platform, Product, ProductStatus, Track,
    };
    let state = FolderState {
        committed: Some(Committed {
            gog_id: DISCO,
            platform: Platform::Windows,
            language: "en-US".to_owned(),
            track: Track::Galaxy,
            products: vec![Product {
                id: DISCO,
                version: None,
                build: Some("59781086834418085".to_owned()),
                status: ProductStatus::Installed,
            }],
            files: Vec::new(),
        }),
        ..FolderState::new(
            std::fs::canonicalize(&folder).unwrap(),
            Some("disco".to_owned()),
        )
    };
    FolderLock::exclusive(&f.paths(), &state.folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
    api.with(|config| {
        config.products.insert(
            DISCO,
            product(
                DISCO,
                "Disco Elysium",
                &[installer("windows", "en-US", "1")],
                &[],
            ),
        );
        config.builds.insert(
            DISCO,
            gog_api::builds(&[("59781086834418086", None), ("59781086834418085", None)]),
        );
    });
}

#[test]
fn check_lists_a_game_with_an_update_and_its_dlc() {
    let (f, api) = world();
    let out = f.gog(&api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["NAME", "INSTALLED", "AVAILABLE", "STATE"]
    );
    let row = lines[1];
    for part in [
        "rimworld",
        "1.6.4871 rev588",
        "1.6.4900 rev600",
        "update available",
    ] {
        assert!(row.contains(part), "{text}");
    }
    // Only the DLC that differ or aren't known are listed, indented.
    assert!(
        lines.contains(&"  DLC Royalty: 1.6.4871 rev588 -> 1.6.4900 rev600"),
        "{text}"
    );
    assert!(lines.contains(&"  DLC Ideology: version unknown"), "{text}");
    assert_eq!(stderr(&out), "");
}

#[test]
fn check_names_one_game_and_notes_games_it_cant_update() {
    let (f, api) = world();
    disco(&f, &api);
    let out = f.gog(&api, &["update", "--check", "disco"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("update on GOG"), "{text}");
    assert!(
        text.contains("Updating Galaxy builds comes in a later release."),
        "{text}"
    );
    assert!(!text.contains("rimworld"), "{text}");

    let all = f.gog(&api, &["update", "--check", "--all"]);
    assert_success(&all);
    let text = stdout(&all);
    assert!(
        text.contains("rimworld") && text.contains("disco"),
        "{text}"
    );
}

#[test]
fn check_json_has_the_plans_shape() {
    let (f, api) = world();
    disco(&f, &api);
    let out = f.gog(&api, &["update", "--check", "--json"]);
    assert_success(&out);
    let json: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["errors"], json!([]));
    assert_eq!(
        json["environments"],
        json!([
            {
                "name": "disco", "id": DISCO.to_string(), "track": "galaxy",
                "installed": "59781086834418085", "available": "59781086834418086",
                "update": "galaxy", "applicable": false, "reason": "galaxy", "dlc": [],
            },
            {
                "name": "rimworld", "id": RIMWORLD.to_string(), "track": "installer",
                "installed": "1.6.4871 rev588", "available": "1.6.4900 rev600",
                "update": "available", "applicable": true, "reason": null,
                "dlc": [
                    {"id": RIMWORLD_DLC[0].to_string(), "title": "Royalty", "state": "available"},
                    {"id": RIMWORLD_DLC[1].to_string(), "title": "Ideology", "state": "unknown"},
                ],
            },
        ])
    );

    let one = f.gog(&api, &["update", "--check", "rimworld", "--json"]);
    assert_success(&one);
    let one: Value = serde_json::from_str(&stdout(&one)).unwrap();
    assert_eq!(one["environments"].as_array().unwrap().len(), 1);
    assert_eq!(one["environments"][0]["name"], "rimworld");
}

#[test]
fn a_game_that_cant_be_checked_exits_1_with_the_rest() {
    let (f, api) = world();
    disco(&f, &api);
    api.with(|config| {
        config.product_status.insert(DISCO, 500);
    });
    let out = f.gog(&api, &["update", "--check", "--json", "--refresh"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let json: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["environments"].as_array().unwrap().len(), 1);
    assert_eq!(json["environments"][0]["name"], "rimworld");
    assert_eq!(json["errors"][0]["name"], "disco");
    assert_eq!(json["errors"][0]["id"], DISCO.to_string());
    assert!(
        json["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("500")
    );

    let text = f.gog(&api, &["update", "--check", "--refresh"]);
    assert_eq!(text.status.code(), Some(1));
    assert!(stdout(&text).contains("rimworld"));
    assert!(
        stderr(&text).contains("Failed disco: "),
        "{}",
        stderr(&text)
    );

    // A named game has no partial result to print.
    let named = f.gog(&api, &["update", "--check", "disco", "--json", "--refresh"]);
    assert_eq!(named.status.code(), Some(1));
    assert_eq!(stdout(&named), "");
    assert!(stderr(&named).contains("error: "), "{}", stderr(&named));
}

#[test]
fn check_says_to_log_in_when_logged_out() {
    let f = Fixture::new();
    let api = GogApi::start();
    for args in [&["update", "--check"][..], &["update", "--check", "--json"]] {
        let out = f.gog(&api, args);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("error: Log in to GOG first: wineo import gog --login."),
            "{}",
            stderr(&out)
        );
        assert_eq!(stdout(&out), "");
    }
}

#[test]
fn check_refuses_environments_that_are_not_gog_games() {
    let (f, api) = world();
    f.environment("plain", "");
    let out = f.gog(&api, &["update", "--check", "plain"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("plain doesn't track a GOG game."),
        "{}",
        stderr(&out)
    );
    let missing = f.gog(&api, &["update", "--check", "nope"]);
    assert_eq!(missing.status.code(), Some(1));
}

#[test]
fn check_refuses_a_portable_folder() {
    let (f, api) = world();
    let portable = f.root.path().join("portable");
    let out = f
        .gog_command(&api)
        .arg("--portable")
        .arg(&portable)
        .args(["update", "--check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("GOG games live in the host library. Run this without --portable."),
        "{}",
        stderr(&out)
    );
}

#[test]
fn check_with_no_gog_games_says_so() {
    let f = Fixture::new();
    let api = GogApi::start();
    logged_in(&f, &api);
    let out = f.gog(&api, &["update", "--check"]);
    assert_success(&out);
    assert_eq!(stdout(&out), "No GOG games to check.\n");
    let json = f.gog(&api, &["update", "--check", "--json"]);
    assert_success(&json);
    assert_eq!(
        stdout(&json),
        "{\"environments\":[],\"errors\":[],\"warnings\":[]}\n"
    );
}

#[test]
fn applying_updates_isnt_available_yet() {
    let (f, api) = world();
    for args in [
        &["update", "rimworld"][..],
        &["update", "--all"],
        &["update", "--reinstall", "rimworld"],
        &["update"],
    ] {
        let out = f.gog(&api, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            stderr(&out).contains("error: wineo update NAME isn't available yet."),
            "{}",
            stderr(&out)
        );
        assert_eq!(stdout(&out), "");
    }
}

#[test]
fn the_command_line_rejects_what_cant_go_together() {
    let f = Fixture::new();
    for args in [
        &["update", "rimworld", "--all"][..],
        &["update", "--check", "--reinstall"],
        &["update", "--json"],
        &["update", "--all", "--json"],
    ] {
        let out = f.run_at_home(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}

#[test]
fn names_complete_to_the_environments_that_record_a_gog_game() {
    let (f, _api) = world();
    f.environment("plain", "");
    f.environment("rimmed", "");
    let out = f
        .wineo()
        .env("COMPLETE", "fish")
        .env("_CLAP_COMPLETE_INDEX", "2")
        .args(["--", "wineo", "update", "ri"])
        .output()
        .unwrap();
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.lines()
            .any(|line| line.split('\t').next() == Some("rimworld")),
        "{text}"
    );
    assert!(
        !text.contains("rimmed") && !text.contains("plain"),
        "{text}"
    );
}

#[test]
fn a_saved_copy_standing_in_gives_a_warning_in_text_and_json() {
    let (f, api) = world();
    assert_success(&f.gog(&api, &["update", "--check"]));
    api.with(|config| {
        config.product_status.insert(RIMWORLD, 500);
    });

    let out = f.gog(&api, &["update", "--check", "--json", "--refresh"]);
    assert_success(&out);
    let json: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.starts_with("Couldn't refresh GOG product details: "),
        "{warning}"
    );
    assert!(
        stderr(&out).contains(&format!("warning: {warning}")),
        "{}",
        stderr(&out)
    );
    assert_eq!(json["environments"].as_array().unwrap().len(), 1);

    let text = f.gog(&api, &["update", "--check", "--refresh"]);
    assert_success(&text);
    assert!(stderr(&text).contains("warning: Couldn't refresh GOG product details: "));
}
