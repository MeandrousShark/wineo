use std::process::ExitCode;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

#[derive(clap::Args)]
pub struct Args {
    /// Name of the environment
    #[arg(short, long)]
    name: Option<String>,

    /// Prefix directory, defaults to ~/.local/share/wineo/prefixes/<name>
    #[arg(long, value_name = "PATH")]
    dir: Option<String>,

    /// Runner to create the prefix with, defaults to wine
    #[arg(long, value_name = "NAME")]
    runner: Option<String>,

    /// Prefix architecture, win64 or win32
    #[arg(long, value_name = "ARCH")]
    arch: Option<Arch>,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let Some(name) = args.name else {
        bail!("guided creation isn't available yet, pass a name with -n");
    };
    let global = GlobalConfig::load(paths)?;
    let runner = Runner::resolve(args.runner.as_deref().unwrap_or(&global.defaults.runner))?;

    let mut created_prefix = false;
    let environment = Environment::create(
        paths,
        &global,
        &runner,
        &NewEnvironment {
            name,
            dir: args.dir,
            arch: args.arch,
        },
        |dir| {
            created_prefix = true;
            let dir = paths
                .contract(dir)
                .unwrap_or_else(|_| dir.display().to_string());
            eprintln!("Creating a wine prefix in {dir}, this takes a few seconds");
        },
    )?;
    if created_prefix {
        println!("Created environment {}", environment.name);
    } else {
        println!(
            "Created environment {} using the existing prefix at {}",
            environment.name, environment.config.prefix
        );
    }
    println!(
        "Add a target with: wineo set {} targets.<target>.exe=<path>",
        environment.name
    );
    Ok(ExitCode::SUCCESS)
}
