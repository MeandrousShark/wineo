use std::ffi::OsString;

use jiff::tz::TimeZone;

use super::*;

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn session(target: &str, start: &str, end: &str, kind: End) -> Session {
    Session::new(target.to_owned(), ts(start), ts(end), kind)
}

#[test]
fn append_and_read_round_trip_sessions() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("stats/game.jsonl");
    let a = session(
        "game",
        "2026-09-20T19:02:11Z",
        "2026-09-20T23:53:31Z",
        End::Exited,
    );
    let b = session(
        "game",
        "2026-09-21T10:00:00Z",
        "2026-09-21T10:00:05Z",
        End::Failed,
    );
    append(&path, &a).unwrap();
    append(&path, &b).unwrap();
    let log = read(&path).unwrap();
    assert_eq!(log.sessions, vec![a, b]);
    assert_eq!(log.imported, None);
    assert!(log.warnings.is_empty());
}

#[test]
fn a_missing_log_reads_as_empty() {
    let root = tempfile::tempdir().unwrap();
    let log = read(&root.path().join("stats/absent.jsonl")).unwrap();
    assert_eq!(log, Log::default());
}

#[test]
fn a_torn_last_line_is_skipped_with_a_warning() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("game.jsonl");
    let good = session(
        "game",
        "2026-09-20T19:00:00Z",
        "2026-09-20T20:00:00Z",
        End::Exited,
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "{}\n{{\"target\":\"game\",\"started_at\":\"2026-09-21T09:00:00Z\"",
            serde_json::to_string(&good).unwrap()
        ),
    )
    .unwrap();
    let log = read(&path).unwrap();
    assert_eq!(log.sessions, vec![good]);
    assert_eq!(log.warnings.len(), 1);
    assert!(log.warnings[0].contains("line 2"));
}

#[test]
fn unknown_fields_are_ignored() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("game.jsonl");
    fs::write(
            &path,
            "{\"target\":\"game\",\"started_at\":\"2026-09-20T19:00:00Z\",\"ended_at\":\"2026-09-20T20:00:00Z\",\"seconds\":3600,\"end\":\"exited\",\"new_field\":42}\n",
        )
        .unwrap();
    let log = read(&path).unwrap();
    assert_eq!(log.sessions.len(), 1);
    assert!(log.warnings.is_empty());
}

#[test]
fn importing_twice_appends_another_line_and_the_summary_uses_the_last() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("game.jsonl");
    let before = session(
        "game",
        "2026-09-20T19:00:00Z",
        "2026-09-20T20:00:00Z",
        End::Exited,
    );
    let after = session(
        "game",
        "2026-09-22T19:00:00Z",
        "2026-09-22T20:00:00Z",
        End::Exited,
    );
    append(&path, &before).unwrap();
    append_line(
        &path,
        &ImportedLine {
            imported: Imported {
                seconds: 342_000,
                last_played: Some(ts("2026-06-01T10:00:00Z")),
            },
        },
    )
    .unwrap();
    append(&path, &after).unwrap();
    append_line(
        &path,
        &ImportedLine {
            imported: Imported {
                seconds: 400_000,
                last_played: None,
            },
        },
    )
    .unwrap();

    // Append-only: both imported lines are still on disk, with the
    // sessions on either side of them kept too.
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 4, "{text}");
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("imported"))
            .count(),
        2,
        "{text}"
    );

    let log = read(&path).unwrap();
    assert_eq!(log.sessions, vec![before, after]);
    // The last `imported` line wins.
    assert_eq!(
        log.imported,
        Some(Imported {
            seconds: 400_000,
            last_played: None,
        })
    );
}

#[test]
fn summary_of_an_empty_log_has_nothing() {
    let summary = summarize(
        &Log::default(),
        None,
        ts("2026-09-26T12:00:00Z"),
        &TimeZone::UTC,
    );
    assert_eq!(summary.seconds, 0);
    assert_eq!(summary.sessions, 0);
    assert_eq!(summary.average_seconds, None);
    assert_eq!(summary.longest, None);
    assert_eq!(summary.first_played, None);
    assert_eq!(summary.last_played, None);
    assert!(!summary.running);
    assert!(summary.targets.is_empty());
}

