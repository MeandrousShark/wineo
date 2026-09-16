use std::fs;
use std::io;
use std::process::ExitCode;

use anyhow::{Context, bail};
use wineo_core::name;
use wineo_core::paths::Paths;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to show, or the global config if left out
    name: Option<String>,

    /// Print the file's path instead of its contents
    #[arg(long)]
    path: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    // Deliberately doesn't parse the file, so a broken config can still be found and fixed.
    let file = match &args.name {
        Some(env) if !name::is_valid(env) => bail!(wineo_core::Error::InvalidName(env.clone())),
        Some(env) => {
            let file = paths.environment_config(env);
            if !file.exists() {
                bail!(wineo_core::Error::UnknownEnvironment(env.clone()));
            }
            file
        }
        None => paths.global_config(),
    };

    if args.path {
        println!("{}", file.display());
        return Ok(ExitCode::SUCCESS);
    }
    match fs::read_to_string(&file) {
        Ok(text) => print!("{text}"),
        Err(err) if err.kind() == io::ErrorKind::NotFound && args.name.is_none() => {
            eprintln!(
                "No global config at {}, defaults are in use",
                file.display()
            );
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", file.display()));
        }
    }
    Ok(ExitCode::SUCCESS)
}
