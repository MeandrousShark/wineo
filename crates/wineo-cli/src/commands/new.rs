use std::cell::Cell;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::cancel::Token;
use wineo_core::config::{Arch, ArtFetch, GlobalConfig};
use wineo_core::create::{
    self, AfterInstall, CreateEvent, CreateRequest, ExeCheck, Hooks, Step, Stepped, TargetOutcome,
};
use wineo_core::environment::Environment;
use wineo_core::installer::{Outcome, Output};
use wineo_core::launch::{self, Launch};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::programs::Listing;
use wineo_core::steamgriddb::Key;
use wineo_core::supervisor::{Notice, Watched};

use super::art::{Access, AutoFetch, KeyCheck, Pick, Picked};
use super::transfer::Cancellation;
use super::{components, desktop, external_runners, programs, setup};
use crate::cancel;
use crate::prompt::{self, Canceled, Prompt, Terminal};
use crate::wizard::{self, Guided, Prefill};

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
    let access = Access::steamgriddb(paths);
    if args.native {
        return new_native(
            paths,
            &name,
            args.exe.as_deref(),
            args.desktop,
            args.art.then_some(&access),
        );
    }
    new_wine(
        paths,
        args,
        &name,
        &access,
        &Token::new(),
        &mut crate::output::stdout(),
    )
}

