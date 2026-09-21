use anyhow::{Context, bail};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use wineo_core::config::{Arch, GlobalConfig, Target};
use wineo_core::heroic::{
    HeroicGameKind, HeroicImport, HeroicIssue, HeroicIssueKind, HeroicRunnerKind, HeroicSource,
    SettingsReview,
};
use wineo_core::import::{self, ImportRequest, ImportedSettings, Inspection};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

use super::import::{choose_runner, finish_guided, import_reviewed};
use super::transfer::Cancellation;
use crate::picker;
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};

#[derive(clap::Args, Default)]
pub struct Args {
    /// Read Heroic's config from this directory
    #[arg(long)]
    pub config_dir: Option<String>,
    /// List Heroic sideload candidates without prompting
    #[arg(long)]
    pub list: bool,
    /// Heroic game id
    #[arg(long)]
    pub game: Option<String>,
    /// Name for the new environment
    #[arg(short, long)]
    pub name: Option<String>,
    /// Installed runner or runner-family alias
    #[arg(long)]
    pub runner: Option<String>,
    /// Keep the Heroic prefix at its original location
    #[arg(long, conflicts_with = "dir")]
    pub adopt: bool,
    /// Copy destination, defaults to the configured prefix root
    #[arg(long)]
    pub dir: Option<String>,
    /// Resolve a missing registry architecture marker
    #[arg(long)]
    pub arch: Option<Arch>,
    /// Review and edit choices even when flags supply all required values
    #[arg(long)]
    pub interactive: bool,
}

struct OutputOnly;

