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
use wineo_core::import::{self, ExternalDependency, ImportRequest, ImportedSettings, Inspection};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

use super::desktop;
use super::import::{choose_runner, finish_guided, import_reviewed};
use super::transfer::Cancellation;
use crate::picker;
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};
use crate::style;

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
    /// Create a desktop entry, named after the Heroic title
    #[arg(long)]
    pub desktop: bool,
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

/// Whether to write a desktop entry for the imported target, and what to
/// call it. Kept separate from `HeroicImport` since it isn't a Heroic
/// setting, and separate from `ImportRequest` since it isn't generic to
/// every import.
struct DesktopChoice {
    enabled: bool,
    name: String,
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
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
        let desktop_enabled = args.desktop;
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
        suppress_covered_settings(&mut imported);
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
        let desktop_choice = DesktopChoice {
            enabled: desktop_enabled,
            name: imported.title.clone(),
        };
        let mut output = Prompt::new(OutputOnly, io::stdout());
        show_game_summary(&mut output, paths, &imported, verbose)?;
        let resolved = Runners::new(paths, &global)?.resolve(&request.runner)?;
        let ctx = ReviewContext {
            paths,
            global: &global,
            source: &source,
        };
        output.say(final_facts_text(
            &ctx,
            &imported,
            &inspection,
            &request,
            &resolved,
            verbose,
        ))?;
        reject_unresolved(&imported)?;
        ensure_source_unchanged(&source, &imported)?;
        let settings = imported_settings(&imported, &inspection, &request, &desktop_choice);
        let log_body = verbose.then(|| {
            log_text(
                &global,
                &source,
                &imported,
                &inspection,
                &request,
                &desktop_choice,
            )
        });
        let destination = resolved_destination(paths, &global, &request, &inspection)?;
        let environment = import_reviewed(paths, &global, request, &inspection, Some(&settings))?;
        desktop::reconcile(paths, &global, &environment, Some(&resolved), &destination)?;
        println!(
            "Imported {}. Add or edit targets with wineo setup {}",
            environment.name, environment.name
        );
        if let Some(body) = log_body {
            print_log_path(paths, &environment.name, &body);
        }
        return Ok(ExitCode::SUCCESS);
    }

    anyhow::ensure!(
        prompt::interactive(),
        "guided Heroic import needs a terminal; use --list or specify --game, -n, and --runner"
    );
    let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
    let source = source_guided(paths, args.config_dir.as_deref(), &mut prompt)?;
    match guided(paths, &global, source, args, &mut prompt, verbose) {
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
    verbose: bool,
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
    show_game_summary(prompt, paths, &imported, verbose)?;

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
        ensure_selected_runner(paths, global, &imported, runner, prompt, proton, verbose)?
    } else {
        choose_heroic_runner(paths, global, &imported, prompt, proton, verbose)?
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
    suppress_covered_settings(&mut imported);
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
    let mut desktop_choice = DesktopChoice {
        enabled: true,
        name: imported.title.clone(),
    };
    let ctx = ReviewContext {
        paths,
        global,
        source: &source,
    };
    final_review(
        &ctx,
        &mut imported,
        &inspection,
        &mut request,
        &mut desktop_choice,
        prompt,
        verbose,
    )?;
    let settings = imported_settings(&imported, &inspection, &request, &desktop_choice);
    let log_body = verbose.then(|| {
        log_text(
            global,
            &source,
            &imported,
            &inspection,
            &request,
            &desktop_choice,
        )
    });
    ensure_source_unchanged(&source, &imported)?;
    let destination = resolved_destination(paths, global, &request, &inspection)?;
    let environment = import_reviewed(paths, global, request, &inspection, Some(&settings))?;
    desktop::reconcile(paths, global, &environment, Some(&resolved), &destination)?;
    if let Some(body) = log_body {
        print_log_path(paths, &environment.name, &body);
    }
    finish_guided(paths, global, &environment, prompt)
}

#[derive(Clone, Copy)]
enum EditField {
    Executable,
    WorkingDirectory,
    Arguments,
    Environment,
    GameMode,
    Name,
    Destination,
    Desktop,
}

/// Bundles what every review function needs but doesn't itself change, to
/// stay under clippy's argument-count limit.
struct ReviewContext<'a> {
    paths: &'a Paths,
    global: &'a GlobalConfig,
    source: &'a HeroicSource,
}

