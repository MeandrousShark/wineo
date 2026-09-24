use anyhow::{Context, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use wineo_core::components::Component;
use wineo_core::config::{Arch, GlobalConfig, Target};
use wineo_core::download::Provider;
use wineo_core::environment::{self, Environment, HeroicIdSnapshot};
use wineo_core::heroic::{
    HeroicCandidate, HeroicGameKind, HeroicImport, HeroicIssue, HeroicIssueKind, HeroicRunnerKind,
    HeroicSharedPrefix, HeroicSource, ReviewItem, SettingsReview,
};
use wineo_core::import::{
    self, ExternalDependency, ImportRequest, ImportedSettings, Inspection, RunnerCopyInspection,
};
use wineo_core::install::{self, Wanted};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

use super::components;
use super::desktop;
use super::download;
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
    /// Import every importable Heroic sideload at once
    #[arg(long, conflicts_with_all = ["game", "name", "list", "dir", "interactive"])]
    pub all: bool,
    /// Skip the confirmation and allow running without a terminal
    #[arg(short, long)]
    pub yes: bool,
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

#[derive(Clone, Default)]
struct ReviewEvidence {
    issues: Vec<HeroicIssue>,
    settings: SettingsReview,
    shared_prefix: Vec<HeroicSharedPrefix>,
    runner_dependencies: Vec<ExternalDependency>,
}

impl ReviewEvidence {
    fn new(imported: &HeroicImport, shared_prefix: Vec<HeroicSharedPrefix>) -> Self {
        Self {
            issues: imported.issues.clone(),
            settings: imported.settings_review.clone(),
            shared_prefix,
            runner_dependencies: Vec::new(),
        }
    }

    fn resolved(&mut self, imported: &HeroicImport, runner: &Runner) {
        self.issues.retain(|issue| {
            let field = issue.field.as_deref();
            !(runner.is_proton() && field.is_some_and(|field| COVERED_BY_PROTON.contains(&field)))
                && !(field == Some("wineVersion")
                    && matches!(
                        issue.kind,
                        HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
                    ))
                && !(matches!(field, Some("wineVersion") | Some("winePrefix"))
                    && issue.kind == HeroicIssueKind::Conflict)
                && !(field == Some("autoInstallDxvk") && imported.dxvk.is_some())
                && !(field == Some("autoInstallVkd3d") && imported.vkd3d.is_some())
        });
        self.settings = imported.settings_review.clone();
    }
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
    if args.all {
        return match run_all(paths, &global, args, verbose) {
            Ok(code) => Ok(code),
            Err(error) if error.is::<Canceled>() => {
                eprintln!("Import canceled");
                Ok(ExitCode::from(130))
            }
            Err(error) => Err(error),
        };
    }
    let needs_guided =
        args.interactive || args.game.is_none() || args.name.is_none() || args.runner.is_none();
    if !needs_guided {
        let source = source(paths, args.config_dir.as_deref())?;
        let game_id = args.game.as_deref().unwrap();
        let mut imported = source.inspect(game_id)?;
        let mut evidence =
            ReviewEvidence::new(&imported, source.shared_prefix_candidates(game_id)?);
        let name = args.name.clone().unwrap();
        let runner_name = args.runner.clone().unwrap();
        let desktop_enabled = args.desktop;
        let runner = Runners::new(paths, &global)?.resolve(&runner_name)?;
        let inspection = prefix_inspection(&imported, &runner, args.arch)?;
        clear_runner_dependency(&mut imported);
        drop_runner_conflicts(&mut imported);
        suppress_covered_settings(&mut imported, &runner);
        carry_over_components(&mut imported, &runner);
        evidence.resolved(&imported, &runner);
        install::check_available(
            paths,
            &global,
            &name,
            &runner,
            &Wanted {
                dxvk: imported.dxvk.clone(),
                vkd3d: imported.vkd3d.clone(),
            },
        )?;
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
        show_game_summary_with_evidence(&mut output, paths, &imported, &evidence, verbose)?;
        let resolved = Runners::new(paths, &global)?.resolve(&request.runner)?;
        let ctx = ReviewContext {
            paths,
            global: &global,
            source: &source,
            evidence: &evidence,
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
            log_text_with_evidence(
                &global,
                &source,
                &imported,
                &evidence,
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

    if !prompt::interactive()
        && args.runner.is_none()
        && let Some(game) = args.game.as_deref()
    {
        let source = source(paths, args.config_dir.as_deref())?;
        let imported = source.inspect(game)?;
        if imported.runner.source.is_some()
            && let Err(error) = inspect_copyable_runner(&imported)
        {
            bail!(
                "Heroic runner cannot be copied: {error:#}; pass --runner with a compatible host runner"
            );
        }
    }
    anyhow::ensure!(
        prompt::interactive(),
        "guided Heroic import needs a terminal; use --list or specify --game, -n, and --runner"
    );
    let mut prompt =
        Prompt::new(Terminal::open()?, io::stdout()).with_color(style::stdout_enabled());
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
        .filter(|candidate| candidate_filter_issue(candidate).is_none())
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
    let mut evidence = ReviewEvidence::new(&imported, source.shared_prefix_candidates(&game.id)?);
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
    let runner_options = RunnerOptions { proton, verbose };
    let runner_name = if let Some(runner) = args.runner.clone() {
        ensure_selected_runner(
            paths,
            global,
            &imported,
            runner,
            &mut evidence,
            prompt,
            runner_options,
        )?
    } else {
        choose_heroic_runner(
            paths,
            global,
            &imported,
            &mut evidence,
            prompt,
            runner_options,
        )?
    };
    let resolved = Runners::new(paths, global)?.resolve(&runner_name)?;
    let inspection = prefix_inspection(&imported, &resolved, args.arch)?;
    clear_runner_dependency(&mut imported);
    drop_runner_conflicts(&mut imported);
    suppress_covered_settings(&mut imported, &resolved);
    carry_over_components(&mut imported, &resolved);
    evidence.resolved(&imported, &resolved);
    ensure_components_available(
        paths,
        global,
        &name,
        &resolved,
        &Wanted {
            dxvk: imported.dxvk.clone(),
            vkd3d: imported.vkd3d.clone(),
        },
        prompt,
    )?;
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
        evidence: &evidence,
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
        log_text_with_evidence(
            global,
            &source,
            &imported,
            &evidence,
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

// -- Bulk import (`--all`) --

/// A candidate as `--all` displays it before importing: how it's named, and
/// what happens to it.
struct BulkRow {
    name: String,
    title: String,
    status: RowStatus,
}

enum RowStatus {
    Ready { warn: Option<String> },
    Skipped(String),
}

/// A candidate ready to import, with everything already inspected and
/// reviewed so importing it needs no further Heroic reads.
struct ReadyGame {
    name: String,
    title: String,
    imported: HeroicImport,
    evidence: ReviewEvidence,
    inspection: Inspection,
    request: ImportRequest,
}

/// A candidate that's ready except for one missing DXVK/vkd3d-proton build,
/// so it's held back until `--all` decides whether to download it.
struct PendingGame {
    name: String,
    title: String,
    warn: Option<String>,
    component: Component,
    version: Option<String>,
    /// The resolve error's own message, hint included, reused as the skip
    /// reason if this doesn't end up downloaded.
    reason: String,
    imported: HeroicImport,
    evidence: ReviewEvidence,
    inspection: Inspection,
    request: ImportRequest,
}

fn run_all(
    paths: &Paths,
    global: &GlobalConfig,
    args: Args,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        args.yes || prompt::interactive(),
        "bulk Heroic import needs a terminal; pass --yes to skip the confirmation"
    );
    let source = source(paths, args.config_dir.as_deref())?;
    let runners = Runners::new(paths, global)?;
    let mut prompt = if args.yes {
        None
    } else {
        Some(Prompt::new(Terminal::open()?, io::stdout()).with_color(style::stdout_enabled()))
    };

    let (runner_name, runner) = match &args.runner {
        Some(name) => (name.clone(), runners.resolve(name)?),
        None => match runners.newest_proton()? {
            Some(runner) => (runner.selection().to_owned(), runner),
            None => {
                anyhow::ensure!(
                    !args.yes,
                    "no Proton runner is installed; pass --runner or download one first"
                );
                let prompt = prompt.as_mut().expect("interactive was checked above");
                download_latest_ge_proton(paths, global, &runners, prompt)?
            }
        },
    };

    let listing = source.list_candidates()?;
    println!("Checking {} Heroic games...", listing.candidates.len());
    let existing = existing_environments(paths)?;
    let (mut rows, mut ready, already, pending) = classify_candidates(
        paths,
        global,
        &source,
        &listing.candidates,
        &existing,
        &runner_name,
        &runner,
        &args,
    )?;
    let (pending_rows, pending_ready) =
        resolve_pending(paths, global, &runner, pending, args.yes, prompt.as_mut())?;
    rows.extend(pending_rows);
    ready.extend(pending_ready);

    let mode_label = if args.adopt { "adopt" } else { "copy" };
    println!(
        "{}",
        render_bulk_table(&runner_name, mode_label, &rows, &already)
    );
    if ready.is_empty() {
        println!("No Heroic games are ready to import.");
        return Ok(ExitCode::SUCCESS);
    }

    let desktop_enabled = match bulk_desktop_choice(args.desktop, args.yes) {
        Some(value) => value,
        None => {
            let prompt = prompt.as_mut().expect("interactive was checked above");
            match prompt.confirm("Create desktop entries for these games?", true)? {
                Reply::Value(value) => value,
                Reply::Back => return Err(Canceled.into()),
            }
        }
    };

    if !args.yes {
        let prompt = prompt.as_mut().expect("interactive was checked above");
        match prompt.confirm(&format!("Import {} games?", ready.len()), true)? {
            Reply::Value(true) => {}
            _ => return Err(Canceled.into()),
        }
    }

    import_all(
        paths,
        global,
        &source,
        &runner,
        ready,
        desktop_enabled,
        verbose,
    )
}

/// Confirms once, then downloads the newest stable GE-Proton release with
/// the same install routine the guided runner chooser's download uses.
fn download_latest_ge_proton<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    runners: &Runners,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<(String, Runner)> {
    match prompt.confirm(
        "No Proton runner is installed. Download the latest GE-Proton?",
        true,
    )? {
        Reply::Value(true) => {}
        _ => return Err(Canceled.into()),
    }
    let name = download::install_latest(paths, global, Provider::GeProton)?;
    let runner = runners.resolve(&name)?;
    Ok((name, runner))
}

/// An environment already on disk, as much as matching a Heroic candidate
/// against it needs.
#[derive(Clone)]
struct ExistingEnvironment {
    name: String,
    heroic_id: Option<String>,
    snapshot: Option<HeroicIdSnapshot>,
    prefix: PathBuf,
    /// Every target's exe, expanded and resolved the same way as `prefix`.
    target_exes: Vec<PathBuf>,
}

fn existing_environments(paths: &Paths) -> anyhow::Result<Vec<ExistingEnvironment>> {
    let mut found = Vec::new();
    for name in environment::names(paths)? {
        let Ok((loaded, snapshot)) = Environment::load_for_heroic_match(paths, &name) else {
            continue;
        };
        let Ok(prefix) = loaded.prefix(paths) else {
            continue;
        };
        let target_exes = loaded
            .config
            .targets
            .values()
            .filter_map(|target| paths.resolve_stored(&target.exe).ok().flatten())
            .map(|exe| wineo_core::paths::resolve(&exe))
            .collect();
        found.push(ExistingEnvironment {
            name,
            heroic_id: loaded.config.heroic_id.clone(),
            snapshot: Some(snapshot),
            prefix: wineo_core::paths::resolve(&prefix),
            target_exes,
        });
    }
    Ok(found)
}

/// Which existing environment a Heroic candidate was already imported as.
struct ExistingMatch {
    name: String,
    /// Whether that environment predates recording the Heroic id, and so
    /// still needs it backfilled.
    needs_id: bool,
}

/// Matches a Heroic candidate to an environment already imported from it: by
/// its recorded id first, then for older imports, by the prefix it adopted,
/// a target that runs its exe, or its default name.
fn match_existing(
    existing: &[ExistingEnvironment],
    candidate_id: &str,
    candidate_prefix: Option<&Path>,
    candidate_exe: Option<&Path>,
    default_name: &str,
) -> Option<ExistingMatch> {
    if let Some(environment) = existing
        .iter()
        .find(|environment| environment.heroic_id.as_deref() == Some(candidate_id))
    {
        return Some(ExistingMatch {
            name: environment.name.clone(),
            needs_id: false,
        });
    }
    if let Some(prefix) = candidate_prefix {
        let resolved = wineo_core::paths::resolve(prefix);
        if let Some(environment) = existing
            .iter()
            .find(|environment| environment.heroic_id.is_none() && environment.prefix == resolved)
        {
            return Some(ExistingMatch {
                name: environment.name.clone(),
                needs_id: true,
            });
        }
    }
    if let Some(exe) = candidate_exe {
        let resolved = wineo_core::paths::resolve(exe);
        if let Some(environment) = existing.iter().find(|environment| {
            environment.heroic_id.is_none() && environment.target_exes.contains(&resolved)
        }) {
            return Some(ExistingMatch {
                name: environment.name.clone(),
                needs_id: true,
            });
        }
    }
    existing
        .iter()
        .find(|environment| environment.heroic_id.is_none() && environment.name == default_name)
        .map(|environment| ExistingMatch {
            name: environment.name.clone(),
            needs_id: true,
        })
}

fn backfill_match(
    paths: &Paths,
    existing: &mut [ExistingEnvironment],
    matched: &ExistingMatch,
    candidate_id: &str,
) -> anyhow::Result<()> {
    let environment = existing
        .iter_mut()
        .find(|environment| environment.name == matched.name)
        .expect("matched environment is present");
    Environment::record_heroic_id(
        paths,
        &matched.name,
        candidate_id,
        environment
            .snapshot
            .as_ref()
            .expect("disk snapshot is present"),
    )?;
    environment.heroic_id = Some(candidate_id.to_owned());
    Ok(())
}

fn default_name_for(title: &str, executable: Option<&Path>) -> String {
    let exe = executable
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    wineo_core::targets::suggest_name(title, &exe)
}

/// Rows and games sorted three ways: ready, skipped, and pending on a
/// missing DXVK/vkd3d-proton build, plus the environment names already
/// imported.
type Classified = (Vec<BulkRow>, Vec<ReadyGame>, Vec<String>, Vec<PendingGame>);

/// Sorts every Heroic candidate into already imported, skipped with a
/// reason, or ready to import, backfilling `heroic_id` onto legacy matches
/// as it goes.
#[allow(clippy::too_many_arguments)]
fn classify_candidates(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    candidates: &[HeroicCandidate],
    existing: &[ExistingEnvironment],
    runner_name: &str,
    runner: &Runner,
    args: &Args,
) -> anyhow::Result<Classified> {
    let mut rows = Vec::new();
    let mut ready = Vec::new();
    let mut already = Vec::new();
    let mut pending = Vec::new();
    let mut used_names: BTreeSet<String> = BTreeSet::new();
    let mut existing = existing.to_vec();
    let mut shared_by_prefix: BTreeMap<PathBuf, Vec<HeroicSharedPrefix>> = BTreeMap::new();
    for candidate in candidates {
        if let Some(prefix) = candidate.prefix.as_deref() {
            shared_by_prefix
                .entry(wineo_core::paths::resolve(prefix))
                .or_default()
                .push(HeroicSharedPrefix {
                    id: candidate.id.clone(),
                    title: candidate.title.clone(),
                });
        }
    }
    for group in shared_by_prefix.values_mut() {
        group.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
    }

    for candidate in candidates {
        let default_name = default_name_for(&candidate.title, candidate.executable.as_deref());
        if let Some(matched) = match_existing(
            &existing,
            &candidate.id,
            candidate.prefix.as_deref(),
            candidate.executable.as_deref(),
            &default_name,
        ) {
            if matched.needs_id {
                backfill_match(paths, &mut existing, &matched, &candidate.id)?;
            }
            already.push(matched.name);
            continue;
        }
        if let Some(issue) = candidate_filter_issue(candidate) {
            rows.push(BulkRow {
                name: default_name,
                title: candidate.title.clone(),
                status: RowStatus::Skipped(issue.message.clone()),
            });
            continue;
        }
        let mut imported = match source.inspect(&candidate.id) {
            Ok(imported) => imported,
            Err(error) => {
                rows.push(BulkRow {
                    name: default_name,
                    title: candidate.title.clone(),
                    status: RowStatus::Skipped(format!("{error:#}")),
                });
                continue;
            }
        };
        let shared = shared_by_prefix
            .get(&wineo_core::paths::resolve(&imported.prefix))
            .into_iter()
            .flatten()
            .filter(|other| other.id != candidate.id)
            .cloned()
            .collect();
        let mut evidence = ReviewEvidence::new(&imported, shared);
        let candidate_is_proton = imported.runner.kind == HeroicRunnerKind::Proton;
        if candidate_is_proton != runner.is_proton() {
            let reason = if candidate_is_proton {
                "Proton runner, needs --runner"
            } else {
                "Wine runner, needs --runner"
            };
            rows.push(BulkRow {
                name: default_name,
                title: imported.title,
                status: RowStatus::Skipped(reason.to_owned()),
            });
            continue;
        }
        clear_runner_dependency(&mut imported);
        drop_runner_conflicts(&mut imported);
        suppress_covered_settings(&mut imported, runner);
        carry_over_components(&mut imported, runner);
        evidence.resolved(&imported, runner);
        let inspection = match prefix_inspection(&imported, runner, args.arch) {
            Ok(inspection) => inspection,
            Err(error) => {
                rows.push(BulkRow {
                    name: default_name,
                    title: imported.title,
                    status: RowStatus::Skipped(format!("{error:#}")),
                });
                continue;
            }
        };
        let warn = match bulk_review_issues_with_evidence(&mut imported, &evidence, args.adopt) {
            Ok(warn) => warn,
            Err(reason) => {
                rows.push(BulkRow {
                    name: default_name,
                    title: imported.title,
                    status: RowStatus::Skipped(reason),
                });
                continue;
            }
        };
        let name =
            wineo_core::targets::suggest_name(&imported.title, &imported.exe.to_string_lossy());
        if existing.iter().any(|environment| environment.name == name) || used_names.contains(&name)
        {
            let reason = format!("name `{name}` is already taken");
            rows.push(BulkRow {
                name,
                title: imported.title,
                status: RowStatus::Skipped(reason),
            });
            continue;
        }
        used_names.insert(name.clone());
        let destination = if args.adopt {
            None
        } else {
            Some(global.prefix_root(paths)?.join(&name))
        };
        let request = ImportRequest {
            source: imported.prefix.clone(),
            name: name.clone(),
            runner: runner_name.to_owned(),
            destination,
            adopt: args.adopt,
            arch: args.arch,
        };
        let wanted = Wanted {
            dxvk: imported.dxvk.clone(),
            vkd3d: imported.vkd3d.clone(),
        };
        match install::check_available(paths, global, &name, runner, &wanted) {
            Ok(()) => {
                rows.push(BulkRow {
                    name: name.clone(),
                    title: imported.title.clone(),
                    status: RowStatus::Ready { warn },
                });
                ready.push(ReadyGame {
                    name,
                    title: imported.title.clone(),
                    imported,
                    evidence,
                    inspection,
                    request,
                });
            }
            Err(error) => match components::missing_build_component(&error) {
                Some(component) => pending.push(PendingGame {
                    name,
                    title: imported.title.clone(),
                    warn,
                    component,
                    version: components::missing_build_version(&error).map(str::to_owned),
                    reason: error.to_string(),
                    imported,
                    evidence,
                    inspection,
                    request,
                }),
                None => rows.push(BulkRow {
                    name,
                    title: imported.title,
                    status: RowStatus::Skipped(format!("{error:#}")),
                }),
            },
        }
    }
    Ok((rows, ready, already, pending))
}

/// Resolves games only blocked on a missing DXVK/vkd3d-proton build.
/// `--yes` never downloads and fails each one with the same hint `wineo set`
/// gives; interactively, this asks once per missing component, grouped
/// across every game that needs it, before the table.
fn resolve_pending<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    runner: &Runner,
    pending: Vec<PendingGame>,
    yes: bool,
    prompt: Option<&mut Prompt<I, W>>,
) -> anyhow::Result<(Vec<BulkRow>, Vec<ReadyGame>)> {
    resolve_pending_with(
        paths,
        global,
        runner,
        pending,
        yes,
        prompt,
        |component, version| components::download_build(paths, global, component, version),
    )
}

fn resolve_pending_with<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    runner: &Runner,
    mut pending: Vec<PendingGame>,
    yes: bool,
    mut prompt: Option<&mut Prompt<I, W>>,
    mut download: impl FnMut(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<(Vec<BulkRow>, Vec<ReadyGame>)> {
    let mut rows = Vec::new();
    let mut ready = Vec::new();
    let mut offered = Vec::new();

    while !yes && let Some(prompt) = prompt.as_mut() {
        let Some(component) = Component::ALL.into_iter().find(|component| {
            pending.iter().any(|game| game.component == *component) && !offered.contains(component)
        }) else {
            break;
        };
        offered.push(component);
        let versions: Vec<Option<String>> = pending
            .iter()
            .filter(|game| game.component == component)
            .map(|game| game.version.clone())
            .collect();
        let mut unique = Vec::new();
        for version in versions {
            if !unique.contains(&version) {
                unique.push(version);
            }
        }
        let count = pending
            .iter()
            .filter(|game| game.component == component)
            .count();
        let pinned: Vec<&str> = unique.iter().filter_map(|v| v.as_deref()).collect();
        let question = if pinned.is_empty() {
            format!(
                "No matching {} build is installed for {count} game{}. Download the latest release?",
                component.label(),
                if count == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} is missing for {count} game{} ({}). Download the exact requested version{}?",
                component.label(),
                if count == 1 { "" } else { "s" },
                pinned.join(", "),
                if pinned.len() == 1 { "" } else { "s" }
            )
        };
        if !matches!(prompt.confirm(&question, true)?, Reply::Value(true)) {
            continue;
        }
        for version in &unique {
            if version.is_none() && !pinned.is_empty() {
                continue;
            }
            download(component, version.as_deref())?;
        }
        for game in &mut pending {
            let wanted = Wanted {
                dxvk: game.imported.dxvk.clone(),
                vkd3d: game.imported.vkd3d.clone(),
            };
            match install::check_available(paths, global, &game.name, runner, &wanted) {
                Ok(()) => {}
                Err(error) => {
                    if let Some(component) = components::missing_build_component(&error) {
                        game.component = component;
                        game.version = components::missing_build_version(&error).map(str::to_owned);
                    }
                    game.reason = error.to_string();
                }
            }
        }
    }

    for game in pending {
        let wanted = Wanted {
            dxvk: game.imported.dxvk.clone(),
            vkd3d: game.imported.vkd3d.clone(),
        };
        let available = install::check_available(paths, global, &game.name, runner, &wanted);
        if available.is_ok() {
            rows.push(BulkRow {
                name: game.name.clone(),
                title: game.title.clone(),
                status: RowStatus::Ready { warn: game.warn },
            });
            ready.push(ReadyGame {
                name: game.name,
                title: game.title,
                imported: game.imported,
                evidence: game.evidence,
                inspection: game.inspection,
                request: game.request,
            });
        } else {
            rows.push(BulkRow {
                name: game.name,
                title: game.title,
                status: RowStatus::Skipped(game.reason),
            });
        }
    }
    Ok((rows, ready))
}

/// Applies `review_settings`'s guided "yes" defaults without prompting, as
/// `--all` does: conflicting managed settings are dropped and unsupported
/// ones are omitted. Returns a short warning when anything needing
/// attention was kept, or a one-line reason this game can't be imported.
#[cfg(test)]
fn bulk_review_issues(imported: &mut HeroicImport) -> Result<Option<String>, String> {
    let warn = bulk_warning(imported);
    bulk_review_issues_inner(imported, warn)
}

fn bulk_review_issues_with_evidence(
    imported: &mut HeroicImport,
    evidence: &ReviewEvidence,
    adopt: bool,
) -> Result<Option<String>, String> {
    bulk_review_issues_inner(imported, bulk_evidence_warning(evidence, adopt))
}

fn bulk_review_issues_inner(
    imported: &mut HeroicImport,
    warn: Option<String>,
) -> Result<Option<String>, String> {
    if let Some(issue) = imported.issues.iter().find(|issue| {
        issue.kind == HeroicIssueKind::Invalid && issue.field.as_deref() != Some("launcherArgs")
    }) {
        return Err(issue.message.clone());
    }
    if imported
        .issues
        .iter()
        .any(|issue| issue.field.as_deref() == Some("launcherArgs"))
    {
        return Err("invalid launcher arguments".to_owned());
    }
    imported.issues.retain(|issue| {
        !matches!(
            issue.kind,
            HeroicIssueKind::Conflict | HeroicIssueKind::Unsupported
        )
    });
    if let Some(issue) = imported.issues.iter().find(|issue| {
        matches!(
            issue.kind,
            HeroicIssueKind::Missing | HeroicIssueKind::Invalid | HeroicIssueKind::Conflict
        )
    }) {
        return Err(issue.message.clone());
    }
    Ok(warn)
}

/// The short table note for a game `--all` keeps settings needing attention
/// for: EAC/BattlEye specifically, since it's common enough to name, or a
/// generic nudge toward `-v` for anything else.
#[cfg(test)]
fn bulk_warning(imported: &HeroicImport) -> Option<String> {
    let eac_or_battleye = imported.issues.iter().any(|issue| {
        matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        )
    });
    if eac_or_battleye {
        return Some("EAC/BattlEye runtimes aren't imported".to_owned());
    }
    imported
        .issues
        .iter()
        .any(|issue| {
            matches!(
                issue.kind,
                HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
            )
        })
        .then(|| "settings need attention, see -v".to_owned())
}

fn bulk_evidence_warning(evidence: &ReviewEvidence, adopt: bool) -> Option<String> {
    let mut warnings = Vec::new();
    warnings.extend(shared_prefix_lines(&evidence.shared_prefix, adopt, false));
    if let Some(review) = review_evidence_warning(evidence) {
        warnings.push(review);
    }
    (!warnings.is_empty()).then(|| warnings.join("; "))
}

fn review_evidence_warning(evidence: &ReviewEvidence) -> Option<String> {
    let mut warnings = Vec::new();
    for field in ["disableUMU", "autoInstallDxvkNvapi"] {
        if let Some(issue) = evidence
            .issues
            .iter()
            .find(|issue| issue.field.as_deref() == Some(field))
        {
            warnings.push(issue.message.clone());
        }
    }
    let eac = evidence.issues.iter().any(|issue| {
        matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        )
    });
    if eac {
        warnings.push("EAC/BattlEye runtimes aren't imported".to_owned());
    }
    let omitted = evidence
        .issues
        .iter()
        .filter(|issue| {
            issue.kind == HeroicIssueKind::Unsupported
                && !matches!(
                    issue.field.as_deref(),
                    Some("eacRuntime") | Some("battlEyeRuntime")
                )
                && issue.field.as_deref() != Some("disableUMU")
        })
        .count();
    if omitted > 0 {
        warnings.push(format!(
            "{omitted} unsupported setting{} omitted",
            if omitted == 1 { "" } else { "s" }
        ));
    }
    let dropped = evidence
        .issues
        .iter()
        .filter(|issue| dropped_conflict(issue))
        .count();
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} managed conflict{} dropped",
            if dropped == 1 { "" } else { "s" }
        ));
    }
    let attention = evidence.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
        ) && !matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        ) && issue.field.as_deref() != Some("autoInstallDxvkNvapi")
    });
    if attention {
        warnings.push("settings need attention, see -v".to_owned());
    }
    (!warnings.is_empty()).then(|| warnings.join("; "))
}

