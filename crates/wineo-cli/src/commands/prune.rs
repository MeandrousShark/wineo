use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Selector};

use crate::prompt;

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("selection").required(true).args(["id", "older_than", "all"])))]
pub struct Args {
    /// Delete one trash entry by ID
    #[arg(conflicts_with_all = ["older_than", "all"], add = clap_complete::ArgValueCompleter::new(crate::completion::trash_ids))]
    id: Option<String>,

    /// Delete entries strictly older than a positive number of days, e.g. 30d
    #[arg(long, value_name = "DAYS", conflicts_with = "all")]
    older_than: Option<String>,

    /// Delete every restorable trash entry
    #[arg(long)]
    all: bool,

    /// Confirm deletion without a terminal
    #[arg(long)]
    yes: bool,
}

fn parse_age(text: &str) -> anyhow::Result<u32> {
    let Some(days) = text.strip_suffix('d') else {
        bail!("age must be positive whole days, such as 30d");
    };
    if days.starts_with('0') || days.is_empty() || !days.bytes().all(|b| b.is_ascii_digit()) {
        bail!("age must be positive whole days, such as 30d");
    }
    let days: u32 = days.parse()?;
    if days == 0 {
        bail!("age must be positive whole days");
    }
    Ok(days)
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let selection = match (&args.id, &args.older_than, args.all) {
        (Some(id), None, false) => Selector::Id(id),
        (None, Some(age), false) => Selector::OlderThan(parse_age(age)?),
        (None, None, true) => Selector::All,
        _ => bail!("select a trash entry, --older-than, or --all"),
    };
    let review = trash::review_prune(paths, selection)?;
    for (id, error) in &review.errors {
        eprintln!("Could not review trash entry {id}: {error}");
    }
    if review.entries.is_empty() {
        println!("No trash entries selected.");
        return Ok(if review.errors.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    println!("Permanently delete these trash entries:");
    for entry in &review.entries {
        println!(
            "  {}  {}  {}  prefix: {}",
            entry.id,
            entry.name,
            entry.removed_at,
            if entry.include_prefix {
                "included"
            } else {
                "not included"
            }
        );
    }
    if !args.yes {
        if !prompt::interactive() {
            bail!("prune requires a terminal or --yes; nothing was deleted");
        }
        print!("Delete permanently? [y/N] ");
        crate::output::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            println!("Canceled. Nothing was deleted.");
            return Ok(ExitCode::from(130));
        }
    }
    let mut failed = review.errors.len();
    for (id, result) in trash::prune_reviewed(paths, &review) {
        match result {
            Ok(()) => println!("Pruned {id}"),
            Err(err) => {
                failed += 1;
                eprintln!("Failed to prune {id}: {err:#}");
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} trash entries still need attention; run `wineo validate --fix`");
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::parse_age;

    #[test]
    fn age_requires_positive_whole_days() {
        assert_eq!(parse_age("30d").unwrap(), 30);
        for bad in ["0d", "-1d", "1.5d", "1w", "01d", "d", "4294967296d"] {
            assert!(parse_age(bad).is_err(), "{bad}");
        }
    }
}
