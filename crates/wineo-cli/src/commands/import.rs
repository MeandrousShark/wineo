use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::import::{self, ImportRequest, ImportedSettings, Inspection};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

use super::transfer::Cancellation;
use super::{download, import_heroic, setup};
use crate::picker;
use crate::prompt::{self, Canceled, Choice, Input, Prompt, Reply, Terminal, numbered};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    source: Option<Source>,
}

#[derive(clap::Subcommand)]
enum Source {
    /// Copy or adopt an existing Wine or Proton prefix
    Prefix(PrefixArgs),
    /// Import a manually added Heroic game
    Heroic(import_heroic::Args),
}

#[derive(clap::Args, Default)]
struct PrefixArgs {
    /// Prefix directory, or a Proton compatdata directory
    source: Option<String>,
    /// Name for the new environment
    #[arg(short, long)]
    name: Option<String>,
    /// Installed runner or runner-family alias
    #[arg(long)]
    runner: Option<String>,
    /// Keep the prefix at its original location instead of copying it
    #[arg(long, conflicts_with = "dir")]
    adopt: bool,
    /// Copy destination, defaults to the configured prefix root
    #[arg(long)]
    dir: Option<String>,
    /// Resolve a missing registry architecture marker
    #[arg(long)]
    arch: Option<Arch>,
    /// Review and edit choices even when flags supply all required values
    #[arg(long)]
    interactive: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match args.source {
        Some(Source::Heroic(args)) => import_heroic::run(paths, args),
        Some(Source::Prefix(args)) => prefix(paths, args),
        None => {
            anyhow::ensure!(
                prompt::interactive(),
                "guided import needs a terminal; use `wineo import prefix <path> -n <name> --runner <runner>` or `wineo import heroic --list`"
            );
            let choice = {
                let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
                prompt.choose(
                    "Import from where?",
                    "Source",
                    &numbered([
                        ("An existing prefix".to_owned(), false),
                        ("A manually added Heroic game".to_owned(), true),
                    ]),
                    Some("1"),
                    false,
                )
            };
            match choice {
                Ok(Reply::Value(false)) => prefix(paths, PrefixArgs::default()),
                Ok(Reply::Value(true)) => import_heroic::run(paths, import_heroic::Args::default()),
                Ok(Reply::Back) => Ok(ExitCode::SUCCESS),
                Err(error) if error.is::<Canceled>() => Ok(ExitCode::from(130)),
                Err(error) => Err(error),
            }
        }
    }
}

fn prefix(paths: &Paths, args: PrefixArgs) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let guided =
        args.interactive || args.name.is_none() || args.runner.is_none() || args.source.is_none();
    if !guided {
        let request = ImportRequest {
            source: paths.absolute(&picker::clean_path(args.source.as_deref().unwrap()))?,
            name: args.name.unwrap(),
            runner: args.runner.unwrap(),
            destination: args
                .dir
                .as_deref()
                .map(|dir| paths.absolute(&picker::clean_path(dir)))
                .transpose()?,
            adopt: args.adopt,
            arch: args.arch,
        };
        let runner = Runners::new(paths, &global)?.resolve(&request.runner)?;
        let inspection = import::inspect(&request.source, Some(&runner), request.arch)?;
        print_review(
            paths,
            &global,
            &request,
            &inspection,
            &mut Prompt::new(NoInput, io::stdout()),
        )?;
        return match import_reviewed(paths, &global, request, &inspection, None) {
            Ok(environment) => {
                println!(
                    "Imported {}. Add targets with `wineo setup {}`",
                    environment.name, environment.name
                );
                Ok(ExitCode::SUCCESS)
            }
            Err(error) if error.is::<Canceled>() => {
                eprintln!("Import canceled");
                Ok(ExitCode::from(130))
            }
            Err(error) => Err(error),
        };
    }
    anyhow::ensure!(
        prompt::interactive(),
        "guided import needs a terminal; specify a source, -n <name>, and --runner <runner> for noninteractive import"
    );
    let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
    let result = guided_prefix(paths, &global, args, &mut prompt);
    match result {
        Err(error) if error.is::<Canceled>() => {
            eprintln!("Import canceled");
            Ok(ExitCode::from(130))
        }
        other => other,
    }
}

struct NoInput;
impl Input for NoInput {
    fn line(&mut self, _: &str) -> io::Result<crate::prompt::Line> {
        unreachable!("review has no prompts")
    }
}

