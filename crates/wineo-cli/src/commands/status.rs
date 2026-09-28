use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::paths::Paths;
use wineo_core::report::{self, EnvironmentReport, OverviewFilter};
use wineo_core::runtime::{RunMode, RunState, State};
use wineo_core::view::NamedOutput;

use super::json;
use super::table;
use super::time::{absolute, day_label, duration_hm, relative};

/// Width of the label column in `status <name>`'s detail lines. Wide enough
/// for the longest label there is, so no line pushes its value out of line.
const LABEL_WIDTH: usize = 10;
/// Spaces before the label, and between the label and its value.
const LABEL_INDENT: usize = 2;
const LABEL_GAP: usize = 2;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to show, every running environment if left out
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: Option<String>,
    /// Print machine-readable JSON
    #[arg(long)]
    json: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match args.name {
        Some(name) => {
            let environment = Environment::load(paths, &name)?;
            // A broken global config only costs the runner version line.
            let global = GlobalConfig::load(paths).unwrap_or_else(|err| {
                if matches!(err, Error::SchemaVersion { .. }) {
                    eprintln!("warning: {err}");
                }
                GlobalConfig::default()
            });
            let (report, warnings) =
                EnvironmentReport::load_with_warnings(paths, &global, &environment);
            for warning in warnings {
                eprintln!("{warning}");
            }
            let report = report?;
            if args.json {
                let configured = environment.config.wine().map(|wine| wine.runner.as_str());
                json::print(&NamedOutput::new(&report, configured))?;
            } else {
                print!("{}", render(&report, Timestamp::now(), &TimeZone::system()));
            }
            Ok(ExitCode::SUCCESS)
        }
        None => running(paths, args.json),
    }
}