/// `Some` skips asking: `--desktop` always answers yes, and `--yes` alone
/// (without `--desktop`) answers no. `None` means `--all` must ask.
fn bulk_desktop_choice(desktop_flag: bool, yes_flag: bool) -> Option<bool> {
    if desktop_flag {
        Some(true)
    } else if yes_flag {
        Some(false)
    } else {
        None
    }
}

/// The warning every ready (non-skipped) row shares, if there is at least
/// one ready row and they all carry the exact same one. `None` also covers
/// there being no shared warning to pull out.
fn shared_ready_warning(rows: &[BulkRow]) -> Option<&str> {
    let mut ready_warnings = rows.iter().filter_map(|row| match &row.status {
        RowStatus::Ready { warn } => Some(warn.as_deref()),
        RowStatus::Skipped(_) => None,
    });
    let first = ready_warnings.next()??;
    ready_warnings
        .all(|warn| warn == Some(first))
        .then_some(first)
}

fn render_bulk_table(
    runner_name: &str,
    mode_label: &str,
    rows: &[BulkRow],
    already: &[String],
) -> String {
    let shared_warning = shared_ready_warning(rows);
    let name_width = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    let title_width = rows.iter().map(|row| row.title.len()).max().unwrap_or(0);
    let mut out = format!(
        "{}\n",
        style::title(format!(
            "Import {} Heroic games with {runner_name}:",
            rows.len()
        ))
    );
    for row in rows {
        let status = match &row.status {
            RowStatus::Skipped(reason) => format!("skip: {reason}"),
            RowStatus::Ready { warn } => {
                match warn.as_deref().filter(|_| shared_warning.is_none()) {
                    None => mode_label.to_owned(),
                    Some(warning) => format!(
                        "{mode_label}   {}",
                        style::warning(format!("warn: {warning}"))
                    ),
                }
            }
        };
        let name = &row.name;
        let title = &row.title;
        out.push_str(&format!(
            "  {name:name_width$}  {title:title_width$}  {status}\n"
        ));
    }
    if !already.is_empty() {
        out.push_str(&format!("Already imported: {}\n", already.join(", ")));
    }
    if let Some(warning) = shared_warning {
        out.push_str(&format!("{}\n", style::warning(format!("warn: {warning}"))));
    }
    out.pop();
    out
}

