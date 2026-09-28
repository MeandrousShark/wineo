use std::fs::File;
use std::io;
use std::process::ExitCode;

use anyhow::Context;
use wineo_core::cancel::Token;
use wineo_core::environment::Environment;
use wineo_core::log::{Log, Tail};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, Runtime, State};

#[derive(clap::Args)]
pub struct Args {
    /// Environment whose log to print
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Keep printing new output as it's written
    #[arg(short, long, conflicts_with = "previous")]
    follow: bool,

    /// Print only the last N lines
    #[arg(short = 'n', long = "lines", value_name = "N")]
    lines: Option<u64>,

    /// Show the run before the latest one
    #[arg(long)]
    previous: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    // Deliberately doesn't load the config, so a broken one doesn't block reading logs.
    Environment::ensure_exists(paths, &args.name)?;
    let runtime = Runtime::new(paths, &args.name);

    let path = if args.previous {
        runtime.previous_log_path()
    } else {
        runtime.log_path()
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if args.previous {
                eprintln!("{} has no previous log", args.name);
            } else {
                eprintln!(
                    "{} has no log yet, only runs started with `wineo start` are logged",
                    args.name
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        Err(err) => return Err(err).with_context(|| format!("failed to open {}", path.display())),
    };

    // Attached runs aren't logged, so this log is from an earlier detached run.
    // An unreadable state file just means there's no note.
    let attached = runtime.status().is_ok_and(|status| {
        status.state == State::Running
            && status.run.is_some_and(|run| run.mode == RunMode::Attached)
    });
    if attached {
        eprintln!(
            "{} is running attached, so its output is in that terminal",
            args.name
        );
    }

    // Only the current log is ever cleared, so only that one needs coordination.
    let session = (!args.previous).then(|| Log::new(runtime.log_path()));
    let mut reader = Tail::new(file, session);
    if let Some(lines) = args.lines {
        reader.seek_to_last_lines(lines)?;
    }

    let mut stdout = io::stdout().lock();
    let open = reader.stream(&mut stdout)?;

    // Attached mode has no active detached run to follow, and this log is history.
    if open && args.follow && !attached && runtime.is_running()? {
        reader.follow(&runtime, &path, &mut stdout, &Token::new())?;
    }
    Ok(ExitCode::SUCCESS)
}
