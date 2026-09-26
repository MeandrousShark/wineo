//! `wineo stats`: the overview, one environment's detail, and their JSON.

mod common;

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
