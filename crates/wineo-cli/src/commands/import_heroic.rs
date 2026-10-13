use anyhow::bail;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use wineo_core::components::Component;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::download::Provider;
use wineo_core::gamescope::GamescopeSetting;
use wineo_core::heroic::{
    HeroicGameKind, HeroicImport, HeroicIssue, HeroicIssueKind, HeroicRunnerKind, HeroicSource,
    SettingsReview,
};
use wineo_core::heroic_import::{
    self, COVERED_BY_PROTON, DesktopEntry, Entry, Game, GamePlan, InputKind, LibraryOptions,
    LibraryPlan, MissingBuild, Options, Playtime, Policy, Registered, RunnerCopy, RunnerSuggestion,
    SkipReason, Warning, apply, blocker, inspect_copyable_runner, is_dropped_conflict, listing,
    plan_game, plan_library, redact_env_value, runner_dependency_lines, shared_prefix_lines,
    suggest_runner, write_import_log,
};
use wineo_core::import::{self, ExternalDependency};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::steamgriddb::Key;

use super::art::{Access, AutoFetch, KeyCheck, Pick};
use super::components;
use super::desktop;
use super::download;
use super::external_runners;
use super::import::{ReturnToSourceMenu, choose_runner, finish_guided};
use super::transfer::Cancellation;
use crate::picker;
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};
use crate::style;

mod dry_run;
mod native;

#[derive(clap::Args, Default)]
pub struct Args {
    /// Read Heroic's config from this directory
    #[arg(long, value_hint = clap::ValueHint::DirPath)]
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
    /// Copy destination, defaults to the configured prefix root, or DIR/prefixes with --portable
    #[arg(long, value_hint = clap::ValueHint::DirPath)]
    pub dir: Option<String>,
    /// Resolve a missing registry architecture marker
    #[arg(long)]
    pub arch: Option<Arch>,
    /// Create a desktop entry, named after the Heroic title
    #[arg(short, long)]
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
    /// Fetch art from SteamGridDB, searching the Heroic title
    #[arg(long)]
    pub art: bool,
    /// Review the import without changing anything
    #[arg(long, conflicts_with_all = ["interactive", "list"])]
    pub dry_run: bool,
}

impl Args {
    /// Whether a flag answers part of the guided import, which marks a
    /// scripted run that never gets the one-time runner question. Choosing
    /// the config folder, listing, and forcing the review don't answer one.
    fn answers_questions(&self) -> bool {
        self.game.is_some()
            || self.name.is_some()
            || self.runner.is_some()
            || self.adopt
            || self.dir.is_some()
            || self.arch.is_some()
            || self.desktop
            || self.art
            || self.yes
    }
}

struct OutputOnly;

impl Input for OutputOnly {
    fn line(&mut self, _: &str) -> io::Result<crate::prompt::Line> {
        unreachable!("Heroic review should not prompt in noninteractive mode")
    }
}

/// Whether to fetch SteamGridDB art once the import is registered, and the
/// search for it. Kept apart from `HeroicImport` like `DesktopChoice`.
struct ArtChoice {
    /// Art can only be turned on when a key was found.
    key: Option<Key>,
    enabled: bool,
    term: String,
}

impl ArtChoice {
    fn off() -> Self {
        Self {
            key: None,
            enabled: false,
            term: String::new(),
        }
    }

    /// On when the check found a key.
    fn checked(check: KeyCheck, term: &str) -> Self {
        let key = match check {
            KeyCheck::Found(key) => Some(key),
            KeyCheck::Missing | KeyCheck::Failed(_) => None,
        };
        Self {
            enabled: key.is_some(),
            key,
            term: term.to_owned(),
        }
    }

    fn display(&self) -> String {
        if self.enabled {
            format!("fetch from SteamGridDB, searching \"{}\"", self.term)
        } else {
            "none".to_owned()
        }
    }

    fn log_line(&self) -> String {
        format!("  art: {}\n", self.display())
    }
}

/// Checks for a key once, before the review, saying why art is off without one.
fn review_art<I: Input, W: Write>(
    global: &GlobalConfig,
    access: &Access,
    prompt: &mut Prompt<I, W>,
    title: &str,
) -> io::Result<ArtChoice> {
    if global.art.fetch.kinds().is_empty() {
        return Ok(ArtChoice::off());
    }
    let check = (access.check_key)();
    if let Some(line) = check.skip_line() {
        prompt.say(line)?;
    }
    Ok(ArtChoice::checked(check, title))
}

/// Fetches art for a registered import. Callers run it before the desktop
/// reconcile, so the entry has the art icon from the start. Only a canceled
/// pick list is an error.
fn fetch_art<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    env: &str,
    art: &ArtChoice,
    pick: Pick<I, W>,
    err: &mut dyn Write,
) -> Result<(), Canceled> {
    let Some(key) = art.key.as_ref().filter(|_| art.enabled) else {
        return Ok(());
    };
    let request = AutoFetch {
        env,
        term: &art.term,
        kinds: global.art.fetch,
    };
    access.fetch(paths, key, &request, pick, err)
}

pub fn run(
    paths: &Paths,
    args: Args,
    verbose: bool,
    first_question: bool,
) -> anyhow::Result<ExitCode> {
    let access = Access::steamgriddb(paths);
    run_with_access(paths, args, verbose, &access, first_question)
}

