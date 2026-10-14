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
use crate::cancel;
use crate::commands::table;
use crate::picker;
use crate::prompt::{self, Canceled, Input, NoInput, Prompt, Reply, Terminal, numbered};
use crate::style;
use crate::text;

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
            Err(error) if cancel::prompted(&error) => {
                eprintln!("Import canceled");
                Ok(cancel::exit_code())
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
        let mut output = Prompt::new(NoInput, crate::output::stdout());
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
            Err(error) if cancel::prompted(&error) => {
                eprintln!("Import canceled");
                return Ok(cancel::exit_code());
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
            return Ok(cancel::exit_code());
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
        Err(error) if cancel::prompted(&error) => {
            eprintln!("Import canceled");
            Ok(cancel::exit_code())
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
    let (imported, shared) = source.inspect_with_shared_prefix(&game.id)?;
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
                text::plural(count, "", "s")
            )
        } else {
            format!(
                "{} is missing for {count} game{} ({}). Download the exact requested version{}?",
                component.label(),
                text::plural(count, "", "s"),
                pinned.join(", "),
                text::plural(pinned.len(), "", "s")
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
                launch_scripts: !plan.heroic().settings_review.hooks().is_empty(),
            },
            Entry::Ready(Game::Native(plan)) => BulkRow {
                name: plan.name().to_owned(),
                title: plan.heroic().title.clone(),
                status: RowStatus::ReadyNative {
                    warn: warnings_line(&plan.warnings(), false),
                },
                launch_scripts: !plan.heroic().settings_review.hooks().is_empty(),
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
    match warning {
        Warning::SharedPrefix(shared) => shared_prefix_lines(shared, adopt, false).join(" "),
        Warning::UmuDisabled(message) | Warning::NvapiNotInstalled(message) => message.clone(),
        Warning::AntiCheatNotImported => "EAC/BattlEye runtimes aren't imported".to_owned(),
        Warning::Omitted(count) => {
            format!(
                "{count} unsupported setting{} omitted",
                text::plural(*count, "", "s")
            )
        }
        Warning::Dropped(count) => {
            format!(
                "{count} managed conflict{} dropped",
                text::plural(*count, "", "s")
            )
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
    let name_width = text::widest(rows.iter().map(|row| row.name.as_str()));
    let title_width = text::widest(rows.iter().map(|row| row.title.as_str()));
    let mode_width = rows
        .iter()
        .filter_map(|row| match row.status {
            RowStatus::ReadyNative { .. } => Some("native".len()),
            RowStatus::Ready { .. } => Some(mode_label.len()),
            RowStatus::Skipped(_) => None,
        })
        .max()
        .unwrap_or(0);
    let noun = text::plural(rows.len(), "game", "games");
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
        out.push_str(&format!(
            "  {}  {}  {status}\n",
            text::pad(&row.name, name_width),
            text::pad(&row.title, title_width)
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
                    let mut output = Prompt::new(NoInput, crate::output::stdout());
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
            Err(error) if cancel::prompted(&error) => {
                canceled = true;
                break;
            }
            Err(_) => failed.push(name),
        }
    }
    let summary = text::games_summary("Imported", done, total);
    if failed.is_empty() {
        println!("{summary}");
    } else {
        println!("{summary} {} failed: {}", failed.len(), failed.join(", "));
    }
    if canceled {
        let remaining = total - done - failed.len();
        println!("{} canceled.", text::counted(remaining, "game", "games"));
        return Ok(cancel::exit_code());
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
        Err(error) if cancel::prompted(error) => writeln!(output, " canceled")?,
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
    let top = Pick::<NoInput, Vec<u8>>::Top(out);
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
        let width = table::label_column(items.iter().map(|(label, _, _)| *label));
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
    let width = table::label_column(blocks.iter().map(|block| block.label));
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
    review
        .hooks()
        .into_iter()
        .map(|(label, path)| Block {
            label,
            lines: vec![path],
        })
        .collect()
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
            text::plural(hidden, "setting is", "settings are")
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
                text::plural(count, "", "s")
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
mod tests;
