use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::ValueHint;

#[derive(clap::Args)]
pub struct Args {
    /// Folder to write the man pages into, created if missing
    #[arg(value_name = "DIR", value_hint = ValueHint::DirPath)]
    dir: PathBuf,
}

pub fn run(command: clap::Command, args: Args) -> anyhow::Result<ExitCode> {
    fs::create_dir_all(&args.dir).with_context(|| format!("create {}", args.dir.display()))?;
    // Counted here because generate_to doesn't report how many pages it wrote.
    let pages = count_pages(&command);
    clap_mangen::generate_to(command, &args.dir)
        .with_context(|| format!("write man pages to {}", args.dir.display()))?;
    println!("Wrote {pages} pages to {}.", args.dir.display());
    Ok(ExitCode::SUCCESS)
}

// Mirrors generate_to, which skips hidden commands and their children.
fn count_pages(command: &clap::Command) -> usize {
    1 + command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
        .map(count_pages)
        .sum::<usize>()
}
