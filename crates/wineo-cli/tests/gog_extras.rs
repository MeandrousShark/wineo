//! What `import gog` adds after a migration: GOG's playtime and art, against
//! a fake GOG.

mod common;

use common::gog_api::{CODE, GogApi, success_address};
use common::gog_world::{DISCO, RIMWORLD};
use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout, write_script};
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
    assert_eq!(
        stderr(&out),
        "Added a desktop entry for game in rimworld, with no icon\n"
    );
    assert!(text.contains("Migrated RimWorld as rimworld."), "{text}");

    // It is only said once: the game is already migrated now.
    let again = migrate(&f, &api, &[]);
    assert_success(&again);
    assert!(!stdout(&again).contains("playtime"), "{}", stdout(&again));
}

#[test]
fn a_bulk_migration_logs_in_to_gog_once_for_all_its_games() {
    let (f, api) = world();
    f.install_proton("GE-Proton10-4");
    f.heroic_disco(json!({
        "type": "proton",
        "name": "GE-Proton10-4",
        "bin": "/nowhere/GE-Proton10-4/proton",
    }));
    logged_in(&f, &api);
    let renewals = |api: &GogApi| {
        api.requests()
            .iter()
            .filter(|line| line.contains("grant_type=refresh_token"))
            .count()
    };
    let before = renewals(&api);
    api.with(|config| {
        config.playtime.insert(RIMWORLD, 600);
        config.playtime.insert(DISCO, 1200);
        config.art.insert(RIMWORLD, json!({}));
        config.art.insert(DISCO, json!({}));
    });
    let out = f.gog(&api, &["import", "gog", "--all", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("Imported 10 h of GOG playtime."), "{text}");
    assert!(text.contains("Imported 20 h of GOG playtime."), "{text}");
    // Each game still asked GOG for its own playtime and art.
    let requests = api.requests();
    let asked = |part: &str| requests.iter().filter(|line| line.contains(part)).count();
    assert_eq!(asked("/sessions"), 2, "{requests:?}");
    assert_eq!(asked("/v2/games/"), 2, "{requests:?}");
    // The access token lasts the whole batch, so it is fetched once, not
    // once per game.
    assert_eq!(renewals(&api) - before, 1, "{requests:?}");
}

#[test]
fn a_login_revoked_during_a_bulk_migration_reads_as_logged_out_for_later_games() {
    let (f, api) = world();
    f.install_proton("GE-Proton10-4");
    f.heroic_disco(json!({
        "type": "proton",
        "name": "GE-Proton10-4",
        "bin": "/nowhere/GE-Proton10-4/proton",
    }));
    logged_in(&f, &api);
    api.with(|config| {
        config.revoked = true;
        config.art.insert(RIMWORLD, json!({}));
        config.art.insert(DISCO, json!({}));
    });
    let out = f.gog(&api, &["import", "gog", "--all", "--yes"]);
    assert_success(&out);
    let warnings: Vec<_> = stderr(&out)
        .lines()
        .filter(|line| line.contains("playtime"))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        warnings,
        [
            "warning: Couldn't import GOG playtime: Log in to GOG again: wineo import gog --login.",
            "warning: Skipped GOG playtime: not logged in to GOG.",
        ],
        "{}",
        stderr(&out)
    );
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

#[test]
fn programs_launched_through_the_fixture_dont_see_the_gog_override() {
    let f = Fixture::new();
    let dir = f.root.path().join("games");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("probe.sh");
    write_script(
        &script,
        "#!/bin/sh\nprintenv WINEO_TEST_GOG_API > \"$0.out\"\nexit 0\n",
    );
    let config = f.config_dir().join("environments/probe.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        config,
        format!(
            "kind = \"native\"\n\n[targets.game]\nexe = \"{}\"\n",
            script.display()
        ),
    )
    .unwrap();
    let out_file = dir.join("probe.sh.out");
    type Launch<'a> = Box<dyn Fn() -> std::process::Output + 'a>;
    let launches: [Launch<'_>; 3] = [
        Box::new(|| f.run(&["run", "probe"])),
        Box::new(|| f.run_at_home(&["run", "probe"])),
        Box::new(|| f.wineo_at_home().args(["run", "probe"]).output().unwrap()),
    ];
    for launch in launches {
        let output = launch();
        assert_success(&output);
        assert_eq!(std::fs::read_to_string(&out_file).unwrap(), "");
        std::fs::remove_file(&out_file).unwrap();
    }
}
