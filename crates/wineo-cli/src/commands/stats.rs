use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::environment::{self, Environment};
use wineo_core::heroic::HeroicSource;
use wineo_core::paths::Paths;
use wineo_core::stats::{self, Log, Summary};

use super::json::{self, CollectionError, StatsOutput};
use super::time::{day_label, duration_hm};

/// Width of the label column in `stats <name>`'s detail lines, wide enough
/// for `this month`, the longest one there is.
const LABEL_WIDTH: usize = 10;
const LABEL_INDENT: usize = 2;
const LABEL_GAP: usize = 2;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to show, every environment's overview if left out
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: Option<String>,
    /// Print machine-readable JSON
    #[arg(long)]
    json: bool,
    /// Include the raw session log, only with NAME and --json
    #[arg(long, requires_all = ["name", "json"])]
    sessions: bool,
    /// Update every environment's Heroic playtime and exit
    #[arg(long, conflicts_with_all = ["name", "json", "sessions"])]
    import_heroic: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if args.import_heroic {
        return import_heroic(paths);
    }
    let now = Timestamp::now();
    let tz = TimeZone::system();
    match args.name {
        Some(name) => named(paths, &name, now, &tz, args.json, args.sessions),
        None => overview(paths, now, &tz, args.json),
    }
}

fn overview(
    paths: &Paths,
    now: Timestamp,
    tz: &TimeZone,
    json_output: bool,
) -> anyhow::Result<ExitCode> {
    let names = environment::names(paths)?;
    let mut rows: Vec<(String, Summary)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<CollectionError> = Vec::new();
    for name in names {
        match stats::summary(paths, &name, now, tz) {
            Ok((summary, warns)) => {
                warnings.extend(warns);
                rows.push((name, summary));
            }
            Err(err) => errors.push(CollectionError {
                name,
                message: format!("{:#}", anyhow::Error::from(err)),
            }),
        }
    }
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if json_output {
        let environments = rows
            .iter()
            .map(|(name, summary)| StatsOutput::new(name, summary, None))
            .collect();
        json::print(&json::StatsCollectionOutput {
            environments,
            errors: &errors,
        })?;
        return Ok(if errors.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    for error in &errors {
        eprintln!("error: {}", error.message);
    }
    print_overview(&rows, now, tz);
    Ok(if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn print_overview(rows: &[(String, Summary)], now: Timestamp, tz: &TimeZone) {
    if rows.is_empty() {
        return;
    }
    let mut played: Vec<&(String, Summary)> = rows.iter().filter(|(_, s)| s.seconds > 0).collect();
    played.sort_by(|a, b| b.1.seconds.cmp(&a.1.seconds).then_with(|| a.0.cmp(&b.0)));
    if !played.is_empty() {
        let name_w = played.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
        let played_w = played
            .iter()
            .map(|(_, s)| duration_hm(s.seconds).len())
            .max()
            .unwrap_or(0)
            .max("played".len());
        let sessions_w = played
            .iter()
            .map(|(_, s)| s.sessions.to_string().len())
            .max()
            .unwrap_or(0)
            .max("sessions".len());
        println!(
            "{:name_w$}  {:played_w$}  {:>sessions_w$}  last played",
            "", "played", "sessions"
        );
        for (name, summary) in &played {
            let mut last = summary
                .last_played
                .map(|ts| day_label(now, ts, tz))
                .unwrap_or_default();
            if summary.running {
                last.push_str(" (running)");
            }
            println!(
                "{name:name_w$}  {:played_w$}  {:>sessions_w$}  {last}",
                duration_hm(summary.seconds),
                summary.sessions,
            );
        }
    }
    let never = rows.len() - played.len();
    let total: i64 = rows.iter().map(|(_, summary)| summary.seconds).sum();
    println!("{never} never played. Total {}.", duration_hm(total));
}

fn named(
    paths: &Paths,
    name: &str,
    now: Timestamp,
    tz: &TimeZone,
    json_output: bool,
    include_sessions: bool,
) -> anyhow::Result<ExitCode> {
    Environment::ensure_exists(paths, name)?;
    let log = stats::read(&paths.stats_log(name))?;
    for warning in &log.warnings {
        eprintln!("warning: {warning}");
    }
    let running = stats::running_session(paths, name)?;
    let summary = stats::summarize(&log, running.as_ref(), now, tz);
    if json_output {
        let sessions_log = include_sessions.then_some(log.sessions.as_slice());
        json::print(&StatsOutput::new(name, &summary, sessions_log))?;
    } else {
        print!("{}", render_named(name, &log, &summary, now, tz));
    }
    Ok(ExitCode::SUCCESS)
}

/// The duration of the session `summary.last_played` names: the running one
/// so far, or whichever logged session has that start time.
fn last_session_seconds(log: &Log, summary: &Summary, now: Timestamp) -> Option<i64> {
    if summary.running {
        return summary
            .last_played
            .map(|started_at| now.duration_since(started_at).as_secs().max(0));
    }
    let last_played = summary.last_played?;
    log.sessions
        .iter()
        .find(|session| session.started_at == last_played)
        .map(|session| session.seconds)
}

fn render_named(name: &str, log: &Log, summary: &Summary, now: Timestamp, tz: &TimeZone) -> String {
    let mut lines = vec![name.to_owned()];
    if summary.seconds == 0 {
        lines.push(label_line("played", "never"));
        return lines.join("\n") + "\n";
    }
    let sessions_word = if summary.sessions == 1 {
        "1 session".to_owned()
    } else {
        format!("{} sessions", summary.sessions)
    };
    let mut played = format!("{} over {sessions_word}", duration_hm(summary.seconds));
    if summary.sessions == 0 {
        played = format!("{}, all from Heroic", duration_hm(summary.seconds));
    } else if summary.imported_seconds > 0 {
        played.push_str(&format!(
            ", {} from Heroic",
            duration_hm(summary.imported_seconds)
        ));
    }
    lines.push(label_line("played", &played));
    if let Some(average) = summary.average_seconds {
        lines.push(label_line("average", &duration_hm(average)));
    }
    if let Some(longest) = &summary.longest {
        lines.push(label_line(
            "longest",
            &format!(
                "{}, {}",
                duration_hm(longest.seconds),
                day_label(now, longest.started_at, tz)
            ),
        ));
    }
    if let Some(first) = summary.first_played {
        lines.push(label_line("first", &day_label(now, first, tz)));
    }
    if let Some(last) = summary.last_played {
        let mut value = day_label(now, last, tz);
        if let Some(seconds) = last_session_seconds(log, summary, now) {
            value.push_str(&format!(", {}", duration_hm(seconds)));
        }
        if summary.running {
            value.push_str(" (running)");
        }
        lines.push(label_line("last", &value));
    }
    lines.push(label_line(
        "this week",
        &duration_hm(summary.this_week_seconds),
    ));
    lines.push(label_line(
        "this month",
        &duration_hm(summary.this_month_seconds),
    ));
    if summary.targets.len() > 1 {
        let targets = summary
            .targets
            .iter()
            .map(|target| {
                format!(
                    "{} {} ({})",
                    target.target,
                    duration_hm(target.seconds),
                    target.sessions
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(label_line("targets", &targets));
    }
    lines.join("\n") + "\n"
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

/// `wineo stats --import-heroic`: refreshes the `imported` line of every
/// environment with a `heroic_id`, printing one terse line for each it
/// updated. One environment's problem is warned about and skipped rather than
/// stopping the rest, and the command exits 1 if any of them failed.
fn import_heroic(paths: &Paths) -> anyhow::Result<ExitCode> {
    let sources = HeroicSource::discover_from_env()?;
    let mut failed = false;
    for name in environment::names(paths)? {
        let environment = match Environment::load(paths, &name) {
            Ok(environment) => environment,
            Err(error) => {
                eprintln!(
                    "warning: could not read {name}: {:#}",
                    anyhow::Error::from(error)
                );
                failed = true;
                continue;
            }
        };
        let Some(heroic_id) = &environment.config.heroic_id else {
            continue;
        };
        let playtime = sources.iter().find_map(|source| {
            source.playtime(heroic_id).unwrap_or_else(|error| {
                eprintln!(
                    "warning: could not read Heroic's playtime for {name}: {:#}",
                    anyhow::Error::from(error)
                );
                None
            })
        });
        let Some(playtime) = playtime else {
            continue;
        };
        let imported = stats::Imported {
            seconds: playtime.seconds,
            last_played: playtime.last_played,
        };
        if let Err(error) = stats::set_imported(paths, &name, imported) {
            eprintln!(
                "warning: could not import playtime for {name}: {:#}",
                anyhow::Error::from(error)
            );
            failed = true;
            continue;
        }
        println!("{name}: {} from Heroic", duration_hm(playtime.seconds));
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
