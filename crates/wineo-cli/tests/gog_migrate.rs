//! `wineo import gog` moving the games Heroic installed into wineo, against
//! a fake Heroic. Migration is local, so no GOG login or server is involved.

mod common;

use std::fs;

use common::gog_world::{DISCO, RIMWORLD, RIMWORLD_DLC};
use common::pty::Pty;
use common::snapshot::Snapshot;
use common::{Fixture, assert_success, stderr, stdout};
use serde_json::json;
use wineo_core::gog::folder::{ProductStatus, Track};

fn proton(name: &str) -> serde_json::Value {
    json!({"type": "proton", "name": name, "bin": format!("/nowhere/{name}/proton")})
}

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.heroic_gog();
    f
}

fn import(f: &Fixture, args: &[&str]) -> std::process::Output {
    let mut all = vec!["import", "gog"];
    all.extend_from_slice(args);
    f.run_at_home(&all)
}

fn game(id: u64) -> String {
    id.to_string()
}

#[test]
fn a_native_game_becomes_an_environment_and_nothing_of_heroics_changes() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    let heroic = Snapshot::capture(&f.heroic()).unwrap();

    let out = import(&f, &["--game", &game(RIMWORLD), "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    for line in [
        "Migrate RimWorld (1094900565)",
        "  environment  rimworld (new native environment)",
        "  track        installer",
        "  version      1.6.4871 rev588",
        "  language     en",
        "  dlc          2 (1233017772, 1827857764)",
        "Migrated RimWorld as rimworld.",
        "Remove it from Heroic's library, without deleting its files, so both don't update it.",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in:\n{text}"
        );
    }
    assert!(text.contains("  folder       "), "{text}");

    let config = f.created_config("rimworld");
    assert!(config.contains("gog_id = \"1094900565\""), "{config}");
    assert!(config.contains("heroic_id = \"1094900565\""), "{config}");
    let state = f.gog_state(&folder).unwrap();
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    assert!(state.journal.is_none());
    let committed = state.committed.unwrap();
    assert_eq!(committed.track, Track::Installer);
    assert_eq!(committed.language, "en");
    assert_eq!(
        committed.products[0].version.as_deref(),
        Some("1.6.4871 rev588")
    );
    assert_eq!(committed.products[0].status, ProductStatus::Installed);
    let dlc: Vec<u64> = committed.products[1..].iter().map(|p| p.id).collect();
    assert_eq!(dlc, RIMWORLD_DLC);
    assert!(folder.join("mods-1.5/MyMod").is_dir());
    heroic.assert_unchanged(&f.heroic());
}

#[test]
fn a_game_with_an_environment_attaches_to_it_and_keeps_its_settings() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    let env = f.config_dir().join("environments/my-rimworld.toml");
    fs::create_dir_all(env.parent().unwrap()).unwrap();
    let before = format!(
        "# mine\nkind = \"native\"\ntags = [\"game\"]\n\n[targets.game]\nexe = \"{}\"\ndefault = true\n",
        folder.join("start.sh").display()
    );
    fs::write(&env, &before).unwrap();

    let out = import(&f, &["--game", &game(RIMWORLD), "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("  environment  my-rimworld (existing)"),
        "{text}"
    );
    assert!(text.contains("Migrated RimWorld as my-rimworld."), "{text}");
    let after = fs::read_to_string(&env).unwrap();
    assert!(
        after.starts_with("# mine\nkind = \"native\"\ntags = [\"game\"]\n"),
        "{after}"
    );
    assert!(after.contains("gog_id = \"1094900565\""), "{after}");
    assert!(f.created_config("rimworld").is_empty());
    assert_eq!(
        f.gog_state(&folder).unwrap().owner.as_deref(),
        Some("my-rimworld")
    );
}