#[test]
fn summary_adds_totals_average_and_the_longest_session() {
    let log = Log {
        sessions: vec![
            session(
                "game",
                "2026-09-12T20:14:00Z",
                "2026-09-12T21:14:00Z",
                End::Exited,
            ),
            session(
                "game",
                "2026-09-20T19:02:11Z",
                "2026-09-20T23:53:31Z",
                End::Exited,
            ),
            session(
                "editor",
                "2026-09-25T09:00:00Z",
                "2026-09-25T09:13:00Z",
                End::Stopped,
            ),
        ],
        imported: None,
        warnings: Vec::new(),
    };
    let now = ts("2026-09-26T12:00:00Z");
    let summary = summarize(&log, None, now, &TimeZone::UTC);
    assert_eq!(summary.sessions, 3);
    assert_eq!(summary.seconds, 3600 + 17480 + 780);
    assert_eq!(summary.average_seconds, Some((3600 + 17480 + 780) / 3));
    assert_eq!(
        summary.longest,
        Some(Longest {
            seconds: 17480,
            started_at: ts("2026-09-20T19:02:11Z"),
        })
    );
    assert_eq!(summary.first_played, Some(ts("2026-09-12T20:14:00Z")));
    assert_eq!(summary.last_played, Some(ts("2026-09-25T09:00:00Z")));
    assert_eq!(
        summary.targets,
        vec![
            TargetSummary {
                target: "game".to_owned(),
                seconds: 3600 + 17480,
                sessions: 2,
            },
            TargetSummary {
                target: "editor".to_owned(),
                seconds: 780,
                sessions: 1,
            },
        ]
    );
}

#[test]
fn a_running_session_counts_toward_the_numbers_so_far() {
    let log = Log::default();
    let now = ts("2026-09-26T12:00:00Z");
    let running = Running {
        target: "game".to_owned(),
        started_at: ts("2026-09-26T10:00:00Z"),
    };
    let summary = summarize(&log, Some(&running), now, &TimeZone::UTC);
    assert!(summary.running);
    assert_eq!(summary.sessions, 1);
    assert_eq!(summary.seconds, 7200);
    assert_eq!(summary.last_played, Some(running.started_at));
    assert_eq!(summary.longest.unwrap().seconds, 7200);
}

#[test]
fn imported_time_counts_toward_the_total_but_not_sessions_or_average() {
    let log = Log {
        sessions: vec![session(
            "game",
            "2026-09-20T19:00:00Z",
            "2026-09-20T20:00:00Z",
            End::Exited,
        )],
        imported: Some(Imported {
            seconds: 342_000,
            last_played: Some(ts("2026-06-01T10:00:00Z")),
        }),
        warnings: Vec::new(),
    };
    let now = ts("2026-09-26T12:00:00Z");
    let summary = summarize(&log, None, now, &TimeZone::UTC);
    assert_eq!(summary.sessions, 1);
    assert_eq!(summary.average_seconds, Some(3600));
    assert_eq!(summary.imported_seconds, 342_000);
    assert_eq!(summary.seconds, 3600 + 342_000);
    assert_eq!(summary.last_played, Some(ts("2026-09-20T19:00:00Z")));
}

#[test]
fn heroic_last_played_counts_when_it_is_the_latest() {
    let log = Log {
        sessions: Vec::new(),
        imported: Some(Imported {
            seconds: 342_000,
            last_played: Some(ts("2026-06-01T10:00:00Z")),
        }),
        warnings: Vec::new(),
    };
    let summary = summarize(&log, None, ts("2026-09-26T12:00:00Z"), &TimeZone::UTC);
    assert_eq!(summary.last_played, Some(ts("2026-06-01T10:00:00Z")));
    assert_eq!(summary.first_played, None);
}