fn final_review<I: Input, W: Write>(
    ctx: &ReviewContext,
    imported: &mut HeroicImport,
    inspection: &Inspection,
    request: &mut ImportRequest,
    desktop: &mut DesktopChoice,
    prompt: &mut Prompt<I, W>,
    verbose: bool,
) -> anyhow::Result<()> {
    let paths = ctx.paths;
    let global = ctx.global;
    loop {
        let resolved = Runners::new(paths, global)?.resolve(&request.runner)?;
        let destination = resolved_destination(paths, global, request, inspection)?;
        let facts = final_facts_text(ctx, imported, inspection, request, &resolved, verbose);
        let heading = format!("{facts}\n");

        let cwd_display = match &imported.cwd {
            Some(cwd) => style::path(paths, cwd),
            None => "default".to_owned(),
        };
        let copy_to_display = if request.adopt {
            "adopt in place".to_owned()
        } else {
            style::path(paths, &destination)
        };
        let desktop_display = if desktop.enabled { "yes" } else { "no" }.to_owned();
        let items: [(&str, String, EditField); 8] = [
            (
                "exe",
                style::path(paths, &imported.exe),
                EditField::Executable,
            ),
            ("working dir", cwd_display, EditField::WorkingDirectory),
            (
                "arguments",
                display_args(&imported.args),
                EditField::Arguments,
            ),
            (
                "env",
                env_count_label(&imported.env),
                EditField::Environment,
            ),
            (
                "gamemode",
                on_off(imported.gamemode.unwrap_or(false)).to_owned(),
                EditField::GameMode,
            ),
            ("name", request.name.clone(), EditField::Name),
            ("copy to", copy_to_display, EditField::Destination),
            ("desktop", desktop_display, EditField::Desktop),
        ];
        let width = items
            .iter()
            .map(|(label, _, _)| label.len())
            .max()
            .unwrap_or(0)
            + 2;
        let choices = numbered(
            items
                .into_iter()
                .map(|(label, value, field)| (format!("{label:width$}{value}"), field)),
        );
        let choice = prompt.choose_or_enter(
            &heading,
            Some("Close Heroic and the game first."),
            "Choice, or Enter to import",
            &choices,
            true,
        )?;
        match choice {
            Reply::Value(None) => return Ok(()),
            Reply::Value(Some(field)) => match field {
                EditField::Executable => edit_executable(imported, prompt)?,
                EditField::WorkingDirectory => edit_working_directory(imported, prompt)?,
                EditField::Arguments => edit_arguments(imported, prompt)?,
                EditField::Environment => edit_environment(imported, prompt)?,
                EditField::GameMode => edit_gamemode(imported, prompt)?,
                EditField::Name => edit_name(paths, global, request, prompt)?,
                EditField::Destination => edit_destination(paths, global, request, prompt)?,
                EditField::Desktop => edit_desktop(imported, desktop, prompt)?,
            },
            Reply::Back => return Err(Canceled.into()),
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

fn edit_desktop<I: Input, W: Write>(
    imported: &HeroicImport,
    desktop: &mut DesktopChoice,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    match prompt.confirm("Create a desktop entry?", desktop.enabled)? {
        Reply::Value(true) => {
            let current = if desktop.name.is_empty() {
                imported.title.clone()
            } else {
                desktop.name.clone()
            };
            match prompt.text("Desktop entry name", Some(&current), |value| {
                if value.is_empty() {
                    Err("Enter a name".to_owned())
                } else {
                    Ok(value.to_owned())
                }
            })? {
                Reply::Value(name) => {
                    desktop.enabled = true;
                    desktop.name = name;
                }
                Reply::Back => {}
            }
        }
        Reply::Value(false) => desktop.enabled = false,
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
    verbose: bool,
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
    match choose_runner(prompt, paths, global, None, proton, verbose)? {
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
    verbose: bool,
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
    match choose_runner(prompt, paths, global, Some(&selected), proton, verbose)? {
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

fn resolved_destination(
    paths: &Paths,
    global: &GlobalConfig,
    request: &ImportRequest,
    inspection: &Inspection,
) -> anyhow::Result<PathBuf> {
    Ok(if request.adopt {
        inspection.source_root.clone()
    } else {
        request
            .destination
            .clone()
            .unwrap_or(global.prefix_root(paths)?.join(&request.name))
    })
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
    desktop: &DesktopChoice,
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
            desktop: desktop.enabled,
            name: desktop.enabled.then(|| desktop.name.clone()),
        },
    );
    ImportedSettings {
        targets,
        env: imported.env.clone(),
        gamemode: imported.gamemode,
        heroic_id: Some(imported.id.clone()),
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

/// Heroic settings wineo already covers by way of the chosen runner, so
/// they don't need review or a place in the output: Proton bundles its own
/// DXVK, vkd3d-proton, and NVAPI, so a Proton import can ignore whatever
/// Heroic recorded for them.
const COVERED_BY_PROTON: [&str; 3] = [
    "autoInstallDxvk",
    "autoInstallVkd3d",
    "autoInstallDxvkNvapi",
];

fn suppress_covered_settings(imported: &mut HeroicImport) {
    if imported.runner.kind != HeroicRunnerKind::Proton {
        return;
    }
    imported
        .issues
        .retain(|issue| !matches!(issue.field.as_deref(), Some(field) if COVERED_BY_PROTON.contains(&field)));
    imported
        .settings_review
        .unsupported
        .retain(|item| !COVERED_BY_PROTON.contains(&item.field.as_str()));
    imported
        .settings_review
        .carried
        .retain(|item| !COVERED_BY_PROTON.contains(&item.field.as_str()));
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

/// One labeled row of a rendered block, and any continuation lines under it.
struct Block {
    label: &'static str,
    lines: Vec<String>,
}

fn render_blocks(blocks: &[Block]) -> String {
    let width = blocks
        .iter()
        .map(|block| block.label.len())
        .max()
        .unwrap_or(0)
        + 2;
    let mut out = String::new();
    for block in blocks {
        for (index, line) in block.lines.iter().enumerate() {
            let label = if index == 0 { block.label } else { "" };
            out.push_str(&format!("  {label:width$}{line}\n"));
        }
    }
    out.pop();
    out
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn display_args(args: &[String]) -> String {
    if args.is_empty() {
        return "none".to_owned();
    }
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains(char::is_whitespace) {
                format!("{arg:?}")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn env_count_label(env: &BTreeMap<String, String>) -> String {
    match env.len() {
        0 => "none".to_owned(),
        1 => "1 variable".to_owned(),
        n => format!("{n} variables"),
    }
}

/// Merges `enableEsync`/`enableFsync` into one row when they agree, which is
/// the common case, and keeps them apart when they don't.
fn sync_block(review: &SettingsReview) -> Option<Block> {
    let find = |field: &str| {
        review
            .carried
            .iter()
            .find(|item| item.field == field)
            .map(|item| item.detail == "true")
    };
    match (find("enableEsync"), find("enableFsync")) {
        (Some(esync), Some(fsync)) if esync == fsync => Some(Block {
            label: "esync/fsync",
            lines: vec![on_off(esync).to_owned()],
        }),
        (Some(esync), Some(fsync)) => Some(Block {
            label: "esync/fsync",
            lines: vec![format!("esync {}, fsync {}", on_off(esync), on_off(fsync))],
        }),
        (Some(esync), None) => Some(Block {
            label: "esync",
            lines: vec![on_off(esync).to_owned()],
        }),
        (None, Some(fsync)) => Some(Block {
            label: "fsync",
            lines: vec![on_off(fsync).to_owned()],
        }),
        (None, None) => None,
    }
}

/// The settings that are set for this game, one row each. `-v` also shows
/// the Heroic fields normally left out.
fn show_game_summary<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    imported: &HeroicImport,
    verbose: bool,
) -> anyhow::Result<()> {
    let title = style::title(format!("{} (Heroic {})", imported.title, imported.id));
    let mut blocks = Vec::new();

    let mut runner_label = imported.runner.name.clone();
    if imported.runner.source.is_some() {
        runner_label.push_str(" (Heroic's copy)");
    }
    blocks.push(Block {
        label: "runner",
        lines: vec![runner_label],
    });
    blocks.push(Block {
        label: "exe",
        lines: vec![style::path(paths, &imported.exe)],
    });
    if let Some(cwd) = &imported.cwd {
        blocks.push(Block {
            label: "working dir",
            lines: vec![style::path(paths, cwd)],
        });
    }
    if let Some(gamemode) = imported.gamemode {
        blocks.push(Block {
            label: "gamemode",
            lines: vec![on_off(gamemode).to_owned()],
        });
    }
    if let Some(sync) = sync_block(&imported.settings_review) {
        blocks.push(sync);
    }
    if !imported.env.is_empty() {
        blocks.push(Block {
            label: "env",
            lines: imported
                .env
                .iter()
                .map(|(key, value)| format!("{key}={}", display_env_value(key, value)))
                .collect(),
        });
    }

    let mut warnings = Vec::new();
    if imported.issues.iter().any(|issue| {
        matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        )
    }) {
        warnings.push("EAC and BattlEye runtimes aren't imported".to_owned());
    }
    if verbose {
        for issue in &imported.issues {
            if matches!(
                issue.field.as_deref(),
                Some("eacRuntime") | Some("battlEyeRuntime")
            ) {
                continue;
            }
            if matches!(
                issue.kind,
                HeroicIssueKind::Unsupported | HeroicIssueKind::NeedsAttention
            ) {
                warnings.push(format!(
                    "{}: {}",
                    issue.field.as_deref().unwrap_or("?"),
                    issue.message
                ));
            }
        }
    }
    if !warnings.is_empty() {
        blocks.push(Block {
            label: "warning",
            lines: warnings.iter().map(style::warning).collect(),
        });
    }

    let errors: Vec<String> = imported
        .issues
        .iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                HeroicIssueKind::Missing | HeroicIssueKind::Invalid | HeroicIssueKind::Conflict
            )
        })
        .map(|issue| style::error(&issue.message))
        .collect();
    if !errors.is_empty() {
        blocks.push(Block {
            label: "error",
            lines: errors,
        });
    }

    Ok(prompt.say(format!("{title}\n{}", render_blocks(&blocks)))?)
}

/// The facts the final review shows above the editable settings: what stays
/// where it is, and what the runner and links resolve to.
fn final_facts_text(
    ctx: &ReviewContext,
    imported: &HeroicImport,
    inspection: &Inspection,
    request: &ImportRequest,
    resolved: &Runner,
    verbose: bool,
) -> String {
    let paths = ctx.paths;
    let global = ctx.global;
    let source = ctx.source;
    let title = style::title(format!("{} -> {}", imported.title, request.name));
    let mut blocks = Vec::new();

    let prefix_note = if request.adopt {
        "(adopted, launches modify it)"
    } else {
        "(left in place)"
    };
    blocks.push(Block {
        label: "prefix",
        lines: vec![format!(
            "{} {prefix_note}",
            style::path(paths, &inspection.source_root)
        )],
    });

    let runner_label = if request.runner == resolved.name() {
        resolved.name().to_owned()
    } else {
        format!("{} -> {}", request.runner, resolved.name())
    };
    blocks.push(Block {
        label: "runner",
        lines: vec![runner_label],
    });

    if imported.external_dependencies.contains(&imported.exe)
        && let Some(dir) = imported.exe.parent()
    {
        blocks.push(Block {
            label: "game dir",
            lines: vec![format!("{} (stays in place)", style::path(paths, dir))],
        });
    }

    let links = link_lines(paths, source, &inspection.external_dependencies, verbose);
    if !links.is_empty() {
        blocks.push(Block {
            label: "links",
            lines: links,
        });
    }

    if global.gamemode.is_some() || !global.env.is_empty() {
        blocks.push(global_block(global, verbose));
    }

    format!("{title}\n{}", render_blocks(&blocks))
}

fn global_block(global: &GlobalConfig, verbose: bool) -> Block {
    if verbose {
        let mut lines = Vec::new();
        if let Some(enabled) = global.gamemode {
            lines.push(format!("gamemode {}", on_off(enabled)));
        }
        for (key, value) in &global.env {
            lines.push(format!("{key}={}", display_env_value(key, value)));
        }
        Block {
            label: "global",
            lines,
        }
    } else {
        let mut parts = Vec::new();
        if global.gamemode.is_some() {
            parts.push("gamemode".to_owned());
        }
        if !global.env.is_empty() {
            parts.push(env_count_label(&global.env));
        }
        Block {
            label: "global",
            lines: vec![format!("{} apply unless overridden", parts.join(", "))],
        }
    }
}

/// The standard `dosdevices` drive links aren't shown; they're wineo's own
/// doing, not something Heroic left behind.
fn is_dosdevices(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "dosdevices")
}

fn link_lines(
    paths: &Paths,
    source: &HeroicSource,
    deps: &[ExternalDependency],
    verbose: bool,
) -> Vec<String> {
    let visible: Vec<&ExternalDependency> = deps
        .iter()
        .filter(|dep| !is_dosdevices(&dep.source))
        .collect();
    if visible.is_empty() {
        return Vec::new();
    }
    if verbose {
        return visible
            .iter()
            .map(|dep| {
                format!(
                    "{} -> {}",
                    style::path(paths, &dep.source),
                    style::path(paths, &dep.target)
                )
            })
            .collect();
    }
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for dep in &visible {
        *groups
            .entry(link_group_label(paths, source.config_dir(), &dep.target))
            .or_insert(0) += 1;
    }
    groups
        .into_iter()
        .map(|(label, count)| {
            format!(
                "{count} prefix file{} point into {label}",
                if count == 1 { "" } else { "s" }
            )
        })
        .collect()
}

/// Groups links that land in Heroic's own copy of a runner under its name,
/// so a font or ICU symlink flood collapses to one line.
fn link_group_label(paths: &Paths, config_dir: &Path, target: &Path) -> String {
    let tools = config_dir.join("tools");
    if let Ok(rest) = target.strip_prefix(&tools)
        && let Some(name) = rest
            .components()
            .nth(1)
            .and_then(|c| c.as_os_str().to_str())
    {
        return format!("Heroic's {name}");
    }
    style::path(paths, target.parent().unwrap_or(target))
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

/// The full detail an import log records: every Heroic field, every link,
/// and the global settings that apply, none of it collapsed or hidden.
fn log_text(
    global: &GlobalConfig,
    source: &HeroicSource,
    imported: &HeroicImport,
    inspection: &Inspection,
    request: &ImportRequest,
    desktop: &DesktopChoice,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Heroic game: {} ({})\n",
        imported.title, imported.id
    ));
    out.push_str(&format!("  source: {}\n", source.config_dir().display()));
    out.push_str(&format!("  prefix: {}\n", imported.prefix.display()));
    out.push_str(&format!(
        "  runner: {} ({:?})\n",
        imported.runner.name, imported.runner.kind
    ));
    if let Some(runner_source) = &imported.runner.source {
        out.push_str(&format!("    copied from: {}\n", runner_source.display()));
    }
    out.push_str(&format!("  executable: {}\n", imported.exe.display()));
    out.push_str(&format!("  arguments: {:?}\n", imported.args));
    out.push_str(&format!(
        "  working directory: {}\n",
        imported
            .cwd
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "default".to_owned())
    ));
    out.push_str(&format!("  gamemode: {:?}\n", imported.gamemode));
    if !imported.env.is_empty() {
        out.push_str("  environment:\n");
        for (key, value) in &imported.env {
            out.push_str(&format!("    {key}={}\n", display_env_value(key, value)));
        }
    }
    if !imported.external_dependencies.is_empty() {
        out.push_str("  external files:\n");
        for path in &imported.external_dependencies {
            out.push_str(&format!("    {}\n", path.display()));
        }
    }
    out.push_str("  settings review:\n");
    for item in &imported.settings_review.carried {
        out.push_str(&format!("    carried: {} = {}\n", item.field, item.detail));
    }
    for item in &imported.settings_review.needs_attention {
        out.push_str(&format!("    review: {} = {}\n", item.field, item.detail));
    }
    for item in &imported.settings_review.unsupported {
        out.push_str(&format!(
            "    unsupported: {} = {}\n",
            item.field, item.detail
        ));
    }
    if !imported.issues.is_empty() {
        out.push_str("  issues:\n");
        for issue in &imported.issues {
            out.push_str(&format!(
                "    {:?}{}: {}\n",
                issue.kind,
                issue
                    .field
                    .as_deref()
                    .map(|field| format!(" {field}"))
                    .unwrap_or_default(),
                issue.message
            ));
        }
    }
    out.push_str(&format!(
        "  inspection: layout {:?}, arch {}, source {}\n",
        inspection.layout,
        inspection.arch,
        inspection.source_root.display()
    ));
    if !inspection.external_dependencies.is_empty() {
        out.push_str("  links:\n");
        for link in &inspection.external_dependencies {
            out.push_str(&format!(
                "    {} -> {}\n",
                link.source.display(),
                link.target.display()
            ));
        }
    }
    out.push_str(&format!(
        "  import: name {}, runner {}, {}\n",
        request.name,
        request.runner,
        if request.adopt {
            "adopted in place".to_owned()
        } else {
            format!(
                "copied to {}",
                request
                    .destination
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            )
        }
    ));
    out.push_str(&format!(
        "  desktop entry: {}\n",
        if desktop.enabled {
            desktop.name.as_str()
        } else {
            "no"
        }
    ));
    if global.gamemode.is_some() || !global.env.is_empty() {
        out.push_str("  global settings:\n");
        if let Some(enabled) = global.gamemode {
            out.push_str(&format!("    gamemode: {enabled}\n"));
        }
        for (key, value) in &global.env {
            out.push_str(&format!("    {key}={}\n", display_env_value(key, value)));
        }
    }
    out
}

fn write_import_log(paths: &Paths, name: &str, body: &str) -> anyhow::Result<PathBuf> {
    let path = paths.import_log(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "=== {} ===", jiff::Timestamp::now())?;
    write!(file, "{body}")?;
    if !body.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file)?;
    Ok(path)
}

fn print_log_path(paths: &Paths, name: &str, body: &str) {
    match write_import_log(paths, name, body) {
        Ok(path) => println!("Import log: {}", style::path(paths, &path)),
        Err(error) => eprintln!("warning: could not write the import log: {error:#}"),
    }
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

    fn heroic_source(root: &Path) -> HeroicSource {
        HeroicSource::from_config_dir_with_home(root.join("heroic"), root.to_owned())
    }

    fn fake_proton_runner(root: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let dir = root.join(name);
        fs::create_dir_all(dir.join("files/bin")).unwrap();
        let proton = dir.join("proton");
        fs::write(&proton, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&proton, fs::Permissions::from_mode(0o755)).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = dir.join("files/bin").join(executable);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
    }

    #[test]
    fn final_review_edits_executable_before_import() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let source = heroic_source(root.path());
        let (mut imported, inspection, mut request) = reviewed_import(root.path());
        let mut desktop = DesktopChoice {
            enabled: true,
            name: imported.title.clone(),
        };
        let mut prompt = Prompt::new(
            Script(VecDeque::from([
                "1".to_owned(),
                "drive_c/edited.exe".to_owned(),
                "".to_owned(),
            ])),
            Vec::new(),
        );
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
            },
            &mut imported,
            &inspection,
            &mut request,
            &mut desktop,
            &mut prompt,
            false,
        )
        .unwrap();
        assert_eq!(imported.exe, root.path().join("drive_c/edited.exe"));
    }

    #[test]
    fn final_review_cancel_returns_canceled_without_changes() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let source = heroic_source(root.path());
        let (mut imported, inspection, mut request) = reviewed_import(root.path());
        let mut desktop = DesktopChoice {
            enabled: true,
            name: imported.title.clone(),
        };
        let original = imported.exe.clone();
        let mut prompt = Prompt::new(Script::new(&["q"]), Vec::new());
        let error = final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
            },
            &mut imported,
            &inspection,
            &mut request,
            &mut desktop,
            &mut prompt,
            false,
        )
        .unwrap_err();
        assert!(error.is::<Canceled>());
        assert_eq!(imported.exe, original);
    }

    #[test]
    fn final_review_bare_enter_imports_without_editing() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let source = heroic_source(root.path());
        let (mut imported, inspection, mut request) = reviewed_import(root.path());
        let mut desktop = DesktopChoice {
            enabled: true,
            name: imported.title.clone(),
        };
        let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
            },
            &mut imported,
            &inspection,
            &mut request,
            &mut desktop,
            &mut prompt,
            false,
        )
        .unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        assert!(printed.contains("Game -> game"), "{printed}");
        assert!(printed.contains("  8) desktop      yes"), "{printed}");
        assert!(
            printed.contains("Close Heroic and the game first.\nChoice, or Enter to import: "),
            "{printed}"
        );
    }

    #[test]
    fn final_review_toggles_the_desktop_entry() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let source = heroic_source(root.path());
        let (mut imported, inspection, mut request) = reviewed_import(root.path());
        let mut desktop = DesktopChoice {
            enabled: true,
            name: imported.title.clone(),
        };
        let mut prompt = Prompt::new(Script::new(&["8", "n", ""]), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
            },
            &mut imported,
            &inspection,
            &mut request,
            &mut desktop,
            &mut prompt,
            false,
        )
        .unwrap();
        assert!(!desktop.enabled);
    }

    #[test]
    fn game_summary_merges_eac_and_battleye_into_one_warning() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues.push(HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("eacRuntime".to_owned()),
            message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
        });
        imported.issues.push(HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("battlEyeRuntime".to_owned()),
            message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
        });
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary(&mut prompt, &paths, &imported, false).unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        assert_eq!(printed.matches("aren't imported").count(), 1, "{printed}");
        assert!(
            printed.contains("warning      EAC and BattlEye runtimes aren't imported"),
            "{printed}"
        );
    }

    #[test]
    fn covered_proton_settings_are_suppressed() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.runner.kind = HeroicRunnerKind::Proton;
        for field in COVERED_BY_PROTON {
            imported.issues.push(HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some(field.to_owned()),
                message: "Heroic component installation is not imported".to_owned(),
            });
            imported
                .settings_review
                .unsupported
                .push(wineo_core::heroic::ReviewItem {
                    field: field.to_owned(),
                    detail: "true".to_owned(),
                });
        }
        suppress_covered_settings(&mut imported);
        assert!(imported.issues.is_empty());
        assert!(imported.settings_review.unsupported.is_empty());
        // Suppression does not apply to non-Proton runners.
        let (mut wine_import, _, _) = reviewed_import(root.path());
        wine_import.issues.push(HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("autoInstallDxvk".to_owned()),
            message: "Heroic component installation is not imported".to_owned(),
        });
        suppress_covered_settings(&mut wine_import);
        assert_eq!(wine_import.issues.len(), 1);
    }

    #[test]
    fn imported_settings_records_heroic_id_and_desktop_choice() {
        let root = tempdir().unwrap();
        let (imported, inspection, request) = reviewed_import(root.path());
        let desktop = DesktopChoice {
            enabled: true,
            name: "Game".to_owned(),
        };
        let settings = imported_settings(&imported, &inspection, &request, &desktop);
        assert_eq!(settings.heroic_id.as_deref(), Some("game"));
        let target = settings.targets.values().next().unwrap();
        assert!(target.desktop);
        assert_eq!(target.name.as_deref(), Some("Game"));

        let desktop_off = DesktopChoice {
            enabled: false,
            name: "Game".to_owned(),
        };
        let settings = imported_settings(&imported, &inspection, &request, &desktop_off);
        let target = settings.targets.values().next().unwrap();
        assert!(!target.desktop);
        assert_eq!(target.name, None);
    }

    #[test]
    fn import_log_records_every_field_and_link() {
        let root = tempdir().unwrap();
        let global = GlobalConfig::default();
        let source = heroic_source(root.path());
        let (mut imported, inspection, request) = reviewed_import(root.path());
        imported.env.insert("FOO".to_owned(), "bar".to_owned());
        let desktop = DesktopChoice {
            enabled: true,
            name: "Game".to_owned(),
        };
        let body = log_text(&global, &source, &imported, &inspection, &request, &desktop);
        assert!(body.contains("Heroic game: Game (game)"), "{body}");
        assert!(body.contains("FOO=bar"), "{body}");
        assert!(body.contains("desktop entry: Game"), "{body}");

        let paths = paths(root.path());
        let log_path = write_import_log(&paths, "game", &body).unwrap();
        assert_eq!(log_path, paths.import_log("game"));
        let contents = fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("=== "), "{contents}");
        assert!(contents.contains("Heroic game: Game (game)"), "{contents}");

        // A second write appends another timestamped section.
        write_import_log(&paths, "game", &body).unwrap();
        let contents = fs::read_to_string(&log_path).unwrap();
        assert_eq!(contents.matches("=== ").count(), 2, "{contents}");
    }

    #[test]
    fn link_lines_group_by_heroics_runner_and_hide_dosdevices() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let config_dir = root.path().join("heroic");
        let source =
            HeroicSource::from_config_dir_with_home(config_dir.clone(), root.path().to_owned());
        let runner_dir = config_dir.join("tools/proton/GE-Proton11-1");
        let deps = vec![
            ExternalDependency {
                source: PathBuf::from("/prefix/dosdevices/z:"),
                target: PathBuf::from("/"),
            },
            ExternalDependency {
                source: PathBuf::from("/prefix/drive_c/windows/fonts/a.ttf"),
                target: runner_dir.join("files/share/fonts/a.ttf"),
            },
            ExternalDependency {
                source: PathBuf::from("/prefix/drive_c/windows/fonts/b.ttf"),
                target: runner_dir.join("files/share/fonts/b.ttf"),
            },
        ];
        let lines = link_lines(&paths, &source, &deps, false);
        assert_eq!(
            lines,
            vec!["2 prefix files point into Heroic's GE-Proton11-1"]
        );
        let verbose_lines = link_lines(&paths, &source, &deps, true);
        assert_eq!(verbose_lines.len(), 2);
    }

    #[test]
    fn game_summary_shows_aligned_rows_for_a_typical_proton_game() {
        let root = tempdir().unwrap();
        let root = root.path();
        let paths = paths(root);
        fs::create_dir_all(root.join("drive_c")).unwrap();
        fs::write(
            root.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let exe = root.join("Games/Palworld/Palworld.exe");
        let imported = HeroicImport {
            id: "kZXLzzKAFWBP4dpRatpPHu".to_owned(),
            title: "Palworld".to_owned(),
            prefix: root.join("Games/Heroic/Prefixes/default/Palworld"),
            runner: HeroicRunner {
                kind: HeroicRunnerKind::Proton,
                name: "Proton-CachyOS-latest".to_owned(),
                path: Some(root.join(".config/heroic/tools/proton/Proton-CachyOS-latest/proton")),
                source: Some(root.join(".config/heroic/tools/proton/Proton-CachyOS-latest")),
            },
            kind: HeroicGameKind::Windows,
            exe: exe.clone(),
            args: Vec::new(),
            launcher_args: String::new(),
            cwd: exe.parent().map(Path::to_owned),
            env: BTreeMap::from([
                ("DXVK_NVAPI_ALLOW_OTHER_DRIVERS".to_owned(), "1".to_owned()),
                ("PROTON_ENABLE_NVAPI".to_owned(), "1".to_owned()),
                ("PROTON_ENABLE_WAYLAND".to_owned(), "1".to_owned()),
            ]),
            gamemode: Some(true),
            issues: vec![
                HeroicIssue {
                    kind: HeroicIssueKind::Unsupported,
                    field: Some("eacRuntime".to_owned()),
                    message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
                },
                HeroicIssue {
                    kind: HeroicIssueKind::Unsupported,
                    field: Some("battlEyeRuntime".to_owned()),
                    message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
                },
            ],
            external_dependencies: vec![exe.clone()],
            fingerprint: InputFingerprint {
                digest: "heroic".to_owned(),
                files: Vec::new(),
            },
            settings_review: SettingsReview::default(),
        };
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary(&mut prompt, &paths, &imported, false).unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        println!("{printed}");
        assert!(printed.contains("Palworld (Heroic kZXLzzKAFWBP4dpRatpPHu)"));
        assert!(printed.contains("runner       Proton-CachyOS-latest (Heroic's copy)"));
        assert!(printed.contains("gamemode     on"));
        assert!(printed.contains("warning      EAC and BattlEye runtimes aren't imported"));
    }

    #[test]
    fn final_review_matches_the_palworld_example_shape() {
        let root = tempdir().unwrap();
        let root = root.path();
        let paths = paths(root);
        let runner_root = root.join("runners");
        let runner_name = "proton-cachyos-11.0-20260703-slr-x86_64";
        fake_proton_runner(&runner_root, runner_name);
        let global = GlobalConfig {
            defaults: wineo_core::config::Defaults {
                runner_root: Some(runner_root.display().to_string()),
                ..wineo_core::config::Defaults::default()
            },
            ..GlobalConfig::default()
        };
        let config_dir = root.join(".config/heroic");
        let source = HeroicSource::from_config_dir_with_home(config_dir.clone(), root.to_owned());
        let compatdata = root.join("Games/Heroic/Prefixes/default/Palworld");
        fs::create_dir_all(compatdata.join("pfx/drive_c")).unwrap();
        fs::write(
            compatdata.join("pfx/system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let runner_dir = config_dir.join("tools/proton/GE-Proton11-1");
        let mut deps = Vec::new();
        for index in 0..24 {
            deps.push(ExternalDependency {
                source: compatdata
                    .join("pfx/drive_c/windows")
                    .join(format!("font{index}.ttf")),
                target: runner_dir
                    .join("files/share/fonts")
                    .join(format!("font{index}.ttf")),
            });
        }
        let inspection = Inspection {
            source_root: compatdata.clone(),
            wine_prefix: compatdata.join("pfx"),
            layout: import::ImportLayout::Proton,
            arch: Arch::Win64,
            external_dependencies: deps,
            fingerprint: import::inspect(&compatdata, None, None)
                .unwrap()
                .fingerprint,
        };
        let exe = root.join("Games/Palworld/Palworld.exe");
        let imported = HeroicImport {
            id: "kZXLzzKAFWBP4dpRatpPHu".to_owned(),
            title: "Palworld".to_owned(),
            prefix: compatdata.clone(),
            runner: HeroicRunner {
                kind: HeroicRunnerKind::Proton,
                name: "Proton-CachyOS-latest".to_owned(),
                path: Some(config_dir.join("tools/proton/Proton-CachyOS-latest/proton")),
                source: Some(config_dir.join("tools/proton/Proton-CachyOS-latest")),
            },
            kind: HeroicGameKind::Windows,
            exe: exe.clone(),
            args: Vec::new(),
            launcher_args: String::new(),
            cwd: exe.parent().map(Path::to_owned),
            env: BTreeMap::new(),
            gamemode: Some(true),
            issues: Vec::new(),
            external_dependencies: vec![exe.clone()],
            fingerprint: InputFingerprint {
                digest: "heroic".to_owned(),
                files: Vec::new(),
            },
            settings_review: SettingsReview::default(),
        };
        let mut imported = imported;
        let mut request = ImportRequest {
            source: imported.prefix.clone(),
            name: "palworld".to_owned(),
            runner: runner_name.to_owned(),
            destination: Some(root.join(".local/share/wineo/prefixes/palworld")),
            adopt: false,
            arch: None,
        };
        let mut desktop = DesktopChoice {
            enabled: true,
            name: "palworld".to_owned(),
        };
        let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
            },
            &mut imported,
            &inspection,
            &mut request,
            &mut desktop,
            &mut prompt,
            false,
        )
        .unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        println!("{printed}");
        assert!(printed.contains("Palworld -> palworld"));
        assert!(printed.contains("links     24 prefix files point into Heroic's GE-Proton11-1"));
        assert!(printed.contains("8) desktop      yes"));
    }
}