#[test]
fn a_windows_game_adopts_heroics_prefix_with_a_runner() {
    let f = fixture();
    f.install_proton("GE-Proton10-4");
    let folder = f.heroic_disco(proton("GE-Proton10-4"));
    let prefix = f.root.path().join("Prefixes/Disco");
    let heroic = Snapshot::capture(&f.heroic()).unwrap();
    let prefix_before = Snapshot::capture(&prefix).unwrap();

    let out = import(&f, &["--game", &game(DISCO), "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    for line in [
        "  environment  disco-elysium (new Windows environment)",
        "  track        galaxy",
        "  version      build 59781086834418085",
        "  runner       GE-Proton10-4",
        "  prefix       ~/Prefixes/Disco (Heroic's, kept where it is)",
        "Migrated Disco Elysium as disco-elysium.",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in:\n{text}"
        );
    }
    let config = f.created_config("disco-elysium");
    assert!(config.contains("prefix = \"~/Prefixes/Disco\""), "{config}");
    assert!(config.contains("runner = \"GE-Proton10-4\""), "{config}");
    assert!(config.contains("gog_id = \"1771589310\""), "{config}");
    let committed = f.gog_state(&folder).unwrap().committed.unwrap();
    assert_eq!(committed.track, Track::Galaxy);
    assert_eq!(
        committed.products[0].build.as_deref(),
        Some("59781086834418085")
    );
    heroic.assert_unchanged(&f.heroic());
    prefix_before.assert_unchanged(&prefix);
}

#[test]
fn a_windows_game_without_a_runner_wineo_has_fails() {
    let f = fixture();
    f.heroic_disco(proton("GE-Proton10-4"));
    let before = Snapshot::capture(f.root.path()).unwrap();
    let out = import(&f, &["--game", &game(DISCO), "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "error: Disco Elysium needs a Proton runner. Get one with: wineo download ge-proton"
        ),
        "{}",
        stderr(&out)
    );
    before.assert_unchanged(f.root.path());
}

#[test]
fn a_game_already_migrated_is_a_no_op() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    assert_success(&import(&f, &["--game", &game(RIMWORLD), "--yes"]));
    let before = Snapshot::capture(f.root.path()).unwrap();

    for extra in [&["--yes"][..], &["--dry-run"]] {
        let mut args = vec!["--game", "1094900565"];
        args.extend_from_slice(extra);
        let out = import(&f, &args);
        assert_success(&out);
        let text = stdout(&out);
        assert!(
            text.starts_with("Already migrated. rimworld owns "),
            "{text}"
        );
        assert!(text.contains("Games/RimWorld"), "{text}");
        assert!(!text.contains("Migrated"), "{text}");
    }
    // It never asks, so it needs no terminal or --yes either.
    let bare = import(&f, &["--game", "1094900565"]);
    assert_success(&bare);
    assert!(stdout(&bare).starts_with("Already migrated."));
    before.assert_unchanged(f.root.path());
    assert!(f.gog_state(&folder).is_some());
}