impl Input for OutputOnly {
    fn line(&mut self, _: &str) -> io::Result<crate::prompt::Line> {
        unreachable!("Heroic review should not prompt in noninteractive mode")
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    if args.list {
        let sources = sources(paths, args.config_dir.as_deref())?;
        for (index, source) in sources.iter().enumerate() {
            if index > 0 {
                println!();
            }
            list(source)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    let needs_guided =
        args.interactive || args.game.is_none() || args.name.is_none() || args.runner.is_none();
    if !needs_guided {
        let source = source(paths, args.config_dir.as_deref())?;
        let game_id = args.game.as_deref().unwrap();
        let mut imported = source.inspect(game_id)?;
        let name = args.name.clone().unwrap();
        let runner_name = args.runner.clone().unwrap();
        let runner = Runners::new(paths, &global)?.resolve(&runner_name)?;
        let inspection = prefix_inspection(&imported, &runner, args.arch)?;
        clear_runner_dependency(&mut imported);
        imported.issues.retain(|issue| {
            !(issue.kind == HeroicIssueKind::Conflict
                && matches!(
                    issue.field.as_deref(),
                    Some("wineVersion") | Some("winePrefix")
                ))
        });
        let mut request = request(
            paths,
            imported.prefix.clone(),
            name,
            runner_name,
            args,
            &inspection,
        )?;
        if !request.adopt && request.destination.is_none() {
            request.destination = Some(global.prefix_root(paths)?.join(&request.name));
        }
        let mut output = Prompt::new(OutputOnly, io::stdout());
        show_settings(&mut output, &imported)?;
        show_prefix_review(paths, &global, &request, &inspection, &mut output)?;
        reject_unresolved(&imported)?;
        ensure_source_unchanged(&source, &imported)?;
        let settings = imported_settings(&imported, &inspection, &request);
        let environment = import_reviewed(paths, &global, request, &inspection, Some(&settings))?;
        println!(
            "Imported {}. Add or edit targets with wineo setup {}",
            environment.name, environment.name
        );
        return Ok(ExitCode::SUCCESS);
    }

    anyhow::ensure!(
        prompt::interactive(),
        "guided Heroic import needs a terminal; use --list or specify --game, -n, and --runner"
    );
    let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
    let source = source_guided(paths, args.config_dir.as_deref(), &mut prompt)?;
    match guided(paths, &global, source, args, &mut prompt) {
        Ok(code) => Ok(code),
        Err(error) if error.is::<Canceled>() => {
            eprintln!("Import canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

fn sources(paths: &Paths, explicit: Option<&str>) -> anyhow::Result<Vec<HeroicSource>> {
    if let Some(path) = explicit {
        return Ok(vec![HeroicSource::from_config_dir(paths.absolute(path)?)]);
    }
    let sources = HeroicSource::discover_from_env()?;
    anyhow::ensure!(
        !sources.is_empty(),
        "no Heroic configuration found; pass --config-dir /path/to/heroic"
    );
    Ok(sources)
}

fn source(paths: &Paths, explicit: Option<&str>) -> anyhow::Result<HeroicSource> {
    let sources = sources(paths, explicit)?;
    if sources.len() > 1 {
        bail!("multiple Heroic configurations found; choose one with --config-dir");
    }
    Ok(sources.into_iter().next().unwrap())
}

fn source_guided<I: Input, W: Write>(
    paths: &Paths,
    explicit: Option<&str>,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<HeroicSource> {
    let sources = sources(paths, explicit)?;
    if sources.len() == 1 {
        return Ok(sources.into_iter().next().unwrap());
    }
    let choices = numbered(sources.iter().map(|source| {
        (
            format!("{:?}: {}", source.origin(), source.config_dir().display()),
            source.clone(),
        )
    }));
    match prompt.choose(
        "Which Heroic configuration should be imported?",
        "Source",
        &choices,
        Some("1"),
        true,
    )? {
        Reply::Value(source) => Ok(source),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn list(source: &HeroicSource) -> anyhow::Result<ExitCode> {
    let listing = source.list_candidates()?;
    println!(
        "Heroic source: {} ({:?})",
        source.config_dir().display(),
        source.origin()
    );
    for issue in &listing.issues {
        print_issue("source", issue);
    }
    for candidate in listing.candidates {
        println!(
            "{}  {}  {:?}",
            candidate.id, candidate.title, candidate.kind
        );
        if let Some(prefix) = candidate.prefix {
            println!("  prefix: {}", prefix.display());
        }
        if let Some(runner) = candidate.runner {
            println!("  runner: {} ({:?})", runner.name, runner.kind);
        }
        for issue in candidate.issues {
            print_issue("  issue", &issue);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn guided<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    source: HeroicSource,
    args: Args,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ExitCode> {
    let listing = source.list_candidates()?;
    let candidates: Vec<_> = listing
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.kind == HeroicGameKind::Windows
                && !candidate.issues.iter().any(|issue| {
                    matches!(
                        issue.kind,
                        HeroicIssueKind::Missing | HeroicIssueKind::Invalid
                    ) && issue.field.as_deref() != Some("launcherArgs")
                })
                && !candidate.issues.iter().any(|issue| {
                    issue.field.as_deref() == Some("runner")
                        && issue.kind == HeroicIssueKind::Unsupported
                })
        })
        .cloned()
        .collect();
    anyhow::ensure!(
        !candidates.is_empty(),
        "Heroic has no importable Windows sideloads in {}",
        source.config_dir().display()
    );
    let game = if let Some(game) = args.game.clone() {
        candidates
            .iter()
            .find(|candidate| candidate.id == game)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Heroic game {game} is not importable"))?
    } else {
        let choices = numbered(candidates.iter().map(|candidate| {
            (
                format!("{} ({})", candidate.title, candidate.id),
                candidate.id.clone(),
            )
        }));
        let Reply::Value(game_id) =
            prompt.choose("Which Heroic game?", "Game", &choices, None, true)?
        else {
            return Ok(ExitCode::SUCCESS);
        };
        candidates
            .into_iter()
            .find(|candidate| candidate.id == game_id)
            .unwrap()
    };
    let imported = source.inspect(&game.id)?;
    let mut imported = imported;
    show_settings(prompt, &imported)?;

    let suggested_name =
        wineo_core::targets::suggest_name(&imported.title, &imported.exe.to_string_lossy());
    let name_default = args.name.as_deref().unwrap_or(&suggested_name);
    let name = match prompt.text("Environment name", Some(name_default), |value| {
        if let Some(problem) = picker::name_problem(value) {
            Err(problem)
        } else {
            Ok(value.to_owned())
        }
    })? {
        Reply::Value(value) => value,
        Reply::Back => return Ok(ExitCode::SUCCESS),
    };

    let proton = Some(imported.runner.kind == HeroicRunnerKind::Proton);
    let runner_name = if let Some(runner) = args.runner.clone() {
        ensure_selected_runner(paths, global, &imported, runner, prompt, proton)?
    } else {
        choose_heroic_runner(paths, global, &imported, prompt, proton)?
    };
    let resolved = Runners::new(paths, global)?.resolve(&runner_name)?;
    let inspection = prefix_inspection(&imported, &resolved, args.arch)?;
    clear_runner_dependency(&mut imported);
    imported.issues.retain(|issue| {
        !(issue.kind == HeroicIssueKind::Conflict
            && matches!(
                issue.field.as_deref(),
                Some("wineVersion") | Some("winePrefix")
            ))
    });
    review_settings(prompt, &mut imported)?;
    let adopt = if args.adopt {
        true
    } else if args.dir.is_some() {
        false
    } else {
        match prompt.choose(
            "How should the prefix be imported?",
            "Mode",
            &numbered([
                ("Copy it into wineo".to_owned(), false),
                ("Adopt the original files in place".to_owned(), true),
            ]),
            Some("1"),
            true,
        )? {
            Reply::Value(value) => value,
            Reply::Back => return Ok(ExitCode::SUCCESS),
        }
    };
    let dir = if adopt {
        None
    } else if let Some(dir) = args.dir.clone() {
        Some(paths.absolute(&dir)?)
    } else {
        Some(global.prefix_root(paths)?.join(&name))
    };
    let mut request = ImportRequest {
        source: imported.prefix.clone(),
        name,
        runner: runner_name,
        destination: dir,
        adopt,
        arch: args.arch,
    };
    final_review(
        paths,
        global,
        &mut imported,
        &inspection,
        &mut request,
        prompt,
    )?;
    let settings = imported_settings(&imported, &inspection, &request);
    match prompt.confirm("Import this reviewed game?", true)? {
        Reply::Value(true) => {}
        _ => return Ok(ExitCode::SUCCESS),
    }
    ensure_source_unchanged(&source, &imported)?;
    let environment = import_reviewed(paths, global, request, &inspection, Some(&settings))?;
    finish_guided(paths, global, &environment, prompt)
}

#[derive(Clone, Copy)]
enum FinalReview {
    Import,
    Executable,
    WorkingDirectory,
    Arguments,
    Environment,
    GameMode,
    Name,
    Destination,
}

fn final_review<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &mut HeroicImport,
    inspection: &Inspection,
    request: &mut ImportRequest,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    loop {
        show_settings(prompt, imported)?;
        show_prefix_review(paths, global, request, inspection, prompt)?;
        let choice = prompt.choose(
            "Review Heroic import",
            "Choice",
            &numbered([
                ("Import".to_owned(), FinalReview::Import),
                ("Change executable".to_owned(), FinalReview::Executable),
                (
                    "Change working directory".to_owned(),
                    FinalReview::WorkingDirectory,
                ),
                ("Change launch arguments".to_owned(), FinalReview::Arguments),
                (
                    "Edit environment variables".to_owned(),
                    FinalReview::Environment,
                ),
                ("Change GameMode".to_owned(), FinalReview::GameMode),
                ("Change environment name".to_owned(), FinalReview::Name),
                (
                    "Change copy destination or mode".to_owned(),
                    FinalReview::Destination,
                ),
            ]),
            Some("1"),
            true,
        )?;
        let Reply::Value(choice) = choice else {
            return Err(Canceled.into());
        };
        match choice {
            FinalReview::Import => return Ok(()),
            FinalReview::Executable => edit_executable(imported, prompt)?,
            FinalReview::WorkingDirectory => edit_working_directory(imported, prompt)?,
            FinalReview::Arguments => edit_arguments(imported, prompt)?,
            FinalReview::Environment => edit_environment(imported, prompt)?,
            FinalReview::GameMode => edit_gamemode(imported, prompt)?,
            FinalReview::Name => edit_name(paths, global, request, prompt)?,
            FinalReview::Destination => edit_destination(paths, global, request, prompt)?,
        }
    }
}

fn edit_executable<I: Input, W: Write>(
    imported: &mut HeroicImport,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = imported.exe.display().to_string();
    match prompt.text("Executable path", Some(&current), |value| {
        if value.trim().is_empty() {
            Err("Executable path cannot be empty".to_owned())
        } else {
            Ok(value.to_owned())
        }
    })? {
        Reply::Value(value) => imported.exe = review_path(&imported.prefix, &value),
        Reply::Back => {}
    }
    Ok(())
}

fn edit_working_directory<I: Input, W: Write>(
    imported: &mut HeroicImport,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = imported
        .cwd
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    match prompt.text(
        "Working directory (empty for default)",
        Some(&current),
        |value| Ok(value.to_owned()),
    )? {
        Reply::Value(value) if value.is_empty() => imported.cwd = None,
        Reply::Value(value) => imported.cwd = Some(review_path(&imported.prefix, &value)),
        Reply::Back => {}
    }
    Ok(())
}

fn edit_arguments<I: Input, W: Write>(
    imported: &mut HeroicImport,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    match prompt.text("Launch arguments", Some(&imported.launcher_args), |value| {
        wineo_core::heroic::parse_launcher_args(value).map(|_| value.to_owned())
    })? {
        Reply::Value(value) => {
            imported.args = wineo_core::heroic::parse_launcher_args(&value)
                .map_err(|error| anyhow::anyhow!(error))?;
            imported.launcher_args = value;
        }
        Reply::Back => {}
    }
    Ok(())
}

fn edit_environment<I: Input, W: Write>(
    imported: &mut HeroicImport,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    prompt.say("Enter environment values as KEY=VALUE, one per line. An empty line keeps the current values.")?;
    let Reply::Value(values) = prompt.list("environment")? else {
        return Ok(());
    };
    if values.is_empty() {
        return Ok(());
    }
    let mut env = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            bail!("environment entries must use KEY=VALUE");
        };
        if key.is_empty() || key.contains(['=', '\0']) {
            bail!("invalid environment variable name `{key}`");
        }
        if [
            "WINEPREFIX",
            "PROTONPATH",
            "STEAM_COMPAT_DATA_PATH",
            "STEAM_COMPAT_CLIENT_INSTALL_PATH",
            "CX_BOTTLE",
        ]
        .contains(&key)
        {
            bail!("{key} is managed by wineo and cannot be imported");
        }
        env.insert(key.to_owned(), value.to_owned());
    }
    imported.env = env;
    Ok(())
}

fn edit_gamemode<I: Input, W: Write>(
    imported: &mut HeroicImport,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = imported.gamemode.unwrap_or(false);
    match prompt.confirm("Enable GameMode?", current)? {
        Reply::Value(value) => imported.gamemode = Some(value),
        Reply::Back => {}
    }
    Ok(())
}

fn edit_name<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    request: &mut ImportRequest,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let old_default = global.prefix_root(paths)?.join(&request.name);
    let uses_default = request.destination.as_deref() == Some(old_default.as_path());
    match prompt.text("Environment name", Some(&request.name), |value| {
        picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err)
    })? {
        Reply::Value(value) => {
            request.name = value;
            if uses_default && !request.adopt {
                request.destination = Some(global.prefix_root(paths)?.join(&request.name));
            }
        }
        Reply::Back => {}
    }
    Ok(())
}

fn edit_destination<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    request: &mut ImportRequest,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let adopt = match prompt.choose(
        "Prefix import mode",
        "Mode",
        &numbered([
            ("Copy into wineo".to_owned(), false),
            ("Adopt the original files in place".to_owned(), true),
        ]),
        Some(if request.adopt { "2" } else { "1" }),
        true,
    )? {
        Reply::Value(value) => value,
        Reply::Back => return Ok(()),
    };
    request.adopt = adopt;
    if adopt {
        request.destination = None;
        return Ok(());
    }
    let default = request
        .destination
        .clone()
        .unwrap_or(global.prefix_root(paths)?.join(&request.name));
    match prompt.text(
        "Copy destination",
        Some(&default.display().to_string()),
        |value| {
            paths
                .absolute(&picker::clean_path(value))
                .map_err(|error| error.to_string())
        },
    )? {
        Reply::Value(value) => request.destination = Some(value),
        Reply::Back => {}
    }
    Ok(())
}

fn review_path(prefix: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        prefix.join(path)
    }
}

fn choose_heroic_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    prompt: &mut Prompt<I, W>,
    proton: Option<bool>,
) -> anyhow::Result<String> {
    if let Ok(runner) = Runners::new(paths, global)?.resolve(&imported.runner.name)
        && runner_matches_source(paths, global, imported, &runner)
        && proton.is_none_or(|expected| expected == runner.is_proton())
    {
        return Ok(runner.selection().to_owned());
    }
    if let Some(source) = imported.runner.source.as_deref() {
        let label = format!(
            "Copy Heroic runner {} from {}",
            imported.runner.name,
            source.display()
        );
        let choices = numbered([
            (label, true),
            (
                "Choose another installed or downloaded runner".to_owned(),
                false,
            ),
        ]);
        match prompt.choose(
            "Heroic runner is not installed",
            "Runner",
            &choices,
            Some("1"),
            true,
        )? {
            Reply::Value(true) => return copy_heroic_runner(paths, global, imported, prompt),
            Reply::Value(false) => {}
            Reply::Back => return Err(anyhow::anyhow!(Canceled)),
        }
    }
    match choose_runner(prompt, paths, global, None, proton)? {
        Reply::Value(value) => Ok(value),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn ensure_selected_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    selected: String,
    prompt: &mut Prompt<I, W>,
    proton: Option<bool>,
) -> anyhow::Result<String> {
    if let Ok(runner) = Runners::new(paths, global)?.resolve(&selected)
        && (selected != imported.runner.name
            || runner_matches_source(paths, global, imported, &runner))
    {
        return Ok(selected);
    }
    if selected == imported.runner.name
        && let Some(source) = imported.runner.source.as_deref()
    {
        let choices = numbered([
            (
                format!("Copy Heroic runner from {}", source.display()),
                true,
            ),
            ("Choose another runner".to_owned(), false),
        ]);
        match prompt.choose(
            "Selected runner is not installed",
            "Runner",
            &choices,
            Some("1"),
            true,
        )? {
            Reply::Value(true) => return copy_heroic_runner(paths, global, imported, prompt),
            Reply::Value(false) => {}
            Reply::Back => return Err(anyhow::anyhow!(Canceled)),
        }
    }
    match choose_runner(prompt, paths, global, Some(&selected), proton)? {
        Reply::Value(value) => Ok(value),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn runner_matches_source(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    runner: &Runner,
) -> bool {
    let Some(source) = imported.runner.source.as_deref() else {
        return true;
    };
    let Ok(root) = global.runner_root(paths) else {
        return false;
    };
    wineo_core::paths::resolve(source) == wineo_core::paths::resolve(&root.join(runner.name()))
}

fn copy_heroic_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<String> {
    let source = imported
        .runner
        .source
        .as_deref()
        .context("Heroic did not provide a copyable runner path")?;
    let root = global.runner_root(paths)?;
    let name = if wineo_core::name::is_valid_runner(&imported.runner.name)
        && !wineo_core::runner_alias::is_alias(&imported.runner.name)
        && !root.join(&imported.runner.name).exists()
    {
        imported.runner.name.clone()
    } else {
        match prompt.text("Wineo runner name", Some(&imported.runner.name), |value| {
            if !wineo_core::name::is_valid_runner(value) {
                return Err(
                    "runner names may use letters, digits, dots, dashes, and underscores"
                        .to_owned(),
                );
            }
            if wineo_core::runner_alias::is_alias(value) {
                return Err(
                    "latest aliases are managed by wineo; choose a concrete runner name".to_owned(),
                );
            }
            if root.join(value).exists() {
                return Err(format!("runner `{value}` is already installed"));
            }
            Ok(value.to_owned())
        })? {
            Reply::Value(value) => value,
            Reply::Back => return Err(Canceled.into()),
        }
    };
    let cancellation = Cancellation::new()?;
    let receipt = cancellation.result(import::copy_runner(
        source,
        root,
        &name,
        &cancellation.flag,
        |_| {},
    ))?;
    eprintln!(
        "Copied Heroic runner {} to {}",
        receipt.name,
        receipt.path.display()
    );
    eprintln!("This runner copy is retained if the prefix import is canceled.");
    Ok(receipt.name)
}

fn prefix_inspection(
    imported: &HeroicImport,
    runner: &Runner,
    arch: Option<Arch>,
) -> anyhow::Result<Inspection> {
    import::inspect(&imported.prefix, Some(runner), arch).context("inspect Heroic prefix")
}

fn show_prefix_review<I: Input, W: Write>(
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
    prompt.say(format!(
        "Prefix review:\n  source: {}\n  {}: {}\n  runner: {} -> {}\n  layout: {:?}, {}",
        inspection.source_root.display(),
        if request.adopt {
            "adopt in place"
        } else {
            "copy to"
        },
        destination.display(),
        request.runner,
        Runners::new(paths, global)?
            .resolve(&request.runner)?
            .name(),
        inspection.layout,
        inspection.arch
    ))?;
    for link in &inspection.external_dependencies {
        prompt.say(format!(
            "  external prefix file: {} -> {} (kept in place)",
            link.source.display(),
            link.target.display()
        ))?;
    }
    prompt.say("The source game and launcher must be idle before import.")?;
    prompt.say(if request.adopt {
        "Later launches modify the original prefix. `rm --delete-prefix` deletes that original."
    } else {
        "The source prefix stays in place. `rm --delete-prefix` deletes the imported copy."
    })?;
    if global.gamemode.is_some() || !global.env.is_empty() {
        prompt
            .say("Wineo global settings also apply unless overridden by the imported settings:")?;
        if let Some(enabled) = global.gamemode {
            prompt.say(format!("  gamemode: {enabled}"))?;
        }
        for (key, value) in &global.env {
            prompt.say(format!("  {key}={}", display_env_value(key, value)))?;
        }
    }
    Ok(())
}

fn ensure_source_unchanged(source: &HeroicSource, imported: &HeroicImport) -> anyhow::Result<()> {
    anyhow::ensure!(
        source.unchanged(&imported.id, &imported.fingerprint)?,
        "Heroic settings changed while they were being reviewed; inspect the game again"
    );
    Ok(())
}

fn request(
    paths: &Paths,
    prefix: PathBuf,
    name: String,
    runner: String,
    args: Args,
    _inspection: &Inspection,
) -> anyhow::Result<ImportRequest> {
    Ok(ImportRequest {
        source: prefix,
        name,
        runner,
        destination: if args.adopt {
            None
        } else {
            args.dir
                .as_deref()
                .map(|dir| paths.absolute(dir))
                .transpose()?
        },
        adopt: args.adopt,
        arch: args.arch,
    })
}

fn imported_settings(
    imported: &HeroicImport,
    inspection: &Inspection,
    request: &ImportRequest,
) -> ImportedSettings {
    let destination = request
        .destination
        .as_deref()
        .unwrap_or(&inspection.source_root);
    let rebase = |path: &Path| {
        if request.adopt {
            return path.to_owned();
        }
        path.strip_prefix(&inspection.source_root)
            .map(|relative| destination.join(relative))
            .unwrap_or_else(|_| path.to_owned())
    };
    let exe = rebase(&imported.exe);
    let cwd = imported.cwd.as_deref().map(rebase);
    let target_name = wineo_core::targets::suggest_name(&imported.title, &exe.to_string_lossy());
    let mut targets = BTreeMap::new();
    targets.insert(
        target_name,
        Target {
            exe: exe.to_string_lossy().into_owned(),
            args: imported.args.clone(),
            cwd: cwd.map(|path| path.to_string_lossy().into_owned()),
            default: true,
            gamemode: None,
            desktop: false,
            name: Some(imported.title.clone()),
        },
    );
    ImportedSettings {
        targets,
        env: imported.env.clone(),
        gamemode: imported.gamemode,
        ..ImportedSettings::default()
    }
}

fn clear_runner_dependency(imported: &mut HeroicImport) {
    imported.issues.retain(|issue| {
        !(issue.field.as_deref() == Some("wineVersion")
            && matches!(
                issue.kind,
                HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
            ))
    });
    imported
        .settings_review
        .needs_attention
        .retain(|item| item.field != "wineVersion");
    if let Some(source) = imported.runner.source.as_ref() {
        imported.external_dependencies.retain(|path| path != source);
    }
}

fn reject_unresolved(imported: &HeroicImport) -> anyhow::Result<()> {
    let blocking: Vec<_> = imported
        .issues
        .iter()
        .filter(|issue| !matches!(issue.kind, HeroicIssueKind::External))
        .collect();
    if !blocking.is_empty() {
        for issue in blocking {
            print_issue("unresolved", issue);
        }
        bail!("Heroic launch settings need review; rerun with --interactive");
    }
    Ok(())
}

fn review_settings<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    imported: &mut HeroicImport,
) -> anyhow::Result<()> {
    if imported.issues.iter().any(|issue| {
        issue.kind == HeroicIssueKind::Invalid && issue.field.as_deref() != Some("launcherArgs")
    }) {
        bail!("Heroic has invalid launch settings that cannot be imported automatically");
    }
    if imported
        .issues
        .iter()
        .any(|issue| issue.field.as_deref() == Some("launcherArgs"))
    {
        let args = match prompt.text(
            "Correct launcher arguments",
            Some(&imported.launcher_args),
            |value| wineo_core::heroic::parse_launcher_args(value).map(|_| value.to_owned()),
        )? {
            Reply::Value(value) => value,
            Reply::Back => return Err(anyhow::anyhow!(Canceled)),
        };
        imported.args = wineo_core::heroic::parse_launcher_args(&args)
            .map_err(|error| anyhow::anyhow!(error))?;
        imported.launcher_args = args;
        imported
            .issues
            .retain(|issue| issue.field.as_deref() != Some("launcherArgs"));
    }
    let conflicts: Vec<_> = imported
        .issues
        .iter()
        .filter(|issue| issue.kind == HeroicIssueKind::Conflict)
        .cloned()
        .collect();
    if !conflicts.is_empty() {
        for issue in &conflicts {
            print_issue("conflict", issue);
        }
        match prompt.confirm(
            "Remove conflicting Heroic managed settings and continue?",
            false,
        )? {
            Reply::Value(true) => imported.issues.retain(|issue| {
                issue.kind != HeroicIssueKind::Conflict
                    || issue.field.as_deref() == Some("wineVersion")
            }),
            _ => return Err(anyhow::anyhow!(Canceled)),
        }
    }
    let unsupported = imported
        .issues
        .iter()
        .any(|issue| issue.kind == HeroicIssueKind::Unsupported);
    if unsupported {
        match prompt.confirm("Continue with unsupported Heroic settings omitted?", false)? {
            Reply::Value(true) => imported
                .issues
                .retain(|issue| issue.kind != HeroicIssueKind::Unsupported),
            _ => return Err(anyhow::anyhow!(Canceled)),
        }
    }
    let needs_attention = imported.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
        )
    });
    if needs_attention {
        match prompt.confirm(
            "Keep external paths and other settings needing attention?",
            true,
        )? {
            Reply::Value(true) => imported.issues.retain(|issue| {
                !matches!(
                    issue.kind,
                    HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
                )
            }),
            _ => return Err(anyhow::anyhow!(Canceled)),
        }
    }
    if imported.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            HeroicIssueKind::Missing | HeroicIssueKind::Invalid | HeroicIssueKind::Conflict
        )
    }) {
        bail!("Heroic import still has unresolved settings");
    }
    Ok(())
}