#[test]
fn this_week_and_month_bucket_by_local_start_date() {
    // A Saturday: the week starts on Monday the 21st, the month on the 1st.
    let now = ts("2026-09-26T12:00:00Z");
    let log = Log {
        sessions: vec![
            session(
                "game",
                "2026-09-05T10:00:00Z",
                "2026-09-05T11:00:00Z",
                End::Exited,
            ),
            session(
                "game",
                "2026-09-22T10:00:00Z",
                "2026-09-22T11:30:00Z",
                End::Exited,
            ),
            session(
                "game",
                "2026-09-26T08:00:00Z",
                "2026-09-26T08:15:00Z",
                End::Exited,
            ),
        ],
        imported: None,
        warnings: Vec::new(),
    };
    let summary = summarize(&log, None, now, &TimeZone::UTC);
    assert_eq!(summary.this_month_seconds, 3600 + 5400 + 900);
    assert_eq!(summary.this_week_seconds, 5400 + 900);
}

#[test]
fn overview_keeps_name_order_and_sorts_played_rows() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    for name in ["alpha", "beta", "never", "zeta"] {
        fs::write(paths.environments_dir().join(format!("{name}.toml")), "").unwrap();
    }
    for (name, seconds) in [("alpha", 3600), ("beta", 7200), ("zeta", 7200)] {
        let start = ts("2026-09-20T19:00:00Z");
        append(
            &paths.stats_log(name),
            &Session {
                target: "game".to_owned(),
                started_at: start,
                ended_at: start + jiff::SignedDuration::from_secs(seconds),
                seconds,
                end: End::Exited,
            },
        )
        .unwrap();
    }

    let overview = super::overview(&paths).unwrap();
    assert_eq!(
        overview
            .rows
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta", "never", "zeta"]
    );
    assert_eq!(overview.played, [1, 3, 0]);
    assert_eq!(overview.never_played, 1);
    assert_eq!(overview.total_seconds, 18_000);
    assert!(overview.errors.is_empty());
}

/// Steam under `paths`' home, signed in as account 1, with these
/// `(app id, minutes)` played.
fn steam_played(paths: &Paths, played: &[(&str, u64)]) {
    let root = paths.home().join(".local/share/Steam");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("userdata/1/config")).unwrap();
    fs::write(
        root.join("config/loginusers.vdf"),
        "\"users\" { \"76561197960265729\" { \"MostRecent\" \"1\" } }",
    )
    .unwrap();
    let apps: String = played
        .iter()
        .map(|(app_id, minutes)| {
            format!("\"{app_id}\" {{ \"Playtime\" \"{minutes}\" \"LastPlayed\" \"5\" }} ")
        })
        .collect();
    fs::write(
            root.join("userdata/1/config/localconfig.vdf"),
            format!(
                "\"UserLocalConfigStore\" {{ \"Software\" {{ \"Valve\" {{ \"Steam\" {{ \"apps\" {{ {apps} }} }} }} }} }}"
            ),
        )
        .unwrap();
}

