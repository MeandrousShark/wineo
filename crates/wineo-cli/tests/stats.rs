//! `wineo stats`: the overview, one environment's detail, and their JSON.

mod common;

use std::fs;

use common::{Fixture, assert_success, stdout};

fn session(target: &str, started_at: &str, ended_at: &str, seconds: i64, end: &str) -> String {
    format!(
        "{{\"target\":\"{target}\",\"started_at\":\"{started_at}\",\"ended_at\":\"{ended_at}\",\"seconds\":{seconds},\"end\":\"{end}\"}}\n"
    )
}

#[test]
fn stats_with_no_plays_lists_nobody_and_a_zero_total() {
    let f = Fixture::new();
    f.environment("game", "");
    f.environment("other", "");
    let output = f.run(&["stats"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "2 never played. Total 0 m.\n");
}

#[test]
fn the_overview_sorts_by_time_played() {
    let f = Fixture::new();
    f.environment("cyberpunk-2077", "");
    f.environment("the-sims-4", "");
    f.environment("never-played", "");
    f.write_stats_log(
        "cyberpunk-2077",
        &session(
            "game",
            "2026-09-04T18:00:00Z",
            "2026-09-04T20:00:00Z",
            7200,
            "exited",
        ),
    );
    f.write_stats_log(
        "the-sims-4",
        &(session(
            "game",
            "2026-09-12T20:14:00Z",
            "2026-09-12T21:14:00Z",
            3600,
            "exited",
        ) + &session(
            "game",
            "2026-09-20T19:02:11Z",
            "2026-09-21T00:00:00Z",
            17869,
            "exited",
        )),
    );

    let output = f.run(&["stats"]);
    assert_success(&output);
    let text = stdout(&output);
    // the-sims-4 played longer overall, so it sorts first.
    let sims_at = text.find("the-sims-4").unwrap();
    let cyberpunk_at = text.find("cyberpunk-2077").unwrap();
    assert!(sims_at < cyberpunk_at, "{text}");
    assert!(!text.contains("never-played"), "{text}");
    assert!(text.contains("1 never played."), "{text}");
    assert!(text.contains("played"), "{text}");
    assert!(text.contains("sessions"), "{text}");
    assert!(text.contains("last played"), "{text}");
}

#[test]
fn one_environments_detail_covers_targets_and_averages() {
    let f = Fixture::new();
    f.environment("the-sims-4", "");
    f.write_stats_log(
        "the-sims-4",
        &(session(
            "game",
            "2026-09-12T20:14:00Z",
            "2026-09-12T21:14:00Z",
            3600,
            "exited",
        ) + &session(
            "editor",
            "2026-09-25T16:00:00Z",
            "2026-09-25T16:13:00Z",
            780,
            "stopped",
        )),
    );

    let output = f.run(&["stats", "the-sims-4"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.starts_with("the-sims-4\n"), "{text}");
    assert!(
        text.contains("  played      1 h 13 m over 2 sessions\n"),
        "{text}"
    );
    assert!(text.contains("  average     36 m\n"), "{text}");
    assert!(text.contains("  first       "), "{text}");
    assert!(text.contains("  last        "), "{text}");
    assert!(text.contains("  this week   "), "{text}");
    assert!(text.contains("  this month  "), "{text}");
    assert!(
        text.contains("  targets     game 1 h (1), editor 13 m (1)\n"),
        "{text}"
    );
}

#[test]
fn a_never_played_environment_says_so() {
    let f = Fixture::new();
    f.environment("game", "");
    let output = f.run(&["stats", "game"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "game\n  played      never\n");
}

/// A minimal Heroic install with `game_id`'s playtime in `store/timestamp.json`.
fn heroic_config(f: &Fixture, game_id: &str, total_played_minutes: u64, last_played: &str) {
    let heroic = f.config_dir().join("../heroic");
    fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
    fs::create_dir_all(heroic.join("store")).unwrap();
    fs::write(heroic.join("config.json"), "{}").unwrap();
    fs::write(heroic.join("sideload_apps/library.json"), r#"{"games":[]}"#).unwrap();
    fs::write(
        heroic.join("store/timestamp.json"),
        format!(
            r#"{{"{game_id}":{{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"{last_played}","totalPlayed":{total_played_minutes}}}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn import_heroic_updates_existing_environments_and_the_summary_uses_the_last_import() {
    let f = Fixture::new();
    f.environment("game", "");
    let config = f.config_dir().join("environments/game.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("heroic_id = \"abc\"\n{text}")).unwrap();

    heroic_config(&f, "abc", 60, "2026-09-20T19:02:11.000Z");
    let first = f.run(&["stats", "--import-heroic"]);
    assert_success(&first);
    assert!(
        stdout(&first).contains("game: 1 h from Heroic"),
        "{}",
        stdout(&first)
    );
    let log = f.read_stats_log("game");
    assert!(log.contains("\"seconds\":3600"), "{log}");
    assert_eq!(log.lines().count(), 1, "{log}");

    heroic_config(&f, "abc", 120, "2026-09-25T09:00:00.000Z");
    let second = f.run(&["stats", "--import-heroic"]);
    assert_success(&second);
    assert!(
        stdout(&second).contains("game: 2 h from Heroic"),
        "{}",
        stdout(&second)
    );
    // The log is append-only, so importing again adds a second `imported`
    // line rather than rewriting the first.
    let log = f.read_stats_log("game");
    assert!(log.contains("\"seconds\":3600"), "{log}");
    assert!(log.contains("\"seconds\":7200"), "{log}");
    assert_eq!(
        log.lines().filter(|line| line.contains("imported")).count(),
        2,
        "{log}"
    );

    // But the summary uses the last one.
    let named = f.run(&["stats", "game", "--json"]);
    assert_success(&named);
    let json: serde_json::Value = serde_json::from_slice(&stdout(&named).into_bytes()).unwrap();
    assert_eq!(json["imported_seconds"], 7200);
}

#[test]
fn json_shapes_match_the_named_object_and_the_collection_envelope() {
    let f = Fixture::new();
    f.environment("game", "");
    f.write_stats_log(
        "game",
        &session(
            "game",
            "2026-09-12T20:14:00Z",
            "2026-09-12T21:14:00Z",
            3600,
            "exited",
        ),
    );

    let named = f.run(&["stats", "game", "--json"]);
    assert_success(&named);
    let json: serde_json::Value = serde_json::from_slice(&stdout(&named).into_bytes()).unwrap();
    assert_eq!(json["name"], "game");
    assert_eq!(json["seconds"], 3600);
    assert_eq!(json["sessions"], 1);
    assert_eq!(json["average_seconds"], 3600);
    assert_eq!(json["running"], false);
    assert_eq!(json["imported_seconds"], 0);
    assert!(json.get("sessions_log").is_none());

    let with_sessions = f.run(&["stats", "game", "--json", "--sessions"]);
    assert_success(&with_sessions);
    let json: serde_json::Value =
        serde_json::from_slice(&stdout(&with_sessions).into_bytes()).unwrap();
    assert_eq!(json["sessions_log"].as_array().unwrap().len(), 1);

    let collection = f.run(&["stats", "--json"]);
    assert_success(&collection);
    let json: serde_json::Value =
        serde_json::from_slice(&stdout(&collection).into_bytes()).unwrap();
    let environments = json["environments"].as_array().unwrap();
    assert_eq!(environments.len(), 1);
    assert_eq!(environments[0]["name"], "game");
    assert_eq!(json["errors"], serde_json::json!([]));
}