fn show_settings<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    imported: &HeroicImport,
) -> anyhow::Result<()> {
    prompt.say(format!(
        "Heroic game: {} ({})\n  prefix: {}\n  runner: {} ({:?})\n  executable: {}\n  arguments: {:?}\n  working directory: {}\n  gamemode: {:?}",
        imported.title,
        imported.id,
        imported.prefix.display(),
        imported.runner.name,
        imported.runner.kind,
        imported.exe.display(),
        imported.args,
        imported
            .cwd
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "default".to_owned()),
        imported.gamemode
    ))?;
    for path in &imported.external_dependencies {
        prompt.say(format!("  external files stay at {}", path.display()))?;
    }
    if !imported.env.is_empty() {
        prompt.say("  environment:")?;
        for (key, value) in &imported.env {
            prompt.say(format!("    {key}={}", display_env_value(key, value)))?;
        }
    }
    show_review(prompt, &imported.settings_review)?;
    for issue in &imported.issues {
        print_issue("  issue", issue);
    }
    Ok(())
}

fn display_env_value(key: &str, value: &str) -> String {
    let key = key.to_ascii_lowercase();
    if ["pass", "secret", "token", "key", "auth", "credential"]
        .iter()
        .any(|part| key.contains(part))
    {
        "<redacted>".to_owned()
    } else {
        value.to_owned()
    }
}