#[test]
fn an_overview_reads_each_file_once_however_many_steam_games_it_has() {
    for options in [
        OverviewOptions::default(),
        OverviewOptions {
            load_tags: true,
            ..OverviewOptions::default()
        },
    ] {
        let root = tempfile::tempdir().unwrap();
        let paths = tag_paths(root.path());
        fs::create_dir_all(paths.environments_dir()).unwrap();
        for (name, app_id) in [("one", "10"), ("two", "20"), ("three", "30")] {
            fs::write(
                paths.environment_config(name),
                format!("kind = 'proxied'\nlauncher = 'steam'\napp_id = '{app_id}'\n"),
            )
            .unwrap();
        }
        tagged_environment(&paths, "wine", &[]);
        steam_played(&paths, &[("10", 60), ("20", 120)]);

        crate::read_log::clear();
        let overview = overview_with_options(&paths, options).unwrap();

        let seconds: Vec<_> = overview
            .rows
            .iter()
            .map(|(name, summary)| (name.as_str(), summary.seconds, summary.source))
            .collect();
        assert_eq!(
            seconds,
            [
                ("one", 3600, Source::Steam),
                ("three", 0, Source::Steam),
                ("two", 7200, Source::Steam),
                ("wine", 0, Source::Wineo),
            ]
        );
        assert_eq!(overview.steam_games, 3);
        // Steam's files and the global config are read for the whole
        // overview, and each environment's config once.
        for (file, reads) in [
            ("loginusers.vdf", 1),
            ("localconfig.vdf", 1),
            ("config.toml", 1),
            ("one.toml", 1),
            ("two.toml", 1),
            ("three.toml", 1),
            ("wine.toml", 1),
        ] {
            assert_eq!(
                crate::read_log::count(file),
                reads,
                "{file} with load_tags {}",
                options.load_tags
            );
        }
    }
}

fn tag_paths(root: &Path) -> Paths {
    let home = root.join("home");
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

fn tagged_environment(paths: &Paths, name: &str, tags: &[&str]) {
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config(name),
        format!("kind = 'native'\ntags = {tags:?}\n"),
    )
    .unwrap();
}

