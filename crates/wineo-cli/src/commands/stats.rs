use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::environment::Environment;
use wineo_core::heroic::HeroicSource;
use wineo_core::paths::Paths;
use wineo_core::stats::{self, HeroicImportResult, Log, Overview, Summary};
use wineo_core::view::{CollectionError, StatsCollectionOutput, StatsOutput};

use super::json;
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
    match args.name {
        Some(name) => named(
            paths,
            &name,
            Timestamp::now(),
            &TimeZone::system(),
            args.json,
            args.sessions,
        ),
        None => overview(paths, args.json),
    }
}

fn overview(paths: &Paths, json_output: bool) -> anyhow::Result<ExitCode> {
    let overview = stats::overview(paths)?;
    for warning in &overview.warnings {
        eprintln!("warning: {warning}");
    }
    if json_output {
        let environments = overview
            .rows
            .iter()
            .map(|(name, summary)| StatsOutput::new(name, summary, None))
            .collect();
        let errors: Vec<CollectionError> = overview
            .errors
            .iter()
            .map(|error| CollectionError {
                name: error.name.clone(),
                message: error.message.clone(),
            })
            .collect();
        json::print(&StatsCollectionOutput {
            environments,
            errors: &errors,
        })?;
        return Ok(if overview.errors.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    for error in &overview.errors {
        eprintln!("error: {}", error.message);
    }
    print_overview(&overview, &overview.tz);
    Ok(if overview.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn print_overview(overview: &Overview, tz: &TimeZone) {
    if overview.rows.is_empty() {
        return;
    }
    let played: Vec<&(String, Summary)> = overview
        .played
        .iter()
        .map(|&index| &overview.rows[index])
        .collect();
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
                .map(|ts| day_label(overview.now, ts, tz))
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
    println!(
        "{} never played. Total {}.",
        overview.never_played,
        duration_hm(overview.total_seconds)
    );
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
        if let Some(seconds) = stats::last_session_seconds(log, summary, now) {
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
    for result in stats::import_heroic_all(paths, &sources)? {
        match result {
            HeroicImportResult::EnvironmentError { name, error } => {
                eprintln!(
                    "warning: could not read {name}: {:#}",
                    anyhow::Error::from(error)
                );
                failed = true;
            }
            HeroicImportResult::PlaytimeError { name, error } => {
                eprintln!(
                    "warning: could not read Heroic's playtime for {name}: {:#}",
                    anyhow::Error::from(error)
                );
            }
            HeroicImportResult::ImportError { name, error } => {
                eprintln!(
                    "warning: could not import playtime for {name}: {:#}",
                    anyhow::Error::from(error)
                );
                failed = true;
            }
            HeroicImportResult::Imported { name, seconds } => {
                println!("{name}: {} from Heroic", duration_hm(seconds));
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
