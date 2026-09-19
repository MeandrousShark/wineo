use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::install::{self, Wanted};
use wineo_core::launch::{self, Launch};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::templates::{Template, Templates};

use super::{components, exec};

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

    /// Template to take settings from, like gaming
    #[arg(long, value_name = "NAME")]
    template: Option<String>,

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
    let template = match &args.template {
        Some(name) => Some(Templates::new(paths).resolve(name)?),
        None => None,
    };
    let from_template = |pick: fn(&Template) -> Option<String>| template.as_ref().and_then(pick);
    // An explicit flag wins over what the template says.
    let runner_name = args
        .runner
        .clone()
        .or_else(|| from_template(|template| template.runner.clone()))
        .unwrap_or_else(|| global.defaults.runner.clone());
    let arch = args
        .arch
        .or_else(|| template.as_ref().and_then(|template| template.arch));
    // Resolved before anything is created, so a typo never leaves a prefix behind.
    let runner = Runners::new(paths, &global)?.resolve(&runner_name)?;

    if runner.is_proton() && arch == Some(Arch::Win32) {
        bail!(
            "Proton prefixes are always win64, so --arch win32 can't be used with `{}`",
            runner.name()
        );
    }

    // A build the template names has to be there before a prefix is made,
    // the same way its runner does.
    if let Some(template) = &template {
        install::check_available(
            paths,
            &global,
            &name,
            &runner,
            &Wanted {
                dxvk: template.dxvk.clone(),
                vkd3d: template.vkd3d.clone(),
            },
        )?;
    }

    let new = NewEnvironment {
        name,
        dir: args.dir,
        arch,
        template,
    };
    // Held through the installer too, so nothing touches the prefix while it runs.
    let locks = Locks::new(paths);
    let prefix = new.prefix_dir(paths, &global)?;
    let _guard = locks.acquire(Operation::Create, &new.name, &[&prefix])?;

    let mut created_prefix = false;
    let environment = Environment::create(paths, &global, &runner, &new, |dir| {
        created_prefix = true;
        let dir = paths
            .contract(dir)
            .unwrap_or_else(|_| dir.display().to_string());
        if runner.is_proton() {
            eprintln!(
                "Creating a Proton prefix in {dir}. The first umu run also downloads the Steam Linux Runtime, which takes a while"
            );
        } else {
            eprintln!("Creating a wine prefix in {dir}, this takes a few seconds");
        }
    })?;
    if created_prefix {
        println!("Created environment {}", environment.name);
    } else {
        println!(
            "Created environment {} using the existing prefix at {}",
            environment.name, environment.config.prefix
        );
    }

    components::reconcile_config(
        paths,
        &global,
        &environment,
        &runner,
        &environment.prefix(paths)?,
    )?;

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