#[test]
fn tag_breakdown_groups_membership_playtime_and_imports() {
    let root = tempfile::tempdir().unwrap();
    let paths = tag_paths(root.path());
    for (name, tags) in [
        ("hades", vec!["game", "co-op"]),
        ("celeste", vec!["game"]),
        ("never", vec!["game"]),
        ("editor", vec!["work"]),
        ("browser", vec![]),
        ("untouched", vec![]),
    ] {
        tagged_environment(&paths, name, &tags);
    }
    let start = ts("2026-09-20T19:00:00Z");
    append(
        &paths.stats_log("hades"),
        &session(
            "game",
            "2026-09-20T19:00:00Z",
            "2026-09-20T20:00:00Z",
            End::Exited,
        ),
    )
    .unwrap();
    set_imported(
        &paths,
        "celeste",
        Imported {
            seconds: 7200,
            last_played: Some(start),
        },
    )
    .unwrap();
    set_imported(
        &paths,
        "browser",
        Imported {
            seconds: 18_000,
            last_played: None,
        },
    )
    .unwrap();

    let overview = overview_with_options(
        &paths,
        OverviewOptions {
            load_tags: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(overview.errors.is_empty());
    let breakdown = by_tag(&overview);
    assert_eq!(
        breakdown
            .tags
            .iter()
            .map(|(tag, _)| tag.as_str())
            .collect::<Vec<_>>(),
        ["game", "co-op", "work"]
    );
    assert_eq!(
        breakdown.tags[0].1,
        TagTotals {
            seconds: 10_800,
            sessions: 1,
            environments: 3,
            played_environments: 2,
            last_played: Some(start)
        }
    );
    assert_eq!(
        breakdown.tags[1].1,
        TagTotals {
            seconds: 3600,
            sessions: 1,
            environments: 1,
            played_environments: 1,
            last_played: Some(start)
        }
    );
    assert_eq!(
        breakdown.tags[2].1,
        TagTotals {
            environments: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        breakdown.untagged,
        Some(TagTotals {
            seconds: 18_000,
            sessions: 0,
            environments: 2,
            played_environments: 1,
            last_played: None
        })
    );
}

#[test]
fn tag_loads_share_a_trash_scan_but_imports_check_each_time() {
    let root = tempfile::tempdir().unwrap();
    let paths = tag_paths(root.path());
    for index in 0..5 {
        let name = format!("gone-{index}");
        tagged_environment(&paths, &name, &["game"]);
        crate::trash::remove(&paths, &name, false).unwrap();
    }
    for index in 0..4 {
        tagged_environment(&paths, &format!("kept-{index}"), &["game"]);
    }
    let tags = vec!["game".to_owned()];
    for options in [
        OverviewOptions {
            tags: Some(&tags),
            load_tags: false,
        },
        OverviewOptions {
            tags: None,
            load_tags: true,
        },
    ] {
        crate::trash::take_manifest_reads();
        let overview = overview_with_options(&paths, options).unwrap();
        assert_eq!(overview.rows.len(), 4);
        assert!(overview.errors.is_empty());
        assert_eq!(crate::trash::take_manifest_reads(), 5);
    }
    // Imports write, so each one checks the trash fresh.
    crate::trash::take_manifest_reads();
    assert!(import_heroic_all(&paths, &[]).unwrap().is_empty());
    assert_eq!(crate::trash::take_manifest_reads(), 4 * 5);
}

#[test]
fn tag_filter_requires_every_tag_and_breakdown_sorts_ties_by_name() {
    let root = tempfile::tempdir().unwrap();
    let paths = tag_paths(root.path());
    tagged_environment(&paths, "both", &["game", "co-op"]);
    tagged_environment(&paths, "game-only", &["game"]);
    let tags = vec!["game".to_owned(), "co-op".to_owned()];
    let overview = overview_with_options(
        &paths,
        OverviewOptions {
            tags: Some(&tags),
            load_tags: false,
        },
    )
    .unwrap();
    assert_eq!(overview.rows.len(), 1);
    assert_eq!(overview.rows[0].0, "both");
    assert_eq!(overview.never_played, 1);
    assert_eq!(overview.total_seconds, 0);
    let breakdown = by_tag(&overview);
    assert_eq!(
        breakdown
            .tags
            .iter()
            .map(|(tag, _)| tag.as_str())
            .collect::<Vec<_>>(),
        ["co-op", "game"]
    );
    assert!(breakdown.untagged.is_none());
}

#[test]
fn configs_are_required_only_for_tag_queries() {
    let root = tempfile::tempdir().unwrap();
    let paths = tag_paths(root.path());
    tagged_environment(&paths, "broken", &[]);
    fs::write(paths.environment_config("broken"), "invalid = [").unwrap();
    let plain = overview(&paths).unwrap();
    assert_eq!(plain.rows.len(), 1);
    assert!(plain.errors.is_empty());
    assert!(plain.tags.is_empty());
    let tags = vec!["game".to_owned()];
    for options in [
        OverviewOptions {
            tags: Some(&tags),
            load_tags: false,
        },
        OverviewOptions {
            tags: None,
            load_tags: true,
        },
    ] {
        let overview = overview_with_options(&paths, options).unwrap();
        assert!(overview.rows.is_empty());
        assert_eq!(overview.errors.len(), 1);
        assert_eq!(overview.errors[0].name, "broken");
        assert!(overview.errors[0].message.contains("broken.toml"));
    }
}

#[test]
fn tag_breakdown_includes_a_running_session() {
    let root = tempfile::tempdir().unwrap();
    let paths = tag_paths(root.path());
    tagged_environment(&paths, "live", &["game"]);
    let runtime = Runtime::new(&paths, "live");
    let _lock = runtime.try_lock().unwrap().unwrap();
    let mut run = crate::runtime::RunState::running(
        crate::runtime::RunMode::Detached,
        "game".to_owned(),
        std::process::id(),
        None,
        &Default::default(),
    );
    run.started_at = Timestamp::now() - jiff::SignedDuration::from_secs(7200);
    runtime.write_state(&run).unwrap();
    let overview = overview_with_options(
        &paths,
        OverviewOptions {
            load_tags: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(overview.rows[0].1.running);
    let breakdown = by_tag(&overview);
    assert_eq!(
        breakdown.tags[0].1,
        TagTotals {
            seconds: overview.now.duration_since(run.started_at).as_secs(),
            sessions: 1,
            environments: 1,
            played_environments: 1,
            last_played: Some(run.started_at)
        }
    );
}

#[test]
fn last_session_duration_uses_the_session_named_by_last_played() {
    let now = ts("2026-09-26T12:00:00Z");
    let log = Log {
        sessions: vec![
            session(
                "game",
                "2026-09-24T10:00:00Z",
                "2026-09-24T11:00:00Z",
                End::Exited,
            ),
            session(
                "game",
                "2026-09-25T10:00:00Z",
                "2026-09-25T10:30:00Z",
                End::Exited,
            ),
        ],
        ..Log::default()
    };
    let summary = summarize(&log, None, now, &TimeZone::UTC);
    assert_eq!(last_session_seconds(&log, &summary, now), Some(1800));

    let running = Running {
        target: "game".to_owned(),
        started_at: ts("2026-09-26T11:45:00Z"),
    };
    let summary = summarize(&log, Some(&running), now, &TimeZone::UTC);
    assert_eq!(last_session_seconds(&log, &summary, now), Some(900));
}

#[test]
fn copied_log_never_replaces_one_already_there() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let lookup = move |key: &str| (key == "HOME").then(|| OsString::from(&home));
    let from = Paths::select(lookup.clone(), None).unwrap();
    let to = Paths::select(lookup, Some(&root.path().join("usb"))).unwrap();
    fs::create_dir_all(from.stats_log("game").parent().unwrap()).unwrap();
    fs::write(from.stats_log("game"), "source\n").unwrap();
    fs::create_dir_all(to.stats_log("game").parent().unwrap()).unwrap();
    fs::write(to.stats_log("game"), "orphan\n").unwrap();
    assert!(copy_log(&from, &to, "game").is_err());
    assert_eq!(
        fs::read_to_string(to.stats_log("game")).unwrap(),
        "orphan\n"
    );
    assert_eq!(
        fs::read_dir(to.stats_log("game").parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

fn library_paths(root: &Path, names: &[&str]) -> Paths {
    let home = root.join("home");
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    for name in names {
        fs::write(paths.environments_dir().join(format!("{name}.toml")), "").unwrap();
    }
    paths
}

fn append_session(paths: &Paths, name: &str, target: &str, start: &str, seconds: i64) {
    let started_at = ts(start);
    append(
        &paths.stats_log(name),
        &Session::new(
            target.to_owned(),
            started_at,
            started_at + jiff::SignedDuration::from_secs(seconds),
            End::Exited,
        ),
    )
    .unwrap();
}

/// Everything an overview holds except the clock it was taken at.
fn same_overview(a: &Overview, b: &Overview) {
    assert_eq!(a.rows, b.rows);
    assert_eq!(a.tags, b.tags);
    assert_eq!(a.played, b.played);
    assert_eq!(a.total_seconds, b.total_seconds);
    assert_eq!(a.never_played, b.never_played);
    assert_eq!(a.steam_games, b.steam_games);
    assert_eq!(a.unknown_steam_games, b.unknown_steam_games);
    assert_eq!(a.warnings, b.warnings);
    assert_eq!(a.errors, b.errors);
}

fn seconds_of(overview: &Overview, name: &str) -> i64 {
    overview
        .rows
        .iter()
        .find(|(row, _)| row == name)
        .unwrap()
        .1
        .seconds
}

#[test]
fn a_cached_overview_equals_the_uncached_one() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha", "beta", "never", "torn"]);
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    append_session(&paths, "alpha", "tool", "2026-09-21T19:00:00Z", 60);
    append_session(&paths, "beta", "game", "2026-09-22T19:00:00Z", 7200);
    append_session(&paths, "torn", "game", "2026-09-22T19:00:00Z", 100);
    let mut torn = fs::read_to_string(paths.stats_log("torn")).unwrap();
    torn.push_str("{\"target\":\n");
    fs::write(paths.stats_log("torn"), torn).unwrap();

    let mut cache = Cache::new();
    for _ in 0..2 {
        let cached = overview_cached(&paths, &mut cache).unwrap();
        let plain = overview(&paths).unwrap();
        same_overview(&cached, &plain);
        assert_eq!(cached.warnings.len(), 1);
    }
    // Three logs exist; "never" has no file to cache.
    assert_eq!(cache.logs.len(), 3);
}

#[test]
fn an_unchanged_log_is_not_parsed_again() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha", "beta"]);
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    append_session(&paths, "beta", "game", "2026-09-20T19:00:00Z", 60);

    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(cache.parses, 2);
    overview_cached(&paths, &mut cache).unwrap();
    overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(cache.parses, 2);
}

#[test]
fn an_appended_session_is_parsed_and_counted() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha", "beta"]);
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    append_session(&paths, "beta", "game", "2026-09-20T19:00:00Z", 60);
    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();

    append_session(&paths, "alpha", "game", "2026-09-21T19:00:00Z", 1800);
    let after = overview_cached(&paths, &mut cache).unwrap();
    // Only the log that grew is read again.
    assert_eq!(cache.parses, 3);
    assert_eq!(seconds_of(&after, "alpha"), 5400);
    assert_eq!(seconds_of(&after, "beta"), 60);
    same_overview(&after, &overview(&paths).unwrap());
}

#[test]
fn a_log_cut_back_to_an_earlier_size_is_parsed_again() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha"]);
    let log = paths.stats_log("alpha");
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    let short = fs::metadata(&log).unwrap().len();
    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();
    append_session(&paths, "alpha", "game", "2026-09-21T19:00:00Z", 1800);
    assert_eq!(
        seconds_of(&overview_cached(&paths, &mut cache).unwrap(), "alpha"),
        5400
    );

    fs::OpenOptions::new()
        .write(true)
        .open(&log)
        .unwrap()
        .set_len(short)
        .unwrap();
    let cut = overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(cache.parses, 3);
    assert_eq!(seconds_of(&cut, "alpha"), 3600);
}

#[test]
fn a_removed_environment_drops_its_entry() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha", "beta"]);
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    append_session(&paths, "beta", "game", "2026-09-20T19:00:00Z", 60);
    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(cache.logs.len(), 2);

    fs::remove_file(paths.environment_config("beta")).unwrap();
    fs::remove_file(paths.stats_log("beta")).unwrap();
    let after = overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(after.rows.len(), 1);
    assert_eq!(
        cache.logs.keys().collect::<Vec<_>>(),
        [&paths.stats_log("alpha")]
    );
}

#[test]
fn a_filtered_overview_keeps_the_entries_it_skipped() {
    let root = tempfile::tempdir().unwrap();
    let paths = tag_paths(root.path());
    tagged_environment(&paths, "alpha", &[]);
    tagged_environment(&paths, "beta", &["keep"]);
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    append_session(&paths, "beta", "game", "2026-09-20T19:00:00Z", 60);
    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();

    let filter = ["keep".to_owned()];
    let options = OverviewOptions {
        tags: Some(&filter),
        load_tags: false,
    };
    let filtered = overview_inner(&paths, options, Some(&mut cache), Timestamp::now()).unwrap();
    assert_eq!(filtered.rows.len(), 1);
    assert_eq!(cache.logs.len(), 2);
}

/// Rewrites `path` with `contents` and puts its mtime back, as `touch -d`
/// or a copy that keeps times would.
fn rewrite_keeping_mtime(path: &Path, contents: &str, mtime: std::time::SystemTime) {
    fs::write(path, contents).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn a_same_size_rewrite_is_caught() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha"]);
    let log = paths.stats_log("alpha");
    append_session(&paths, "alpha", "gamea", "2026-09-20T19:00:00Z", 3600);
    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();

    let text = fs::read_to_string(&log).unwrap();
    let mtime = fs::metadata(&log).unwrap().modified().unwrap();
    rewrite_keeping_mtime(&log, &text.replace("gamea", "gameb"), mtime);
    assert_eq!(fs::metadata(&log).unwrap().len(), text.len() as u64);
    let after = overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(cache.parses, 2);
    let (_, summary) = &after.rows[0];
    assert_eq!(summary.targets[0].target, "gameb");
}

#[test]
fn a_rename_over_the_log_is_caught() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha"]);
    let log = paths.stats_log("alpha");
    append_session(&paths, "alpha", "gamea", "2026-09-20T19:00:00Z", 3600);
    let mut cache = Cache::new();
    overview_cached(&paths, &mut cache).unwrap();

    let text = fs::read_to_string(&log).unwrap();
    let mtime = fs::metadata(&log).unwrap().modified().unwrap();
    let staged = log.with_extension("new");
    rewrite_keeping_mtime(&staged, &text.replace("gamea", "gameb"), mtime);
    fs::rename(&staged, &log).unwrap();
    let after = overview_cached(&paths, &mut cache).unwrap();
    assert_eq!(cache.parses, 2);
    assert_eq!(after.rows[0].1.targets[0].target, "gameb");
}

#[test]
fn a_later_now_changes_the_overview_without_a_parse() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha"]);
    append_session(&paths, "alpha", "game", "2026-09-23T19:00:00Z", 3600);
    let mut cache = Cache::new();

    let september = overview_cached_at(&paths, &mut cache, ts("2026-09-24T12:00:00Z")).unwrap();
    let october = overview_cached_at(&paths, &mut cache, ts("2026-10-20T12:00:00Z")).unwrap();
    assert_eq!(cache.parses, 1);
    let (_, early) = &september.rows[0];
    let (_, late) = &october.rows[0];
    assert_eq!(
        (early.this_week_seconds, early.this_month_seconds),
        (3600, 3600)
    );
    assert_eq!((late.this_week_seconds, late.this_month_seconds), (0, 0));
    assert_eq!(late.seconds, early.seconds);
    assert_eq!(october.now, ts("2026-10-20T12:00:00Z"));
}

