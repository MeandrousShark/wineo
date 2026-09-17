use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::launch::{self, Launch};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

use super::exec;

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

    /// Run an installer after creating the environment
    #[arg(long, value_name = "PATH")]
    install_exe: Option<String>,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    // Checked up front so a typo fails before the prefix creation wait.
    if let Some(installer) = &args.install_exe {
        launch::check_program_path(paths, installer)?;
    }
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

    let mut installer_exit = None;
    if let Some(installer) = &args.install_exe {
        let launch = Launch::exec(
            paths,
            &global,
            &environment,
            installer,
            &[],
            &BTreeMap::new(),
        )?;
        let code = exec::run_attached(&launch)?;
        // Installers often hand off to another process and exit early.
        launch.runner.wait_idle(&launch.prefix)?;
        if code != 0 {
            eprintln!("The installer exited with code {code}");
            installer_exit = Some(code);
        }
    }

    println!(
        "Add a target with: wineo set {} targets.<target>.exe=<path>",
        environment.name
    );
    Ok(match installer_exit {
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::SUCCESS,
    })
}