fn running(paths: &Paths, json_output: bool) -> anyhow::Result<ExitCode> {
    let now = Timestamp::now();
    let tz = TimeZone::system();
    // Something holding an environment is happening now too, and so may be
    // whatever an unresolved installer left, so both show up here.
    let overview = report::overview(
        paths,
        OverviewFilter {
            tags: None,
            running_only: true,
        },
    )?;
    let gathered = table::format_overview(overview, now, &tz);
    if json_output {
        json::print(&gathered.json())?;
    } else {
        gathered.print_errors();
        if gathered.rows.is_empty() {
            eprintln!("Nothing is running");
        } else {
            for line in table::render(&gathered.rows) {
                println!("{line}");
            }
        }
    }
    Ok(if gathered.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Renders the full `status <name>` block, including its trailing newline.
fn render(report: &EnvironmentReport, now: Timestamp, tz: &TimeZone) -> String {
    let mut lines = vec![header(report, now, tz)];
    if let Some(operation) = report.busy {
        lines.push(label_line("busy", operation.describe()));
    } else if report.unresolved {
        lines.push(label_line(
            "busy",
            &format!(
                "an installer did not finish cleanly, clean it up with `wineo stop {}`",
                report.name
            ),
        ));
    }
    if let Some(run) = &report.status.run {
        lines.push(label_line(
            "target",
            &target_value(run, report.target_exe.as_deref()),
        ));
    }
    if report.native {
        lines.push(label_line("kind", "native"));
    }
    if let Some(runner) = &report.runner {
        lines.push(label_line(
            "runner",
            &runner_value(runner, report.runner_version.as_deref()),
        ));
    }
    if let Some(prefix) = &report.prefix {
        lines.push(label_line("prefix", prefix));
    }
    if !report.tags.is_empty() {
        lines.push(label_line("tags", &report.tags.join(", ")));
    }
    if !report.winetricks.is_empty() {
        let verbs = report
            .winetricks
            .iter()
            .map(|verb| {
                if report.winetricks_missing.contains(verb) {
                    format!("{verb} (missing)")
                } else {
                    verb.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(label_line("winetricks", &verbs));
    }
    if report.gamemode {
        lines.push(label_line("gamemode", "on"));
    }
    for (label, enabled) in [
        ("gamescope", report.gamescope),
        ("mangohud", report.mangohud),
        ("wayland", report.wayland),
    ] {
        if enabled {
            lines.push(label_line(label, "on"));
        }
    }
    if let Some(gameid) = &report.gameid {
        lines.push(label_line("gameid", gameid));
    }
    if let Some(store) = &report.store {
        lines.push(label_line("store", store));
    }
    for (label, component) in [("dxvk", &report.dxvk), ("vkd3d", &report.vkd3d)] {
        if let Some(component) = component {
            lines.push(label_line(label, &report::component_value(component)));
        }
    }
    if !report.desktop.is_empty() {
        lines.push(label_line("desktop", &report.desktop.join(", ")));
    }
    if let Some(played) = &report.played {
        lines.push(label_line("played", &played_value(played, now, tz)));
    }
    if report.status.state == State::Running
        && let Some(run) = &report.status.run
    {
        lines.extend(pid_lines(&report.name, run, now));
    }
    lines.join("\n") + "\n"
}

fn header(report: &EnvironmentReport, now: Timestamp, tz: &TimeZone) -> String {
    let name = &report.name;
    let status = &report.status;
    match (status.state, &status.run) {
        (State::Inactive, _) => format!("{name}: inactive"),
        (State::Running, None) => format!("{name}: running"),
        (State::Running, Some(run)) => {
            let attached = if run.mode == RunMode::Attached {
                " attached"
            } else {
                ""
            };
            format!(
                "{name}: running{attached} since {} ({})",
                absolute(run.started_at, tz),
                relative(now, run.started_at)
            )
        }
        (State::Failed, Some(run)) if status.lost => format!(
            "{name}: failed, wineo lost track of the run started at {} ({})",
            absolute(run.started_at, tz),
            relative(now, run.started_at)
        ),
        (state, Some(run)) => {
            let ended = run.ended_at.unwrap_or(run.started_at);
            let verb = match state {
                State::Exited => "exited",
                State::Failed => "failed",
                State::Stopped => "stopped",
                State::Running | State::Inactive => unreachable!("handled above"),
            };
            // `stopped` never shows a code: it can carry one from a program that
            // happened to exit right as the stop was requested.
            let code = if state == State::Stopped {
                String::new()
            } else {
                run.exit_code
                    .map(|code| format!(" with code {code}"))
                    .unwrap_or_default()
            };
            format!(
                "{name}: {verb}{code} at {} ({})",
                absolute(ended, tz),
                relative(now, ended)
            )
        }
        (_, None) => unreachable!("running and inactive are the only states without a run"),
    }
}

fn target_value(run: &RunState, exe: Option<&str>) -> String {
    match exe {
        Some(exe) => format!("{} ({exe})", run.target),
        None => run.target.clone(),
    }
}

fn played_value(played: &report::Played, now: Timestamp, tz: &TimeZone) -> String {
    let sessions = if played.sessions == 1 {
        "1 session".to_owned()
    } else {
        format!("{} sessions", played.sessions)
    };
    // With no sessions, all of it came from a Heroic import.
    let mut value = if played.sessions == 0 {
        format!("{} from Heroic", duration_hm(played.seconds))
    } else {
        format!("{} over {sessions}", duration_hm(played.seconds))
    };
    if let Some(last_played) = played.last_played {
        value.push_str(&format!(", last {}", day_label(now, last_played, tz)));
    }
    value
}

fn runner_value(name: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{name} ({version})"),
        None => name.to_owned(),
    }
}

fn pid_lines(name: &str, run: &RunState, now: Timestamp) -> Vec<String> {
    match run.program_exited_at {
        None => vec![label_line("pid", &run.pid.to_string())],
        Some(exited_at) => {
            let code = run
                .exit_code
                .map(|code| format!(" with code {code}"))
                .unwrap_or_default();
            vec![
                label_line(
                    "pid",
                    &format!("{}, exited {}{code}", run.pid, relative(now, exited_at)),
                ),
                continuation_line(&if run.native {
                    format!("other processes are keeping {name} running")
                } else {
                    "other processes are keeping the prefix running".to_owned()
                }),
            ]
        }
    }
}

fn label_line(label: &str, value: &str) -> String {
    format!(
        "{}{:width$}{}{value}",
        " ".repeat(LABEL_INDENT),
        label,
        " ".repeat(LABEL_GAP),
        width = LABEL_WIDTH,
    )
}

fn continuation_line(text: &str) -> String {
    format!(
        "{:width$}{text}",
        "",
        width = LABEL_INDENT + LABEL_WIDTH + LABEL_GAP
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

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
            status,
            mode,
            target: "notepad".to_owned(),
            pid: 48213,
            group: Some(48213),
            boot_id: None,
            leader_start: None,
            started_at: ts("2026-09-16T14:02:11Z"),
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
            native: false,
            runner: Some("wine".to_owned()),
            runner_resolved: Some("wine".to_owned()),
            runner_version: Some("wine-11.17".to_owned()),
            prefix: Some("~/.local/share/wineo/prefixes/notepad".to_owned()),
            target_exe: Some("notepad".to_owned()),
            busy: None,
            unresolved: false,
            gamemode: false,
            gamescope: false,
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
            text.contains(
                "  gamemode    on\n  gamescope   on\n  mangohud    on\n  wayland     on\n"
            ),
            "{text}"
        );
    }
}