/// Creates a Wine environment, and runs the installer in it when asked.
fn new_wine(
    paths: &Paths,
    args: Args,
    name: &str,
    access: &Access<'_>,
    install_token: &Token,
    out: &mut dyn Write,
) -> anyhow::Result<ExitCode> {
    // Checked up front so a typo fails before the prefix creation wait.
    if let Some(installer) = &args.install_exe {
        launch::check_program_path(paths, installer)?;
    }
    let request = || CreateRequest {
        name: name.to_owned(),
        dir: args.dir.clone(),
        runner: args.runner.clone(),
        arch: args.arch,
        template_name: args.template.clone(),
        exe: args.exe.clone(),
        desktop: args.desktop,
        ..Default::default()
    };
    // The questions come before the create lock is taken, and before the
    // installer's wait, and the download once the installer has ended. A
    // request that can't be created is refused first, so nobody answers
    // questions for nothing.
    let picked = if args.art && args.install_exe.is_some() {
        create::plan(paths, request())?;
        match pick_art(paths, name, access, out) {
            Ok(picked) => picked,
            Err(err) if cancel::prompted(&err) => {
                eprintln!("Canceled. Nothing was created.");
                return Ok(cancel::exit_code());
            }
            Err(err) => return Err(err),
        }
    } else {
        None
    };
    let step = args.install_exe.as_deref().map(|program| Step::Installer {
        program,
        output: Output::Inherit,
        token: install_token,
    });
    let mut hooks = WineHooks {
        paths,
        exe: args.exe.as_deref(),
        name,
        cancellation: Some(Cancellation::new()?),
        install_token: install_token.clone(),
    };
    let token = hooks.cancellation.as_ref().unwrap().token.clone();
    let created = match create::create_and_install(paths, request(), step, &token, &mut hooks) {
        Ok(created) => created,
        Err(err) if cancel::core(&err) => {
            eprintln!("Canceled. Nothing was created.");
            return Ok(cancel::exit_code());
        }
        Err(err) => return Err(err),
    };
    let name = &created.environment.name;
    if let Stepped::None = created.step {
        // The create lock is already released, so a slow SteamGridDB only
        // holds up this command.
        if args.art && fetch_art(paths, &created.global, name, access, out).is_err() {
            eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
            return Ok(cancel::exit_code());
        }
        if args.exe.is_none() {
            println!("Add a target with: wineo set {name} targets.<target>.exe=<path>");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let ended = Ended {
        paths,
        access,
        name,
        prefix: &created.prefix,
        kinds: created.global.art.fetch,
        picked: picked.as_ref(),
    };
    ended.report(created.step, &mut hooks.cancellation, out)
}

/// An environment whose installer has ended, and the art picked for it.
struct Ended<'a> {
    paths: &'a Paths,
    access: &'a Access<'a>,
    name: &'a str,
    prefix: &'a Path,
    kinds: ArtFetch,
    picked: Option<&'a (Key, Picked)>,
}

impl Ended<'_> {
    /// Reports how the installer ended and returns the exit code. The art
    /// picked before creation is downloaded only when the installer finished,
    /// and otherwise `new` says it wasn't.
    fn report(
        &self,
        step: Stepped<anyhow::Result<ExitCode>>,
        cancellation: &mut Option<Cancellation>,
        out: &mut dyn Write,
    ) -> anyhow::Result<ExitCode> {
        let downloaded = Cell::new(false);
        let result = self.report_step(step, cancellation, out, &downloaded);
        if self.picked.is_some() && !downloaded.get() {
            let _ = writeln!(out, "{}", art_not_fetched(self.name));
        }
        result
    }

    fn report_step(
        &self,
        step: Stepped<anyhow::Result<ExitCode>>,
        cancellation: &mut Option<Cancellation>,
        out: &mut dyn Write,
        downloaded: &Cell<bool>,
    ) -> anyhow::Result<ExitCode> {
        let (paths, name) = (self.paths, self.name);
        let (outcome, resolved) = match step {
            Stepped::Stopped(code) => return code,
            Stepped::None => unreachable!("an installer always has a step"),
            Stepped::NotPrepared(err) => {
                note(&format!("error: {:#}", anyhow::Error::from(err)));
                note(&setup::recovery_message(paths, name));
                return Ok(ExitCode::FAILURE);
            }
            Stepped::Ran { outcome, resolved } => (outcome, resolved),
        };

        let next = create::after_install(&outcome, resolved);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(err) => {
                note(&format!("error: {:#}", anyhow::Error::from(err)));
                note(&setup::recovery_message(paths, name));
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
                if next != AfterInstall::Programs {
                    note("The installer finished, but cleanup is unresolved.");
                    note(&setup::recovery_message(paths, name));
                    return Ok(ExitCode::FAILURE);
                }
                if exit_code != Some(0) {
                    note(&setup::recovery_message(paths, name));
                }
                if let Some((key, picked)) = self.picked {
                    // Nothing listens for the installer's Ctrl+C from here on.
                    drop(cancellation.take());
                    let request = AutoFetch {
                        env: name,
                        term: name,
                        kinds: self.kinds,
                    };
                    self.access.fetch_picked(
                        paths,
                        key,
                        &request,
                        picked,
                        out,
                        &mut crate::output::stderr(),
                    );
                    downloaded.set(true);
                }
                match installed(self.prefix) {
                    Some(listing) => programs::report(name, &listing, false),
                    None => {
                        println!("Add a target with: wineo set {name} targets.<target>.exe=<path>")
                    }
                }
                Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
            }
            Outcome::Canceled { .. } => {
                note("Installer canceled.");
                note(&setup::recovery_message(paths, name));
                Ok(cancel::exit_code())
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
}

/// Said when the game picked for an installer's environment gets no art,
/// because the installer didn't finish.
pub(crate) fn art_not_fetched(name: &str) -> String {
    format!("Art not fetched. Run `wineo art fetch {name}` to fetch it.")
}

/// The terminal side of creating a Wine environment, and of its installer.
struct WineHooks<'a> {
    paths: &'a Paths,
    exe: Option<&'a str>,
    name: &'a str,
    /// Listens for Ctrl+C until the config is saved.
    cancellation: Option<Cancellation>,
    install_token: Token,
}

impl Hooks for WineHooks<'_> {
    /// Ends `new` with this result.
    type Stop = anyhow::Result<ExitCode>;

    fn event(&mut self, event: CreateEvent) {
        let paths = self.paths;
        match event {
            CreateEvent::Warning(warning) => eprintln!("warning: {warning}"),
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
            CreateEvent::Saved => drop(self.cancellation.take()),
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
                    eprintln!("{}", components::describe(&change, self.name));
                }
            }
            CreateEvent::Desktop(changes) => {
                for change in changes {
                    eprintln!("{}", desktop::describe(&change, self.name));
                }
            }
        }
    }

    fn created(&mut self, created: &create::Created) -> Result<(), Self::Stop> {
        let name = &created.environment.name;
        if created.target == TargetOutcome::ExeNotFound {
            if let Some(exe) = self.exe {
                eprintln!(
                    "Created environment {name}, but {exe} was not found. Add a target with: wineo setup {name}"
                );
            }
            return Err(Ok(ExitCode::FAILURE));
        }
        Ok(())
    }

    fn ready(&mut self, _launch: &Launch) -> Result<(), Self::Stop> {
        self.cancellation = Some(Cancellation::with_token(&self.install_token).map_err(Err)?);
        Ok(())
    }

    fn watched(&mut self, watched: Watched) {
        match watched {
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
    art: Option<&Access<'_>>,
) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::create_native(paths, &global, name, exe, desktop, |warning| {
        eprintln!("warning: {warning}")
    })?;
    match exe {
        Some(_) => println!("Created native environment {name} with target {name}"),
        None => {
            println!("Created native environment {name}");
            println!("Add a target with: wineo setup {name}");
        }
    }
    desktop::reconcile_native(paths, &global, &environment)?;
    if let Some(access) = art
        && fetch_art(paths, &global, name, access, &mut crate::output::stdout()).is_err()
    {
        eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
        return Ok(cancel::exit_code());
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
    let mut prompt = prompt::terminal()?;
    let mut guided = Guided { registered: None };
    let access = Access::steamgriddb(paths);
    match wizard::run(paths, &global, &prefill, &mut prompt, &mut guided, &access) {
        Err(err) if cancel::prompted(&err) => {
            match guided.registered {
                Some(name) => {
                    eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.")
                }
                None => eprintln!("Canceled. Nothing was created."),
            }
            Ok(cancel::exit_code())
        }
        other => other,
    }
}

/// Fetches art searching the environment's name, with the pick list when
/// there is a terminal and the top match otherwise.
fn fetch_art(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    access: &Access<'_>,
    out: &mut dyn Write,
) -> Result<(), Canceled> {
    let Some(key) = art_key(global, access) else {
        return Ok(());
    };
    let request = AutoFetch {
        env: name,
        term: name,
        kinds: global.art.fetch,
    };
    let mut prompt = pick_prompt();
    let pick = match &mut prompt {
        Some(prompt) => Pick::Ask(prompt),
        None => Pick::Top(out),
    };
    access.fetch(paths, &key, &request, pick, &mut crate::output::stderr())
}

/// The key to fetch art with, or None after saying why there is no art.
fn art_key(global: &GlobalConfig, access: &Access<'_>) -> Option<Key> {
    if global.art.fetch.kinds().is_empty() {
        return None;
    }
    match (access.check_key)() {
        KeyCheck::Found(key) => Some(key),
        check => {
            check.print_skip();
            None
        }
    }
}

/// The pick list's prompt, when there is a terminal to show it on.
fn pick_prompt() -> Option<Prompt<Terminal, crate::output::Stream<std::io::Stdout>>> {
    prompt::interactive()
        .then(|| Terminal::open().ok())
        .flatten()
        .map(|input| Prompt::new(input, crate::output::stdout()))
}

/// The game for `fetch_art`, picked before the environment exists so the
/// questions are answered ahead of the installer's wait. `Access::fetch_picked` fetches
/// it once the environment is unlocked.
fn pick_art(
    paths: &Paths,
    name: &str,
    access: &Access<'_>,
    out: &mut dyn Write,
) -> anyhow::Result<Option<(Key, Picked)>> {
    let global = GlobalConfig::load(paths)?;
    let Some(key) = art_key(&global, access) else {
        return Ok(None);
    };
    let request = AutoFetch {
        env: name,
        term: name,
        kinds: global.art.fetch,
    };
    let mut prompt = pick_prompt();
    let pick = match &mut prompt {
        Some(prompt) => Pick::Ask(prompt),
        None => Pick::Top(out),
    };
    let picked = access.pick(&key, &request, pick, &mut crate::output::stderr())?;
    Ok(picked.map(|picked| (key, picked)))
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
    use crate::test_script::write_script;
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::rc::Rc;

    use wineo_core::steamgriddb::{self, KeySource, NoKeyring};

    use super::super::art::fake::{Fake, Probe, Seen};
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
            write_script(&path, "#!/bin/sh\nexit 1\n");
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

    const ONE_PIXEL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDATx\x9c\x63\xf8\xcf\xc0\xf0\x1f\0\x05\0\x01\xff\x89\x99\x3d\x1d\0\0\0\0IEND\xaeB`\x82";

    /// What `wineo new --art` left behind, and what SteamGridDB was asked
    /// while it ran.
    struct Run {
        _root: tempfile::TempDir,
        paths: Paths,
        code: anyhow::Result<ExitCode>,
        /// What went to stdout for the art.
        out: String,
        seen: Rc<RefCell<Vec<Seen>>>,
    }

    /// A game with an icon whose image the desktop entry can decode.
    fn hades() -> Fake {
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.icon_only();
        fake.0.insert(
            "https://cdn.example/1".into(),
            (200, ONE_PIXEL_PNG.to_vec()),
        );
        fake
    }

    /// Creates `hades` in a fake wine with an existing prefix, so no prefix is
    /// built, fetching art through `fake`. With `installer` it runs a fake one
    /// instead of adding a target, with a desktop entry otherwise.
    fn new_with_art(fake: Fake, installer: bool) -> Run {
        new_with_runner(fake, installer, "test-wine")
    }

    fn new_with_runner(fake: Fake, installer: bool, runner: &str) -> Run {
        let wine = |_: &Path| "#!/bin/sh\nexit 0\n".to_owned();
        new_with(fake, installer, runner, &wine, &Token::new(), &|_| {})
    }

    /// `new_with_art` with its own fake wine, which gets the temporary root,
    /// and a `before` that runs once everything is in place, given the root.
    fn new_with(
        fake: Fake,
        installer: bool,
        runner: &str,
        wine: &dyn Fn(&Path) -> String,
        token: &Token,
        before: &dyn Fn(&Path),
    ) -> Run {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_dir = paths.default_runner_root().join("test-wine/bin");
        fs::create_dir_all(&runner_dir).unwrap();
        write_script(runner_dir.join("wine"), wine(root.path()));
        write_script(runner_dir.join("wineserver"), "#!/bin/sh\nexit 0\n");
        let prefix = root.path().join("prefixes/hades");
        fs::create_dir_all(&prefix).unwrap();
        fs::write(prefix.join("system.reg"), "#arch=win64\n").unwrap();
        let program = root.path().join("game.exe");
        fs::write(&program, "").unwrap();
        let program = program.display().to_string();
        let mut args = args(false);
        args.dry_run = false;
        args.runner = Some(runner.to_owned());
        args.dir = Some(prefix.display().to_string());
        if installer {
            args.install_exe = Some(program);
        } else {
            args.exe = Some(program);
            args.desktop = true;
        }
        let seen = Rc::new(RefCell::new(Vec::new()));
        let access = Access {
            check_key: Box::new(|| {
                KeyCheck::Found(Key {
                    value: "k".to_owned(),
                    source: KeySource::Variable,
                })
            }),
            transport: Box::new(|| {
                Ok(Box::new(Probe {
                    fake: fake.clone(),
                    paths: paths.clone(),
                    name: "hades".to_owned(),
                    seen: seen.clone(),
                }))
            }),
        };
        before(root.path());
        let mut out = Vec::new();
        let code = new_wine(&paths, args, "hades", &access, token, &mut out);
        drop(access);
        Run {
            _root: root,
            paths,
            code,
            seen,
            out: String::from_utf8(out).unwrap(),
        }
    }

    impl Run {
        fn entry(&self) -> String {
            fs::read_to_string(
                self.paths
                    .applications_dir()
                    .join("wineo-hades-hades.desktop"),
            )
            .unwrap()
        }
    }

    #[test]
    fn art_is_fetched_without_holding_the_environment_lock() {
        let run = new_with_art(hades(), false);
        assert_eq!(run.code.as_ref().unwrap(), &ExitCode::SUCCESS);
        let seen = run.seen.borrow();
        assert!(!seen.is_empty());
        for request in seen.iter() {
            assert!(request.exists && request.lock_free, "{}", request.url);
        }
        assert_eq!(
            wineo_core::art::saved_game(&run.paths, "hades").unwrap(),
            Some(7)
        );
        assert!(run.paths.art_dir("hades").join("icon.png").is_file());
        let entry = run.entry();
        assert!(entry.contains("\nIcon=wineo-hades-hades\n"), "{entry}");
        assert!(
            run.out.starts_with(&format!(
                "{TOP_PICK}icon: fetched SteamGridDB 1\ngrid: nothing found\n"
            )),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_failed_art_fetch_leaves_the_environment_and_its_entry() {
        let run = new_with_art(Fake::default(), false);
        assert_eq!(run.code.as_ref().unwrap(), &ExitCode::SUCCESS);
        assert!(run.seen.borrow().iter().all(|request| request.lock_free));
        assert_eq!(
            wineo_core::art::saved_game(&run.paths, "hades").unwrap(),
            None
        );
        let entry = run.entry();
        assert!(!entry.contains("Icon="), "{entry}");
        assert_eq!(run.out, "");
    }

    #[test]
    fn a_request_that_cant_be_created_asks_nothing_before_an_installer() {
        let run = new_with_runner(hades(), true, "missing-wine");
        let err = run.code.as_ref().unwrap_err();
        assert!(err.to_string().contains("missing-wine"), "{err:#}");
        assert!(run.seen.borrow().is_empty());
        assert!(!run.paths.environment_config("hades").exists());
    }

    const TOP_PICK: &str = "hades: art from SteamGridDB game 7, Hades (2020)\n";

    const NOT_FETCHED: &str = "Art not fetched. Run `wineo art fetch hades` to fetch it.\n";

    #[test]
    fn a_finished_installer_gets_its_art_and_no_hint() {
        let run = new_with_art(hades(), true);
        assert!(
            run.out.contains("icon: fetched SteamGridDB 1\n"),
            "{}",
            run.out
        );
        assert!(!run.out.contains("Art not fetched"), "{}", run.out);
    }

    #[test]
    fn an_installer_that_cant_be_prepared_says_the_art_was_not_fetched() {
        // The installer goes away while the environment is being created.
        let wine = |root: &Path| {
            format!(
                "#!/bin/sh\nrm -f '{}'\nexit 0\n",
                root.join("game.exe").display()
            )
        };
        let run = new_with(hades(), true, "test-wine", &wine, &Token::new(), &|_| {});
        assert_eq!(run.code.as_ref().unwrap(), &ExitCode::FAILURE);
        assert_eq!(run.out.matches(NOT_FETCHED).count(), 1, "{}", run.out);
        assert!(!run.out.contains("icon:"), "{}", run.out);
        assert_eq!(
            wineo_core::art::saved_game(&run.paths, "hades").unwrap(),
            None
        );
    }

    #[test]
    fn a_canceled_installer_says_the_art_was_not_fetched() {
        // The installer runs until wine is asked to end the session, with a
        // bound so a failed test can't leave it running.
        let wine = |root: &Path| {
            let (started, stop) = (root.join("started"), root.join("stop"));
            format!(
                "#!/bin/sh\ncase \"$*\" in\n*wineboot*) touch '{stop}'; exit 0 ;;\n*game.exe*)\ntouch '{started}'\ni=0\nwhile [ ! -e '{stop}' ] && [ \"$i\" -lt 200 ]; do sleep 0.1; i=$((i + 1)); done\n;;\nesac\nexit 0\n",
                started = started.display(),
                stop = stop.display(),
            )
        };
        let token = Token::new();
        // Stands in for Ctrl+C once the installer is running.
        let cancel = |root: &Path| {
            let (marker, token) = (root.join("started"), token.clone());
            std::thread::spawn(move || {
                for _ in 0..200 {
                    if marker.exists() {
                        token.cancel();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
        };
        let run = new_with(hades(), true, "test-wine", &wine, &token, &cancel);
        assert_eq!(run.code.as_ref().unwrap(), &cancel::exit_code());
        assert_eq!(run.out.matches(NOT_FETCHED).count(), 1, "{}", run.out);
        assert!(!run.out.contains("icon:"), "{}", run.out);
    }

    #[test]
    fn with_an_installer_the_search_comes_before_creation_and_the_download_after() {
        let run = new_with_art(hades(), true);
        assert_eq!(run.code.as_ref().unwrap(), &ExitCode::SUCCESS);
        let seen = run.seen.borrow();
        let (search, downloads) = seen.split_first().expect("the search is asked");
        assert!(search.url.contains("/search/"), "{}", search.url);
        assert!(!search.exists, "nothing exists while the questions run");
        assert!(!downloads.is_empty());
        for request in downloads {
            assert!(request.exists && request.lock_free, "{}", request.url);
        }
        assert_eq!(
            wineo_core::art::saved_game(&run.paths, "hades").unwrap(),
            Some(7)
        );
        assert!(run.paths.art_dir("hades").join("icon.png").is_file());
        // The pick comes first, then what the download stored.
        assert!(
            run.out.starts_with(&format!(
                "{TOP_PICK}icon: fetched SteamGridDB 1\ngrid: nothing found\n"
            )),
            "{}",
            run.out
        );
    }

    /// Reports `step` as the end of the installer for `hades`, with a game
    /// already picked, returning what `new` printed for the art.
    fn ended(step: Stepped<anyhow::Result<ExitCode>>) -> (ExitCode, String, usize) {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let seen = Rc::new(RefCell::new(Vec::new()));
        let fake = hades();
        let access = Access {
            check_key: Box::new(|| {
                KeyCheck::Found(Key {
                    value: "k".to_owned(),
                    source: KeySource::Variable,
                })
            }),
            transport: Box::new(|| {
                Ok(Box::new(Probe {
                    fake: fake.clone(),
                    paths: paths.clone(),
                    name: "hades".to_owned(),
                    seen: seen.clone(),
                }))
            }),
        };
        let mut out = Vec::new();
        let picked = pick_art(&paths, "hades", &access, &mut out)
            .unwrap()
            .expect("a game is picked");
        // Only the download counts below, so the pick's own line goes.
        let (before, mut out) = (seen.borrow().len(), Vec::new());
        let ended = Ended {
            paths: &paths,
            access: &access,
            name: "hades",
            prefix: root.path(),
            kinds: ArtFetch::default(),
            picked: Some(&picked),
        };
        let code = ended.report(step, &mut None, &mut out).unwrap();
        let requests = seen.borrow().len() - before;
        (code, String::from_utf8(out).unwrap(), requests)
    }

    fn unsettled() -> wineo_core::Error {
        wineo_core::Error::Unsettled("it could not be confirmed".to_owned())
    }

    /// The installer didn't finish, so the art is not downloaded, and the
    /// hint is the only thing said about it.
    fn assert_not_fetched((code, out, requests): (ExitCode, String, usize), expected: ExitCode) {
        assert_eq!(code, expected);
        assert_eq!(out, NOT_FETCHED);
        assert_eq!(requests, 0);
    }

    #[test]
    fn a_failed_installer_start_says_the_art_was_not_fetched() {
        assert_not_fetched(
            ended(Stepped::Ran {
                outcome: Err(unsettled()),
                resolved: true,
            }),
            ExitCode::FAILURE,
        );
    }

    #[test]
    fn an_unresolved_installer_says_the_art_was_not_fetched() {
        assert_not_fetched(
            ended(Stepped::Ran {
                outcome: Ok(Outcome::Unresolved {
                    exit_code: None,
                    canceled: false,
                    error: unsettled(),
                }),
                resolved: true,
            }),
            ExitCode::FAILURE,
        );
    }

    #[test]
    fn a_finished_installer_with_unresolved_cleanup_says_the_art_was_not_fetched() {
        assert_not_fetched(
            ended(Stepped::Ran {
                outcome: Ok(Outcome::Finished { exit_code: Some(0) }),
                resolved: false,
            }),
            ExitCode::FAILURE,
        );
    }

    #[test]
    fn a_canceled_installer_report_says_the_art_was_not_fetched() {
        assert_not_fetched(
            ended(Stepped::Ran {
                outcome: Ok(Outcome::Canceled { exit_code: None }),
                resolved: true,
            }),
            cancel::exit_code(),
        );
    }

    #[test]
    fn a_stopped_creation_says_the_art_was_not_fetched() {
        assert_not_fetched(
            ended(Stepped::Stopped(Ok(ExitCode::FAILURE))),
            ExitCode::FAILURE,
        );
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
