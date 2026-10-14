use std::process::ExitCode;

use clap::Subcommand;
use wineo_core::config::GlobalConfig;
use wineo_core::external_runners::Source;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::{CandidateKind, Runners};

use crate::text;

#[derive(clap::Args)]
pub struct Args {
    /// Also list directories that aren't usable as runners, and why
    #[arg(long)]
    all: bool,

    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Remove an installed runner
    Remove(RemoveArgs),
}

#[derive(clap::Args)]
struct RemoveArgs {
    #[arg(value_name = "NAME", add = clap_complete::ArgValueCompleter::new(crate::completion::removable_build_names))]
    name: String,

    /// Confirm removal without a terminal
    #[arg(long)]
    yes: bool,
}

/// One line: what it's called, what it is, whose it is, what version, and a
/// last column. That's the path for the system wine, the only build not in a
/// folder, and a note about a hidden different build for the rest.
struct Row {
    name: String,
    kind: String,
    source: String,
    detail: String,
    last: String,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if let Some(Action::Remove(args)) = args.action {
        return super::remove_build::run(
            paths,
            &args.name,
            "runner",
            wineo_core::build_lock::Kind::Runner,
            args.yes,
            |paths, name| Ok(wineo_core::runners::size(paths, name)?),
            |paths, name| Ok(wineo_core::runners::remove(paths, name)?),
        );
    }

    let global = GlobalConfig::load(paths)?;
    let runners = Runners::new(paths, &global)?;
    let listing = runners.list()?;

    // The system wine leads, since that is what every config starts out using.
    let mut rows: Vec<_> = runners
        .candidates(&listing)
        .into_iter()
        .map(|candidate| match (candidate.kind, candidate.runner) {
            (CandidateKind::System, Ok(runner)) => Row {
                name: candidate.name,
                kind: runner.kind_name().to_owned(),
                source: String::new(),
                detail: version(&runner),
                last: runner
                    .wine_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            },
            (CandidateKind::Installed, Ok(runner)) => Row {
                name: candidate.name,
                kind: runner.kind_name().to_owned(),
                source: runner.owner(paths).to_string(),
                detail: version(&runner),
                last: different_note(&listing.different_builds(runner.name())),
            },
            (CandidateKind::Alias, Ok(runner)) => Row {
                name: candidate.name,
                kind: "alias".to_owned(),
                source: String::new(),
                detail: format!("-> {}", runner.name()),
                last: String::new(),
            },
            (kind, Err(err)) => Row {
                name: candidate.name,
                kind: if kind == CandidateKind::Alias {
                    "alias".to_owned()
                } else {
                    String::new()
                },
                source: String::new(),
                detail: err.to_string(),
                last: String::new(),
            },
        })
        .collect();
    if args.all {
        rows.extend(listing.skipped.iter().map(|(name, reason)| Row {
            name: name.clone(),
            kind: String::new(),
            source: String::new(),
            detail: format!("skipped, {}", reason.as_str()),
            last: String::new(),
        }));
        rows.extend(listing.external_skipped.iter().map(|skipped| {
            Row {
                name: skipped
                    .directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                kind: String::new(),
                source: skipped.source.to_string(),
                detail: format!("skipped, {}", skipped.reason.as_str()),
                last: String::new(),
            }
        }));
    }

    // Display width, not bytes or chars, since a folder's name may be wide.
    let width = |column: fn(&Row) -> &str| text::widest(rows.iter().map(column));
    // A detail only needs padding when something follows it, so a long alias
    // error doesn't push the last column off the screen.
    let details = rows
        .iter()
        .filter(|row| !row.last.is_empty())
        .map(|row| text::width(&row.detail))
        .max()
        .unwrap_or_default();
    let (names, kinds, sources) = (
        width(|row| &row.name),
        width(|row| &row.kind),
        width(|row| &row.source),
    );
    for row in &rows {
        let line = format!(
            "{}  {}  {}  {}  {}",
            text::pad(&row.name, names),
            text::pad(&row.kind, kinds),
            text::pad(&row.source, sources),
            text::pad(&row.detail, details),
            row.last
        );
        println!("{}", line.trim_end());
    }

    if listing.runners.is_empty() {
        eprintln!(
            "No runners installed. Extract one into {}",
            runners.root().display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Which launchers have a different build under the same name than the one
/// in use, since only the first copy is ever used.
fn different_note(sources: &[Source]) -> String {
    let mut names: Vec<String> = Vec::new();
    for source in sources {
        if !names.contains(&source.to_string()) {
            names.push(source.to_string());
        }
    }
    match names.as_slice() {
        [] => String::new(),
        [one] => format!("({one} has a different build)"),
        [first, second] => format!("({first} and {second} have different builds)"),
        [rest @ .., last] => format!("({}, and {last} have different builds)", rest.join(", ")),
    }
}

fn version(runner: &Runner) -> String {
    runner
        .version()
        .unwrap_or_else(|| "unknown version".to_owned())
}