fn guided_prefix<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    args: PrefixArgs,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ExitCode> {
    let mut source = args.source.unwrap_or_default();
    let mut name = args.name.unwrap_or_default();
    let mut runner = args.runner;
    let mut adopt = args.adopt;
    let mut dir = args.dir;
    let mut arch = args.arch;
    let mut step = 0;
    loop {
        match step {
            0 => {
                match prompt.text("Existing prefix", Some(&source), |text| {
                    let path = paths
                        .absolute(&picker::clean_path(text))
                        .map_err(|err| err.to_string())?;
                    if path.is_dir() {
                        Ok(path.display().to_string())
                    } else {
                        Err("Choose an existing prefix directory".to_owned())
                    }
                })? {
                    Reply::Value(value) => {
                        source = value;
                        step = 1;
                    }
                    Reply::Back => return Ok(ExitCode::SUCCESS),
                }
            }
            1 => {
                let suggested = wineo_core::targets::suggest_name(
                    PathBuf::from(&source)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("imported"),
                    &source,
                );
                match prompt.text(
                    "Install name",
                    Some(if name.is_empty() { &suggested } else { &name }),
                    |text| {
                        if let Some(problem) = picker::name_problem(text) {
                            return Err(problem);
                        }
                        if paths.environment_config(text).symlink_metadata().is_ok() {
                            return Err(format!("`{text}` is already registered"));
                        }
                        Ok(text.to_owned())
                    },
                )? {
                    Reply::Value(value) => {
                        name = value;
                        step = 2;
                    }
                    Reply::Back => step = 0,
                }
            }
            2 => {
                match prompt.choose(
                    "Copy the prefix, or use its original files?",
                    "Mode",
                    &numbered([
                        ("Copy into wineo".to_owned(), false),
                        (
                            "Adopt in place (launches modify the original)".to_owned(),
                            true,
                        ),
                    ]),
                    Some(if adopt { "2" } else { "1" }),
                    true,
                )? {
                    Reply::Value(value) => {
                        adopt = value;
                        step = if adopt { 4 } else { 3 };
                    }
                    Reply::Back => step = 1,
                }
            }
            3 => {
                let default = global.prefix_root(paths)?.join(&name).display().to_string();
                match prompt.text(
                    "Copy destination",
                    Some(dir.as_deref().unwrap_or(&default)),
                    |text| {
                        paths
                            .absolute(&picker::clean_path(text))
                            .map(|p| p.display().to_string())
                            .map_err(|e| e.to_string())
                    },
                )? {
                    Reply::Value(value) => {
                        dir = Some(value);
                        step = 4;
                    }
                    Reply::Back => step = 2,
                }
            }
            4 => {
                let detected = import::inspect(&source, None, arch).ok();
                let proton = detected
                    .as_ref()
                    .map(|i| i.layout == import::ImportLayout::Proton);
                match choose_runner(prompt, paths, global, runner.as_deref(), proton)? {
                    Reply::Value(value) => {
                        runner = Some(value);
                        step = 5;
                    }
                    Reply::Back => step = if adopt { 2 } else { 3 },
                }
            }
            _ => {
                let request = ImportRequest {
                    source: paths.absolute(&source)?,
                    name: name.clone(),
                    runner: runner.clone().unwrap(),
                    destination: if adopt {
                        None
                    } else {
                        dir.as_deref().map(|p| paths.absolute(p)).transpose()?
                    },
                    adopt,
                    arch,
                };
                let resolved = Runners::new(paths, global)?.resolve(&request.runner)?;
                let inspection = match import::inspect(&request.source, Some(&resolved), arch) {
                    Ok(value) => value,
                    Err(error) => {
                        prompt.say(format!("Can't import: {error:#}"))?;
                        if error.to_string().contains("#arch") {
                            match prompt.choose(
                                "The prefix has no architecture marker. Which architecture is it?",
                                "Architecture",
                                &numbered([
                                    ("win64".to_owned(), Arch::Win64),
                                    ("win32".to_owned(), Arch::Win32),
                                ]),
                                None,
                                true,
                            )? {
                                Reply::Value(value) => arch = Some(value),
                                Reply::Back => step = 4,
                            }
                        } else {
                            step = 4;
                        }
                        continue;
                    }
                };
                print_review(paths, global, &request, &inspection, prompt)?;
                #[derive(Clone)]
                enum Review {
                    Import,
                    Source,
                    Name,
                    Mode,
                    Runner,
                }
                match prompt.choose(
                    "Ready to import?",
                    "Choice",
                    &numbered([
                        ("Import".to_owned(), Review::Import),
                        ("Change source".to_owned(), Review::Source),
                        ("Change name".to_owned(), Review::Name),
                        ("Change destination or mode".to_owned(), Review::Mode),
                        ("Change runner".to_owned(), Review::Runner),
                    ]),
                    Some("1"),
                    true,
                )? {
                    Reply::Back | Reply::Value(Review::Runner) => step = 4,
                    Reply::Value(Review::Source) => step = 0,
                    Reply::Value(Review::Name) => step = 1,
                    Reply::Value(Review::Mode) => step = 2,
                    Reply::Value(Review::Import) => {
                        match import_reviewed(paths, global, request, &inspection, None) {
                            Ok(environment) => {
                                return finish_guided(paths, global, &environment, prompt);
                            }
                            Err(error) if error.is::<Canceled>() => return Err(error),
                            Err(error) => {
                                prompt.say(format!("Import failed: {error:#}"))?;
                                step = 0;
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn choose_runner<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    global: &GlobalConfig,
    current: Option<&str>,
    proton: Option<bool>,
) -> anyhow::Result<Reply<String>> {
    loop {
        let runners = Runners::new(paths, global)?;
        let mut entries = Vec::new();
        let mut candidates = runners.list()?.runners;
        if let Ok(system) = Runner::system() {
            candidates.insert(0, system);
        }
        for alias in wineo_core::runner_alias::ALIASES {
            if let Ok(runner) = runners.resolve(alias) {
                candidates.push(runner);
            }
        }
        for runner in candidates {
            if proton.is_some_and(|expected| expected != runner.is_proton()) {
                continue;
            }
            entries.push((
                if runner.selection() == runner.name() {
                    runner.name().to_owned()
                } else {
                    format!("{} -> {}", runner.selection(), runner.name())
                },
                Some(runner.selection().to_owned()),
            ));
        }
        let default = entries
            .iter()
            .position(|(_, name)| name.as_deref() == current)
            .map(|i| (i + 1).to_string());
        let mut choices = numbered(entries);
        if proton != Some(false) {
            choices.push(Choice::letter('d', "Download a Proton runner", None));
        }
        match prompt.choose(
            "Which runner?",
            "Runner",
            &choices,
            default.as_deref(),
            true,
        )? {
            Reply::Back => return Ok(Reply::Back),
            Reply::Value(Some(name)) => return Ok(Reply::Value(name)),
            Reply::Value(None) => {
                if let Reply::Value(name) =
                    download::choose_and_install(paths, global, prompt, &download::RUNNERS, false)?
                {
                    return Ok(Reply::Value(name));
                }
            }
        }
    }
}

fn print_review<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    request: &ImportRequest,
    inspection: &Inspection,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let destination = if request.adopt {
        inspection.source_root.clone()
    } else {
        request
            .destination
            .clone()
            .unwrap_or(global.prefix_root(paths)?.join(&request.name))
    };
    let runner = Runners::new(paths, global)?.resolve(&request.runner)?;
    prompt.say(format!(
        "Import {}\n  source: {}\n  {}: {}\n  runner: {} -> {}\n  layout: {:?}, {}",
        request.name,
        inspection.source_root.display(),
        if request.adopt {
            "adopt in place"
        } else {
            "copy to"
        },
        destination.display(),
        request.runner,
        runner.name(),
        inspection.layout,
        inspection.arch
    ))?;
    for link in &inspection.external_dependencies {
        prompt.say(format!(
            "  external: {} -> {} (kept in place)",
            link.source.display(),
            link.target.display()
        ))?;
    }
    prompt.say("The source game and launcher must be idle. External game files and saves stay where they are.")?;
    prompt.say(if request.adopt {
        "Later launches modify the original prefix. `rm --delete-prefix` deletes that original."
    } else {
        "The source prefix stays in place. `rm --delete-prefix` deletes the imported copy."
    })?;
    Ok(())
}

pub(crate) fn import_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    request: ImportRequest,
    inspection: &Inspection,
    settings: Option<&ImportedSettings>,
) -> anyhow::Result<Environment> {
    let cancellation = Cancellation::new()?;
    let count = AtomicU64::new(0);
    let result = import::execute_reviewed(
        paths,
        global,
        request,
        Some(inspection),
        settings,
        &cancellation.flag,
        |progress| {
            if let import::ImportProgress::Copied { .. } = progress {
                let count = count.fetch_add(1, Ordering::Relaxed) + 1;
                if count.is_multiple_of(500) {
                    eprintln!("Copied {count} entries");
                }
            }
        },
    );
    // A completed registration is retained even if cancellation arrives at the end.
    match result {
        Ok(environment) => Ok(environment),
        Err(error) => cancellation.result(Err(error)),
    }
}

pub(crate) fn finish_guided<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ExitCode> {
    prompt.say(format!(
        "Imported {}. Continue later with `wineo setup {}`",
        environment.name, environment.name
    ))?;
    let result = (|| match prompt.confirm("Set up launch targets now?", true)? {
        Reply::Value(true) => setup::setup(paths, global, &environment.name, prompt),
        _ => Ok(ExitCode::SUCCESS),
    })();
    if result
        .as_ref()
        .is_err_and(|error: &anyhow::Error| error.is::<Canceled>())
    {
        prompt.say(format!(
            "Kept {} and its prefix. Continue with `wineo setup {}`",
            environment.name, environment.name
        ))?;
    }
    result
}
