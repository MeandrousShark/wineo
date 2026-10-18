use anyhow::bail;
use std::io::{self, Write};
use std::process::ExitCode;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::heroic::{HeroicGameKind, HeroicIssue, HeroicSource};
use wineo_core::heroic_import::{
    self, GamePlan, Options, Playtime, Policy, Registered, apply, blocker, inspect_copyable_runner,
    listing, plan_game, write_import_log,
};
use wineo_core::paths::Paths;
use wineo_core::steamgriddb::Key;

use super::art::{Access, AutoFetch, KeyCheck, Pick};
use super::desktop;
use super::external_runners;
use super::import::{ReturnToSourceMenu, finish_guided};
use super::transfer::Cancellation;
use crate::cancel;
use crate::picker;
use crate::prompt::{self, Canceled, Input, NoInput, Prompt, Reply, Terminal, numbered};
use crate::style;

mod bulk;
mod dry_run;
mod native;
mod review;
mod runner;
mod summary;

use bulk::run_all;
use review::{ReviewContext, final_review, reject_unresolved, review_settings};
pub(crate) use runner::{ChosenRunner, ensure_components_available, pick_runner};
use summary::{final_facts_text, show_game_summary};

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
        for warning in &registered.warnings {
            eprintln!("warning: {warning}");
        }
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
    for warning in &registered.warnings {
        eprintln!("warning: {warning}");
    }
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
