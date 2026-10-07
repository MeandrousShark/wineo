use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::create::{self, CreateEvent, CreateRequest, ExeCheck, TargetOutcome};
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
use super::{components, desktop, external_runners, programs, setup};
use crate::prompt::{self, Canceled, Prompt, Terminal};
use crate::wizard::{self, Guided, Prefill};

/// The shell's code for a command ended by Ctrl+C.
const CANCELED: u8 = 130;
const NEEDS_NAME: &str = "wineo new needs a terminal without -n. Use: wineo new -n <name> [-t <template>] [--native] [--exe <path> [-d]] [--install-exe <path>]";

#[derive(clap::Args)]
pub struct Args {
    /// Name of the environment. Without one, wineo asks for everything
    #[arg(short, long)]
    name: Option<String>,

    /// Ask for everything even with a name, starting from the flags given
    #[arg(long)]
    interactive: bool,

    /// Review what would be created without changing anything
    #[arg(long, conflicts_with = "interactive")]
    dry_run: bool,

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
    if args.dry_run && args.name.is_none() {
        bail!(NEEDS_NAME);
    }
    if args.desktop {
        wineo_core::desktop::supported(paths)?;
    }
    if args.dry_run {
        return dry_run(
            paths,
            args,
            &Access::steamgriddb(paths),
            &mut crate::output::stdout(),
            &mut crate::output::stderr(),
        );
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

fn dry_run(
    paths: &Paths,
    args: Args,
    access: &Access<'_>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<ExitCode> {
    let name = args.name.as_deref().expect("dry runs require a name");
    if let Some(installer) = &args.install_exe {
        launch::check_program_path(paths, installer)?;
    }
    let global = GlobalConfig::load(paths)?;
    if args.native {
        let plan = Environment::plan_native(paths, &global, name, args.exe.as_deref())?;
        writeln!(out, "Would create native environment {}", plan.name)?;
        if let Some(exe) = &plan.exe {
            writeln!(
                out,
                "  target: {} -> {}",
                plan.name,
                paths.contract_text(exe)
            )?;
            if args.desktop {
                writeln!(out, "  desktop entry: {}", plan.name)?;
            }
        }
    } else {
        let plan = create::plan(
            paths,
            CreateRequest {
                name: name.to_owned(),
                dir: args.dir,
                runner: args.runner,
                arch: args.arch,
                template_name: args.template,
                exe: args.exe,
                desktop: args.desktop,
                ..Default::default()
            },
        )?;
        if plan.exe_check() == Some(ExeCheck::Missing) {
            bail!("{} was not found.", plan.exe().unwrap());
        }
        let runner = plan.runner();
        if plan.template_components_omitted() {
            writeln!(
                err,
                "{} bundles its own DXVK and vkd3d-proton, so the template's are left out",
                runner.name()
            )?;
        }
        writeln!(out, "Would create environment {name}")?;
        writeln!(
            out,
            "  prefix: {} ({})",
            paths.contract(plan.prefix())?,
            if plan.adopted() {
                "existing prefix, adopted"
            } else {
                "new"
            }
        )?;
        let resolved = external_runners::with_owner(paths, runner, runner.name().to_owned());
        if runner.selection() == runner.name() {
            writeln!(out, "  runner: {resolved}")?;
        } else {
            writeln!(out, "  runner: {} -> {resolved}", runner.selection())?;
        }
        match plan.template_name() {
            Some(template) => writeln!(out, "  template: {template}, {}", plan.arch())?,
            None => writeln!(out, "  arch: {}", plan.arch())?,
        }
        let components = plan.resolved_components();
        let mut builds = Vec::new();
        if let Some(build) = &components.dxvk {
            builds.push(format!("dxvk {build}"));
        }
        if let Some(build) = &components.vkd3d {
            builds.push(format!("vkd3d-proton {build}"));
        }
        if !builds.is_empty() {
            writeln!(out, "  components: {}", builds.join(", "))?;
        }
        if let Some(exe) = plan.exe() {
            let check = if plan.exe_check() == Some(ExeCheck::Unchecked) {
                " (checked once the prefix exists)"
            } else {
                ""
            };
            writeln!(
                out,
                "  target: {name} -> {}{check}",
                paths.contract_text(exe)
            )?;
        }
        // Clap lets -d through with --install-exe, but the real run only
        // makes an entry for an --exe target.
        if plan.desktop() && plan.exe().is_some() {
            writeln!(out, "  desktop entry: {}", plan.target_name())?;
        }
        if let Some(installer) = args.install_exe {
            writeln!(out, "  installer: {}", paths.contract_text(&installer))?;
        }
    }
    if args.art {
        access.review(name, global.art.fetch, out, err);
    }
    writeln!(out, "Dry run, nothing was created.")?;
    Ok(ExitCode::SUCCESS)
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
        bail!(NEEDS_NAME);
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
    let mut prompt =
        Prompt::new(Terminal::open()?, crate::output::stdout()).with_first_question(true);
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
    if global.art.fetch.kinds().is_empty() {
        return Ok(());
    }
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
        .map(|input| Prompt::new(input, crate::output::stdout()));
    let mut stdout = crate::output::stdout();
    let pick = match &mut prompt {
        Some(prompt) => Pick::Ask(prompt),
        None => Pick::Top(&mut stdout),
    };
    access.fetch(paths, &key, &request, pick, &mut crate::output::stderr())
}

/// The terminal may already be gone after a SIGHUP, and a failed write must
/// not abort what is left to report.
fn note(message: &str) {
    let _ = writeln!(crate::output::stderr(), "{message}");
}

/// What an installer left behind. A scan that fails is not worth failing `new`
/// over, since the environment is already made either way.
fn installed(prefix: &Path) -> Option<Listing> {
    let wine_prefix = prefix::wine_prefix_in(prefix)?;
    wineo_core::programs::scan(&wine_prefix).ok()
}

#[cfg(test)]
use crate::snapshot;

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use wineo_core::steamgriddb::{self, NoKeyring};

    use super::super::art::fake::Fake;
    use super::*;

    fn paths(root: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
    }

    fn args(native: bool) -> Args {
        Args {
            name: Some("hades".to_owned()),
            interactive: false,
            dry_run: true,
            dir: None,
            runner: None,
            template: None,
            arch: None,
            install_exe: None,
            art: true,
            native,
            exe: None,
            desktop: false,
        }
    }

    fn saved_key(paths: &Paths) -> KeyCheck {
        match steamgriddb::find_key(paths, |_| None, &NoKeyring) {
            Ok(Some(key)) => KeyCheck::Found(key),
            Ok(None) => KeyCheck::Missing,
            Err(error) => KeyCheck::Failed(error.into()),
        }
    }

    fn review(fake: Fake, native: bool) -> (String, String) {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let key_file = steamgriddb::key_file(&paths);
        fs::create_dir_all(key_file.parent().unwrap()).unwrap();
        fs::write(&key_file, "test-key\n").unwrap();
        // An installed runner avoids global PATH changes in these parallel tests.
        let runner_dir = paths.default_runner_root().join("test-wine/bin");
        fs::create_dir_all(&runner_dir).unwrap();
        for exe in ["wine", "wineserver"] {
            let path = runner_dir.join(exe);
            fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut args = args(native);
        if !native {
            args.runner = Some("test-wine".to_owned());
        }
        let access = Access {
            check_key: Box::new(|| saved_key(&paths)),
            transport: Box::new(|| Ok(Box::new(fake.clone()))),
        };
        let before = snapshot::Snapshot::capture(root.path()).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            dry_run(&paths, args, &access, &mut out, &mut err).unwrap(),
            ExitCode::SUCCESS
        );
        before.assert_unchanged(root.path());
        assert_eq!(wineo_core::art::saved_game(&paths, "hades").unwrap(), None);
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn dry_run_art_lists_only_three_matches_without_saving_for_wine_and_native() {
        let mut fake = Fake::default();
        fake.0.insert(
            format!("{}/search/autocomplete/hades", steamgriddb::API),
            (200, br#"{"success":true,"data":[{"id":7,"name":"Hades","release_date":1600000000},{"id":8,"name":"Hades II"},{"id":9,"name":"Hades Demo"},{"id":10,"name":"Fourth"}]}"#.to_vec()),
        );
        for native in [false, true] {
            let (out, err) = review(fake.clone(), native);
            assert!(
                out.ends_with(concat!(
                    "Art from SteamGridDB, searching \"hades\":\n",
                    "  1. Hades (2020), game 7 <- would be picked automatically\n",
                    "  2. Hades II, game 8\n",
                    "  3. Hades Demo, game 9\n",
                    "Dry run, nothing was created.\n"
                )),
                "{out}"
            );
            assert!(!out.contains("Fourth"), "{out}");
            assert_eq!(err, "");
        }
    }

    #[test]
    fn dry_run_art_search_failures_warn_without_failing_or_writing() {
        let mut empty = Fake::default();
        empty.search("hades", false);
        let mut rejected = Fake::default();
        rejected.0.insert(
            format!("{}/search/autocomplete/hades", steamgriddb::API),
            (401, Vec::new()),
        );
        for (fake, message) in [
            (Fake::default(), "connection to"),
            (empty, "no SteamGridDB games match `hades`"),
            (rejected, "SteamGridDB rejected the key"),
        ] {
            let (out, err) = review(fake, true);
            assert_eq!(
                out,
                "Would create native environment hades\nDry run, nothing was created.\n"
            );
            assert!(err.starts_with("warning: art fetch failed: "), "{err}");
            assert!(err.contains(message), "{err}");
        }
    }

    #[test]
    fn dry_run_art_skips_connection_when_key_lookup_fails_or_is_disabled() {
        for disabled in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let paths = paths(root.path());
            if disabled {
                let config = paths.global_config();
                fs::create_dir_all(config.parent().unwrap()).unwrap();
                fs::write(config, "[art]\nfetch = []\n").unwrap();
            }
            let checked = Cell::new(false);
            let access = Access {
                check_key: Box::new(|| {
                    checked.set(true);
                    KeyCheck::Failed(anyhow::anyhow!("the keyring failed: dismissed"))
                }),
                transport: Box::new(|| panic!("skipped art must not connect")),
            };
            let before = snapshot::Snapshot::capture(root.path()).unwrap();
            let mut out = Vec::new();
            let mut err = Vec::new();
            assert_eq!(
                dry_run(&paths, args(true), &access, &mut out, &mut err).unwrap(),
                ExitCode::SUCCESS
            );
            before.assert_unchanged(root.path());
            assert_eq!(checked.get(), !disabled);
            assert_eq!(
                String::from_utf8(err).unwrap(),
                if disabled {
                    ""
                } else {
                    "warning: the keyring failed: dismissed, skipping art\n"
                }
            );
        }
    }
}
