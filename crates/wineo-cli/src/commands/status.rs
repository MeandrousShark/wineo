use std::process::ExitCode;

use jiff::Timestamp;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::chain;
use wineo_core::paths::Paths;
use wineo_core::report::{self, EnvironmentReport, KindReport, OverviewFilter};
use wineo_core::runtime::{RunKind, RunMode, RunState, State};
use wineo_core::stats::Source;
use wineo_core::view::NamedOutput;

use super::json;
use super::table::{self, LABEL_GAP, LABEL_INDENT, LABEL_WIDTH, label_line};
use wineo_core::time::{LocalZone, Zone, absolute, day_label, duration_hm, relative};

use crate::text;

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
                eprintln!("warning: {}", chain(&err));
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
                print!("{}", render(&report, Timestamp::now(), &LocalZone::new()));
            }
            Ok(ExitCode::SUCCESS)
        }
        None => running(paths, args.json),
    }
}

fn running(paths: &Paths, json_output: bool) -> anyhow::Result<ExitCode> {
    let now = Timestamp::now();
    let tz = LocalZone::new();
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
fn render(report: &EnvironmentReport, now: Timestamp, tz: &dyn Zone) -> String {
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
        let target_detail = match &report.kind {
            KindReport::Proxied { title, .. } => title.as_deref(),
            KindReport::Wine | KindReport::Native => report.target_exe.as_deref(),
        };
        lines.push(label_line("target", &target_value(run, target_detail)));
    }
    match &report.kind {
        KindReport::Wine => {}
        KindReport::Native => lines.push(label_line("kind", "native")),
        KindReport::Proxied {
            launcher,
            app_id,
            title,
            ..
        } => {
            lines.push(label_line("kind", "proxied"));
            lines.push(label_line("launcher", launcher.as_str()));
            lines.push(label_line("app id", app_id));
            if let Some(title) = title {
                lines.push(label_line("title", title));
            }
        }
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
    if let Some(hooks) = &report.hooks {
        let phases: Vec<_> = [("before", &hooks.before), ("after", &hooks.after)]
            .into_iter()
            .filter_map(|(phase, command)| command.as_ref().map(|_| phase))
            .collect();
        if !phases.is_empty() {
            lines.push(label_line("hooks", &phases.join(", ")));
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

fn header(report: &EnvironmentReport, now: Timestamp, tz: &dyn Zone) -> String {
    let name = &report.name;
    let status = &report.status;
    if let Some(install) = table::unfinished_install(status.state, &report.kind) {
        return match &install.problem {
            Some(reason) => format!("{name}: {} ({reason})", install.label()),
            None => format!("{name}: {}", install.label()),
        };
    }
    match (status.state, &status.run) {
        (State::Inactive, _) => format!("{name}: inactive"),
        (State::SteamRunning, _) => format!("{name}: running (started from Steam)"),
        (State::Starting, None) => format!("{name}: starting"),
        (State::Starting, Some(run)) => format!(
            "{name}: starting since {} ({})",
            absolute(run.started_at, tz),
            relative(now, run.started_at)
        ),
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
        (State::Stopped, Some(run)) if run.lost => {
            let ended = run.ended_at.unwrap_or(run.started_at);
            format!(
                "{name}: stopped, wineo lost track of the run at {} ({})",
                absolute(ended, tz),
                relative(now, ended)
            )
        }
        (state, Some(run)) => {
            let ended = run.ended_at.unwrap_or(run.started_at);
            let verb = match state {
                State::Exited => "exited",
                State::Failed => "failed",
                State::Stopped => "stopped",
                State::Running | State::Starting | State::SteamRunning | State::Inactive => {
                    unreachable!("handled above")
                }
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

fn played_value(played: &report::Played, now: Timestamp, tz: &dyn Zone) -> String {
    if played.source == Source::Steam {
        return if played.playtime_unknown {
            "unknown".to_owned()
        } else {
            format!("{} on Steam", duration_hm(played.seconds))
        };
    }
    let sessions = text::counted(played.sessions, "session", "sessions");
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
                continuation_line(&match run.kind() {
                    RunKind::Native | RunKind::Proxied(_) => {
                        format!("other processes are keeping {name} running")
                    }
                    RunKind::Wine => "other processes are keeping the prefix running".to_owned(),
                }),
            ]
        }
    }
}

fn continuation_line(text: &str) -> String {
    format!(
        "{:width$}{text}",
        "",
        width = LABEL_INDENT + LABEL_WIDTH + LABEL_GAP
    )
}

#[cfg(test)]
mod tests;