#[test]
fn a_running_session_appears_and_ends_on_a_warm_cache() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["live"]);
    append_session(&paths, "live", "game", "2026-09-20T19:00:00Z", 3600);
    let mut cache = Cache::new();
    let idle = overview_cached(&paths, &mut cache).unwrap();
    assert!(!idle.rows[0].1.running);

    let runtime = Runtime::new(&paths, "live");
    let lock = runtime.try_lock().unwrap().unwrap();
    let mut run = crate::runtime::RunState::running(
        crate::runtime::RunMode::Detached,
        "game".to_owned(),
        std::process::id(),
        None,
        &Default::default(),
    );
    run.started_at = Timestamp::now() - jiff::SignedDuration::from_secs(7200);
    runtime.write_state(&run).unwrap();
    let live = overview_cached(&paths, &mut cache).unwrap();
    let (_, summary) = &live.rows[0];
    assert!(summary.running);
    assert_eq!(summary.sessions, 2);
    assert!(summary.seconds >= 3600 + 7200);

    drop(lock);
    let ended = overview_cached(&paths, &mut cache).unwrap();
    assert!(!ended.rows[0].1.running);
    assert_eq!(ended.rows[0].1.seconds, 3600);
    // The log never changed, so it was parsed once through all three.
    assert_eq!(cache.parses, 1);
}

#[test]
fn a_log_that_cannot_be_read_errors_as_before_and_is_not_cached() {
    let root = tempfile::tempdir().unwrap();
    let paths = library_paths(root.path(), &["alpha", "broken"]);
    append_session(&paths, "alpha", "game", "2026-09-20T19:00:00Z", 3600);
    // A directory where the log should be stats and then fails to read.
    fs::create_dir_all(paths.stats_log("broken")).unwrap();

    let mut cache = Cache::new();
    for _ in 0..2 {
        let cached = overview_cached(&paths, &mut cache).unwrap();
        let plain = overview(&paths).unwrap();
        same_overview(&cached, &plain);
        assert_eq!(cached.errors.len(), 1);
    }
    assert_eq!(cache.logs.len(), 1);
}
