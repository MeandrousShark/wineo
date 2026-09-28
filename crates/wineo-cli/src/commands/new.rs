use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::error::Error;
use wineo_core::install::{self, Wanted};
use wineo_core::installer::{Installer, Outcome, Output};
use wineo_core::launch;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix::{self, PrefixEvent};
use wineo_core::programs::Listing;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{self, Batch, Spec, Written};
use wineo_core::templates::Templates;

use super::art::{Access, AutoFetch, KeyCheck, Pick};
use super::transfer::Cancellation;
use super::{components, desktop, programs, setup};
use crate::prompt::{self, Canceled, Prompt, Terminal};
use crate::wizard::{self, Guided, Prefill};

/// The shell's code for a command ended by Ctrl+C.
const CANCELED: u8 = 130;

#[derive(clap::Args)]
pub struct Args {
    /// Name of the environment. Without one, wineo asks for everything
    #[arg(short, long)]
    name: Option<String>,

    /// Ask for everything even with a name, starting from the flags given
    #[arg(long)]
    interactive: bool,

    /// Prefix directory, defaults to ~/.local/share/wineo/prefixes/<name>, or DIR/prefixes/<name> with --portable
    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    dir: Option<String>,

    /// Runner to create the prefix with, defaults to wine
    #[arg(long, value_name = "NAME")]
    runner: Option<String>,

    /// Template to take settings from, like gaming
    #[arg(short, long, value_name = "NAME")]
    template: Option<String>,

    /// Prefix architecture, win64 or win32
    #[arg(long, value_name = "ARCH")]
    arch: Option<Arch>,