fn run_with_access(
    paths: &Paths,
    args: Args,
    verbose: bool,
    access: &Access,
    first_question: bool,
) -> anyhow::Result<ExitCode> {
    let return_to_source_menu = !first_question;
    if args.desktop {
        wineo_core::desktop::supported(paths)?;
    }
    let global = GlobalConfig::load(paths)?;
    if args.list {
        let sources = sources(paths, args.config_dir.as_deref())?;
        for (index, source) in sources.iter().enumerate() {
            if index > 0 {
                println!();
            }
            list(paths, &global, source)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if args.all {
        return match run_all(paths, &global, access, args, verbose) {
            Ok(code) => Ok(code),
            Err(error) if error.is::<Canceled>() => {
                eprintln!("Import canceled");
                Ok(ExitCode::from(130))
            }
            Err(error) => Err(error),
        };
    }
    if !args.interactive
        && let (Some(game), Some(_)) = (&args.game, &args.name)
    {
        let source = source(paths, args.config_dir.as_deref())?;
        let is_native = source
            .list_candidates()?
            .candidates
            .iter()
            .any(|candidate| candidate.id == *game && candidate.kind == HeroicGameKind::Native);
        if is_native {
            return native::direct(paths, &global, access, &source, &args);
        }
    }
    let needs_guided =
        args.interactive || args.game.is_none() || args.name.is_none() || args.runner.is_none();
    if !needs_guided {
        let source = source(paths, args.config_dir.as_deref())?;
        let game_id = args.game.as_deref().unwrap();
        let name = args.name.clone().unwrap();
        let runner_name = args.runner.clone().unwrap();
        let mut plan = plan_game(
            paths,
            &global,
            &source,
            game_id,
            &runner_name,
            plan_options(paths, &args, Some(name))?,
        )?;
        plan.check_builds(paths, &global)?;
        plan.set_desktop_enabled(args.desktop);
        let art = if !args.dry_run && args.art && !global.art.fetch.kinds().is_empty() {
            let check = (access.check_key)();
            check.print_skip();
            ArtChoice::checked(check, &plan.resolved().title)
        } else {
            ArtChoice::off()
        };
        let mut output = Prompt::new(OutputOnly, crate::output::stdout());
        show_game_summary(&mut output, paths, plan.resolved(), verbose)?;
        let ctx = ReviewContext {
            paths,
            global: &global,
            source: &source,
            verbose,
        };
        output.say(final_facts_text(&ctx, &plan, verbose))?;
        plan.resolve(Policy::Strict);
        reject_unresolved(&plan)?;
        if args.dry_run {
            super::import::check_review(paths, &global, plan.request(), plan.inspection())?;
            if args.art {
                access.review(
                    &plan.heroic().title,
                    global.art.fetch,
                    &mut crate::output::stdout(),
                    &mut crate::output::stderr(),
                );
            }
            println!("Dry run, nothing was imported.");
            return Ok(ExitCode::SUCCESS);
        }
        let log_body = verbose.then(|| plan.log_text(&global, &source) + &art.log_line());
        let registered = match apply_plan(paths, &global, &source, &plan) {
            Ok(registered) => registered,
            Err(error) if error.is::<Canceled>() => {
                eprintln!("Import canceled");
                return Ok(ExitCode::from(130));
            }
            Err(error) => return Err(error),
        };
        report_playtime(&registered.playtime);
        let environment = &registered.environment;
        let mut prompt = (art.enabled && prompt::interactive())
            .then(|| Terminal::open().ok())
            .flatten()
            .map(|input| Prompt::new(input, crate::output::stdout()));
        let mut stdout = crate::output::stdout();
        let pick = match &mut prompt {
            Some(prompt) => Pick::Ask(prompt),
            None => Pick::Top(&mut stdout),
        };
        let fetched = fetch_art(
            paths,
            &global,
            access,
            &environment.name,
            &art,
            pick,
            &mut crate::output::stderr(),
        );
        drop(prompt);
        finish_registered(paths, &global, &registered)?;
        if fetched.is_err() {
            let name = &environment.name;
            eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
            return Ok(ExitCode::from(130));
        }
        println!(
            "Imported {}. Add or edit targets with wineo setup {}",
            environment.name, environment.name
        );
        if let Some(body) = log_body {
            print_log_path(paths, &environment.name, &body);
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !args.dry_run
        && !prompt::interactive()
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
        !args.dry_run && prompt::interactive(),
        "guided Heroic import needs a terminal; use --list or specify --game, -n, and --runner"
    );
    let mut prompt = Prompt::new(Terminal::open()?, crate::output::stdout())
        .with_color(style::stdout_enabled())
        .with_first_question(first_question);
    let result = source_guided(
        paths,
        args.config_dir.as_deref(),
        &mut prompt,
        return_to_source_menu,
    )
    .and_then(|source| guided(paths, &global, access, source, args, &mut prompt, verbose));
    match result {
        Ok(code) => Ok(code),
        Err(error) if error.is::<Canceled>() => {
            eprintln!("Import canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

fn sources(paths: &Paths, explicit: Option<&str>) -> anyhow::Result<Vec<HeroicSource>> {
    let sources = heroic_import::sources(paths, explicit)?;
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

struct GuidedSource {
    source: HeroicSource,
    return_to_source_menu: bool,
}

fn source_guided<I: Input, W: Write>(
    paths: &Paths,
    explicit: Option<&str>,
    prompt: &mut Prompt<I, W>,
    return_to_source_menu: bool,
) -> anyhow::Result<GuidedSource> {
    let sources = sources(paths, explicit)?;
    if sources.len() == 1 {
        return Ok(GuidedSource {
            source: sources.into_iter().next().unwrap(),
            return_to_source_menu,
        });
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
        Reply::Value(source) => Ok(GuidedSource {
            source,
            return_to_source_menu,
        }),
        Reply::Back if return_to_source_menu => Err(ReturnToSourceMenu.into()),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn list(paths: &Paths, global: &GlobalConfig, source: &HeroicSource) -> anyhow::Result<ExitCode> {
    let listing = listing(paths, global, source)?;
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
        if candidate.kind == HeroicGameKind::Native {
            native::list_lines(&candidate);
            continue;
        }
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
    access: &Access,
    source: GuidedSource,
    args: Args,
    prompt: &mut Prompt<I, W>,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    let GuidedSource {
        source,
        return_to_source_menu,
    } = source;
    let listing = listing(paths, global, &source)?;
    let candidates: Vec<_> = listing
        .candidates
        .iter()
        .filter(|candidate| blocker(candidate).is_none())
        .cloned()
        .collect();
    anyhow::ensure!(
        !candidates.is_empty(),
        "Heroic has no importable sideloads in {}",
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
            return if return_to_source_menu {
                Err(ReturnToSourceMenu.into())
            } else {
                Err(Canceled.into())
            };
        };
        candidates
            .into_iter()
            .find(|candidate| candidate.id == game_id)
            .unwrap()
    };
    if game.kind == HeroicGameKind::Native {
        return native::guided(paths, global, access, &source, &game, &args, prompt);
    }
    let imported = source.inspect(&game.id)?;
    let shared = source.shared_prefix_candidates(&game.id)?;
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
        Reply::Back => return Err(Canceled.into()),
    };

    // Asking here, before the suggestion, lets Heroic's own build be used in
    // place.
    let reloaded;
    let global = if !args.answers_questions() && external_runners::due(paths, global) {
        match external_runners::offer(prompt, paths)? {
            Reply::Value(_) => {
                reloaded = GlobalConfig::load(paths)?;
                &reloaded
            }
            Reply::Back => return Err(Canceled.into()),
        }
    } else {
        global
    };

    let chosen = pick_runner(
        paths,
        global,
        &imported,
        args.runner.as_deref(),
        prompt,
        verbose,
    )?;
    let mut plan = GamePlan::new(
        paths,
        global,
        imported,
        shared,
        &chosen.name,
        plan_options(paths, &args, Some(name))?,
    )?;
    plan.set_runner_dependencies(chosen.dependencies);
    ensure_components_available(paths, global, &plan, prompt)?;
    review_settings(prompt, &mut plan)?;
    if !args.adopt && args.dir.is_none() {
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
            Reply::Value(true) => plan.set_adopt(),
            Reply::Value(false) => {}
            Reply::Back => return Err(Canceled.into()),
        }
    }
    plan.set_desktop_enabled(paths.portable_root().is_none());
    let mut art = review_art(global, access, prompt, &plan.resolved().title)?;
    let ctx = ReviewContext {
        paths,
        global,
        source: &source,
        verbose,
    };
    final_review(&ctx, &mut plan, &mut art, prompt)?;
    let log_body = verbose.then(|| plan.log_text(global, &source) + &art.log_line());
    let registered = apply_plan(paths, global, &source, &plan)?;
    report_playtime(&registered.playtime);
    let environment = &registered.environment;
    let fetched = fetch_art(
        paths,
        global,
        access,
        &environment.name,
        &art,
        Pick::Ask(prompt),
        &mut crate::output::stderr(),
    );
    finish_registered(paths, global, &registered)?;
    if fetched.is_err() {
        prompt.say(format!(
            "Kept {} and its prefix. Continue with `wineo setup {}`",
            environment.name, environment.name
        ))?;
        return Err(Canceled.into());
    }
    if let Some(body) = log_body {
        print_log_path(paths, &environment.name, &body);
    }
    finish_guided(paths, global, environment, prompt)
}

/// Applies `plan`, stopping cleanly when a signal asks it to.
fn apply_plan(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    plan: &GamePlan,
) -> anyhow::Result<Registered> {
    let cancellation = Cancellation::new()?;
    apply_plan_with_cancellation(paths, global, source, plan, &cancellation)
}

fn apply_plan_with_cancellation(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    plan: &GamePlan,
    cancellation: &Cancellation,
) -> anyhow::Result<Registered> {
    let result = apply(
        paths,
        global,
        source,
        plan,
        &cancellation.token,
        super::transfer::copy_progress(),
    );
    // A completed registration is retained even if cancellation arrives at the end.
    match result {
        Ok(registered) => Ok(registered),
        Err(error) => cancellation.result(Err(error)),
    }
}

/// Says what went wrong with carrying over Heroic's playtime, if anything.
pub(super) fn report_playtime(playtime: &Playtime) {
    match playtime {
        Playtime::None | Playtime::Imported => {}
        Playtime::Unreadable(reason) => {
            eprintln!("warning: could not read Heroic's playtime: {reason}");
        }
        Playtime::NotRecorded(reason) => {
            eprintln!("warning: could not import playtime from Heroic: {reason}");
        }
    }
}

/// Writes the desktop entries for a registered import, and says what changed.
pub(super) fn finish_registered(
    paths: &Paths,
    global: &GlobalConfig,
    registered: &Registered,
) -> anyhow::Result<()> {
    for change in registered.finish(paths, global)? {
        eprintln!(
            "{}",
            desktop::describe(&change, &registered.environment.name)
        );
    }
    Ok(())
}

fn plan_options(paths: &Paths, args: &Args, name: Option<String>) -> anyhow::Result<Options> {
    Ok(Options {
        name,
        adopt: args.adopt,
        destination: if args.adopt {
            None
        } else {
            args.dir
                .as_deref()
                .map(|dir| paths.absolute(dir))
                .transpose()?
        },
        arch: args.arch,
    })
}

// -- Bulk import (`--all`) --

/// A candidate as `--all` displays it before importing: how it's named, and
/// what happens to it.
struct BulkRow {
    name: String,
    title: String,
    status: RowStatus,
    launch_scripts: bool,
}

enum RowStatus {
    Ready {
        warn: Option<String>,
    },
    /// A native game, which has no prefix to copy or adopt.
    ReadyNative {
        warn: Option<String>,
    },
    Skipped(String),
}

fn run_all(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    args: Args,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        args.dry_run || args.yes || prompt::interactive(),
        "bulk Heroic import needs a terminal; pass --yes to skip the confirmation"
    );
    let source = source(paths, args.config_dir.as_deref())?;
    let runners = Runners::new(paths, global)?;
    let mut prompt = if args.yes || args.dry_run {
        None
    } else {
        Some(
            Prompt::new(Terminal::open()?, crate::output::stdout())
                .with_color(style::stdout_enabled())
                .with_first_question(true),
        )
    };

    let mut listing = listing(paths, global, &source)?;
    // Native games need no runner, so a library of only those never asks for one.
    let has_windows = listing
        .candidates
        .iter()
        .any(|candidate| candidate.kind != HeroicGameKind::Native);
    let candidate_count = listing.candidates.len();
    // Printed after the table, where it explains why fewer games are listed
    // than were checked.
    let mut hidden_note = None;
    let runner = if !has_windows && args.runner.is_none() {
        None
    } else {
        match &args.runner {
            Some(name) => {
                runners.resolve(name)?;
                Some(name.clone())
            }
            None => match runners.newest_proton()? {
                Some(runner) => Some(runner.selection().to_owned()),
                None if args.dry_run && !args.yes => {
                    println!("Would offer to download the latest GE-Proton.");
                    let count = listing
                        .candidates
                        .iter()
                        .filter(|candidate| candidate.kind != HeroicGameKind::Native)
                        .count();
                    let (noun, need, target) = if count == 1 {
                        ("game", "it needs", "it")
                    } else {
                        ("games", "they need", "them")
                    };
                    hidden_note = Some(format!(
                        "{count} Windows {noun} not shown: {need} a Proton runner, which wineo would offer to download. Pass --runner to preview {target}."
                    ));
                    listing
                        .candidates
                        .retain(|candidate| candidate.kind == HeroicGameKind::Native);
                    None
                }
                None => {
                    anyhow::ensure!(
                        !args.yes,
                        "no Proton runner is installed; pass --runner or download one first"
                    );
                    let prompt = prompt.as_mut().expect("interactive was checked above");
                    Some(download_latest_ge_proton(paths, global, &runners, prompt)?)
                }
            },
        }
    };

    println!("Checking {candidate_count} Heroic games...");
    let mut library = plan_library(
        paths,
        global,
        &listing,
        runner.as_deref(),
        &LibraryOptions {
            adopt: args.adopt,
            arch: args.arch,
        },
    )?;
    if !args.dry_run {
        library.record_backfills(paths)?;
        resolve_pending(paths, global, &mut library, args.yes, prompt.as_mut())?;
    } else if args.yes {
        library.settle(paths, global);
    }
    let runner_name = library.runner().unwrap_or_default();

    let mode_label = if args.adopt { "adopt" } else { "copy" };
    println!(
        "{}",
        render_bulk_table(
            runner_name,
            mode_label,
            &bulk_rows(&library),
            library.already()
        )
    );
    if let Some(note) = hidden_note {
        println!("{note}");
    }
    if args.dry_run {
        let failed = dry_run::library(paths, global, access, &library, args.art, args.yes)?;
        println!("Dry run, nothing was imported.");
        return Ok(if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    if library.ready_count() == 0 {
        println!("No Heroic games are ready to import.");
        return Ok(ExitCode::SUCCESS);
    }
    let portable = paths.portable_root().is_some();
    let desktop_enabled = match bulk_desktop_choice(args.desktop, args.yes || portable) {
        Some(value) => value,
        None => {
            let prompt = prompt.as_mut().expect("interactive was checked above");
            match prompt.confirm("Create desktop entries for these games?", true)? {
                Reply::Value(value) => value,
                Reply::Back => return Err(Canceled.into()),
            }
        }
    };

    let art = if global.art.fetch.kinds().is_empty() {
        None
    } else {
        bulk_art(access, args.art, args.yes, prompt.as_mut())?
    };

    if !args.yes {
        let prompt = prompt.as_mut().expect("interactive was checked above");
        let count = library.ready_count();
        match prompt.confirm(&format!("Import {count} games?"), true)? {
            Reply::Value(true) => {}
            _ => return Err(Canceled.into()),
        }
    }

    let bulk = Bulk {
        paths,
        global,
        access,
        source: &source,
        desktop: desktop_enabled,
        art,
        verbose,
    };
    import_all(&bulk, library.into_ready())
}

/// The key `--all` fetches art with, or None for no art. `--art` answers yes
/// ahead of time and `--yes` without it answers no, as with desktop entries.
/// A missing key is only noted when art was asked for or would be asked about.
fn bulk_art<I: Input, W: Write>(
    access: &Access,
    art_flag: bool,
    yes_flag: bool,
    mut prompt: Option<&mut Prompt<I, W>>,
) -> anyhow::Result<Option<Key>> {
    if yes_flag && !art_flag {
        return Ok(None);
    }
    let key = match (access.check_key)() {
        KeyCheck::Found(key) => key,
        check => {
            match (prompt.as_mut(), check.skip_line()) {
                (Some(prompt), Some(line)) => prompt.say(line)?,
                _ => check.print_skip(),
            }
            return Ok(None);
        }
    };
    if art_flag {
        return Ok(Some(key));
    }
    let prompt = prompt.expect("interactive was checked above");
    match prompt.confirm("Fetch art for these games?", true)? {
        Reply::Value(wanted) => Ok(wanted.then_some(key)),
        Reply::Back => Err(Canceled.into()),
    }
}

fn download_latest_ge_proton<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    runners: &Runners,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<String> {
    match prompt.confirm(
        "No Proton runner is installed. Download the latest GE-Proton?",
        true,
    )? {
        Reply::Value(true) => {}
        _ => return Err(Canceled.into()),
    }
    let name = download::install_latest(paths, global, Provider::GeProton)?;
    runners.resolve(&name)?;
    Ok(name)
}

fn resolve_pending<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    library: &mut LibraryPlan,
    yes: bool,
    prompt: Option<&mut Prompt<I, W>>,
) -> anyhow::Result<()> {
    resolve_pending_with(paths, global, library, yes, prompt, |component, version| {
        components::download_build(paths, global, component, version)
    })
}

fn resolve_pending_with<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    library: &mut LibraryPlan,
    yes: bool,
    mut prompt: Option<&mut Prompt<I, W>>,
    mut download: impl FnMut(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut offered = Vec::new();

    while !yes && let Some(prompt) = prompt.as_mut() {
        let Some(offer) = library
            .pending_builds()
            .into_iter()
            .find(|offer| !offered.contains(&offer.component))
        else {
            break;
        };
        let (component, count, unique) = (offer.component, offer.games, offer.versions);
        offered.push(component);
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
        library.recheck(paths, global);
    }

    library.settle(paths, global);
    Ok(())
}

/// The bulk table's rows, in the order the plan lists its games.
fn bulk_rows(library: &LibraryPlan) -> Vec<BulkRow> {
    library
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Ready(Game::Windows(plan)) => BulkRow {
                name: plan.name().to_owned(),
                title: plan.heroic().title.clone(),
                status: RowStatus::Ready {
                    warn: warnings_line(&plan.warnings(), plan.is_adopt()),
                },
                launch_scripts: has_hooks(&plan.heroic().settings_review),
            },
            Entry::Ready(Game::Native(plan)) => BulkRow {
                name: plan.name().to_owned(),
                title: plan.heroic().title.clone(),
                status: RowStatus::ReadyNative {
                    warn: warnings_line(&plan.warnings(), false),
                },
                launch_scripts: has_hooks(&plan.heroic().settings_review),
            },
            Entry::Pending(game) => BulkRow {
                name: game.plan().name().to_owned(),
                title: game.plan().heroic().title.clone(),
                status: RowStatus::Skipped(game.reason().to_owned()),
                launch_scripts: false,
            },
            Entry::Skipped(skipped) => BulkRow {
                name: skipped.name.clone(),
                title: skipped.title.clone(),
                status: RowStatus::Skipped(skip_text(&skipped.reason)),
                launch_scripts: false,
            },
        })
        .collect()
}

/// The text a warning has in the bulk table and the final review. Core only
/// says what the warning is, this is the wording.
fn warning_text(warning: &Warning, adopt: bool) -> String {
    let plural = |count: &usize| if *count == 1 { "" } else { "s" };
    match warning {
        Warning::SharedPrefix(shared) => shared_prefix_lines(shared, adopt, false).join(" "),
        Warning::UmuDisabled(message) | Warning::NvapiNotInstalled(message) => message.clone(),
        Warning::AntiCheatNotImported => "EAC/BattlEye runtimes aren't imported".to_owned(),
        Warning::Omitted(count) => {
            format!("{count} unsupported setting{} omitted", plural(count))
        }
        Warning::Dropped(count) => {
            format!("{count} managed conflict{} dropped", plural(count))
        }
        Warning::NeedsAttention(_) => "settings need attention, see -v".to_owned(),
    }
}

/// Every warning on one line, or None when there is nothing to warn about.
fn warnings_line(warnings: &[Warning], adopt: bool) -> Option<String> {
    let texts: Vec<String> = warnings
        .iter()
        .map(|warning| warning_text(warning, adopt))
        .collect();
    (!texts.is_empty()).then(|| texts.join("; "))
}

fn skip_text(reason: &SkipReason) -> String {
    match reason {
        SkipReason::LauncherArgs => "invalid launcher arguments".to_owned(),
        SkipReason::NeedsRunner { proton: true } => "Proton runner, needs --runner".to_owned(),
        SkipReason::NeedsRunner { proton: false } => "Wine runner, needs --runner".to_owned(),
        SkipReason::NameTaken(name) => format!("name `{name}` is already taken"),
        SkipReason::Blocked(message)
        | SkipReason::Unreadable(message)
        | SkipReason::Unavailable(message) => message.clone(),
    }
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
        RowStatus::ReadyNative { .. } | RowStatus::Skipped(_) => None,
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
    let mode_width = rows
        .iter()
        .filter_map(|row| match row.status {
            RowStatus::ReadyNative { .. } => Some("native".len()),
            RowStatus::Ready { .. } => Some(mode_label.len()),
            RowStatus::Skipped(_) => None,
        })
        .max()
        .unwrap_or(0);
    let noun = if rows.len() == 1 { "game" } else { "games" };
    let heading = if runner_name.is_empty() {
        format!("Import {} Heroic {noun}:", rows.len())
    } else {
        format!("Import {} Heroic {noun} with {runner_name}:", rows.len())
    };
    let mut out = format!("{}\n", style::title(heading));
    for row in rows {
        let (mode, warn) = match &row.status {
            RowStatus::Skipped(reason) => (format!("skip: {reason}"), None),
            RowStatus::ReadyNative { warn } => ("native".to_owned(), warn.as_deref()),
            RowStatus::Ready { warn } => (
                mode_label.to_owned(),
                warn.as_deref().filter(|_| shared_warning.is_none()),
            ),
        };
        let mut parts = Vec::new();
        if row.launch_scripts && !matches!(row.status, RowStatus::Skipped(_)) {
            parts.push("runs launch scripts".to_owned());
        }
        if let Some(warning) = warn {
            parts.push(style::warning(format!("warn: {warning}")).to_string());
        }
        // Copy and native rows share the table, so the mode is padded for the
        // notes after it to line up.
        let status = if parts.is_empty() {
            mode
        } else {
            format!("{mode:mode_width$}   {}", parts.join("   "))
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

/// What every game in `--all` is imported with.
struct Bulk<'a> {
    paths: &'a Paths,
    global: &'a GlobalConfig,
    access: &'a Access<'a>,
    source: &'a HeroicSource,
    desktop: bool,
    /// The key to fetch art with, or None for no art.
    art: Option<Key>,
    verbose: bool,
}

fn import_all(bulk: &Bulk, games: Vec<Game>) -> anyhow::Result<ExitCode> {
    let paths = bulk.paths;
    let verbose = bulk.verbose;
    let total = games.len();
    let mut done = 0usize;
    let mut failed = Vec::new();
    let cancellation = Cancellation::new()?;
    let mut canceled = false;
    for game in games {
        if cancellation.token.is_canceled() {
            canceled = true;
            break;
        }
        let name = game.name().to_owned();
        // Art lines wait for the progress line to end.
        let (mut art_out, mut art_err) = (Vec::new(), Vec::new());
        let imported = match &game {
            Game::Windows(plan) => {
                if verbose {
                    let mut output = Prompt::new(OutputOnly, crate::output::stdout());
                    show_game_summary(&mut output, paths, plan.resolved(), verbose)?;
                }
                import_progress(&mut crate::output::stdout().lock(), &name, || {
                    import_one(bulk, plan, &cancellation, &mut art_out, &mut art_err)
                })?
            }
            Game::Native(plan) => {
                import_progress(&mut crate::output::stdout().lock(), &name, || {
                    native::import_one(bulk, plan, &mut art_out, &mut art_err).map(|()| None)
                })?
            }
        };
        crate::output::stdout().write_all(&art_out)?;
        crate::output::stderr().write_all(&art_err)?;
        match imported {
            Ok(log_body) => {
                if let Some(body) = log_body {
                    print_log_path(paths, &name, &body);
                }
                done += 1;
            }
            Err(error) if error.is::<Canceled>() => {
                canceled = true;
                break;
            }
            Err(_) => failed.push(name),
        }
    }
    let noun = if total == 1 { "game" } else { "games" };
    if failed.is_empty() {
        println!("Imported {done} of {total} {noun}.");
    } else {
        println!(
            "Imported {done} of {total} {noun}. {} failed: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    if canceled {
        let remaining = total - done - failed.len();
        let noun = if remaining == 1 { "game" } else { "games" };
        println!("{remaining} {noun} canceled.");
        return Ok(ExitCode::from(130));
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
        Err(error) if error.is::<Canceled>() => writeln!(output, " canceled")?,
        Err(error) => writeln!(output, " failed: {error:#}")?,
    }
    output.flush()?;
    Ok(result)
}

/// Imports one game of `--all`. Art is fetched with the top match, and its
/// lines and warnings go to `out` and `err`. Art failures never fail the game.
fn import_one(
    bulk: &Bulk,
    game: &GamePlan,
    cancellation: &Cancellation,
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
) -> anyhow::Result<Option<String>> {
    let (paths, global) = (bulk.paths, bulk.global);
    let mut plan = game.clone();
    plan.set_desktop_enabled(bulk.desktop);
    let art = ArtChoice {
        key: bulk.art.clone(),
        enabled: bulk.art.is_some(),
        term: plan.heroic().title.clone(),
    };
    let log_body = bulk
        .verbose
        .then(|| plan.log_text(global, bulk.source) + &art.log_line());
    let registered = apply_plan_with_cancellation(paths, global, bulk.source, &plan, cancellation)?;
    report_playtime(&registered.playtime);
    let environment = &registered.environment;
    let top = Pick::<OutputOnly, Vec<u8>>::Top(out);
    // A top match can't be canceled.
    let _ = fetch_art(
        paths,
        global,
        bulk.access,
        &environment.name,
        &art,
        top,
        err,
    );
    finish_registered(paths, global, &registered)?;
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
    Art,
}

/// Bundles what every review function needs but doesn't itself change, to
/// stay under clippy's argument-count limit.
struct ReviewContext<'a> {
    paths: &'a Paths,
    global: &'a GlobalConfig,
    source: &'a HeroicSource,
    verbose: bool,
}

fn final_review<I: Input, W: Write>(
    ctx: &ReviewContext,
    plan: &mut GamePlan,
    art: &mut ArtChoice,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let paths = ctx.paths;
    let global = ctx.global;
    let verbose = ctx.verbose;
    loop {
        let facts = final_facts_text(ctx, plan, verbose);
        let heading = format!("{facts}\n");

        let imported = plan.resolved();
        let cwd_display = match &imported.cwd {
            Some(cwd) => style::path(paths, cwd),
            None => "default".to_owned(),
        };
        let copy_to_display = if plan.is_adopt() {
            "adopt in place".to_owned()
        } else {
            style::path(paths, plan.destination())
        };
        let desktop_display = if plan.desktop().enabled { "yes" } else { "no" }.to_owned();
        let mut items: Vec<(&str, String, EditField)> = vec![
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
            ("name", plan.name().to_owned(), EditField::Name),
            ("copy to", copy_to_display, EditField::Destination),
            ("desktop", desktop_display, EditField::Desktop),
        ];
        let mut note = "Close Heroic and the game first.".to_owned();
        if art.key.is_some() {
            items.push(("art", art.display(), EditField::Art));
        }
        let width = items
            .iter()
            .map(|(label, _, _)| label.len())
            .max()
            .unwrap_or(0)
            + 2;
        if art.key.is_none() {
            // Unnumbered without a key, lined up with the numbered rows.
            let indent = " ".repeat(items.len().to_string().len() + 4);
            note = format!("{indent}{:width$}{}\n{note}", "art", art.display());
        }
        let choices = numbered(
            items
                .into_iter()
                .map(|(label, value, field)| (format!("{label:width$}{value}"), field)),
        );
        let choice = prompt.review(&heading, Some(&note), "import", &choices, true)?;
        match choice {
            Reply::Value(None) => return Ok(()),
            Reply::Value(Some(field)) => match field {
                EditField::Executable => edit_executable(paths, plan, prompt)?,
                EditField::WorkingDirectory => edit_working_directory(paths, plan, prompt)?,
                EditField::Arguments => edit_arguments(plan, prompt)?,
                EditField::Environment => edit_environment(plan, prompt)?,
                EditField::GameMode => edit_gamemode(plan, prompt)?,
                EditField::Name => edit_name(paths, global, plan, prompt)?,
                EditField::Destination => edit_destination(paths, global, plan, prompt)?,
                EditField::Desktop => {
                    let title = plan.resolved().title.clone();
                    let edited = edit_desktop(paths, &title, plan.desktop().clone(), prompt)?;
                    plan.set_desktop_enabled(edited.enabled);
                    if edited.enabled {
                        plan.set_desktop_name(&edited.name)?;
                    }
                }
                EditField::Art => edit_art(art, prompt)?,
            },
            Reply::Back => return Err(Canceled.into()),
        }
    }
}

fn edit_executable<I: Input, W: Write>(
    paths: &Paths,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.resolved().exe.display().to_string();
    match prompt.text("Executable path", Some(&current), |value| {
        if value.trim().is_empty() {
            Err("Executable path cannot be empty".to_owned())
        } else {
            Ok(value.to_owned())
        }
    })? {
        Reply::Value(value) => {
            let exe = plan.resolve_path(paths, &picker::clean_path(&value))?;
            plan.set_executable(exe)?;
        }
        Reply::Back => {}
    }
    Ok(())
}

fn edit_working_directory<I: Input, W: Write>(
    paths: &Paths,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan
        .resolved()
        .cwd
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    match prompt.text(
        "Working directory (empty for default)",
        Some(&current),
        |value| Ok(value.to_owned()),
    )? {
        Reply::Value(value) if value.is_empty() => plan.set_working_directory(None),
        Reply::Value(value) => {
            let cwd = plan.resolve_path(paths, &picker::clean_path(&value))?;
            plan.set_working_directory(Some(cwd));
        }
        Reply::Back => {}
    }
    Ok(())
}

fn edit_arguments<I: Input, W: Write>(
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.resolved().launcher_args.clone();
    match prompt.text("Launch arguments", Some(&current), |value| {
        wineo_core::heroic::parse_launcher_args(value).map(|_| value.to_owned())
    })? {
        Reply::Value(value) => plan.set_launcher_args(&value)?,
        Reply::Back => {}
    }
    Ok(())
}

fn edit_environment<I: Input, W: Write>(
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    prompt.say("Enter environment values as KEY=VALUE, one per line. An empty line keeps the current values.")?;
    let Reply::Value(values) = prompt.list("environment")? else {
        return Ok(());
    };
    if values.is_empty() {
        return Ok(());
    }
    plan.set_environment(values)?;
    Ok(())
}

fn edit_gamemode<I: Input, W: Write>(
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.resolved().gamemode.unwrap_or(false);
    match prompt.confirm("Enable GameMode?", current)? {
        Reply::Value(value) => plan.set_gamemode(value),
        Reply::Back => {}
    }
    Ok(())
}

fn edit_name<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.name().to_owned();
    match prompt.text("Environment name", Some(&current), |value| {
        picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err)
    })? {
        Reply::Value(value) => plan.set_name(paths, global, &value)?,
        Reply::Back => {}
    }
    Ok(())
}

fn edit_destination<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let adopt = match prompt.choose(
        "Prefix import mode",
        "Mode",
        &numbered([
            ("Copy into wineo".to_owned(), false),
            ("Adopt the original files in place".to_owned(), true),
        ]),
        Some(if plan.is_adopt() { "2" } else { "1" }),
        true,
    )? {
        Reply::Value(value) => value,
        Reply::Back => return Ok(()),
    };
    if adopt {
        plan.set_adopt();
        return Ok(());
    }
    plan.set_copy(paths, global, None)?;
    let default = plan.destination().display().to_string();
    match prompt.text("Copy destination", Some(&default), |value| {
        paths
            .absolute(&picker::clean_path(value))
            .map_err(|error| error.to_string())
    })? {
        Reply::Value(value) => plan.set_copy(paths, global, Some(value))?,
        Reply::Back => {}
    }
    Ok(())
}

fn edit_desktop<I: Input, W: Write>(
    paths: &Paths,
    title: &str,
    mut desktop: DesktopEntry,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<DesktopEntry> {
    if let Err(err) = wineo_core::desktop::supported(paths) {
        desktop.enabled = false;
        prompt.say(format!("{err}."))?;
        return Ok(desktop);
    }
    match prompt.confirm("Create a desktop entry?", desktop.enabled)? {
        Reply::Value(true) => {
            let current = if desktop.name.is_empty() {
                title.to_owned()
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
    Ok(desktop)
}

fn edit_art<I: Input, W: Write>(
    art: &mut ArtChoice,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    match prompt.confirm("Fetch art from SteamGridDB?", art.enabled)? {
        Reply::Value(true) => {
            match prompt.text("Search for", Some(&art.term), |value| {
                if value.is_empty() {
                    Err("Enter a search term.".to_owned())
                } else {
                    Ok(value.to_owned())
                }
            })? {
                Reply::Value(term) => {
                    art.enabled = true;
                    art.term = term;
                }
                Reply::Back => {}
            }
        }
        Reply::Value(false) => art.enabled = false,
        Reply::Back => {}
    }
    Ok(())
}

/// The runner picked for a game, and what copying Heroic's runner left linked
/// outside wineo.
pub(super) struct ChosenRunner {
    pub(super) name: String,
    pub(super) dependencies: Vec<ExternalDependency>,
}

impl ChosenRunner {
    pub(super) fn named(name: String) -> Self {
        Self {
            name,
            dependencies: Vec::new(),
        }
    }
}

/// Asks about the game's runner, or takes the `selected` one when it's there.
pub(super) fn pick_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    requested: Option<&str>,
    prompt: &mut Prompt<I, W>,
    verbose: bool,
) -> anyhow::Result<ChosenRunner> {
    match suggest_runner(paths, global, imported, requested)? {
        RunnerSuggestion::Use(name) => return Ok(ChosenRunner::named(name)),
        RunnerSuggestion::Copy(copy) => {
            let (heading, label, other) = match requested {
                None => (
                    "Heroic runner is not installed",
                    format!(
                        "Copy Heroic runner {} from {}",
                        copy.name,
                        copy.source.display()
                    ),
                    "Choose another installed or downloaded runner",
                ),
                Some(_) => (
                    "Selected runner is not installed",
                    format!("Copy Heroic runner from {}", copy.source.display()),
                    "Choose another runner",
                ),
            };
            let choices = numbered([(label, true), (other.to_owned(), false)]);
            match prompt.choose(heading, "Runner", &choices, Some("1"), true)? {
                Reply::Value(true) => {
                    return copy_heroic_runner(paths, global, imported, &copy, prompt);
                }
                Reply::Value(false) => {}
                Reply::Back => return Err(anyhow::anyhow!(Canceled)),
            }
        }
        RunnerSuggestion::Choose { copy_blocked, .. } => {
            if let Some(reason) = copy_blocked {
                prompt.say(format!(
                    "Heroic runner cannot be copied: {reason}. Choose a compatible host runner."
                ))?;
            }
        }
    }
    let proton = Some(imported.runner.kind == HeroicRunnerKind::Proton);
    match choose_runner(prompt, paths, global, requested, proton, verbose)? {
        Reply::Value(value) => Ok(ChosenRunner::named(value)),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn copy_heroic_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    copy: &RunnerCopy,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ChosenRunner> {
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
    let _source_guard = copy.lock_source(paths, global)?;
    let receipt = cancellation.published_result(import::copy_runner_reviewed(
        &copy.inspection,
        root,
        &name,
        &cancellation.token,
        |_| {},
    ))?;
    eprintln!(
        "Copied Heroic runner {} to {}",
        receipt.name,
        receipt.path.display()
    );
    eprintln!("This runner copy is retained if the prefix import is canceled.");
    for line in runner_dependency_lines(&receipt.external_dependencies, false) {
        eprintln!("Runner dependency: {line}");
    }
    Ok(ChosenRunner {
        name: receipt.name,
        dependencies: receipt.external_dependencies,
    })
}

/// Offers to download a missing DXVK/vkd3d-proton build for guided Heroic
/// import, the same way guided `new` does, one component at a time until
/// everything resolves or a person declines. Declining leaves the usual
/// error, so this changes nothing about what ends up refused.
pub(super) fn ensure_components_available<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    ensure_components_available_with(paths, global, plan, prompt, |component, version| {
        components::download_build(paths, global, component, version)
    })
}

fn ensure_components_available_with<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &GamePlan,
    prompt: &mut Prompt<I, W>,
    mut download: impl FnMut(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    loop {
        let Some(missing) = plan.missing_build(paths, global)? else {
            return Ok(());
        };
        let MissingBuild {
            component,
            ref version,
            ..
        } = missing;
        let question = match version.as_deref() {
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
            version.as_deref(),
            &question,
            &mut download,
        )? {
            return Err(missing.into_error().into());
        }
    }
}

fn reject_unresolved(plan: &GamePlan) -> anyhow::Result<()> {
    let blocking: Vec<_> = plan.blocking().collect();
    if !blocking.is_empty() {
        for finding in blocking {
            print_issue("unresolved", &finding.issue);
        }
        bail!("Heroic launch settings need review; rerun with --interactive");
    }
    Ok(())
}

fn review_settings<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    plan: &mut GamePlan,
) -> anyhow::Result<()> {
    // The summary already showed the dropped conflicts, the omitted settings,
    // and the ones needing attention as warnings, and the final review edits
    // the paths that need it. Only what can't be imported blocks, or needs an
    // answer.
    plan.resolve(Policy::Defaults);
    if plan
        .blocking()
        .any(|finding| finding.issue.kind == HeroicIssueKind::Invalid)
    {
        bail!("Heroic has invalid launch settings that cannot be imported automatically");
    }
    if plan.needs_input() == Some(InputKind::LauncherArgs) {
        let current = plan.resolved().launcher_args.clone();
        let args = match prompt.text("Correct launcher arguments", Some(&current), |value| {
            wineo_core::heroic::parse_launcher_args(value).map(|_| value.to_owned())
        })? {
            Reply::Value(value) => value,
            Reply::Back => return Err(anyhow::anyhow!(Canceled)),
        };
        plan.set_launcher_args(&args)?;
    }
    if plan.blocking().next().is_some() {
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

fn hook_blocks(review: &SettingsReview) -> Vec<Block> {
    ["before hook", "after hook"]
        .into_iter()
        .filter_map(|label| {
            review
                .carried
                .iter()
                .find(|item| item.field == label)
                .map(|item| Block {
                    label,
                    lines: vec![display_control_escaped(&item.detail)],
                })
        })
        .collect()
}

fn has_hooks(review: &SettingsReview) -> bool {
    ["before hook", "after hook"]
        .into_iter()
        .any(|field| review.carried.iter().any(|item| item.field == field))
}

fn display_control_escaped(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            escaped.extend(character.escape_debug());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn gamescope_display(setting: &GamescopeSetting) -> String {
    match setting {
        GamescopeSetting::Off => "off".to_owned(),
        GamescopeSetting::On(game) => {
            let args = game.args(false);
            if args.is_empty() {
                "on".to_owned()
            } else {
                format!("on: {}", display_args(&args))
            }
        }
    }
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
    if imported.enable_wine_wayland || imported.wayland == Some(false) {
        blocks.push(Block {
            label: "wayland",
            lines: vec![
                if imported.wayland == Some(false) {
                    "off"
                } else if imported.wayland == Some(true) {
                    "on"
                } else {
                    "requested"
                }
                .to_owned(),
            ],
        });
    }
    if let Some(mangohud) = imported.mangohud {
        blocks.push(Block {
            label: "mangohud",
            lines: vec![on_off(mangohud).to_owned()],
        });
    }
    if let Some(gamescope) = &imported.gamescope {
        blocks.push(Block {
            label: "gamescope",
            lines: vec![gamescope_display(gamescope)],
        });
    }
    blocks.extend(hook_blocks(&imported.settings_review));
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
                .map(|(key, value)| format!("{key}={}", redact_env_value(key, value)))
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
            HeroicIssueKind::Conflict if is_dropped_conflict(issue) => {
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
                && !is_dropped_conflict(issue)
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
fn final_facts_text(ctx: &ReviewContext, plan: &GamePlan, verbose: bool) -> String {
    let paths = ctx.paths;
    let global = ctx.global;
    let source = ctx.source;
    let imported = plan.resolved();
    let inspection = plan.inspection();
    let request = plan.request();
    let resolved = plan.runner();
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

    let runner_label = external_runners::with_owner(
        paths,
        resolved,
        if request.runner == resolved.name() {
            resolved.name().to_owned()
        } else {
            format!("{} -> {}", request.runner, resolved.name())
        },
    );
    blocks.push(Block {
        label: "runner",
        lines: vec![runner_label],
    });
    if imported.enable_wine_wayland || imported.wayland == Some(false) {
        blocks.push(Block {
            label: "wayland",
            lines: vec![
                if imported.wayland == Some(false) {
                    "off"
                } else if imported.wayland == Some(true) {
                    "on"
                } else {
                    "not imported"
                }
                .to_owned(),
            ],
        });
    }
    if let Some(mangohud) = imported.mangohud {
        blocks.push(Block {
            label: "mangohud",
            lines: vec![on_off(mangohud).to_owned()],
        });
    }
    if let Some(gamescope) = &imported.gamescope {
        blocks.push(Block {
            label: "gamescope",
            lines: vec![gamescope_display(gamescope)],
        });
    }
    blocks.extend(hook_blocks(&imported.settings_review));
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

    let shared = shared_prefix_lines(plan.shared_prefix(), request.adopt, verbose);
    if !shared.is_empty() {
        blocks.push(Block {
            label: "shared prefix",
            lines: shared,
        });
    }
    let runner_deps = runner_dependency_lines(plan.runner_dependencies(), verbose);
    if !runner_deps.is_empty() {
        blocks.push(Block {
            label: "runner links",
            lines: runner_deps,
        });
    }
    let review_warnings: Vec<Warning> = plan
        .warnings()
        .into_iter()
        .filter(|warning| !matches!(warning, Warning::SharedPrefix(_)))
        .collect();
    if let Some(warn) = warnings_line(&review_warnings, request.adopt) {
        blocks.push(Block {
            label: "warning",
            lines: vec![style::warning(warn)],
        });
    }
    if verbose {
        let detail: Vec<String> = imported
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

fn global_block(global: &GlobalConfig, verbose: bool) -> Block {
    if verbose {
        let mut lines = Vec::new();
        if let Some(enabled) = global.gamemode {
            lines.push(format!("gamemode {}", on_off(enabled)));
        }
        for (key, value) in &global.env {
            lines.push(format!("{key}={}", redact_env_value(key, value)));
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

fn print_log_path(paths: &Paths, name: &str, body: &str) {
    match write_import_log(paths, name, body) {
        Ok(path) => println!("Import log: {}", style::path(paths, &path)),
        Err(error) => eprintln!("warning: could not write the import log: {error:#}"),
    }
}

/// Test helpers shared with `native`'s tests.
#[cfg(test)]
mod test_access {
    use super::{Access, KeyCheck};
    use crate::commands::art::fake::Fake;
    use wineo_core::steamgriddb::{Key, KeySource};

    fn key_check(found: bool) -> KeyCheck {
        if found {
            KeyCheck::Found(Key {
                value: "k".into(),
                source: KeySource::Variable,
            })
        } else {
            KeyCheck::Missing
        }
    }

    /// An access whose key check answers `found` and counts its calls, and
    /// whose SteamGridDB is `fake`.
    pub(super) fn access<'a>(
        found: bool,
        lookups: &'a std::cell::Cell<usize>,
        fake: Fake,
    ) -> Access<'a> {
        Access {
            check_key: Box::new(move || {
                lookups.set(lookups.get() + 1);
                key_check(found)
            }),
            transport: Box::new(move || Ok(Box::new(fake.clone()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_access::access;
    use crate::test_script::write_script;
    use std::collections::VecDeque;
    use std::fs;

    use std::path::PathBuf;

    use tempfile::tempdir;
    use wineo_core::heroic::{HeroicRunner, InputFingerprint};
    use wineo_core::heroic_import::{PendingGame, Resolution};
    use wineo_core::import;
    use wineo_core::runner::Runner;

    use super::*;
    use crate::commands::art::fake::Fake;
    use crate::prompt::Script;

    fn paths(home: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
    }

    fn reviewed_import(root: &Path) -> HeroicImport {
        fs::create_dir_all(root.join("drive_c")).unwrap();
        fs::write(
            root.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        HeroicImport {
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
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            enable_wine_wayland: false,
            enable_hdr: false,
            dxvk: None,
            vkd3d: None,
            issues: Vec::new(),
            external_dependencies: Vec::new(),
            fingerprint: InputFingerprint {
                digest: "heroic".to_owned(),
                files: Vec::new(),
            },
            settings_review: SettingsReview::default(),
        }
    }

    /// A global config whose runners are the ones a test writes in `root/runners`.
    fn global_with_runners(root: &Path) -> GlobalConfig {
        GlobalConfig {
            defaults: wineo_core::config::Defaults {
                runner_root: Some(root.join("runners").display().to_string()),
                ..wineo_core::config::Defaults::default()
            },
            ..GlobalConfig::default()
        }
    }

    /// Plans `imported` against a fake wine runner, named `game` and copied to
    /// `root/copy`.
    fn plan_for(root: &Path, imported: HeroicImport) -> GamePlan {
        let runners = root.join("runners");
        if !runners.join("wine-tkg").exists() {
            fake_wine_runner(&runners, "wine-tkg");
        }
        GamePlan::new(
            &paths(root),
            &global_with_runners(root),
            imported,
            Vec::new(),
            "wine-tkg",
            Options {
                name: Some("game".to_owned()),
                destination: Some(root.join("copy")),
                ..Options::default()
            },
        )
        .unwrap()
    }

    fn heroic_source(root: &Path) -> HeroicSource {
        HeroicSource::from_config_dir_with_home(root.join("heroic"), root.to_owned())
    }

    fn fake_proton_runner(root: &Path, name: &str) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("files/bin")).unwrap();
        let proton = dir.join("proton");
        write_script(&proton, "#!/bin/sh\n");
        for executable in ["wine", "wineserver"] {
            let path = dir.join("files/bin").join(executable);
            write_script(&path, "#!/bin/sh\n");
        }
        fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
    }

    fn fake_wine_runner(root: &Path, name: &str) -> Runner {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("bin")).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = dir.join("bin").join(executable);
            write_script(&path, "#!/bin/sh\n");
        }
        Runners::at(root.to_owned()).resolve(name).unwrap()
    }

    #[test]
    fn final_review_edits_executable_before_import() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = global_with_runners(root.path());
        let source = heroic_source(root.path());
        let mut plan = plan_for(root.path(), reviewed_import(root.path()));
        plan.set_desktop_enabled(true);
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
                verbose: false,
            },
            &mut plan,
            &mut ArtChoice::off(),
            &mut prompt,
        )
        .unwrap();
        assert_eq!(plan.resolved().exe, root.path().join("drive_c/edited.exe"));
    }

    #[test]
    fn edited_paths_expand_home_and_preserve_prefix_relative_paths() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let prefix = root.path().join("prefix");
        let mut plan = plan_for(&prefix, reviewed_import(&prefix));
        let mut prompt = Prompt::new(Script::new(&["\"~/Games/My Game/game.exe\""]), Vec::new());
        edit_executable(&paths, &mut plan, &mut prompt).unwrap();
        assert_eq!(
            plan.resolved().exe,
            root.path().join("Games/My Game/game.exe")
        );
        let mut prompt = Prompt::new(Script::new(&["~/Games/My\\ Game"]), Vec::new());
        edit_working_directory(&paths, &mut plan, &mut prompt).unwrap();
        assert_eq!(plan.resolved().cwd, Some(root.path().join("Games/My Game")));

        let mut prompt = Prompt::new(
            Script::new(&["drive_c/Program\\ Files/game.exe"]),
            Vec::new(),
        );
        edit_executable(&paths, &mut plan, &mut prompt).unwrap();
        assert_eq!(
            plan.resolved().exe,
            prefix.join("drive_c/Program Files/game.exe")
        );

        let absolute = root.path().join("External Game/game.exe");
        let edited = plan
            .resolve_path(
                &paths,
                &picker::clean_path(&format!("'{}'", absolute.display())),
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
        let mut imported = reviewed_import(root.path());
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
        imported
            .settings_review
            .unsupported
            .push(wineo_core::heroic::ReviewItem {
                field: "wrapperOptions".to_owned(),
                detail: "ignored".to_owned(),
            });
        let mut plan = plan_for(root.path(), imported);
        plan.resolve(Policy::Defaults);
        assert_eq!(plan.skip_reason(), None);
        let warning = warnings_line(&plan.warnings(), false).unwrap();
        assert!(warning.contains("unsupported setting omitted"), "{warning}");
        assert!(warning.contains("managed conflict dropped"), "{warning}");
        assert!(warning.contains("Wine NVAPI requires"), "{warning}");

        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary(&mut prompt, &paths, plan.resolved(), true).unwrap();
        let output = String::from_utf8_lossy(prompt.output());
        assert!(output.contains("wrapperOptions"), "{output}");
        assert!(output.contains("WINEPREFIX"), "{output}");
        assert!(output.contains("\"one word\""), "{output}");
        assert!(output.contains("say\\\"hello"), "{output}");
        let log = plan.log_text(&global, &source);
        assert!(log.contains("wrapperOptions"), "{log}");
        assert!(log.contains("WINEPREFIX"), "{log}");
        assert!(!log.contains("\u{1b}["), "{log}");
    }

    #[test]
    fn resolved_layout_conflicts_do_not_reappear_as_current_errors() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let mut imported = reviewed_import(root.path());
        imported.issues = ["wineVersion", "winePrefix", "WINEPREFIX"]
            .into_iter()
            .map(|field| HeroicIssue {
                kind: HeroicIssueKind::Conflict,
                field: Some(field.to_owned()),
                message: format!("{field} conflicts"),
            })
            .collect();
        let plan = plan_for(root.path(), imported);
        assert_eq!(
            warnings_line(&plan.warnings(), false).as_deref(),
            Some("1 managed conflict dropped")
        );
        let mut output = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary(&mut output, &paths, plan.resolved(), true).unwrap();
        let text = String::from_utf8_lossy(output.output());
        assert!(!text.contains("wineVersion conflicts"), "{text}");
        assert!(!text.contains("winePrefix conflicts"), "{text}");
        assert!(text.contains("WINEPREFIX is dropped"), "{text}");
    }

    #[test]
    fn import_progress_flushes_before_blocked_copy_and_completes_same_line() {
        use std::sync::atomic::{AtomicUsize, Ordering};
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
                let cancelled = wineo_core::cancel::Token::new();
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
        let global = global_with_runners(root.path());
        let source = heroic_source(root.path());
        let mut plan = plan_for(root.path(), reviewed_import(root.path()));
        plan.set_desktop_enabled(true);
        let original = plan.resolved().exe.clone();
        let mut prompt = Prompt::new(Script::new(&["q"]), Vec::new());
        let error = final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
                verbose: false,
            },
            &mut plan,
            &mut ArtChoice::off(),
            &mut prompt,
        )
        .unwrap_err();
        assert!(error.is::<Canceled>());
        assert_eq!(plan.resolved().exe, original);
    }

    #[test]
    fn final_review_bare_enter_imports_without_editing() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = global_with_runners(root.path());
        let source = heroic_source(root.path());
        let mut plan = plan_for(root.path(), reviewed_import(root.path()));
        plan.set_desktop_enabled(true);
        let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
                verbose: false,
            },
            &mut plan,
            &mut ArtChoice::off(),
            &mut prompt,
        )
        .unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        assert!(printed.contains("Game -> game"), "{printed}");
        assert!(printed.contains("  8) desktop      yes"), "{printed}");
        assert!(
            printed.contains(
                "Close Heroic and the game first.\n\nPress Enter to import, or pick a number. Esc to go back, Ctrl+C to cancel.\nChoice: "
            ),
            "{printed}"
        );
    }

    #[test]
    fn final_review_toggles_the_desktop_entry() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = global_with_runners(root.path());
        let source = heroic_source(root.path());
        let mut plan = plan_for(root.path(), reviewed_import(root.path()));
        plan.set_desktop_enabled(true);
        let mut prompt = Prompt::new(Script::new(&["8", "n", ""]), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
                verbose: false,
            },
            &mut plan,
            &mut ArtChoice::off(),
            &mut prompt,
        )
        .unwrap();
        assert!(!plan.desktop().enabled);
    }

    #[test]
    fn game_summary_merges_eac_and_battleye_into_one_warning() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let mut imported = reviewed_import(root.path());
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
        let mut imported = reviewed_import(root.path());
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

        // Reviewing settles them without asking anything, since the script
        // has no answers to give.
        let mut plan = plan_for(root.path(), imported);
        review_settings(&mut prompt, &mut plan).unwrap();
        assert_eq!(plan.blocking().count(), 0);
        assert_eq!(plan.needs_input(), None);
        let settled: Vec<_> = plan
            .findings()
            .iter()
            .map(|finding| finding.resolution.clone())
            .collect();
        assert_eq!(
            settled,
            [
                Resolution::Kept,
                Resolution::Kept,
                Resolution::Dropped,
                Resolution::Omitted
            ]
        );
    }

    #[test]
    fn ensure_components_available_passes_through_when_nothing_is_missing() {
        let root = tempdir().unwrap();
        let plan = plan_for(root.path(), reviewed_import(root.path()));
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        ensure_components_available(
            &paths(root.path()),
            &global_with_runners(root.path()),
            &plan,
            &mut prompt,
        )
        .unwrap();
    }

    #[test]
    fn ensure_components_available_declining_leaves_the_usual_hinted_error() {
        let root = tempdir().unwrap();
        let mut imported = reviewed_import(root.path());
        imported.dxvk = Some("latest".to_owned());
        let plan = plan_for(root.path(), imported);
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        let error = ensure_components_available(
            &paths(root.path()),
            &global_with_runners(root.path()),
            &plan,
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
        let mut imported = HeroicImport {
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
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            enable_wine_wayland: false,
            enable_hdr: false,
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
        imported.settings_review.carried.extend([
            wineo_core::heroic::ReviewItem {
                field: "before hook".to_owned(),
                detail: "/Heroic scripts/before\nreview: 'game'.sh".to_owned(),
            },
            wineo_core::heroic::ReviewItem {
                field: "after hook".to_owned(),
                detail: "/Heroic scripts/after.sh".to_owned(),
            },
        ]);
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        show_game_summary(&mut prompt, &paths, &imported, false).unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        println!("{printed}");
        assert!(printed.contains("Palworld (Heroic kZXLzzKAFWBP4dpRatpPHu)"));
        assert!(printed.contains("runner       Proton-CachyOS-latest (Heroic's copy)"));
        assert!(printed.contains("gamemode     on"));
        assert!(printed.contains("before hook  /Heroic scripts/before\\nreview: 'game'.sh"));
        assert!(printed.contains("after hook   /Heroic scripts/after.sh"));
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
        // Fonts that link into Heroic's own copy of a runner.
        let runner_dir = config_dir.join("tools/proton/GE-Proton11-1");
        fs::create_dir_all(compatdata.join("pfx/drive_c/windows")).unwrap();
        for index in 0..24 {
            std::os::unix::fs::symlink(
                runner_dir
                    .join("files/share/fonts")
                    .join(format!("font{index}.ttf")),
                compatdata
                    .join("pfx/drive_c/windows")
                    .join(format!("font{index}.ttf")),
            )
            .unwrap();
        }
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
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            enable_wine_wayland: false,
            enable_hdr: false,
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
        let mut plan = GamePlan::new(
            &paths,
            &global,
            imported,
            Vec::new(),
            runner_name,
            Options {
                name: Some("palworld".to_owned()),
                destination: Some(root.join(".local/share/wineo/prefixes/palworld")),
                ..Options::default()
            },
        )
        .unwrap();
        plan.set_desktop_enabled(true);
        plan.set_desktop_name("palworld").unwrap();
        let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
                verbose: false,
            },
            &mut plan,
            &mut ArtChoice::off(),
            &mut prompt,
        )
        .unwrap();
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        println!("{printed}");
        assert!(printed.contains("Palworld -> palworld"));
        assert!(printed.contains("links     24 prefix files point into Heroic's GE-Proton11-1"));
        assert!(printed.contains("8) desktop      yes"));
    }

    // -- --all (bulk import) --

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
                launch_scripts: false,
            },
            BulkRow {
                name: "hades".to_owned(),
                title: "Hades".to_owned(),
                status: RowStatus::Ready {
                    warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
                },
                launch_scripts: false,
            },
            BulkRow {
                name: "celeste".to_owned(),
                title: "Celeste".to_owned(),
                status: RowStatus::Skipped("Wine runner, needs --runner".to_owned()),
                launch_scripts: false,
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
    fn render_bulk_table_marks_windows_and_native_games_with_scripts() {
        let rows = vec![
            BulkRow {
                name: "palworld".to_owned(),
                title: "Palworld".to_owned(),
                status: RowStatus::Ready { warn: None },
                launch_scripts: true,
            },
            BulkRow {
                name: "native".to_owned(),
                title: "Native Game".to_owned(),
                status: RowStatus::ReadyNative { warn: None },
                launch_scripts: true,
            },
        ];
        let text = render_bulk_table("", "copy", &rows, &[]);
        assert!(
            text.lines()
                .any(|line| line.ends_with("copy     runs launch scripts")),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line.ends_with("native   runs launch scripts")),
            "{text}"
        );
    }

    #[test]
    fn control_characters_are_escaped_only_for_hook_display() {
        assert_eq!(
            display_control_escaped("/Heroic scripts/a\nreview\r\u{1b}.sh"),
            "/Heroic scripts/a\\nreview\\r\\u{1b}.sh"
        );
        assert_eq!(
            display_control_escaped("/Heroic scripts/a 'quoted' $dollar; ok.sh"),
            "/Heroic scripts/a 'quoted' $dollar; ok.sh"
        );
    }

    #[test]
    fn render_bulk_table_pluralizes_with_and_without_a_runner() {
        for count in [0, 1, 2] {
            for runner in ["", "test-wine"] {
                let rows: Vec<_> = (0..count)
                    .map(|index| BulkRow {
                        name: format!("game{index}"),
                        title: format!("Game {index}"),
                        status: RowStatus::ReadyNative { warn: None },
                        launch_scripts: false,
                    })
                    .collect();
                let noun = if count == 1 { "game" } else { "games" };
                let suffix = if runner.is_empty() {
                    String::new()
                } else {
                    format!(" with {runner}")
                };
                let text = render_bulk_table(runner, "copy", &rows, &[]);
                assert_eq!(
                    text.lines().next().unwrap(),
                    format!("Import {count} Heroic {noun}{suffix}:")
                );
            }
        }
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
                launch_scripts: false,
            },
            BulkRow {
                name: "hades".to_owned(),
                title: "Hades".to_owned(),
                status: RowStatus::Ready {
                    warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
                },
                launch_scripts: false,
            },
            BulkRow {
                name: "celeste".to_owned(),
                title: "Celeste".to_owned(),
                status: RowStatus::Skipped("Wine runner, needs --runner".to_owned()),
                launch_scripts: false,
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
            launch_scripts: false,
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
                launch_scripts: false,
            }]),
            None
        );
    }

    pub(super) fn pending_game_wanting(
        root: &Path,
        component: Component,
        reason: &str,
        dxvk: Option<&str>,
        vkd3d: Option<&str>,
        version: Option<&str>,
    ) -> Entry {
        let mut imported = reviewed_import(root);
        imported.dxvk = dxvk.map(str::to_owned);
        imported.vkd3d = vkd3d.map(str::to_owned);
        Entry::Pending(PendingGame::new(
            plan_for(root, imported),
            component,
            version.map(str::to_owned),
            reason.to_owned(),
        ))
    }

    /// A plan whose only games are waiting on builds.
    fn waiting(games: Vec<Entry>) -> LibraryPlan {
        LibraryPlan::from_entries(Some("wine-tkg".to_owned()), Vec::new(), games)
    }

    fn pending_game(root: &Path, component: Component, reason: &str) -> Entry {
        pending_game_wanting(
            root,
            component,
            reason,
            Some("latest"),
            (component == Component::Vkd3d).then_some("latest"),
            None,
        )
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
        let mut imported = reviewed_import(root.path());
        imported.dxvk = Some("dxvk-2.7".to_owned());
        let plan = plan_for(root.path(), imported);
        let mut prompt = Prompt::new(Script::new(&["y"]), Vec::new());
        let mut requests = Vec::new();
        ensure_components_available_with(
            &paths,
            &global,
            &plan,
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
        assert_eq!(plan.wanted().dxvk.as_deref(), Some("dxvk-2.7"));
    }

    #[test]
    fn guided_accepts_both_components_in_dependency_order() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let mut imported = reviewed_import(root.path());
        imported.dxvk = Some("latest".to_owned());
        imported.vkd3d = Some("latest".to_owned());
        let plan = plan_for(root.path(), imported);
        let mut prompt = Prompt::new(Script::new(&["y", "y"]), Vec::new());
        let mut requests = Vec::new();
        ensure_components_available_with(
            &paths,
            &global,
            &plan,
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
        let pinned = || {
            pending_game_wanting(
                root.path(),
                Component::Dxvk,
                "missing",
                Some("dxvk-2.7"),
                None,
                Some("dxvk-2.7"),
            )
        };
        let mut library = waiting(vec![pinned(), pinned()]);
        let mut prompt = Prompt::new(Script::new(&["y"]), Vec::new());
        let mut requests = Vec::new();
        resolve_pending_with(
            &paths,
            &global,
            &mut library,
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
        let rows = bulk_rows(&library);
        assert_eq!(library.into_ready().len(), 2);
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
        let game = pending_game_wanting(
            root.path(),
            Component::Dxvk,
            "dxvk missing",
            Some("latest"),
            Some("latest"),
            None,
        );
        let mut library = waiting(vec![game]);
        let mut prompt = Prompt::new(Script::new(&["y", "y"]), Vec::new());
        let mut requested = Vec::new();
        resolve_pending_with(
            &paths,
            &global,
            &mut library,
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
        let rows = bulk_rows(&library);
        assert_eq!(library.into_ready().len(), 1);
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
        let mut library = waiting(vec![pending_game(
            root.path(),
            Component::Dxvk,
            "no DXVK build, run wineo download dxvk",
        )]);
        resolve_pending::<Script, Vec<u8>>(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut library,
            true,
            None,
        )
        .unwrap();
        let rows = bulk_rows(&library);
        assert!(library.into_ready().is_empty());
        assert_eq!(rows.len(), 1);
        let RowStatus::Skipped(reason) = &rows[0].status else {
            panic!("expected a skip row");
        };
        assert!(reason.contains("wineo download dxvk"), "{reason}");
    }

    #[test]
    fn resolve_pending_interactive_decline_keeps_the_original_reason() {
        let root = tempdir().unwrap();
        let mut library = waiting(vec![pending_game(
            root.path(),
            Component::Dxvk,
            "no dxvk builds are installed",
        )]);
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        resolve_pending(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut library,
            false,
            Some(&mut prompt),
        )
        .unwrap();
        let rows = bulk_rows(&library);
        assert!(library.into_ready().is_empty());
        assert_eq!(rows.len(), 1);
        assert!(
            matches!(&rows[0].status, RowStatus::Skipped(reason) if reason == "no dxvk builds are installed")
        );
    }

    #[test]
    fn resolve_pending_asks_once_per_component_across_every_game_that_needs_it() {
        let root = tempdir().unwrap();
        let mut library = waiting(vec![
            pending_game(root.path(), Component::Dxvk, "dxvk missing"),
            pending_game(root.path(), Component::Dxvk, "dxvk missing"),
        ]);
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        resolve_pending(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut library,
            false,
            Some(&mut prompt),
        )
        .unwrap();
        assert_eq!(bulk_rows(&library).len(), 2);
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        // One question covering both games, not one each.
        assert_eq!(printed.matches("Download the latest release?").count(), 1);
        assert!(printed.contains("for 2 games"), "{printed}");
    }

    // -- art --

    /// Runs the final review on `answers` with `art`, returning the transcript.
    fn review_with_art(root: &Path, art: &mut ArtChoice, answers: &[&str]) -> String {
        let paths = paths(root);
        let global = global_with_runners(root);
        let source = heroic_source(root);
        let mut plan = plan_for(root, reviewed_import(root));
        plan.set_desktop_enabled(true);
        let mut prompt = Prompt::new(Script::new(answers), Vec::new());
        final_review(
            &ReviewContext {
                paths: &paths,
                global: &global,
                source: &source,
                verbose: false,
            },
            &mut plan,
            art,
            &mut prompt,
        )
        .unwrap();
        String::from_utf8_lossy(prompt.output()).into_owned()
    }

    #[test]
    fn guided_review_without_a_key_notes_it_and_shows_art_none() {
        let root = tempdir().unwrap();
        let lookups = std::cell::Cell::new(0);
        let access = access(false, &lookups, Fake::default());
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        let mut art = review_art(&GlobalConfig::default(), &access, &mut prompt, "Game").unwrap();
        assert_eq!(
            String::from_utf8_lossy(prompt.output()),
            "note: run `wineo art key` to fetch art from SteamGridDB\n"
        );
        assert!(!art.enabled);

        // 9 is not a choice, since the row can't be edited without a key.
        let printed = review_with_art(root.path(), &mut art, &["9", ""]);
        assert!(printed.contains("8) desktop      yes\n"), "{printed}");
        assert!(
            printed.contains("\n     art          none\nClose Heroic and the game first.\n"),
            "{printed}"
        );
        assert!(
            printed.contains("`9` is not one of the choices"),
            "{printed}"
        );
        assert!(!art.enabled);
    }

    #[test]
    fn guided_review_with_a_key_fetches_by_default_and_edits_the_row() {
        let root = tempdir().unwrap();
        let lookups = std::cell::Cell::new(0);
        let access = access(true, &lookups, Fake::default());
        let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
        let mut art = review_art(&GlobalConfig::default(), &access, &mut prompt, "Game").unwrap();
        assert_eq!(prompt.output().len(), 0);
        assert!(art.enabled);

        let printed = review_with_art(root.path(), &mut art, &["9", "n", ""]);
        assert!(
            printed.contains("9) art          fetch from SteamGridDB, searching \"Game\"\n"),
            "{printed}"
        );
        assert!(
            printed.contains("Fetch art from SteamGridDB? [Y/n]: "),
            "{printed}"
        );
        assert!(printed.contains("9) art          none\n"), "{printed}");
        assert!(!art.enabled);

        let printed = review_with_art(root.path(), &mut art, &["9", "y", "Game Two", ""]);
        assert!(printed.contains("Search for [Game]: "), "{printed}");
        assert!(
            printed.contains("9) art          fetch from SteamGridDB, searching \"Game Two\"\n"),
            "{printed}"
        );
        assert!(art.enabled);
        assert_eq!(art.term, "Game Two");
        assert_eq!(
            art.log_line(),
            "  art: fetch from SteamGridDB, searching \"Game Two\"\n"
        );
    }

    #[test]
    fn bulk_art_asks_only_with_a_key_and_follows_the_flags() {
        let lookups = std::cell::Cell::new(0);
        let ask = |found: bool, art: bool, yes: bool, answers: &[&str]| {
            let access = access(found, &lookups, Fake::default());
            let mut prompt = Prompt::new(Script::new(answers), Vec::new());
            let key = bulk_art(&access, art, yes, (!yes).then_some(&mut prompt)).unwrap();
            (
                key.is_some(),
                String::from_utf8_lossy(prompt.output()).into_owned(),
            )
        };

        let (fetch, printed) = ask(true, false, false, &[""]);
        assert!(fetch);
        assert_eq!(printed, "Fetch art for these games? [Y/n]: ");
        let (fetch, _) = ask(true, false, false, &["n"]);
        assert!(!fetch);

        let (fetch, printed) = ask(false, false, false, &[]);
        assert!(!fetch);
        assert_eq!(
            printed,
            "note: run `wineo art key` to fetch art from SteamGridDB\n"
        );

        let (fetch, printed) = ask(true, true, false, &[]);
        assert!(fetch);
        assert_eq!(printed, "");
        let (fetch, _) = ask(true, true, true, &[]);
        assert!(fetch);

        lookups.set(0);
        let (fetch, _) = ask(true, false, true, &[]);
        assert!(!fetch);
        assert_eq!(lookups.get(), 0);
    }

    /// A Heroic config with one sideloaded game, Alpha, and a fake wine
    /// runner, classified the way `--all` does.
    fn bulk_fixture(root: &Path) -> (Paths, GlobalConfig, HeroicSource, GamePlan) {
        let paths = paths(root);
        let prefix = root.join("heroic-prefix");
        fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let exe = prefix.join("drive_c/Games/Game.exe");
        fs::write(&exe, "game executable").unwrap();
        let config = root.join("heroic");
        fs::create_dir_all(config.join("sideload_apps")).unwrap();
        fs::create_dir_all(config.join("GamesConfig")).unwrap();
        fs::write(
            config.join("config.json"),
            format!(
                r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
                prefix.display()
            ),
        )
        .unwrap();
        fs::write(config.join("GamesConfig/a.json"), r#"{"a":{}}"#).unwrap();
        fs::write(
            config.join("sideload_apps/library.json"),
            format!(
                r#"{{"games":[{{"app_name":"a","title":"Alpha","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        )
        .unwrap();
        let runner_root = root.join("runners");
        fake_wine_runner(&runner_root, "fake");
        let global = GlobalConfig {
            defaults: wineo_core::config::Defaults {
                runner_root: Some(runner_root.display().to_string()),
                prefix_root: Some(root.join("prefixes").display().to_string()),
                ..wineo_core::config::Defaults::default()
            },
            ..GlobalConfig::default()
        };
        let source = heroic_source(root);
        let listing = listing(&paths, &global, &source).unwrap();
        let library = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap();
        let mut ready = library.into_ready();
        assert_eq!(ready.len(), 1);
        let Game::Windows(game) = ready.remove(0) else {
            panic!("expected a Windows game");
        };
        (paths, global, source, game)
    }

    fn import_bulk_game(
        root: &Path,
        fake: Fake,
    ) -> (anyhow::Result<Option<String>>, String, String, Paths) {
        let (paths, global, source, game) = bulk_fixture(root);
        let lookups = std::cell::Cell::new(0);
        let access = access(true, &lookups, fake);
        let bulk = Bulk {
            paths: &paths,
            global: &global,
            access: &access,
            source: &source,
            desktop: true,
            art: Some(Key {
                value: "k".into(),
                source: wineo_core::steamgriddb::KeySource::Variable,
            }),
            verbose: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let cancellation = Cancellation::new().unwrap();
        let result = import_one(&bulk, &game, &cancellation, &mut out, &mut err);
        (
            result,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
            paths,
        )
    }

    #[test]
    fn bulk_art_uses_the_heroic_title_before_the_desktop_entry() {
        let root = tempdir().unwrap();
        let mut fake = Fake::default();
        fake.search("Alpha", true);
        fake.icon_only();
        let (result, out, err, paths) = import_bulk_game(root.path(), fake);
        result.unwrap();
        assert!(
            out.starts_with(
                "alpha: art from SteamGridDB game 7, Hades (2020)\nicon: fetched SteamGridDB 1\n"
            ),
            "{out}"
        );
        assert_eq!(err, "");
        assert!(paths.art_dir("alpha").join("icon.png").exists());
        let entries: Vec<String> = fs::read_dir(paths.applications_dir())
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contains("\nX-Wineo-Icon=art:"), "{}", entries[0]);
    }

    #[test]
    fn a_failed_bulk_art_fetch_warns_without_failing_the_game() {
        let root = tempdir().unwrap();
        let (result, out, err, paths) = import_bulk_game(root.path(), Fake::default());
        result.unwrap();
        assert_eq!(out, "");
        assert!(err.contains("warning: art fetch failed:"), "{err}");
        assert!(paths.environment_config("alpha").exists());
    }
}
