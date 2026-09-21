use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::install::{self, Wanted};
use wineo_core::installer::{Installer, Outcome, Output};
use wineo_core::launch;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::programs::Listing;
use wineo_core::runners::Runners;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::templates::{Template, Templates};

use super::{components, desktop, programs};

/// The shell's code for a command ended by Ctrl+C.
const CANCELED: u8 = 130;

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
    let guard = locks.acquire(Operation::Create, &new.name, &[&prefix])?;

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
    desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)?;

    let Some(program) = &args.install_exe else {
        println!(
            "Add a target with: wineo set {} targets.<target>.exe=<path>",
            environment.name
        );
        return Ok(ExitCode::SUCCESS);
    };
    let installer = Installer::prepare_with(guard, paths, &global, &environment.name, program)?;
    let name = &environment.name;
    let outcome = installer.run(
        Duration::from_secs(global.stop_timeout.into()),
        Output::Inherit,
        |watched| match watched {
            Watched::Notice(Notice::Stopping) => {
                note("Canceling the installer, press Ctrl+C again to force it");
            }
            Watched::Notice(Notice::Killing) => {
                note("Killing everything the installer left running");
            }
            Watched::Notice(Notice::Warning(err)) => {
                note(&format!("warning: {:#}", anyhow::Error::from(err)));
            }
            Watched::Exited(_) => {}
        },
    )?;

    match outcome {
        Outcome::Finished { exit_code } => {
            let code = exit_code.unwrap_or(1);
            match exit_code {
                Some(0) => {}
                Some(code) => eprintln!(
                    "The installer exited with code {code}. {name} and its prefix were kept, continue with `wineo setup {name}`"
                ),
                None => eprintln!("The installer's exit code could not be collected"),
            }
            match installed(&prefix) {
                Some(listing) => programs::report(name, &listing, false),
                None => println!("Add a target with: wineo set {name} targets.<target>.exe=<path>"),
            }
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Outcome::Canceled { .. } => {
            note(&format!(
                "The installer was canceled. {name} and its prefix were kept, continue with `wineo setup {name}`"
            ));
            Ok(ExitCode::from(CANCELED))
        }
        Outcome::Unresolved {
            exit_code, error, ..
        } => {
            // Both failures are reported, but the one that leaves something
            // possibly still running decides what to do next.
            note(&format!(
                "error: wineo could not confirm that everything the installer started has stopped: {:#}",
                anyhow::Error::from(error)
            ));
            match exit_code {
                Some(0) => note("The installer itself finished"),
                Some(code) => note(&format!("The installer exited with code {code}")),
                None => {}
            }
            note(&format!(
                "{name} and its prefix were kept. Clean up whatever may still be running with `wineo stop {name}` or `wineo setup {name}`"
            ));
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The terminal may already be gone after a SIGHUP, and a failed write must
/// not abort what is left to report.
fn note(message: &str) {
    let _ = writeln!(io::stderr(), "{message}");
}

/// What an installer left behind. A scan that fails is not worth failing `new`
/// over, since the environment is already made either way.
fn installed(prefix: &Path) -> Option<Listing> {
    let wine_prefix = prefix::wine_prefix_in(prefix)?;
    wineo_core::programs::scan(&wine_prefix).ok()
}
