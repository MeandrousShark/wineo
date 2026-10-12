//! What `import gog` adds after a migration: GOG's playtime and art, against
//! a fake GOG.

mod common;

use common::gog_api::{CODE, GogApi, success_address};
use common::gog_world::RIMWORLD;
use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout};
use serde_json::json;

fn logged_in(f: &Fixture, api: &GogApi) {
    let mut command = f.gog_command(api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

fn world() -> (Fixture, GogApi) {
    let f = Fixture::new();
    f.heroic_gog();
    f.heroic_rimworld();
    let api = GogApi::start();
    (f, api)
}

fn migrate(f: &Fixture, api: &GogApi, extra: &[&str]) -> std::process::Output {
    let id = RIMWORLD.to_string();
    let mut args = vec!["import", "gog", "--game", &id, "--yes"];
    args.extend_from_slice(extra);
    f.gog(api, &args)
}

fn asked_for_extras(api: &GogApi) -> bool {
    api.requests()
        .iter()
        .any(|line| line.contains("/sessions") || line.contains("/v2/games/"))
}

#[test]
fn a_migration_imports_gogs_playtime() {
    let (f, api) = world();
    logged_in(&f, &api);
    api.with(|config| {
        config.playtime.insert(RIMWORLD, 41 * 60 + 5);
        config.art.insert(RIMWORLD, json!({}));
    });
    let out = migrate(&f, &api, &[]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("Imported 41 h of GOG playtime.\n"), "{text}");
    // GOG has no art for it, which isn't worth a line.
    assert!(!text.contains("Added "), "{text}");
    assert_eq!(stderr(&out), "");
    assert!(text.contains("Migrated RimWorld as rimworld."), "{text}");

    // It is only said once: the game is already migrated now.
    let again = migrate(&f, &api, &[]);
    assert_success(&again);
    assert!(!stdout(&again).contains("playtime"), "{}", stdout(&again));
}

#[test]
fn a_gog_outage_only_warns() {
    let (f, api) = world();
    logged_in(&f, &api);
    // No art is listed, so GOG answers 404 for it.
    api.with(|config| {
        config.playtime.insert(RIMWORLD, 600);
    });
    let out = migrate(&f, &api, &[]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("Imported 10 h of GOG playtime."),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("warning: Couldn't get GOG artwork"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn logged_out_the_playtime_is_skipped_with_a_note() {
    let (f, api) = world();
    api.with(|config| {
        config.art.insert(RIMWORLD, json!({}));
    });
    let out = migrate(&f, &api, &[]);
    assert_success(&out);
    assert!(
        stderr(&out).contains("warning: Skipped GOG playtime: not logged in to GOG."),
        "{}",
        stderr(&out)
    );
    assert!(!stdout(&out).contains("playtime"), "{}", stdout(&out));
    assert!(stdout(&out).contains("Migrated RimWorld as rimworld."));
}

#[test]
fn a_dry_run_asks_gog_for_nothing() {
    let (f, api) = world();
    logged_in(&f, &api);
    api.with(|config| {
        config.playtime.insert(RIMWORLD, 600);
    });
    let out = migrate(&f, &api, &["--dry-run"]);
    assert_success(&out);
    assert!(!stdout(&out).contains("playtime"), "{}", stdout(&out));
    assert!(!asked_for_extras(&api), "{:?}", api.requests());
}
