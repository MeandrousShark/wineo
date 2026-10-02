use std::process::ExitCode;

use clap::Subcommand;
use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

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

/// One line: what it's called, what it is, what version, and where. Only the
/// system wine fills the path column, since the rest live in the runners root.
struct Row {
    name: String,
    kind: String,
    detail: String,
    path: String,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if let Some(Action::Remove(args)) = args.action {
        return super::remove_build::run(
            paths,
            &args.name,
            "runner",
            wineo_core::build_lock::Kind::Runner,
            args.yes,
            |paths, name| wineo_core::runners::size(paths, name).map_err(anyhow::Error::from),
            |paths, name| wineo_core::runners::remove(paths, name).map_err(anyhow::Error::from),
        );
    }

    let global = GlobalConfig::load(paths)?;
    let runners = Runners::new(paths, &global)?;
    let listing = runners.list()?;

    // The system wine leads, since that is what every config starts out using.
    let mut rows = vec![match Runner::system() {
        Ok(runner) => Row {
            name: Runner::SYSTEM.to_owned(),
            kind: runner.kind_name().to_owned(),
            detail: version(&runner),
            path: runner
                .wine_path()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        },
        Err(err) => Row {
            name: Runner::SYSTEM.to_owned(),
            kind: String::new(),
            detail: err.to_string(),
            path: String::new(),
        },
    }];
    rows.extend(listing.runners.iter().map(|runner| Row {
        name: runner.name().to_owned(),
        kind: runner.kind_name().to_owned(),
        detail: version(runner),
        path: String::new(),
    }));
    for alias in wineo_core::runner_alias::ALIASES {
        let detail = match runners.resolve(alias) {
            Ok(runner) => format!("-> {}", runner.name()),
            Err(err) => err.to_string(),
        };
        rows.push(Row {
            name: alias.to_owned(),
            kind: "alias".to_owned(),
            detail,
            path: String::new(),
        });
    }
    if args.all {
        rows.extend(listing.skipped.iter().map(|(name, reason)| Row {
            name: name.clone(),
            kind: String::new(),
            detail: format!("skipped, {}", reason.as_str()),
            path: String::new(),
        }));
    }

    let width = |column: fn(&Row) -> &str| {
        rows.iter()
            .map(|row| column(row).len())
            .max()
            .unwrap_or_default()
    };
    let (names, kinds, details) = (
        width(|row| &row.name),
        width(|row| &row.kind),
        width(|row| &row.detail),
    );
    for row in &rows {
        let line = format!(
            "{:names$}  {:kinds$}  {:details$}  {}",
            row.name, row.kind, row.detail, row.path
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

fn version(runner: &Runner) -> String {
    runner
        .version()
        .unwrap_or_else(|| "unknown version".to_owned())
}
