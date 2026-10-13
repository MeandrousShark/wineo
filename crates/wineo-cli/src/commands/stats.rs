use std::process::ExitCode;

use jiff::Timestamp;
use wineo_core::environment::Environment;
use wineo_core::heroic::HeroicSource;
use wineo_core::paths::Paths;
use wineo_core::stats::{self, HeroicImportResult, Log, Overview, OverviewOptions, Summary};
use wineo_core::view::{CollectionError, StatsCollectionOutput, StatsOutput, TagBreakdownOutput};

use super::{json, table};
use wineo_core::time::{LocalZone, Zone, day_label, duration_hm};

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
    /// Show environments with every given tag
    #[arg(
        long = "tag",
        value_name = "TAG",
        conflicts_with_all = ["name", "import_heroic"],
        add = clap_complete::ArgValueCompleter::new(crate::completion::tags)
    )]
    tags: Vec<String>,
    /// Show playtime and membership grouped by tag
    #[arg(long, conflicts_with_all = ["name", "import_heroic"])]
    by_tag: bool,
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
            &LocalZone::new(),
            args.json,
            args.sessions,
        ),
        None => overview(paths, args.json, &args.tags, args.by_tag),
    }
}

fn overview(
    paths: &Paths,
    json_output: bool,
    tags: &[String],
    by_tag: bool,
) -> anyhow::Result<ExitCode> {
    let overview = stats::overview_with_options(
        paths,
        OverviewOptions {
            tags: (!tags.is_empty()).then_some(tags),
            load_tags: by_tag,
        },
    )?;
    for warning in &overview.warnings {
        eprintln!("warning: {warning}");
    }
    if json_output {
        let errors: Vec<CollectionError> = overview
            .errors
            .iter()
            .map(|error| CollectionError {
                name: error.name.clone(),
                message: error.message.clone(),
            })
            .collect();
        if by_tag {
            let breakdown = stats::by_tag(&overview);
            json::print(&TagBreakdownOutput::new(&breakdown, &errors))?;
        } else {
            let environments = overview
                .rows
                .iter()
                .map(|(name, summary)| StatsOutput::new(name, summary, None))
                .collect();
            json::print(&StatsCollectionOutput {
                environments,
                errors: &errors,
            })?;
        }
        return Ok(if overview.errors.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    for error in &overview.errors {
        eprintln!("error: {}", error.message);
    }
    if overview.rows.is_empty() && !tags.is_empty() {
        println!("No environments are tagged {}", tags.join(" and "));
    } else if by_tag {
        print_by_tag(&overview);
    } else {
        print_overview(&overview, &overview.tz);
    }
    Ok(if overview.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn print_by_tag(overview: &Overview) {
    let breakdown = stats::by_tag(overview);
    let totals: Vec<_> = breakdown
        .tags
        .iter()
        .map(|(tag, totals)| (tag.as_str(), totals))
        .chain(
            breakdown
                .untagged
                .as_ref()
                .map(|totals| ("(untagged)", totals)),
        )
        .collect();
    if totals.is_empty() {
        return;
    }
    let sessions_w = totals
        .iter()
        .map(|(_, totals)| totals.sessions.to_string().len())
        .max()
        .unwrap_or(0)
        .max("sessions".len());
    let mut rows = vec![[
        String::new(),
        "played".to_owned(),
        format!("{:>sessions_w$}", "sessions"),
        "played by".to_owned(),
        "last played".to_owned(),
    ]];
    rows.extend(totals.iter().map(|(tag, totals)| {
        [
            (*tag).to_owned(),
            duration_hm(totals.seconds),
            format!("{:>sessions_w$}", totals.sessions),
            format!("{} of {}", totals.played_environments, totals.environments),
            totals
                .last_played
                .map(|ts| day_label(overview.now, ts, &overview.tz))
                .unwrap_or_default(),
        ]
    }));
    for line in table::render_columns(&rows) {
        println!("{line}");
    }
    if overview.tags.values().any(|tags| tags.len() > 1) {
        println!("An environment with several tags counts under each.");
    }
    print_steam_notes(overview);
}

fn print_overview(overview: &Overview, tz: &dyn Zone) {
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
            .map(|(_, summary)| overview_sessions(summary).len())
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
                overview_sessions(summary),
            );
        }
    }
    println!(
        "{} never played. Total {}.",
        overview.never_played,
        duration_hm(overview.total_seconds)
    );
    print_steam_notes(overview);
}

fn overview_sessions(summary: &Summary) -> String {
    if summary.source == stats::Source::Steam {
        "-".to_owned()
    } else {
        summary.sessions.to_string()
    }
}

fn print_steam_notes(overview: &Overview) {
    if overview.steam_games > 0 {
        println!("Steam games count Steam's total, without sessions.");
    }
    match overview.unknown_steam_games {
        0 => {}
        1 => println!("1 Steam game's playtime is unknown."),
        count => println!("{count} Steam games' playtime is unknown."),
    }
}

fn named(
    paths: &Paths,
    name: &str,
    now: Timestamp,
    tz: &dyn Zone,
    json_output: bool,
    include_sessions: bool,
) -> anyhow::Result<ExitCode> {
    Environment::ensure_exists(paths, name)?;
    let (summary, log, warnings) = stats::summary_with_log(paths, name, now, tz)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if json_output {
        let sessions_log = include_sessions
            .then(|| log.as_ref().map(|log| log.sessions.as_slice()))
            .flatten();
        json::print(&StatsOutput::new(name, &summary, sessions_log))?;
    } else {
        print!("{}", render_named(name, log.as_ref(), &summary, now, tz));
    }
    Ok(ExitCode::SUCCESS)
}

fn render_named(
    name: &str,
    log: Option<&Log>,
    summary: &Summary,
    now: Timestamp,
    tz: &dyn Zone,
) -> String {
    let mut lines = vec![name.to_owned()];
    if summary.source == stats::Source::Steam {
        if summary.playtime_unknown {
            lines.push(label_line(
                "played",
                "unknown (Steam's playtime can't be read)",
            ));
        } else {
            let mut value = format!("{} on Steam", duration_hm(summary.seconds));
            if let Some(last) = summary.last_played {
                value.push_str(&format!(", last {}", day_label(now, last, tz)));
            }
            lines.push(label_line("played", &value));
        }
        return lines.join("\n") + "\n";
    }
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
        if let Some(seconds) = log.and_then(|log| stats::last_session_seconds(log, summary, now)) {
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