    /// Run an installer after creating the environment
    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::FilePath)]
    install_exe: Option<String>,

    /// Fetch art from SteamGridDB, searching the environment's name
    #[arg(long)]
    art: bool,

    /// Make the environment native, running Linux programs instead of Windows ones
    #[arg(long, conflicts_with_all = ["dir", "runner", "template", "arch", "install_exe"])]
    native: bool,

    /// Program for the default target, added once the environment exists: a
    /// path or a name on PATH for --native, a Unix or Windows path for Wine
    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::FilePath, conflicts_with = "install_exe")]
    exe: Option<String>,

    /// Make a desktop entry for the --exe target
    #[arg(short = 'd', long = "desktop", requires = "exe")]
    desktop: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if args.desktop {
        wineo_core::desktop::supported(paths)?;
    }
    let name = match args.name.clone() {
        Some(name) if !args.interactive => name,
        _ => return guided(paths, args),
    };
    if args.native {
        return new_native(paths, &name, args.exe.as_deref(), args.desktop, args.art);
    }
    // Checked up front so a typo fails before the prefix creation wait.
    if let Some(installer) = &args.install_exe {
        launch::check_program_path(paths, installer)?;
    }
    let global = GlobalConfig::load(paths)?;
    let template = match &args.template {
        Some(name) => Some(Templates::new(paths).resolve(name)?),
        None => None,
    };
    let runners = Runners::new(paths, &global)?;
    // An explicit flag wins over what the template says.
    let runner_name = match (&args.runner, &template) {
        (Some(runner), _) => runner.clone(),
        (None, Some(template)) => template
            .runner_name(&runners, &global.defaults.runner)?
            .unwrap_or_else(|| global.defaults.runner.clone()),
        (None, None) => global.defaults.runner.clone(),
    };
    let arch = args
        .arch
        .or_else(|| template.as_ref().and_then(|template| template.arch));
    // Resolved before anything is created, so a typo never leaves a prefix behind.
    let runner = runners.resolve(&runner_name)?;

    if runner.is_proton() && arch == Some(Arch::Win32) {
        bail!(
            "Proton prefixes are always win64, so --arch win32 can't be used with `{}`",
            runner.name()
        );
    }

    let template = template.map(|template| {
        let (fitted, left_out) = template.for_runner(&runner);
        if left_out {
            eprintln!(
                "{} bundles its own DXVK and vkd3d-proton, so the template's are left out",
                runner.name()
            );
        }
        fitted
    });
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

    let cancellation = Cancellation::new()?;
    let log = paths.runtime_dir(&new.name).join("create.log");
    let mut created_prefix = false;
    let environment = match Environment::create(
        paths,
        &global,
        &runner,
        &new,
        &cancellation.token,
        Output::Log(&log),
        |event| {
            if let PrefixEvent::Started { dir } = event {
                created_prefix = true;
                let dir = paths
                    .contract(&dir)
                    .unwrap_or_else(|_| dir.display().to_string());
                if runner.is_proton() {
                    eprintln!(
                        "Creating a Proton prefix in {dir}. The first umu run also downloads the Steam Linux Runtime, which takes a while"
                    );
                } else {
                    eprintln!("Creating a wine prefix in {dir}, this takes a few seconds");
                }
            }
        },
    ) {
        Ok(environment) => environment,
        Err(Error::Canceled(_)) => {
            eprintln!("Canceled. Nothing was created.");
            return Ok(ExitCode::from(CANCELED));
        }
        Err(err) => return Err(err.into()),
    };
    // Released so the art picker and other prompts get Ctrl+C as before.
    drop(cancellation);
    let environment = match &args.exe {
        Some(exe) if add_default_target(paths, &environment, exe, args.desktop)? => {
            println!(
                "{}",
                created_message(&environment, created_prefix, paths, true)?
            );
            Environment::load(paths, &environment.name)?
        }
        Some(exe) => {
            eprintln!(
                "Created environment {}, but {exe} was not found. Add a target with: wineo setup {}",
                environment.name, environment.name
            );
            return Ok(ExitCode::FAILURE);
        }
        None => {
            println!(
                "{}",
                created_message(&environment, created_prefix, paths, false)?
            );
            environment
        }
    };

    components::reconcile_config(
        paths,
        &global,
        &environment,
        &runner,
        &environment.prefix(paths)?,
    )?;
    desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)?;
    // Before the installer, so the pick list isn't left waiting behind it.
    if args.art && fetch_art(paths, &global, &environment.name).is_err() {
        let name = &environment.name;
        eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
        return Ok(ExitCode::from(CANCELED));
    }

    let Some(program) = &args.install_exe else {
        if args.exe.is_none() {
            println!(
                "Add a target with: wineo set {} targets.<target>.exe=<path>",
                environment.name
            );
        }
        return Ok(ExitCode::SUCCESS);
    };
    let installer = match Installer::prepare_with_runner(
        guard,
        paths,
        &global,
        &environment.name,
        program,
        &runner,
    ) {
        Ok(installer) => installer,
        Err(err) => {
            note(&format!("error: {:#}", anyhow::Error::from(err)));
            note(&setup::recovery_message(paths, &environment.name));
            return Ok(ExitCode::FAILURE);
        }
    };
    let name = &environment.name;
    let cancellation = Cancellation::new()?;
    let outcome = match installer.run(
        &cancellation.token,
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
            Watched::Notice(Notice::Heartbeat) => {}
            Watched::Exited(_) => {}
        },
    ) {
        Ok(outcome) => outcome,
        Err(err) => {
            note(&format!("error: {:#}", anyhow::Error::from(err)));
            note(&setup::recovery_message(paths, &environment.name));
            return Ok(ExitCode::FAILURE);
        }
    };

    match outcome {
        Outcome::Finished { exit_code } => {
            let code = exit_code.unwrap_or(1);
            match exit_code {
                Some(0) => {}
                Some(code) => eprintln!("The installer exited with code {code}."),
                None => eprintln!("The installer's exit code could not be collected"),
            }
            if Runtime::new(paths, name).check_resolved(name).is_err() {
                note("The installer finished, but cleanup is unresolved.");
                note(&setup::recovery_message(paths, name));
                return Ok(ExitCode::FAILURE);
            }
            if exit_code != Some(0) {
                note(&setup::recovery_message(paths, name));
            }
            match installed(&prefix) {
                Some(listing) => programs::report(name, &listing, false),
                None => println!("Add a target with: wineo set {name} targets.<target>.exe=<path>"),
            }
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Outcome::Canceled { .. } => {
            note("Installer canceled.");
            note(&setup::recovery_message(paths, name));
            Ok(ExitCode::from(CANCELED))
        }
        Outcome::Unresolved {
            exit_code, error, ..
        } => {
            // Both failures are reported, but the one that leaves something
            // possibly still running decides what to do next.
            note(&format!(
                "error: couldn't confirm the installer's processes stopped: {:#}",
                anyhow::Error::from(error)
            ));
            match exit_code {
                Some(0) => note("The installer itself finished"),
                Some(code) => note(&format!("The installer exited with code {code}")),
                None => {}
            }
            note(&setup::recovery_message(paths, name));
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Creates a native environment, with a default target named after it
/// running `exe` when one is given.
fn new_native(
    paths: &Paths,
    name: &str,
    exe: Option<&str>,
    desktop: bool,
    art: bool,
) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::create_native(paths, &global, name, exe, desktop)?;
    match exe {
        Some(_) => println!("Created native environment {name} with target {name}"),
        None => {
            println!("Created native environment {name}");
            println!("Add a target with: wineo setup {name}");
        }
    }
    desktop::reconcile_native(paths, &global, &environment)?;
    if art && fetch_art(paths, &global, name).is_err() {
        eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
        return Ok(ExitCode::from(CANCELED));
    }
    Ok(ExitCode::SUCCESS)
}

/// The plain "Created environment" line, extended with the default target
/// `--exe` added, and the "existing prefix" note carried over either way.
fn created_message(
    environment: &Environment,
    created_prefix: bool,
    paths: &Paths,
    with_target: bool,
) -> anyhow::Result<String> {
    let mut message = format!("Created environment {}", environment.name);
    if with_target {
        message.push_str(&format!(" with target {}", environment.name));
    }
    if !created_prefix {
        let prefix = paths.shown(&environment.wine()?.prefix);
        if with_target {
            message.push_str(&format!(", using the existing prefix at {prefix}"));
        } else {
            message.push_str(&format!(" using the existing prefix at {prefix}"));
        }
    }
    Ok(message)
}

/// Adds `exe` as the environment's default target, named after it, checked
/// against the prefix that now exists. `Ok(false)` means the exe wasn't
/// found, so the environment is kept without a target.
fn add_default_target(
    paths: &Paths,
    environment: &Environment,
    exe: &str,
    desktop: bool,
) -> anyhow::Result<bool> {
    let prefix = environment.prefix(paths)?;
    if wineo_core::desktop::check(paths, &prefix, exe).is_err() {
        return Ok(false);
    }
    let mut batch = Batch::new(environment.config.targets.clone());
    batch.add(
        &environment.name,
        Spec {
            exe: exe.to_owned(),
            args: Vec::new(),
            cwd: None,
            name: None,
            desktop,
        },
    )?;
    batch.set_default(Some(&environment.name));
    match targets::write(paths, &environment.name, &batch)? {
        Written::Saved => Ok(true),
        Written::Changed(_) => bail!(
            "{}'s config changed while its target was being added",
            environment.name
        ),
    }
}

/// Asks for everything, starting from whatever flags were given.
fn guided(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if !prompt::interactive() {
        bail!(
            "wineo new needs a terminal without -n. Use: wineo new -n <name> [-t <template>] [--native] [--exe <path> [-d]] [--install-exe <path>]"
        );
    }
    let global = GlobalConfig::load(paths)?;
    let prefill = Prefill {
        name: args.name,
        dir: args.dir,
        runner: args.runner,
        template: args.template,
        arch: args.arch,
        install_exe: args.install_exe,
        native: args.native,
        exe: args.exe,
    };
    let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
    let mut guided = Guided { registered: None };
    let access = Access::steamgriddb(paths);
    match wizard::run(paths, &global, &prefill, &mut prompt, &mut guided, &access) {
        Err(err) if err.is::<Canceled>() => {
            match guided.registered {
                Some(name) => {
                    eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.")
                }
                None => eprintln!("Canceled. Nothing was created."),
            }
            Ok(ExitCode::from(CANCELED))
        }
        other => other,
    }
}

/// Fetches art searching the environment's name, with the pick list when
/// there is a terminal and the top match otherwise.
fn fetch_art(paths: &Paths, global: &GlobalConfig, name: &str) -> Result<(), Canceled> {
    let access = Access::steamgriddb(paths);
    let key = match (access.check_key)() {
        KeyCheck::Found(key) => key,
        check => {
            check.print_skip();
            return Ok(());
        }
    };
    let request = AutoFetch {
        env: name,
        term: name,
        kinds: global.art.fetch,
    };
    let mut prompt = prompt::interactive()
        .then(|| Terminal::open().ok())
        .flatten()
        .map(|input| Prompt::new(input, io::stdout()));
    let mut stdout = io::stdout();
    let pick = match &mut prompt {
        Some(prompt) => Pick::Ask(prompt),
        None => Pick::Top(&mut stdout),
    };
    access.fetch(paths, &key, &request, pick, &mut io::stderr())
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
