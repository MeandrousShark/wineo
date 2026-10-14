use std::collections::BTreeMap;

use jiff::tz::TimeZone;
use wineo_core::runtime::{RunStatus, Status};

use super::*;

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn run(status: RunStatus, mode: RunMode) -> RunState {
    RunState {
        runner: None,
        prefix: None,
        native: false,
        launcher: None,
        status,
        lost: false,
        phase: None,
        mode,
        target: "notepad".to_owned(),
        pid: 48213,
        group: Some(48213),
        boot_id: None,
        leader_start: None,
        started_at: ts("2026-09-16T14:02:11Z"),
        running_at: None,
        heartbeat_at: None,
        program_exited_at: None,
        ended_at: None,
        exit_code: None,
        requested_target: None,
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}

fn report(status: wineo_core::runtime::Status) -> EnvironmentReport {
    EnvironmentReport {
        name: "notepad".to_owned(),
        tags: Vec::new(),
        winetricks: Vec::new(),
        winetricks_missing: Vec::new(),
        status,
        kind: KindReport::Wine,
        runner: Some("wine".to_owned()),
        runner_resolved: Some("wine".to_owned()),
        runner_version: Some("wine-11.17".to_owned()),
        prefix: Some("~/.local/share/wineo/prefixes/notepad".to_owned()),
        target_exe: Some("notepad".to_owned()),
        busy: None,
        unresolved: false,
        gamemode: false,
        gamescope: false,
        hooks: None,
        mangohud: false,
        wayland: false,
        gameid: None,
        store: None,
        dxvk: None,
        vkd3d: None,
        desktop: Vec::new(),
        played: None,
    }
}

fn now() -> Timestamp {
    ts("2026-09-16T14:05:11Z")
}

#[test]
fn running_detached_shows_target_runner_prefix_and_pid() {
    let status = Status {
        state: State::Running,
        run: Some(run(RunStatus::Running, RunMode::Detached)),
        lost: false,
    };
    assert_eq!(
        render(&report(status), now(), &TimeZone::UTC),
        "notepad: running since 2026-09-16 14:02:11 (3 min ago)\n\
         \x20\x20target      notepad (notepad)\n\
         \x20\x20runner      wine (wine-11.17)\n\
         \x20\x20prefix      ~/.local/share/wineo/prefixes/notepad\n\
         \x20\x20pid         48213\n"
    );
}

#[test]
fn running_attached_says_so_in_the_header() {
    let status = Status {
        state: State::Running,
        run: Some(run(RunStatus::Running, RunMode::Attached)),
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.starts_with("notepad: running attached since 2026-09-16 14:02:11 (3 min ago)\n"),
        "{text}"
    );
}

#[test]
fn running_without_a_run_yet_has_no_target_or_pid() {
    let status = Status {
        state: State::Running,
        run: None,
        lost: false,
    };
    assert_eq!(
        render(&report(status), now(), &TimeZone::UTC),
        "notepad: running\n\
         \x20\x20runner      wine (wine-11.17)\n\
         \x20\x20prefix      ~/.local/share/wineo/prefixes/notepad\n"
    );
}

#[test]
fn steam_starting_and_client_runs_have_distinct_headers() {
    let mut run = run(RunStatus::Running, RunMode::Detached);
    run.launcher = Some(wineo_core::config::Launcher::Steam);
    run.target = "steam".to_owned();
    run.phase = Some(wineo_core::runtime::RunPhase::Starting);
    let mut starting = report(Status {
        state: State::Starting,
        run: Some(run),
        lost: false,
    });
    starting.kind = KindReport::Proxied {
        launcher: wineo_core::config::Launcher::Steam,
        app_id: "289070".to_owned(),
        title: Some("Civilization VI".to_owned()),
        install: wineo_core::steam::AppState::Installed.into(),
    };
    let text = render(&starting, now(), &TimeZone::UTC);
    assert!(text.starts_with("notepad: starting since "), "{text}");
    assert!(
        text.contains("target      steam (Civilization VI)"),
        "{text}"
    );
    assert!(!text.contains("pid "), "{text}");

    starting.status = Status {
        state: State::SteamRunning,
        run: None,
        lost: false,
    };
    let text = render(&starting, now(), &TimeZone::UTC);
    assert!(
        text.starts_with("notepad: running (started from Steam)\n"),
        "{text}"
    );
}

#[test]
fn program_exited_while_prefix_stays_up_shows_two_pid_lines() {
    let mut run = run(RunStatus::Running, RunMode::Detached);
    run.program_exited_at = Some(ts("2026-09-16T14:04:11Z"));
    run.exit_code = Some(0);
    let status = Status {
        state: State::Running,
        run: Some(run),
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.ends_with(
            "  pid         48213, exited 1 min ago with code 0\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20other processes are keeping the prefix running\n"
        ),
        "{text}"
    );
}

#[test]
fn a_native_handoff_names_the_environment_rather_than_a_prefix() {
    let mut run = run(RunStatus::Running, RunMode::Detached);
    run.native = true;
    run.program_exited_at = Some(ts("2026-09-16T14:04:11Z"));
    let status = Status {
        state: State::Running,
        run: Some(run),
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.ends_with(
            "\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20other processes are keeping notepad running\n"
        ),
        "{text}"
    );
}

#[test]
fn exited_drops_the_pid_line_and_keeps_the_rest() {
    let mut run = run(RunStatus::Exited, RunMode::Detached);
    run.ended_at = Some(ts("2026-09-16T14:05:02Z"));
    run.exit_code = Some(0);
    let status = Status {
        state: State::Exited,
        run: Some(run),
        lost: false,
    };
    assert_eq!(
        render(&report(status), now(), &TimeZone::UTC),
        "notepad: exited with code 0 at 2026-09-16 14:05:02 (9 s ago)\n\
         \x20\x20target      notepad (notepad)\n\
         \x20\x20runner      wine (wine-11.17)\n\
         \x20\x20prefix      ~/.local/share/wineo/prefixes/notepad\n"
    );
}

#[test]
fn failed_with_code() {
    let mut run = run(RunStatus::Failed, RunMode::Detached);
    run.ended_at = Some(ts("2026-09-16T14:05:02Z"));
    run.exit_code = Some(3);
    let status = Status {
        state: State::Failed,
        run: Some(run),
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.starts_with("notepad: failed with code 3 at 2026-09-16 14:05:02 (9 s ago)\n"),
        "{text}"
    );
}

#[test]
fn failed_without_a_code() {
    let mut run = run(RunStatus::Failed, RunMode::Detached);
    run.ended_at = Some(ts("2026-09-16T14:05:02Z"));
    let status = Status {
        state: State::Failed,
        run: Some(run),
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.starts_with("notepad: failed at 2026-09-16 14:05:02 (9 s ago)\n"),
        "{text}"
    );
}

#[test]
fn stopped_never_shows_a_code() {
    let mut run = run(RunStatus::Stopped, RunMode::Detached);
    run.ended_at = Some(ts("2026-09-16T14:05:02Z"));
    run.exit_code = Some(0);
    let status = Status {
        state: State::Stopped,
        run: Some(run),
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.starts_with("notepad: stopped at 2026-09-16 14:05:02 (9 s ago)\n"),
        "{text}"
    );
}

#[test]
fn lost_uses_started_at_and_never_shows_a_code() {
    let mut run = run(RunStatus::Running, RunMode::Detached);
    run.exit_code = Some(1);
    let status = Status {
        state: State::Failed,
        run: Some(run),
        lost: true,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(
        text.starts_with(
            "notepad: failed, wineo lost track of the run started at \
             2026-09-16 14:02:11 (3 min ago)\n"
        ),
        "{text}"
    );
}

#[test]
fn inactive_shows_only_runner_and_prefix() {
    let status = Status {
        state: State::Inactive,
        run: None,
        lost: false,
    };
    assert_eq!(
        render(&report(status), now(), &TimeZone::UTC),
        "notepad: inactive\n\
         \x20\x20runner      wine (wine-11.17)\n\
         \x20\x20prefix      ~/.local/share/wineo/prefixes/notepad\n"
    );
}

#[test]
fn target_without_a_matching_exe_drops_the_parens() {
    let status = Status {
        state: State::Exited,
        run: Some({
            let mut run = run(RunStatus::Exited, RunMode::Detached);
            run.ended_at = Some(ts("2026-09-16T14:05:02Z"));
            run.exit_code = Some(0);
            run
        }),
        lost: false,
    };
    let mut report = report(status);
    report.target_exe = None;
    let text = render(&report, now(), &TimeZone::UTC);
    assert!(text.contains("\n  target      notepad\n"), "{text}");
}

#[test]
fn a_played_line_shows_when_the_environment_has_sessions() {
    let status = Status {
        state: State::Inactive,
        run: None,
        lost: false,
    };
    let mut report = report(status);
    report.played = Some(wineo_core::report::Played {
        seconds: 65_100,
        sessions: 11,
        last_played: Some(now()),
        source: Source::Wineo,
        playtime_unknown: false,
    });
    let text = render(&report, now(), &TimeZone::UTC);
    assert!(
        text.contains("\n  played      18 h 5 m over 11 sessions, last today\n"),
        "{text}"
    );
}

#[test]
fn a_played_line_with_only_heroic_time_says_so() {
    let status = Status {
        state: State::Inactive,
        run: None,
        lost: false,
    };
    let mut report = report(status);
    report.played = Some(wineo_core::report::Played {
        seconds: 349_200,
        sessions: 0,
        last_played: Some(now()),
        source: Source::Wineo,
        playtime_unknown: false,
    });
    let text = render(&report, now(), &TimeZone::UTC);
    assert!(
        text.contains("\n  played      97 h from Heroic, last today\n"),
        "{text}"
    );
}

#[test]
fn a_never_played_environment_has_no_played_line() {
    let status = Status {
        state: State::Inactive,
        run: None,
        lost: false,
    };
    let text = render(&report(status), now(), &TimeZone::UTC);
    assert!(!text.contains("played"), "{text}");
}

#[test]
fn runner_without_a_version_drops_the_parens() {
    let status = Status {
        state: State::Inactive,
        run: None,
        lost: false,
    };
    let mut report = report(status);
    report.runner_version = None;
    let text = render(&report, now(), &TimeZone::UTC);
    assert!(text.contains("\n  runner      wine\n"), "{text}");
}

#[test]
fn launch_settings_show_next_to_gamemode_when_on() {
    let mut report = report(Status {
        state: State::Inactive,
        run: None,
        lost: false,
    });
    report.gamemode = true;
    report.gamescope = true;
    report.mangohud = true;
    report.wayland = true;
    let text = render(&report, now(), &TimeZone::UTC);
    assert!(
        text.contains("  gamemode    on\n  gamescope   on\n  mangohud    on\n  wayland     on\n"),
        "{text}"
    );
}
