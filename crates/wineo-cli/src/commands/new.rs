use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::create::{self, CreateEvent, CreateRequest, TargetOutcome};
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::installer::{Installer, Outcome, Output};
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::programs::Listing;
use wineo_core::runtime::Runtime;
use wineo_core::supervisor::{Notice, Watched};

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
    let mut cancellation = Some(Cancellation::new()?);
    let token = cancellation.as_ref().unwrap().token.clone();
    let created = match create::run(
        paths,
        CreateRequest {
            name,
            dir: args.dir,
            runner: args.runner,
            arch: args.arch,
            template_name: args.template,
            exe: args.exe.clone(),
            desktop: args.desktop,
            ..Default::default()
        },
        &token,
        |event| match event {
            CreateEvent::TemplateComponentsOmitted { runner } => eprintln!(
                "{runner} bundles its own DXVK and vkd3d-proton, so the template's are left out"
            ),
            CreateEvent::BuildingPrefix { dir, runner } => {
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
            CreateEvent::Saved => drop(cancellation.take()),
            CreateEvent::Registered {
                name,
                stored_prefix,
                created_prefix,
                target,
            } => {
                if target != TargetOutcome::ExeNotFound {
                    println!(
                        "{}",
                        created_message(
                            &name,
                            &stored_prefix,
                            created_prefix,
                            paths,
                            target == TargetOutcome::Added
                        )
                    );
                }
            }
            CreateEvent::Components(changes) => {
                for change in changes {
                    eprintln!(
                        "{}",
                        components::describe(&change, args.name.as_deref().unwrap())
                    );
                }
            }
            CreateEvent::Desktop(changes) => {
                for change in changes {
                    eprintln!(
                        "{}",
                        desktop::describe(&change, args.name.as_deref().unwrap())
                    );
                }
            }
        },
    ) {
        Ok(created) => created,
        Err(err)
            if err
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled(_))) =>
        {
            eprintln!("Canceled. Nothing was created.");
            return Ok(ExitCode::from(CANCELED));
        }
        Err(err) => return Err(err),
    };
    drop(cancellation);
    let create::Created {
        global,
        environment,
        runner,
        prefix,
        target,
        guard,
        ..
    } = created;
    if target == TargetOutcome::ExeNotFound {
        if let Some(exe) = &args.exe {
            eprintln!(
                "Created environment {}, but {exe} was not found. Add a target with: wineo setup {}",
                environment.name, environment.name
            );
        }
        return Ok(ExitCode::FAILURE);
    }
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
    name: &str,
    stored_prefix: &str,
    created_prefix: bool,
    paths: &Paths,
    with_target: bool,
) -> String {
    let mut message = format!("Created environment {name}");
    if with_target {
        message.push_str(&format!(" with target {name}"));
    }
    if !created_prefix {
        let prefix = paths.shown(stored_prefix);
        if with_target {
            message.push_str(&format!(", using the existing prefix at {prefix}"));
        } else {
            message.push_str(&format!(" using the existing prefix at {prefix}"));
        }
    }
    message
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