#[test]
fn a_game_heroic_doesnt_have_changes_nothing() {
    let f = fixture();
    f.heroic_rimworld();
    let before = Snapshot::capture(f.root.path()).unwrap();
    let out = import(&f, &["--game", "999", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "error: Heroic hasn't installed GOG game 999. Installing games comes in a later release."
        ),
        "{}",
        stderr(&out)
    );
    before.assert_unchanged(f.root.path());

    // No Heroic at all says the same.
    let bare = Fixture::new();
    let out = import(&bare, &["--game", "999", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Installing games comes in a later release."));
}

#[test]
fn migrating_without_a_terminal_needs_yes() {
    let f = fixture();
    f.heroic_rimworld();
    let before = Snapshot::capture(f.root.path()).unwrap();
    for args in [&["--game", "1094900565"][..], &["--all"]] {
        let out = import(&f, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            stderr(&out).contains("error: Migrating without a terminal needs --yes."),
            "{}",
            stderr(&out)
        );
        assert_eq!(stdout(&out), "");
    }
    let guided = import(&f, &[]);
    assert_eq!(guided.status.code(), Some(1));
    assert!(
        stderr(&guided).contains("needs a terminal"),
        "{}",
        stderr(&guided)
    );
    before.assert_unchanged(f.root.path());
}

#[test]
fn a_dry_run_shows_the_review_and_changes_nothing() {
    let f = fixture();
    f.install_proton("GE-Proton10-4");
    f.heroic_rimworld();
    f.heroic_disco(proton("GE-Proton10-4"));
    let before = Snapshot::capture(f.root.path()).unwrap();

    let one = import(&f, &["--game", "1094900565", "--dry-run"]);
    assert_success(&one);
    let text = stdout(&one);
    assert!(text.contains("Migrate RimWorld (1094900565)"), "{text}");
    assert!(text.ends_with("Dry run, nothing was migrated.\n"), "{text}");
    assert!(!text.contains("Migrated"), "{text}");

    let all = import(&f, &["--all", "--dry-run"]);
    assert_success(&all);
    let text = stdout(&all);
    assert!(
        text.contains("Migrate RimWorld") && text.contains("Migrate Disco Elysium"),
        "{text}"
    );
    assert!(text.ends_with("Dry run, nothing was migrated.\n"), "{text}");
    before.assert_unchanged(f.root.path());
}

#[test]
fn a_dry_run_fails_like_the_real_run_would() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    // Someone else's environment owns the folder.
    f.gog_folder(&folder, Some("other"), RIMWORLD, false);
    let before = Snapshot::capture(f.root.path()).unwrap();
    let out = import(&f, &["--game", "1094900565", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("belongs to other."),
        "{}",
        stderr(&out)
    );
    before.assert_unchanged(f.root.path());
}

#[test]
fn all_continues_past_a_failed_game_and_says_which() {
    let f = fixture();
    f.heroic_rimworld();
    // No Proton runner is installed, so the Windows game can't be set up.
    f.heroic_disco(proton("GE-Proton10-4"));

    let out = import(&f, &["--all", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("Migrated RimWorld as rimworld."), "{text}");
    assert!(text.contains("Migrated 1 of 1 game."), "{text}");
    assert!(text.contains("1 failed: Disco Elysium"), "{text}");
    assert!(
        stderr(&out).contains("Failed Disco Elysium: Disco Elysium needs a Proton runner."),
        "{}",
        stderr(&out)
    );
    assert!(f.created_config("rimworld").contains("gog_id"));
    assert!(f.created_config("disco-elysium").is_empty());

    // The game that did migrate is reported as such on the next run.
    f.install_proton("GE-Proton10-4");
    let again = import(&f, &["--all", "--yes"]);
    assert_success(&again);
    let text = stdout(&again);
    assert!(text.contains("Already migrated. rimworld owns "), "{text}");
    assert!(text.contains("Migrated 1 of 1 game."), "{text}");
    assert!(text.contains("1 already migrated: rimworld"), "{text}");
    assert!(f.created_config("disco-elysium").contains("gog_id"));
}

#[test]
fn a_dry_run_of_all_reports_the_game_that_would_fail() {
    let f = fixture();
    f.heroic_rimworld();
    f.heroic_disco(proton("GE-Proton10-4"));
    let before = Snapshot::capture(f.root.path()).unwrap();
    let out = import(&f, &["--all", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("Migrate RimWorld"),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).ends_with("Dry run, nothing was migrated.\n"));
    assert!(
        stderr(&out).contains("Disco Elysium: would fail: Disco Elysium needs a Proton runner."),
        "{}",
        stderr(&out)
    );
    before.assert_unchanged(f.root.path());
}

#[test]
fn all_with_no_heroic_games_has_nothing_to_do() {
    let f = fixture();
    let out = import(&f, &["--all", "--yes"]);
    assert_success(&out);
    assert_eq!(stdout(&out), "Heroic has no GOG games.\n");
}

#[test]
fn a_game_folder_that_is_gone_is_named() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    fs::remove_dir_all(&folder).unwrap();
    let out = import(&f, &["--game", "1094900565", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Heroic's game folder ") && stderr(&out).contains("doesn't exist."),
        "{}",
        stderr(&out)
    );
}

#[test]
fn migrating_refuses_a_portable_folder() {
    let f = fixture();
    f.heroic_rimworld();
    let portable = f.root.path().join("portable");
    for args in [
        &["import", "gog", "--game", "1094900565", "--yes"][..],
        &["import", "gog", "--all", "--yes"],
        &["import", "gog", "--game", "1094900565", "--dry-run"],
    ] {
        let out = f
            .wineo_at_home()
            .arg("--portable")
            .arg(&portable)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out)
                .contains("GOG games live in the host library. Run this without --portable."),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn a_single_game_asks_before_migrating() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    let mut command = f.wineo_at_home();
    command.args(["import", "gog", "--game", "1094900565"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Migrate RimWorld (1094900565)");
    pty.expect("Press Enter to migrate it, Esc or Ctrl+C to cancel.");
    pty.expect("Migrate it?");
    assert!(
        f.gog_state(&folder).is_none(),
        "nothing is written before the answer"
    );
    pty.send("");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Migrated RimWorld as rimworld."),
        "{output}"
    );
    assert_eq!(
        f.gog_state(&folder).unwrap().owner.as_deref(),
        Some("rimworld")
    );
}

#[test]
fn canceling_the_question_exits_130_and_changes_nothing() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    let before = Snapshot::capture(f.root.path()).unwrap();
    let mut command = f.wineo_at_home();
    command.args(["import", "gog", "--game", "1094900565"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Migrate it?");
    pty.ctrl_c();
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Import canceled."), "{output}");
    assert!(f.created_config("rimworld").is_empty());
    before.assert_unchanged(f.root.path());
    assert!(f.gog_state(&folder).is_none());
}

#[test]
fn the_picker_lists_the_games_to_migrate() {
    let f = fixture();
    f.install_proton("GE-Proton10-4");
    f.heroic_rimworld();
    let disco = f.heroic_disco(proton("GE-Proton10-4"));
    let mut command = f.wineo_at_home();
    command.args(["import", "gog"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Which GOG game?");
    pty.expect("Disco Elysium (1771589310, Windows)");
    pty.expect("RimWorld (1094900565, Linux)");
    pty.expect("Game: ");
    pty.send("1");
    // Heroic's own Proton isn't here, so a runner has to be picked.
    pty.expect("Which runner?");
    pty.expect("Runner: ");
    pty.send("1");
    pty.expect("Migrate Disco Elysium (1771589310)");
    pty.expect("Migrate it?");
    pty.send("");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Migrated Disco Elysium as disco-elysium."),
        "{output}"
    );
    assert_eq!(
        f.gog_state(&disco).unwrap().owner.as_deref(),
        Some("disco-elysium")
    );

    // What is migrated no longer shows up.
    let mut command = f.wineo_at_home();
    command.args(["import", "gog"]);
    let mut pty = Pty::spawn(command);
    pty.expect("RimWorld (1094900565, Linux)");
    pty.ctrl_c();
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(!output.contains("Disco Elysium (1771589310"), "{output}");
}

#[test]
fn the_guided_import_menu_offers_gog() {
    let f = fixture();
    let folder = f.heroic_rimworld();
    let mut command = f.wineo_at_home();
    command.arg("import");
    let mut pty = Pty::spawn(command);
    pty.expect("Import from where?");
    pty.expect("A GOG game");
    pty.expect("Source");
    pty.send("4");
    pty.expect("Which GOG game?");
    pty.expect("Game: ");
    pty.send("1");
    pty.expect("Migrate it?");
    pty.send("");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert_eq!(
        f.gog_state(&folder).unwrap().owner.as_deref(),
        Some("rimworld")
    );
}