fn import_all(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    runner: &Runner,
    ready: Vec<ReadyGame>,
    desktop_enabled: bool,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    let total = ready.len();
    let mut done = 0usize;
    let mut failed = Vec::new();
    for game in ready {
        if verbose {
            let mut output = Prompt::new(OutputOnly, io::stdout());
            show_game_summary_with_evidence(
                &mut output,
                paths,
                &game.imported,
                &game.evidence,
                verbose,
            )?;
        }
        let desktop = DesktopChoice {
            enabled: desktop_enabled,
            name: game.title.clone(),
        };
        match import_progress(&mut io::stdout().lock(), &game.name, || {
            import_one(paths, global, source, runner, &game, &desktop, verbose)
        })? {
            Ok(log_body) => {
                if let Some(body) = log_body {
                    print_log_path(paths, &game.name, &body);
                }
                done += 1;
            }
            Err(_) => {
                failed.push(game.name);
            }
        }
    }
    if failed.is_empty() {
        println!("Imported {done} of {total} games.");
    } else {
        println!(
            "Imported {done} of {total} games. {} failed: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    Ok(if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn import_progress<W: Write, T>(
    output: &mut W,
    name: &str,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<anyhow::Result<T>> {
    write!(output, "Importing {name}...")?;
    output.flush()?;
    let result = operation();
    match &result {
        Ok(_) => writeln!(output, " done")?,
        Err(error) => writeln!(output, " failed: {error:#}")?,
    }
    output.flush()?;
    Ok(result)
}

fn import_one(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    runner: &Runner,
    game: &ReadyGame,
    desktop: &DesktopChoice,
    verbose: bool,
) -> anyhow::Result<Option<String>> {
    ensure_source_unchanged(source, &game.imported)?;
    let settings = imported_settings(&game.imported, &game.inspection, &game.request, desktop);
    let log_body = verbose.then(|| {
        log_text_with_evidence(
            global,
            source,
            &game.imported,
            &game.evidence,
            &game.inspection,
            &game.request,
            desktop,
        )
    });
    let destination = resolved_destination(paths, global, &game.request, &game.inspection)?;
    let environment = import_reviewed(
        paths,
        global,
        game.request.clone(),
        &game.inspection,
        Some(&settings),
    )?;
    desktop::reconcile(paths, global, &environment, Some(runner), &destination)?;
    Ok(log_body)
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
    evidence: &'a ReviewEvidence,
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
                EditField::Executable => edit_executable(paths, imported, prompt)?,
                EditField::WorkingDirectory => edit_working_directory(paths, imported, prompt)?,
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
    paths: &Paths,
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
        Reply::Value(value) => imported.exe = review_path(paths, &imported.prefix, &value)?,
        Reply::Back => {}
    }
    Ok(())
}

fn edit_working_directory<I: Input, W: Write>(
    paths: &Paths,
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
        Reply::Value(value) => imported.cwd = Some(review_path(paths, &imported.prefix, &value)?),
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

fn review_path(paths: &Paths, prefix: &Path, value: &str) -> anyhow::Result<PathBuf> {
    let clean = picker::clean_path(value);
    let path = Path::new(&clean);
    if path.is_absolute() || clean == "~" || clean.starts_with("~/") {
        paths.absolute(&clean).map_err(Into::into)
    } else {
        Ok(prefix.join(path))
    }
}

#[derive(Clone, Copy)]
struct RunnerOptions {
    proton: Option<bool>,
    verbose: bool,
}

fn choose_heroic_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    evidence: &mut ReviewEvidence,
    prompt: &mut Prompt<I, W>,
    options: RunnerOptions,
) -> anyhow::Result<String> {
    if let Ok(runner) = Runners::new(paths, global)?.resolve(&imported.runner.name)
        && runner_matches_source(paths, global, imported, &runner)
        && options
            .proton
            .is_none_or(|expected| expected == runner.is_proton())
    {
        return Ok(runner.selection().to_owned());
    }
    if let Some(source) = imported.runner.source.as_deref() {
        let reviewed = match inspect_copyable_runner(imported) {
            Ok(reviewed) => Some(reviewed),
            Err(error) => {
                prompt.say(format!(
                    "Heroic runner cannot be copied: {error:#}. Choose a compatible host runner."
                ))?;
                None
            }
        };
        if let Some(reviewed) = reviewed {
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
                Reply::Value(true) => {
                    return copy_heroic_runner(
                        paths, global, imported, &reviewed, evidence, prompt,
                    );
                }
                Reply::Value(false) => {}
                Reply::Back => return Err(anyhow::anyhow!(Canceled)),
            }
        }
    }
    match choose_runner(prompt, paths, global, None, options.proton, options.verbose)? {
        Reply::Value(value) => Ok(value),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn ensure_selected_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    selected: String,
    evidence: &mut ReviewEvidence,
    prompt: &mut Prompt<I, W>,
    options: RunnerOptions,
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
        let reviewed = match inspect_copyable_runner(imported) {
            Ok(reviewed) => Some(reviewed),
            Err(error) => {
                prompt.say(format!(
                    "Heroic runner cannot be copied: {error:#}. Choose a compatible host runner."
                ))?;
                None
            }
        };
        if let Some(reviewed) = reviewed {
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
                Reply::Value(true) => {
                    return copy_heroic_runner(
                        paths, global, imported, &reviewed, evidence, prompt,
                    );
                }
                Reply::Value(false) => {}
                Reply::Back => return Err(anyhow::anyhow!(Canceled)),
            }
        }
    }
    match choose_runner(
        prompt,
        paths,
        global,
        Some(&selected),
        options.proton,
        options.verbose,
    )? {
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
    reviewed: &RunnerCopyInspection,
    evidence: &mut ReviewEvidence,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<String> {
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
    let receipt = cancellation.published_result(import::copy_runner_reviewed(
        reviewed,
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
    evidence.runner_dependencies = receipt.external_dependencies.clone();
    for line in runner_dependency_lines(&receipt.external_dependencies, false) {
        eprintln!("Runner dependency: {line}");
    }
    Ok(receipt.name)
}

fn inspect_copyable_runner(imported: &HeroicImport) -> anyhow::Result<RunnerCopyInspection> {
    let source = imported
        .runner
        .source
        .as_deref()
        .context("Heroic did not provide a runner source")?;
    let reviewed = import::inspect_runner_copy(source)?;
    let compatible = match imported.runner.kind {
        HeroicRunnerKind::Proton => reviewed.supports_proton,
        HeroicRunnerKind::Wine | HeroicRunnerKind::System => reviewed.supports_wine,
        HeroicRunnerKind::Unknown => false,
    };
    anyhow::ensure!(
        compatible,
        "{} is not a compatible {} build",
        source.display(),
        if imported.runner.kind == HeroicRunnerKind::Proton {
            "Proton"
        } else {
            "Wine"
        }
    );
    Ok(reviewed)
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
        dxvk: imported.dxvk.clone(),
        vkd3d: imported.vkd3d.clone(),
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

/// Drops the wineVersion/winePrefix Conflict issues, which just flag that
/// Heroic's own runner choice doesn't apply once wineo picks its own.
fn drop_runner_conflicts(imported: &mut HeroicImport) {
    imported.issues.retain(|issue| {
        !(issue.kind == HeroicIssueKind::Conflict
            && matches!(
                issue.field.as_deref(),
                Some("wineVersion") | Some("winePrefix")
            ))
    });
}

/// The issue that makes `candidate` unimportable, the same rule guided
/// selection and `--all` both filter candidates by. `None` means it can be
/// imported.
fn candidate_filter_issue(candidate: &HeroicCandidate) -> Option<&HeroicIssue> {
    if candidate.kind != HeroicGameKind::Windows {
        return candidate
            .issues
            .iter()
            .find(|issue| issue.kind == HeroicIssueKind::Unsupported);
    }
    candidate.issues.iter().find(|issue| {
        (matches!(
            issue.kind,
            HeroicIssueKind::Missing | HeroicIssueKind::Invalid
        ) && issue.field.as_deref() != Some("launcherArgs"))
            || (issue.field.as_deref() == Some("runner")
                && issue.kind == HeroicIssueKind::Unsupported)
    })
}

/// Heroic settings wineo already covers by way of the chosen runner, so
/// they don't need review or a place in the output: Proton bundles its own
/// DXVK, vkd3d-proton, and NVAPI, so a Proton import can ignore whatever
/// Heroic recorded for them. What matters is the runner wineo ends up
/// using, not what Heroic itself recorded.
const COVERED_BY_PROTON: [&str; 3] = [
    "autoInstallDxvk",
    "autoInstallVkd3d",
    "autoInstallDxvkNvapi",
];

fn suppress_covered_settings(imported: &mut HeroicImport, runner: &Runner) {
    if !runner.is_proton() {
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

/// Heroic's `autoInstallDxvk`/`autoInstallVkd3d` become `dxvk`/`vkd3d =
/// "latest"` for a Wine (or system) chosen runner, which install their own
/// way; a Proton runner still drops them, which `suppress_covered_settings`
/// already handles.
fn carry_over_components(imported: &mut HeroicImport, runner: &Runner) {
    if runner.is_proton() {
        return;
    }
    if take_unsupported_issue(imported, "autoInstallDxvk") {
        imported.dxvk = Some(wineo_core::components::LATEST.to_owned());
        imported.settings_review.carried.push(ReviewItem {
            field: "autoInstallDxvk".to_owned(),
            detail: "dxvk=latest".to_owned(),
        });
    }
    if take_unsupported_issue(imported, "autoInstallVkd3d") {
        imported.vkd3d = Some(wineo_core::components::LATEST.to_owned());
        imported.settings_review.carried.push(ReviewItem {
            field: "autoInstallVkd3d".to_owned(),
            detail: "vkd3d=latest".to_owned(),
        });
    }
}

/// Removes the Unsupported issue for `field`, if there is one, and says
/// whether there was: Heroic's own setting was on.
fn take_unsupported_issue(imported: &mut HeroicImport, field: &str) -> bool {
    let had = imported.issues.iter().any(|issue| {
        issue.kind == HeroicIssueKind::Unsupported && issue.field.as_deref() == Some(field)
    });
    if had {
        imported
            .issues
            .retain(|issue| issue.field.as_deref() != Some(field));
        imported
            .settings_review
            .unsupported
            .retain(|item| item.field != field);
    }
    had
}

/// Offers to download a missing DXVK/vkd3d-proton build for guided Heroic
/// import, the same way guided `new` does, one component at a time until
/// everything resolves or a person declines. Declining leaves the usual
/// error, so this changes nothing about what ends up refused.
fn ensure_components_available<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    wanted: &Wanted,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    ensure_components_available_with(
        paths,
        global,
        name,
        runner,
        wanted,
        prompt,
        |component, version| components::download_build(paths, global, component, version),
    )
}

fn ensure_components_available_with<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    wanted: &Wanted,
    prompt: &mut Prompt<I, W>,
    mut download: impl FnMut(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    loop {
        match install::check_available(paths, global, name, runner, wanted) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let Some(component) = components::missing_build_component(&error) else {
                    return Err(error.into());
                };
                let version = components::missing_build_version(&error);
                let question = match version {
                    Some(name) => format!(
                        "{} build `{name}` is not installed. Download that exact version?",
                        component.label()
                    ),
                    None => format!(
                        "No matching {} build is installed. Download the latest release?",
                        component.label()
                    ),
                };
                if !components::offer_download_with(
                    prompt,
                    component,
                    version,
                    &question,
                    &mut download,
                )? {
                    return Err(error.into());
                }
            }
        }
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
    // The summary already showed these as warnings. Conflicting settings are
    // ones wineo manages itself, unsupported ones have nowhere to go, and the
    // final review edits the paths needing attention.
    imported.issues.retain(|issue| match issue.kind {
        HeroicIssueKind::Conflict => !dropped_conflict(issue),
        HeroicIssueKind::Unsupported
        | HeroicIssueKind::NeedsAttention
        | HeroicIssueKind::External => false,
        _ => true,
    });
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
            if arg.is_empty() || arg.contains(char::is_whitespace) || arg.contains(['"', '\\']) {
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
/// A conflict with a setting wineo manages itself, which import drops
/// rather than refusing over. Runner and prefix conflicts still block.
fn dropped_conflict(issue: &HeroicIssue) -> bool {
    issue.kind == HeroicIssueKind::Conflict
        && !matches!(
            issue.field.as_deref(),
            Some("wineVersion") | Some("winePrefix")
        )
}

/// Proton handles a prefix with both registry layouts, so the conflict
/// Heroic's reader raises for it is not worth showing before the runner is
/// picked. It is dropped for real once one is.
fn covered_prefix_conflict(imported: &HeroicImport, issue: &HeroicIssue) -> bool {
    imported.runner.kind == HeroicRunnerKind::Proton
        && issue.kind == HeroicIssueKind::Conflict
        && issue.field.as_deref() == Some("winePrefix")
}

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
    if !imported.args.is_empty() {
        blocks.push(Block {
            label: "arguments",
            lines: vec![display_args(&imported.args)],
        });
    }
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
    if let Some(dxvk) = &imported.dxvk {
        blocks.push(Block {
            label: "dxvk",
            lines: vec![dxvk.clone()],
        });
    }
    if let Some(vkd3d) = &imported.vkd3d {
        blocks.push(Block {
            label: "vkd3d",
            lines: vec![vkd3d.clone()],
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
    for issue in &imported.issues {
        if matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        ) {
            continue;
        }
        let field = issue.field.as_deref().unwrap_or("?");
        match issue.kind {
            HeroicIssueKind::NeedsAttention if verbose => {
                warnings.push(format!("{field}: {}", issue.message));
            }
            HeroicIssueKind::NeedsAttention => warnings.push(issue.message.clone()),
            HeroicIssueKind::Unsupported if verbose => {
                warnings.push(format!("{field}: {}", issue.message));
            }
            HeroicIssueKind::Unsupported if field == "disableUMU" => {
                warnings.push(issue.message.clone());
            }
            HeroicIssueKind::Conflict if dropped_conflict(issue) => {
                warnings.push(format!("{field} is dropped, wineo manages it"));
            }
            _ => {}
        }
    }
    let hidden = imported
        .issues
        .iter()
        .filter(|issue| {
            issue.kind == HeroicIssueKind::Unsupported
                && issue.field.as_deref() != Some("disableUMU")
                && !matches!(
                    issue.field.as_deref(),
                    Some("eacRuntime") | Some("battlEyeRuntime")
                )
                && !issue
                    .field
                    .as_deref()
                    .is_some_and(|field| COVERED_BY_PROTON.contains(&field))
        })
        .count();
    if !verbose && hidden > 0 {
        warnings.push(format!(
            "{hidden} more Heroic {} not imported, -v lists them",
            if hidden == 1 {
                "setting is"
            } else {
                "settings are"
            }
        ));
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
            ) && !covered_prefix_conflict(imported, issue)
                && !dropped_conflict(issue)
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

fn show_game_summary_with_evidence<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    imported: &HeroicImport,
    evidence: &ReviewEvidence,
    verbose: bool,
) -> anyhow::Result<()> {
    let mut view = imported.clone();
    view.issues = evidence.issues.clone();
    view.settings_review = evidence.settings.clone();
    show_game_summary(prompt, paths, &view, verbose)
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
    let evidence = ctx.evidence;
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
    if let Some(dxvk) = &imported.dxvk {
        blocks.push(Block {
            label: "dxvk",
            lines: vec![dxvk.clone()],
        });
    }
    if let Some(vkd3d) = &imported.vkd3d {
        blocks.push(Block {
            label: "vkd3d",
            lines: vec![vkd3d.clone()],
        });
    }
    if !imported.args.is_empty() {
        blocks.push(Block {
            label: "arguments",
            lines: vec![display_args(&imported.args)],
        });
    }

    let shared = shared_prefix_lines(&evidence.shared_prefix, request.adopt, verbose);
    if !shared.is_empty() {
        blocks.push(Block {
            label: "shared prefix",
            lines: shared,
        });
    }
    let runner_deps = runner_dependency_lines(&evidence.runner_dependencies, verbose);
    if !runner_deps.is_empty() {
        blocks.push(Block {
            label: "runner links",
            lines: runner_deps,
        });
    }
    if let Some(warn) = review_evidence_warning(evidence) {
        blocks.push(Block {
            label: "warning",
            lines: vec![style::warning(warn)],
        });
    }
    if verbose {
        let detail: Vec<String> = evidence
            .issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.kind,
                    HeroicIssueKind::Conflict
                        | HeroicIssueKind::Unsupported
                        | HeroicIssueKind::NeedsAttention
                )
            })
            .map(|issue| {
                format!(
                    "{}: {}",
                    issue.field.as_deref().unwrap_or("setting"),
                    issue.message
                )
            })
            .collect();
        if !detail.is_empty() {
            blocks.push(Block {
                label: "review",
                lines: detail,
            });
        }
    }

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

fn shared_prefix_lines(shared: &[HeroicSharedPrefix], adopt: bool, verbose: bool) -> Vec<String> {
    if shared.is_empty() {
        return Vec::new();
    }
    let names = shared
        .iter()
        .map(|game| {
            if verbose {
                format!("{} ({})", game.title, game.id)
            } else {
                game.title.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!(
        "{} other Heroic game{} {} this prefix: {names}. {}",
        shared.len(),
        if shared.len() == 1 { "" } else { "s" },
        if shared.len() == 1 { "uses" } else { "use" },
        if adopt {
            "Adoption shares mutable state."
        } else {
            "Copying includes the whole prefix."
        }
    )]
}

fn runner_dependency_lines(deps: &[ExternalDependency], verbose: bool) -> Vec<String> {
    if deps.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{} external link{} remain required by the copied runner",
        deps.len(),
        if deps.len() == 1 { "" } else { "s" }
    )];
    if verbose {
        lines.extend(
            deps.iter()
                .map(|dep| format!("{} -> {}", dep.source.display(), dep.target.display())),
        );
    }
    lines
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

fn log_text_with_evidence(
    global: &GlobalConfig,
    source: &HeroicSource,
    imported: &HeroicImport,
    evidence: &ReviewEvidence,
    inspection: &Inspection,
    request: &ImportRequest,
    desktop: &DesktopChoice,
) -> String {
    let mut view = imported.clone();
    view.issues = evidence.issues.clone();
    view.settings_review = evidence.settings.clone();
    let mut out = log_text(global, source, &view, inspection, request, desktop);
    for line in shared_prefix_lines(&evidence.shared_prefix, request.adopt, true) {
        out.push_str(&format!("  shared prefix: {line}\n"));
    }
    for line in runner_dependency_lines(&evidence.runner_dependencies, true) {
        out.push_str(&format!("  runner dependency: {line}\n"));
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
            dxvk: None,
            vkd3d: None,
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

    fn fake_wine_runner(root: &Path, name: &str) -> Runner {
        use std::os::unix::fs::PermissionsExt;
        let dir = root.join(name);
        fs::create_dir_all(dir.join("bin")).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = dir.join("bin").join(executable);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Runners::at(root.to_owned()).resolve(name).unwrap()
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
                evidence: &ReviewEvidence::default(),
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
    fn edited_paths_expand_home_and_preserve_prefix_relative_paths() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let prefix = root.path().join("prefix");
        let (mut imported, inspection, mut request) = reviewed_import(&prefix);
        let mut prompt = Prompt::new(Script::new(&["\"~/Games/My Game/game.exe\""]), Vec::new());
        edit_executable(&paths, &mut imported, &mut prompt).unwrap();
        assert_eq!(imported.exe, root.path().join("Games/My Game/game.exe"));
        let mut prompt = Prompt::new(Script::new(&["~/Games/My\\ Game"]), Vec::new());
        edit_working_directory(&paths, &mut imported, &mut prompt).unwrap();
        assert_eq!(imported.cwd, Some(root.path().join("Games/My Game")));

        let mut prompt = Prompt::new(
            Script::new(&["drive_c/Program\\ Files/game.exe"]),
            Vec::new(),
        );
        edit_executable(&paths, &mut imported, &mut prompt).unwrap();
        assert_eq!(imported.exe, prefix.join("drive_c/Program Files/game.exe"));
        request.destination = Some(root.path().join("copy"));
        let settings = imported_settings(
            &imported,
            &inspection,
            &request,
            &DesktopChoice {
                enabled: false,
                name: String::new(),
            },
        );
        let target = settings.targets.values().next().unwrap();
        assert_eq!(
            target.exe,
            root.path()
                .join("copy/drive_c/Program Files/game.exe")
                .display()
                .to_string()
        );
        assert_eq!(
            target.cwd.as_deref(),
            Some(root.path().join("Games/My Game").to_str().unwrap())
        );

        let absolute = root.path().join("External Game/game.exe");
        let edited = review_path(
            &paths,
            &imported.prefix,
            &format!("'{}'", absolute.display()),
        )
        .unwrap();
        assert_eq!(edited, absolute);
    }

    #[test]
    fn review_evidence_survives_bulk_defaults_and_reaches_output_and_log() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let source = heroic_source(root.path());
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let (mut imported, inspection, request) = reviewed_import(root.path());
        imported.args = vec!["one word".to_owned(), "say\"hello".to_owned()];
        imported.issues = vec![
            HeroicIssue {
                kind: HeroicIssueKind::Conflict,
                field: Some("WINEPREFIX".to_owned()),
                message: "managed by wineo".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("wrapperOptions".to_owned()),
                message: "not imported".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::NeedsAttention,
                field: Some("autoInstallDxvkNvapi".to_owned()),
                message:
                    "Wine NVAPI requires dxvk-nvapi DLLs; enabling variables does not install them"
                        .to_owned(),
            },
        ];
        imported.settings_review.unsupported.push(ReviewItem {
            field: "wrapperOptions".to_owned(),
            detail: "ignored".to_owned(),
        });
        let mut evidence = ReviewEvidence::new(&imported, Vec::new());
        evidence.resolved(&imported, &runner);
        let warning = bulk_review_issues_with_evidence(&mut imported, &evidence, false)
            .unwrap()
            .unwrap();
        assert!(warning.contains("unsupported setting omitted"), "{warning}");
        assert!(warning.contains("managed conflict dropped"), "{warning}");
        assert!(warning.contains("Wine NVAPI requires"), "{warning}");
        assert!(
            imported
                .issues
                .iter()
                .all(|issue| issue.kind != HeroicIssueKind::Unsupported)
        );

        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary_with_evidence(&mut prompt, &paths, &imported, &evidence, true).unwrap();
        let output = String::from_utf8_lossy(prompt.output());
        assert!(output.contains("wrapperOptions"), "{output}");
        assert!(output.contains("WINEPREFIX"), "{output}");
        assert!(output.contains("\"one word\""), "{output}");
        assert!(output.contains("say\\\"hello"), "{output}");
        let log = log_text_with_evidence(
            &global,
            &source,
            &imported,
            &evidence,
            &inspection,
            &request,
            &DesktopChoice {
                enabled: false,
                name: String::new(),
            },
        );
        assert!(log.contains("wrapperOptions"), "{log}");
        assert!(log.contains("WINEPREFIX"), "{log}");
        assert!(!log.contains("\u{1b}["), "{log}");
    }

    #[test]
    fn resolved_layout_conflicts_do_not_reappear_as_current_errors() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues = ["wineVersion", "winePrefix", "WINEPREFIX"]
            .into_iter()
            .map(|field| HeroicIssue {
                kind: HeroicIssueKind::Conflict,
                field: Some(field.to_owned()),
                message: format!("{field} conflicts"),
            })
            .collect();
        let mut evidence = ReviewEvidence::new(&imported, Vec::new());
        drop_runner_conflicts(&mut imported);
        evidence.resolved(&imported, &runner);
        assert_eq!(evidence.issues.len(), 1);
        assert_eq!(evidence.issues[0].field.as_deref(), Some("WINEPREFIX"));
        assert_eq!(
            review_evidence_warning(&evidence).as_deref(),
            Some("1 managed conflict dropped")
        );
        let mut output = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary_with_evidence(&mut output, &paths, &imported, &evidence, true).unwrap();
        let text = String::from_utf8_lossy(output.output());
        assert!(!text.contains("wineVersion conflicts"), "{text}");
        assert!(!text.contains("winePrefix conflicts"), "{text}");
        assert!(text.contains("WINEPREFIX is dropped"), "{text}");
    }

    #[test]
    fn runner_copy_preflight_checks_access_and_kind() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.runner.source = Some(root.path().join("missing"));
        assert!(inspect_copyable_runner(&imported).is_err());
        let runner_root = root.path().join("runners");
        fake_wine_runner(&runner_root, "wine-tkg");
        imported.runner.source = Some(runner_root.join("wine-tkg"));
        imported.runner.kind = HeroicRunnerKind::Wine;
        assert!(inspect_copyable_runner(&imported).is_ok());
        imported.runner.kind = HeroicRunnerKind::Proton;
        assert!(inspect_copyable_runner(&imported).is_err());
    }

    #[test]
    fn import_progress_flushes_before_blocked_copy_and_completes_same_line() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex, mpsc};

        struct VisibleWriter {
            bytes: Arc<Mutex<Vec<u8>>>,
            flushes: Arc<AtomicUsize>,
        }
        impl Write for VisibleWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.bytes.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let root = tempdir().unwrap();
        let source_root = root.path().join("source-runners");
        fake_wine_runner(&source_root, "wine-tkg");
        let reviewed = import::inspect_runner_copy(source_root.join("wine-tkg")).unwrap();
        let destination_root = root.path().join("copied-runners");
        let installed = destination_root.join("wine-copy");
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let flushes = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = VisibleWriter {
            bytes: bytes.clone(),
            flushes: flushes.clone(),
        };
        let worker = std::thread::spawn(move || {
            let mut writer = writer;
            import_progress(&mut writer, "hades", || {
                let cancelled = AtomicBool::new(false);
                let mut paused = false;
                import::copy_runner_reviewed(
                    &reviewed,
                    destination_root,
                    "wine-copy",
                    &cancelled,
                    |progress| {
                        if !paused && matches!(progress, import::ImportProgress::Copied { .. }) {
                            paused = true;
                            started_tx.send(()).unwrap();
                            release_rx.recv().unwrap();
                        }
                    },
                )
            })
            .unwrap()
            .unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(&*bytes.lock().unwrap(), b"Importing hades...");
        assert_eq!(flushes.load(Ordering::SeqCst), 1);
        assert!(!installed.exists());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(&*bytes.lock().unwrap(), b"Importing hades... done\n");
        assert!(installed.join("bin/wine").is_file());

        let mut failed = Vec::new();
        assert!(
            import_progress(&mut failed, "alpha", || -> anyhow::Result<()> {
                anyhow::bail!("copy failed")
            })
            .unwrap()
            .is_err()
        );
        assert_eq!(failed, b"Importing alpha... failed: copy failed\n");
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
                evidence: &ReviewEvidence::default(),
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
                evidence: &ReviewEvidence::default(),
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
                evidence: &ReviewEvidence::default(),
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
    fn review_items_are_summary_warnings_not_prompts() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues = vec![
            HeroicIssue {
                kind: HeroicIssueKind::NeedsAttention,
                field: Some("targetExe".to_owned()),
                message: "executable path was relative".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::External,
                field: Some("external files".to_owned()),
                message: "game files or saves remain outside the imported prefix".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Conflict,
                field: Some("WINEPREFIX".to_owned()),
                message: "Heroic setting conflicts with a wineo-managed variable".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("gamescope".to_owned()),
                message: "gamescope is not imported".to_owned(),
            },
        ];
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary(&mut prompt, &paths(root.path()), &imported, false).unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        assert!(
            printed.contains("executable path was relative"),
            "{printed}"
        );
        assert!(!printed.contains("remain outside"), "{printed}");
        assert!(
            printed.contains("WINEPREFIX is dropped, wineo manages it"),
            "{printed}"
        );
        assert!(
            printed.contains("1 more Heroic setting is not imported"),
            "{printed}"
        );
        assert!(!printed.contains("error"), "{printed}");

        review_settings(&mut prompt, &mut imported).unwrap();
        assert!(imported.issues.is_empty());
    }

    #[test]
    fn covered_proton_settings_are_suppressed() {
        let root = tempdir().unwrap();
        let runners_root = root.path().join("runners");
        fake_proton_runner(&runners_root, "GE-Proton9-20");
        let proton = Runners::at(runners_root.clone())
            .resolve("GE-Proton9-20")
            .unwrap();
        let wine = fake_wine_runner(&runners_root, "wine-tkg");

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
        suppress_covered_settings(&mut imported, &proton);
        assert!(imported.issues.is_empty());
        assert!(imported.settings_review.unsupported.is_empty());
        // Suppression follows the chosen wineo runner, not what Heroic itself
        // recorded: a Wine choice leaves the issue for `carry_over_components`.
        let (mut wine_import, _, _) = reviewed_import(root.path());
        wine_import.runner.kind = HeroicRunnerKind::Proton;
        wine_import.issues.push(HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("autoInstallDxvk".to_owned()),
            message: "Heroic component installation is not imported".to_owned(),
        });
        suppress_covered_settings(&mut wine_import, &wine);
        assert_eq!(wine_import.issues.len(), 1);
    }

    #[test]
    fn carry_over_components_sets_dxvk_and_vkd3d_for_a_wine_runner() {
        let root = tempdir().unwrap();
        let wine = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let (mut imported, _, _) = reviewed_import(root.path());
        for field in ["autoInstallDxvk", "autoInstallVkd3d"] {
            imported.issues.push(HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some(field.to_owned()),
                message: "Heroic component installation is not imported".to_owned(),
            });
        }
        carry_over_components(&mut imported, &wine);
        assert_eq!(imported.dxvk.as_deref(), Some("latest"));
        assert_eq!(imported.vkd3d.as_deref(), Some("latest"));
        assert!(imported.issues.is_empty(), "{:?}", imported.issues);
    }

    #[test]
    fn carry_over_components_leaves_a_proton_choice_alone() {
        let root = tempdir().unwrap();
        let runners_root = root.path().join("runners");
        fake_proton_runner(&runners_root, "GE-Proton9-20");
        let proton = Runners::at(runners_root).resolve("GE-Proton9-20").unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues.push(HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("autoInstallDxvk".to_owned()),
            message: "Heroic component installation is not imported".to_owned(),
        });
        carry_over_components(&mut imported, &proton);
        assert_eq!(imported.dxvk, None);
        assert_eq!(imported.issues.len(), 1);
    }

    #[test]
    fn imported_settings_carries_dxvk_and_vkd3d() {
        let root = tempdir().unwrap();
        let (mut imported, inspection, request) = reviewed_import(root.path());
        imported.dxvk = Some("latest".to_owned());
        imported.vkd3d = Some("latest".to_owned());
        let desktop = DesktopChoice {
            enabled: false,
            name: String::new(),
        };
        let settings = imported_settings(&imported, &inspection, &request, &desktop);
        assert_eq!(settings.dxvk.as_deref(), Some("latest"));
        assert_eq!(settings.vkd3d.as_deref(), Some("latest"));
    }

    #[test]
    fn ensure_components_available_passes_through_when_nothing_is_missing() {
        let root = tempdir().unwrap();
        let wine = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        ensure_components_available(
            &paths(root.path()),
            &GlobalConfig::default(),
            "game",
            &wine,
            &Wanted::default(),
            &mut prompt,
        )
        .unwrap();
    }

    #[test]
    fn ensure_components_available_declining_leaves_the_usual_hinted_error() {
        let root = tempdir().unwrap();
        let wine = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        let error = ensure_components_available(
            &paths(root.path()),
            &GlobalConfig::default(),
            "game",
            &wine,
            &Wanted {
                dxvk: Some("latest".to_owned()),
                vkd3d: None,
            },
            &mut prompt,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("no dxvk builds are installed"),
            "{message}"
        );
        assert!(
            message.contains("Get one with: wineo download dxvk"),
            "{message}"
        );
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        assert!(
            printed.contains("No matching DXVK build is installed. Download the latest release?"),
            "{printed}"
        );
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
            dxvk: None,
            vkd3d: None,
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
                HeroicIssue {
                    kind: HeroicIssueKind::Conflict,
                    field: Some("winePrefix".to_owned()),
                    message: "prefix contains both direct and nested Proton registry markers"
                        .to_owned(),
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
        assert!(!printed.contains("registry markers"), "{printed}");
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
            dxvk: None,
            vkd3d: None,
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
                evidence: &ReviewEvidence::default(),
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

    // -- --all (bulk import) --

    fn candidate(kind: HeroicGameKind, issues: Vec<HeroicIssue>) -> HeroicCandidate {
        HeroicCandidate {
            id: "game".to_owned(),
            title: "Game".to_owned(),
            kind,
            prefix: None,
            runner: None,
            executable: None,
            issues,
        }
    }

    #[test]
    fn candidate_filter_issue_allows_a_clean_windows_candidate() {
        assert!(candidate_filter_issue(&candidate(HeroicGameKind::Windows, Vec::new())).is_none());
    }

    #[test]
    fn candidate_filter_issue_allows_invalid_launcher_args() {
        let candidate = candidate(
            HeroicGameKind::Windows,
            vec![HeroicIssue {
                kind: HeroicIssueKind::Invalid,
                field: Some("launcherArgs".to_owned()),
                message: "bad args".to_owned(),
            }],
        );
        assert!(candidate_filter_issue(&candidate).is_none());
    }

    #[test]
    fn candidate_filter_issue_rejects_non_windows_kinds() {
        let candidate = candidate(
            HeroicGameKind::Native,
            vec![HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("platform".to_owned()),
                message: "native games are not Windows prefix imports".to_owned(),
            }],
        );
        let issue = candidate_filter_issue(&candidate).unwrap();
        assert_eq!(issue.message, "native games are not Windows prefix imports");
    }

    #[test]
    fn candidate_filter_issue_rejects_missing_settings() {
        let candidate = candidate(
            HeroicGameKind::Windows,
            vec![HeroicIssue {
                kind: HeroicIssueKind::Missing,
                field: Some("exe".to_owned()),
                message: "no executable set".to_owned(),
            }],
        );
        let issue = candidate_filter_issue(&candidate).unwrap();
        assert_eq!(issue.message, "no executable set");
    }

    #[test]
    fn candidate_filter_issue_rejects_unsupported_runner() {
        let candidate = candidate(
            HeroicGameKind::Windows,
            vec![HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("runner".to_owned()),
                message: "only manually added sideloaded games are supported".to_owned(),
            }],
        );
        assert!(candidate_filter_issue(&candidate).is_some());
    }

    #[test]
    fn bulk_review_issues_drops_conflicts_and_unsupported_but_keeps_attention_as_a_warning() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues = vec![
            HeroicIssue {
                kind: HeroicIssueKind::Conflict,
                field: Some("useGameMode".to_owned()),
                message: "conflicts with wineo's own setting".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("autoInstallDxvk".to_owned()),
                message: "not imported".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::NeedsAttention,
                field: Some("wrapperOptions".to_owned()),
                message: "review this manually".to_owned(),
            },
        ];
        let warn = bulk_review_issues(&mut imported).unwrap();
        assert_eq!(warn.as_deref(), Some("settings need attention, see -v"));
        // Conflict and unsupported issues are gone; the kept attention item stays.
        assert_eq!(imported.issues.len(), 1);
        assert_eq!(imported.issues[0].kind, HeroicIssueKind::NeedsAttention);
    }

    #[test]
    fn bulk_review_issues_names_eac_and_battleye_specifically() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues = vec![
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("eacRuntime".to_owned()),
                message: "not imported".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("battlEyeRuntime".to_owned()),
                message: "not imported".to_owned(),
            },
        ];
        let warn = bulk_review_issues(&mut imported).unwrap();
        assert_eq!(
            warn.as_deref(),
            Some("EAC/BattlEye runtimes aren't imported")
        );
    }

    #[test]
    fn bulk_review_issues_has_no_warning_when_nothing_needs_attention() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        assert_eq!(bulk_review_issues(&mut imported).unwrap(), None);
    }

    #[test]
    fn bulk_review_issues_rejects_invalid_settings() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues = vec![HeroicIssue {
            kind: HeroicIssueKind::Invalid,
            field: Some("wrapperOptions".to_owned()),
            message: "cannot be parsed".to_owned(),
        }];
        let error = bulk_review_issues(&mut imported).unwrap_err();
        assert_eq!(error, "cannot be parsed");
    }

    #[test]
    fn bulk_review_issues_rejects_invalid_launcher_args() {
        let root = tempdir().unwrap();
        let (mut imported, _, _) = reviewed_import(root.path());
        imported.issues = vec![HeroicIssue {
            kind: HeroicIssueKind::Invalid,
            field: Some("launcherArgs".to_owned()),
            message: "unbalanced quotes".to_owned(),
        }];
        assert!(bulk_review_issues(&mut imported).is_err());
    }

    fn existing(
        name: &str,
        heroic_id: Option<&str>,
        prefix: &Path,
        target_exes: &[&Path],
    ) -> ExistingEnvironment {
        ExistingEnvironment {
            name: name.to_owned(),
            heroic_id: heroic_id.map(str::to_owned),
            snapshot: None,
            prefix: prefix.to_owned(),
            target_exes: target_exes.iter().map(|exe| exe.to_path_buf()).collect(),
        }
    }

    #[test]
    fn match_existing_matches_by_recorded_id_first() {
        let existing = vec![existing(
            "palworld",
            Some("abc"),
            Path::new("/prefixes/palworld"),
            &[],
        )];
        let matched = match_existing(&existing, "abc", None, None, "something-else").unwrap();
        assert_eq!(matched.name, "palworld");
        assert!(!matched.needs_id);
    }

    #[test]
    fn match_existing_does_not_claim_another_id_through_legacy_fallbacks() {
        let prefix = Path::new("/prefixes/shared");
        let exe = Path::new("/games/shared.exe");
        let existing = vec![
            existing("shared", Some("game-a"), prefix, &[exe]),
            existing("legacy", None, prefix, &[exe]),
        ];
        let matched =
            match_existing(&existing, "game-b", Some(prefix), Some(exe), "shared").unwrap();
        assert_eq!(matched.name, "legacy");
        assert!(matched.needs_id);

        let recorded_only = &existing[..1];
        assert!(
            match_existing(recorded_only, "game-b", Some(prefix), Some(exe), "shared").is_none()
        );
        let matched =
            match_existing(recorded_only, "game-a", Some(prefix), Some(exe), "other").unwrap();
        assert_eq!(matched.name, "shared");
        assert!(!matched.needs_id);
    }

    #[test]
    fn backfill_match_rejects_stale_classification_without_changing_the_config() {
        for change in ["config", "id", "removed"] {
            let root = tempdir().unwrap();
            let paths = paths(root.path());
            let path = paths.environment_config("legacy");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let prefix = root.path().join("shared-prefix");
            let original = format!(
                "# keep this\nprefix = {:?}\nrunner = \"wine\"\narch = \"win64\"\n",
                prefix.to_string_lossy()
            );
            fs::write(&path, &original).unwrap();
            let mut existing = existing_environments(&paths).unwrap();
            let matched =
                match_existing(&existing, "candidate", Some(&prefix), None, "other").unwrap();
            assert!(matched.needs_id);

            let changed = match change {
                "config" => original.replace("wine", "other"),
                "id" => format!("{original}heroic_id = \"different\"\n"),
                _ => String::new(),
            };
            if change == "removed" {
                fs::remove_file(&path).unwrap();
            } else {
                fs::write(&path, &changed).unwrap();
            }
            let error = backfill_match(&paths, &mut existing, &matched, "candidate").unwrap_err();
            assert!(
                error.to_string().contains("retry the import")
                    || error.to_string().contains("does not exist"),
                "{error:#}"
            );
            assert_eq!(existing[0].heroic_id, None);
            if change == "removed" {
                assert!(!path.exists());
            } else {
                assert_eq!(fs::read_to_string(&path).unwrap(), changed);
            }
        }
    }

    #[test]
    fn match_existing_falls_back_to_the_adopted_prefix() {
        let existing = vec![existing(
            "zomboid",
            None,
            Path::new("/prefixes/zomboid"),
            &[],
        )];
        let matched = match_existing(
            &existing,
            "different-id",
            Some(Path::new("/prefixes/zomboid")),
            None,
            "not-the-name",
        )
        .unwrap();
        assert_eq!(matched.name, "zomboid");
        assert!(matched.needs_id, "a legacy match should be backfilled");
    }

    #[test]
    fn match_existing_falls_back_to_a_target_running_the_same_exe() {
        let existing = vec![existing(
            "Zomboid",
            None,
            Path::new("/prefixes/other"),
            &[Path::new(
                "/home/me/Games/ProjectZomboid/ProjectZomboid64.exe",
            )],
        )];
        let matched = match_existing(
            &existing,
            "5HYciurSM5YdUGzasx38E7",
            Some(Path::new("/elsewhere")),
            Some(Path::new(
                "/home/me/Games/ProjectZomboid/ProjectZomboid64.exe",
            )),
            "project-zomboid",
        )
        .unwrap();
        assert_eq!(matched.name, "Zomboid");
        assert!(matched.needs_id, "a legacy match should be backfilled");
    }

    #[test]
    fn match_existing_falls_back_to_the_default_name() {
        let existing = vec![existing("hades", None, Path::new("/prefixes/other"), &[])];
        let matched = match_existing(
            &existing,
            "different-id",
            Some(Path::new("/elsewhere")),
            None,
            "hades",
        )
        .unwrap();
        assert_eq!(matched.name, "hades");
        assert!(matched.needs_id);
    }

    #[test]
    fn match_existing_returns_none_when_nothing_matches() {
        let existing = vec![existing("hades", None, Path::new("/prefixes/hades"), &[])];
        assert!(
            match_existing(
                &existing,
                "id",
                Some(Path::new("/elsewhere")),
                Some(Path::new("/elsewhere/Game.exe")),
                "not-hades"
            )
            .is_none()
        );
    }

    #[test]
    fn bulk_desktop_choice_follows_desktop_and_yes_flags() {
        assert_eq!(bulk_desktop_choice(true, true), Some(true));
        assert_eq!(bulk_desktop_choice(true, false), Some(true));
        assert_eq!(bulk_desktop_choice(false, true), Some(false));
        assert_eq!(bulk_desktop_choice(false, false), None);
    }

    #[test]
    fn render_bulk_table_matches_the_plan_example_shape() {
        let rows = vec![
            BulkRow {
                name: "palworld".to_owned(),
                title: "Palworld".to_owned(),
                status: RowStatus::Ready { warn: None },
            },
            BulkRow {
                name: "hades".to_owned(),
                title: "Hades".to_owned(),
                status: RowStatus::Ready {
                    warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
                },
            },
            BulkRow {
                name: "celeste".to_owned(),
                title: "Celeste".to_owned(),
                status: RowStatus::Skipped("Wine runner, needs --runner".to_owned()),
            },
        ];
        let already = vec!["zomboid".to_owned()];
        let text = render_bulk_table("GE-Proton11-1", "copy", &rows, &already);
        let expected = format!(
            "Import 3 Heroic games with GE-Proton11-1:\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\nAlready imported: zomboid",
            "palworld",
            "Palworld",
            "copy",
            "hades",
            "Hades",
            "copy   warn: EAC/BattlEye runtimes aren't imported",
            "celeste",
            "Celeste",
            "skip: Wine runner, needs --runner",
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn render_bulk_table_collapses_a_warning_every_ready_row_shares() {
        let rows = vec![
            BulkRow {
                name: "palworld".to_owned(),
                title: "Palworld".to_owned(),
                status: RowStatus::Ready {
                    warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
                },
            },
            BulkRow {
                name: "hades".to_owned(),
                title: "Hades".to_owned(),
                status: RowStatus::Ready {
                    warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
                },
            },
            BulkRow {
                name: "celeste".to_owned(),
                title: "Celeste".to_owned(),
                status: RowStatus::Skipped("Wine runner, needs --runner".to_owned()),
            },
        ];
        let text = render_bulk_table("GE-Proton11-1", "copy", &rows, &[]);
        let expected = format!(
            "Import 3 Heroic games with GE-Proton11-1:\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\nwarn: EAC/BattlEye runtimes aren't imported",
            "palworld",
            "Palworld",
            "copy",
            "hades",
            "Hades",
            "copy",
            "celeste",
            "Celeste",
            "skip: Wine runner, needs --runner",
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn shared_ready_warning_requires_every_ready_row_to_agree() {
        let same = |name: &str, warn: Option<&str>| BulkRow {
            name: name.to_owned(),
            title: name.to_owned(),
            status: RowStatus::Ready {
                warn: warn.map(str::to_owned),
            },
        };
        assert_eq!(
            shared_ready_warning(&[same("a", Some("x")), same("b", Some("x"))]),
            Some("x")
        );
        assert_eq!(
            shared_ready_warning(&[same("a", Some("x")), same("b", Some("y"))]),
            None
        );
        assert_eq!(
            shared_ready_warning(&[same("a", Some("x")), same("b", None)]),
            None
        );
        assert_eq!(shared_ready_warning(&[same("a", None)]), None);
        assert_eq!(
            shared_ready_warning(&[BulkRow {
                name: "a".to_owned(),
                title: "a".to_owned(),
                status: RowStatus::Skipped("nope".to_owned()),
            }]),
            None
        );
    }

    fn pending_game(root: &Path, component: Component, reason: &str) -> PendingGame {
        let (mut imported, inspection, request) = reviewed_import(root);
        imported.dxvk = Some("latest".to_owned());
        if component == Component::Vkd3d {
            imported.vkd3d = Some("latest".to_owned());
        }
        PendingGame {
            name: "hades".to_owned(),
            title: "Hades".to_owned(),
            warn: None,
            component,
            version: None,
            reason: reason.to_owned(),
            imported,
            evidence: ReviewEvidence::default(),
            inspection,
            request,
        }
    }

    fn install_component_fixture(
        paths: &Paths,
        global: &GlobalConfig,
        component: Component,
        name: &str,
    ) {
        let root = wineo_core::components::Builds::new(paths, global, component)
            .unwrap()
            .root()
            .to_owned();
        let (x64, x32) = component.arch_dirs();
        let marker = if component == Component::Dxvk {
            "dxgi.dll"
        } else {
            "d3d12.dll"
        };
        for arch in [x64, x32] {
            let dir = root.join(name).join(arch);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(marker), b"fixture").unwrap();
        }
    }

    #[test]
    fn guided_accepts_exact_pinned_component_and_resumes() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let wanted = Wanted {
            dxvk: Some("dxvk-2.7".to_owned()),
            vkd3d: None,
        };
        let mut prompt = Prompt::new(Script::new(&["y"]), Vec::new());
        let mut requests = Vec::new();
        ensure_components_available_with(
            &paths,
            &global,
            "game",
            &runner,
            &wanted,
            &mut prompt,
            |component, version| {
                requests.push((component, version.map(str::to_owned)));
                install_component_fixture(&paths, &global, component, version.unwrap());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            requests,
            vec![(Component::Dxvk, Some("dxvk-2.7".to_owned()))]
        );
        let output = String::from_utf8_lossy(prompt.output());
        assert!(output.contains("Download that exact version?"), "{output}");
        assert_eq!(wanted.dxvk.as_deref(), Some("dxvk-2.7"));
    }

    #[test]
    fn guided_accepts_both_components_in_dependency_order() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let wanted = Wanted {
            dxvk: Some("latest".to_owned()),
            vkd3d: Some("latest".to_owned()),
        };
        let mut prompt = Prompt::new(Script::new(&["y", "y"]), Vec::new());
        let mut requests = Vec::new();
        ensure_components_available_with(
            &paths,
            &global,
            "game",
            &runner,
            &wanted,
            &mut prompt,
            |component, version| {
                requests.push((component, version.map(str::to_owned)));
                let name = if component == Component::Dxvk {
                    "dxvk-2.7"
                } else {
                    "vkd3d-proton-2.14"
                };
                install_component_fixture(&paths, &global, component, name);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            requests,
            vec![(Component::Dxvk, None), (Component::Vkd3d, None)]
        );
    }

    #[test]
    fn bulk_accepts_one_component_prompt_for_shared_pin_and_resumes() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut a = pending_game(root.path(), Component::Dxvk, "missing");
        a.imported.dxvk = Some("dxvk-2.7".to_owned());
        a.version = Some("dxvk-2.7".to_owned());
        let mut b = pending_game(root.path(), Component::Dxvk, "missing");
        b.name = "second".to_owned();
        b.imported.dxvk = Some("dxvk-2.7".to_owned());
        b.version = Some("dxvk-2.7".to_owned());
        let mut prompt = Prompt::new(Script::new(&["y"]), Vec::new());
        let mut requests = Vec::new();
        let (rows, ready) = resolve_pending_with(
            &paths,
            &global,
            &runner,
            vec![a, b],
            false,
            Some(&mut prompt),
            |component, version| {
                requests.push((component, version.map(str::to_owned)));
                install_component_fixture(&paths, &global, component, version.unwrap());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            requests,
            vec![(Component::Dxvk, Some("dxvk-2.7".to_owned()))]
        );
        assert_eq!(ready.len(), 2);
        assert!(
            rows.iter()
                .all(|row| matches!(row.status, RowStatus::Ready { .. }))
        );
        assert_eq!(
            String::from_utf8_lossy(prompt.output())
                .matches("Download the exact requested version?")
                .count(),
            1
        );
    }

    #[test]
    fn bulk_downloads_dxvk_then_vkd3d_and_resumes() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut game = pending_game(root.path(), Component::Dxvk, "dxvk missing");
        game.imported.vkd3d = Some("latest".to_owned());
        let pending = vec![game];
        let mut prompt = Prompt::new(Script::new(&["y", "y"]), Vec::new());
        let mut requested = Vec::new();
        let (rows, ready) = resolve_pending_with(
            &paths,
            &global,
            &runner,
            pending,
            false,
            Some(&mut prompt),
            |component, version| {
                requested.push((component, version.map(str::to_owned)));
                let name = if component == Component::Dxvk {
                    "dxvk-2.7"
                } else {
                    "vkd3d-proton-2.14"
                };
                install_component_fixture(&paths, &global, component, name);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            requested,
            vec![(Component::Dxvk, None), (Component::Vkd3d, None)]
        );
        assert_eq!(ready.len(), 1);
        assert!(matches!(rows[0].status, RowStatus::Ready { .. }));
        assert_eq!(
            String::from_utf8_lossy(prompt.output())
                .matches("Download the latest release?")
                .count(),
            2
        );
    }

    #[test]
    fn resolve_pending_with_yes_skips_without_asking() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let pending = vec![pending_game(
            root.path(),
            Component::Dxvk,
            "no DXVK build, run wineo download dxvk",
        )];
        let (rows, ready) = resolve_pending::<Script, Vec<u8>>(
            &paths(root.path()),
            &GlobalConfig::default(),
            &runner,
            pending,
            true,
            None,
        )
        .unwrap();
        assert!(ready.is_empty());
        assert_eq!(rows.len(), 1);
        let RowStatus::Skipped(reason) = &rows[0].status else {
            panic!("expected a skip row");
        };
        assert!(reason.contains("wineo download dxvk"), "{reason}");
    }

    #[test]
    fn resolve_pending_interactive_decline_keeps_the_original_reason() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let pending = vec![pending_game(
            root.path(),
            Component::Dxvk,
            "no dxvk builds are installed",
        )];
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        let (rows, ready) = resolve_pending(
            &paths(root.path()),
            &GlobalConfig::default(),
            &runner,
            pending,
            false,
            Some(&mut prompt),
        )
        .unwrap();
        assert!(ready.is_empty());
        assert_eq!(rows.len(), 1);
        assert!(
            matches!(&rows[0].status, RowStatus::Skipped(reason) if reason == "no dxvk builds are installed")
        );
    }

    #[test]
    fn resolve_pending_asks_once_per_component_across_every_game_that_needs_it() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let pending = vec![
            pending_game(root.path(), Component::Dxvk, "dxvk missing"),
            pending_game(root.path(), Component::Dxvk, "dxvk missing"),
        ];
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        let (rows, _) = resolve_pending(
            &paths(root.path()),
            &GlobalConfig::default(),
            &runner,
            pending,
            false,
            Some(&mut prompt),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        // One question covering both games, not one each.
        assert_eq!(printed.matches("Download the latest release?").count(), 1);
        assert!(printed.contains("for 2 games"), "{printed}");
    }
}