fn show_review<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    review: &SettingsReview,
) -> anyhow::Result<()> {
    for item in &review.carried {
        prompt.say(format!("  carried: {} = {}", item.field, item.detail))?;
    }
    for item in &review.needs_attention {
        prompt.say(format!("  review: {} = {}", item.field, item.detail))?;
    }
    for item in &review.unsupported {
        prompt.say(format!("  unsupported: {} = {}", item.field, item.detail))?;
    }
    Ok(())
}

fn print_issue(prefix: &str, issue: &HeroicIssue) {
    eprintln!(
        "{prefix}: {}{}",
        issue
            .field
            .as_deref()
            .map(|field| format!("{field}: "))
            .unwrap_or_default(),
        issue.message
    );
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;

    use tempfile::tempdir;
    use wineo_core::heroic::{HeroicRunner, InputFingerprint};
    use wineo_core::import;

    use super::*;
    use crate::prompt::Script;

    fn paths(home: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
    }

    fn reviewed_import(root: &Path) -> (HeroicImport, Inspection, ImportRequest) {
        fs::create_dir_all(root.join("drive_c")).unwrap();
        fs::write(
            root.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let inspection = import::inspect(root, None, None).unwrap();
        let imported = HeroicImport {
            id: "game".to_owned(),
            title: "Game".to_owned(),
            prefix: root.to_owned(),
            runner: HeroicRunner {
                kind: HeroicRunnerKind::System,
                name: "wine".to_owned(),
                path: None,
                source: None,
            },
            kind: HeroicGameKind::Windows,
            exe: root.join("drive_c/game.exe"),
            args: Vec::new(),
            launcher_args: String::new(),
            cwd: Some(root.join("drive_c")),
            env: BTreeMap::new(),
            gamemode: Some(true),
            issues: Vec::new(),
            external_dependencies: Vec::new(),
            fingerprint: InputFingerprint {
                digest: "heroic".to_owned(),
                files: Vec::new(),
            },
            settings_review: SettingsReview::default(),
        };
        let request = ImportRequest {
            source: root.to_owned(),
            name: "game".to_owned(),
            runner: "wine".to_owned(),
            destination: Some(root.join("copy")),
            adopt: false,
            arch: None,
        };
        (imported, inspection, request)
    }

    #[test]
    fn final_review_edits_executable_before_import() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let (mut imported, inspection, mut request) = reviewed_import(root.path());
        let mut prompt = Prompt::new(
            Script(VecDeque::from([
                "2".to_owned(),
                "drive_c/edited.exe".to_owned(),
                "1".to_owned(),
            ])),
            Vec::new(),
        );
        final_review(
            &paths,
            &global,
            &mut imported,
            &inspection,
            &mut request,
            &mut prompt,
        )
        .unwrap();
        assert_eq!(imported.exe, root.path().join("drive_c/edited.exe"));
    }

    #[test]
    fn final_review_cancel_returns_canceled_without_changes() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let (mut imported, inspection, mut request) = reviewed_import(root.path());
        let original = imported.exe.clone();
        let mut prompt = Prompt::new(Script::new(&["q"]), Vec::new());
        let error = final_review(
            &paths,
            &global,
            &mut imported,
            &inspection,
            &mut request,
            &mut prompt,
        )
        .unwrap_err();
        assert!(error.is::<Canceled>());
        assert_eq!(imported.exe, original);
    }
}
