//! `wineo stats`: the overview, one environment's detail, and their JSON.

mod common;

use std::fs;

use common::{Fixture, assert_success, stderr, stdout};

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
fn import_heroic_is_fail_soft_and_exits_1_if_any_environment_failed() {
    let f = Fixture::new();
    f.environment("good", "");
    f.environment("bad", "");
    for name in ["good", "bad"] {
        let config = f.config_dir().join(format!("environments/{name}.toml"));
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, format!("heroic_id = \"{name}-id\"\n{text}")).unwrap();
    }
    heroic_config(&f, "good-id", 60, "2026-09-20T19:02:11.000Z");
    // Both games have playtime, so both try to write their imported line.
    let heroic = f.config_dir().join("../heroic");
    fs::write(
        heroic.join("store/timestamp.json"),
        r#"{"good-id":{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"2026-09-20T19:02:11.000Z","totalPlayed":60},"bad-id":{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"2026-09-20T19:02:11.000Z","totalPlayed":90}}"#,
    )
    .unwrap();
    // A directory in place of "bad"'s log file makes writing it fail.
    fs::create_dir_all(f.stats_log("bad")).unwrap();

    let output = f.run(&["stats", "--import-heroic"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("good: 1 h from Heroic"),
        "{}",
        stdout(&output)
    );
    assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
    // "good" still got its import despite "bad" failing.
    assert!(f.read_stats_log("good").contains("\"seconds\":3600"));
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

#[test]
fn sessions_is_refused_without_a_name_or_without_json() {
    let f = Fixture::new();
    f.environment("game", "");

    let without_name = f.run(&["stats", "--json", "--sessions"]);
    assert!(!without_name.status.success());
    assert!(
        stderr(&without_name).contains("required"),
        "{}",
        stderr(&without_name)
    );

    let without_json = f.run(&["stats", "game", "--sessions"]);
    assert!(!without_json.status.success());
    assert!(
        stderr(&without_json).contains("required"),
        "{}",
        stderr(&without_json)
    );
}

#[test]
fn import_heroic_conflicts_with_name_json_and_sessions() {
    let f = Fixture::new();
    f.environment("game", "");

    for args in [
        &["stats", "--import-heroic", "game"][..],
        &["stats", "--import-heroic", "--json"][..],
        &["stats", "--import-heroic", "--json", "--sessions", "game"][..],
    ] {
        let output = f.run(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr(&output).contains("cannot be used with"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

fn tagged_library() -> Fixture {
    let f = Fixture::new();
    f.environment("hades", "tags = ['game', 'co-op']\n");
    f.environment("celeste", "tags = ['game']\n");
    f.environment("never", "tags = ['game']\n");
    f.environment("editor", "tags = ['work']\n");
    f.environment("office", "tags = ['work']\n");
    f.environment("browser", "");
    f.write_stats_log(
        "hades",
        &session(
            "game",
            "2026-09-04T18:00:00Z",
            "2026-09-04T19:00:00Z",
            3600,
            "exited",
        ),
    );
    f.write_stats_log(
        "celeste",
        "{\"imported\":{\"seconds\":7200,\"last_played\":\"2026-09-05T18:00:00Z\"}}\n",
    );
    f.write_stats_log(
        "browser",
        &session(
            "app",
            "2026-09-17T18:00:00Z",
            "2026-09-17T23:00:00Z",
            18_000,
            "stopped",
        ),
    );
    f
}

#[test]
fn tag_filter_prints_the_overview_with_filtered_totals() {
    let f = tagged_library();
    let output = f.run(&["stats", "--tag", "game"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        concat!(
            "         played  sessions  last played\n",
            "celeste  2 h            0  Sep 5\n",
            "hades    1 h            1  Sep 4\n",
            "1 never played. Total 3 h.\n",
        )
    );
    let both = f.run(&["stats", "--tag", "game", "--tag", "co-op"]);
    assert_success(&both);
    assert!(stdout(&both).contains("0 never played. Total 1 h."));
    assert!(!stdout(&both).contains("celeste"));
}

#[test]
fn by_tag_prints_membership_sorted_totals_and_the_untagged_row_last() {
    let f = tagged_library();
    let output = f.run(&["stats", "--by-tag"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        concat!(
            "            played  sessions  played by  last played\n",
            "game        3 h            1  2 of 3     Sep 5\n",
            "co-op       1 h            1  1 of 1     Sep 4\n",
            "work        0 m            0  0 of 2\n",
            "(untagged)  5 h            1  1 of 1     Sep 17\n",
            "An environment with several tags counts under each.\n",
        )
    );
}

#[test]
fn by_tag_filter_limits_membership_and_the_note_to_included_environments() {
    let f = tagged_library();
    let work = f.run(&["stats", "--by-tag", "--tag", "work"]);
    assert_success(&work);
    assert_eq!(
        stdout(&work),
        concat!(
            "      played  sessions  played by  last played\n",
            "work  0 m            0  0 of 2\n",
        )
    );
    let game = f.run(&["stats", "--by-tag", "--tag", "game"]);
    assert_success(&game);
    let text = stdout(&game);
    assert!(text.contains("co-op"), "{text}");
    assert!(
        text.contains("An environment with several tags counts under each."),
        "{text}"
    );
    assert!(!text.contains("work"), "{text}");
    assert!(!text.contains("(untagged)"), "{text}");
}

#[test]
fn tag_filter_with_no_match_exits_successfully_and_joins_tags() {
    let f = tagged_library();
    for extra in [vec![], vec!["--by-tag"]] {
        let mut args = vec!["stats", "--tag", "game", "--tag", "work"];
        args.extend(extra);
        let output = f.run(&args);
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            "No environments are tagged game and work\n"
        );
    }
    let single = f.run(&["stats", "--tag", "absent"]);
    assert_success(&single);
    assert_eq!(stdout(&single), "No environments are tagged absent\n");
}

fn json_output(output: &std::process::Output) -> serde_json::Value {
    assert_success(output);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn tag_json_filters_the_existing_envelope_and_requires_all_tags() {
    let f = tagged_library();
    let json = json_output(&f.run(&["stats", "--tag", "game", "--json"]));
    assert_eq!(
        json["environments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["celeste", "hades", "never"]
    );
    assert_eq!(json["errors"], serde_json::json!([]));
    assert!(json.get("tags").is_none());
    let both = json_output(&f.run(&["stats", "--tag", "game", "--tag", "co-op", "--json"]));
    assert_eq!(both["environments"].as_array().unwrap().len(), 1);
    assert_eq!(both["environments"][0]["name"], "hades");
    let empty = json_output(&f.run(&["stats", "--tag", "absent", "--json"]));
    assert_eq!(empty, serde_json::json!({"environments": [], "errors": []}));
}

#[test]
fn by_tag_json_matches_the_planned_shape_and_null_untagged_bucket() {
    let f = tagged_library();
    let json = json_output(&f.run(&["stats", "--by-tag", "--json"]));
    assert_eq!(
        json,
        serde_json::json!({
            "tags": [
                {"tag": "game", "seconds": 10800, "sessions": 1, "environments": 3, "played_environments": 2, "last_played": "2026-09-05T18:00:00Z"},
                {"tag": "co-op", "seconds": 3600, "sessions": 1, "environments": 1, "played_environments": 1, "last_played": "2026-09-04T18:00:00Z"},
                {"tag": "work", "seconds": 0, "sessions": 0, "environments": 2, "played_environments": 0, "last_played": null}
            ],
            "untagged": {"seconds": 18000, "sessions": 1, "environments": 1, "played_environments": 1, "last_played": "2026-09-17T18:00:00Z"},
            "errors": []
        })
    );
    let filtered = json_output(&f.run(&[
        "stats", "--by-tag", "--tag", "game", "--tag", "co-op", "--json",
    ]));
    assert_eq!(filtered["untagged"], serde_json::Value::Null);
    assert_eq!(filtered["tags"][0]["tag"], "co-op");
    assert_eq!(filtered["tags"][1]["tag"], "game");
    assert_eq!(filtered["tags"][0]["environments"], 1);
    let empty = json_output(&f.run(&["stats", "--by-tag", "--tag", "absent", "--json"]));
    assert_eq!(
        empty,
        serde_json::json!({"tags": [], "untagged": null, "errors": []})
    );
}

#[test]
fn tag_flags_conflict_with_name_import_heroic_and_sessions() {
    let f = Fixture::new();
    for flags in [vec!["--tag", "game"], vec!["--by-tag"]] {
        for conflicting in [
            vec!["game"],
            vec!["--import-heroic"],
            vec!["--sessions", "--json", "game"],
        ] {
            let mut args = vec!["stats"];
            args.extend(&flags);
            args.extend(conflicting);
            let output = f.run(&args);
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            assert!(
                stderr(&output).contains("cannot be used with"),
                "{args:?}: {}",
                stderr(&output)
            );
        }
    }
}

#[test]
fn tag_queries_report_broken_configs_and_keep_readable_results() {
    let f = tagged_library();
    f.environment("broken", "");
    fs::write(
        f.config_dir().join("environments/broken.toml"),
        "invalid = [",
    )
    .unwrap();
    let plain = f.run(&["stats", "--json"]);
    assert_success(&plain);
    assert!(
        json_output(&plain)["environments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "broken")
    );
    for flags in [vec!["--tag", "game"], vec!["--by-tag"]] {
        let mut args = vec!["stats"];
        args.extend(&flags);
        let text = f.run(&args);
        assert_eq!(text.status.code(), Some(1));
        assert!(stderr(&text).contains("broken.toml"), "{}", stderr(&text));
        assert!(stdout(&text).contains("game") || stdout(&text).contains("hades"));
        args.push("--json");
        let output = f.run(&args);
        assert_eq!(output.status.code(), Some(1));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["errors"].as_array().unwrap().len(), 1);
        assert_eq!(json["errors"][0]["name"], "broken");
        assert!(
            json["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("broken.toml")
        );
    }
}
