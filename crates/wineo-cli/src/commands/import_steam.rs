use std::collections::HashSet;
use std::io::{self, Write};
use std::process::ExitCode;

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;
use wineo_core::steam_import::{self, ApplyOutcome, EntryState, Plan, PlanOutcome};
use wineo_core::steamgriddb::Key;

use super::art::{self, KeyCheck};
use super::import::ReturnToSourceMenu;
use super::transfer::Cancellation;
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};
use crate::style;

#[derive(clap::Args, Default)]
pub struct Args {
    /// List installed Steam games without prompting
    #[arg(long)]
    pub list: bool,
    /// Steam app id
    #[arg(long)]
    pub game: Option<String>,
    /// Name for the new environment
    #[arg(short, long)]
    pub name: Option<String>,
    /// Import every installed Steam game
    #[arg(long, conflicts_with_all = ["game", "name", "list"])]
    pub all: bool,
    /// Skip the bulk confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Fetch missing art from SteamGridDB by Steam app id
    #[arg(long)]
    pub art: bool,
    /// Review the import without changing anything
    #[arg(long, conflicts_with = "list")]
    pub dry_run: bool,
}

struct OutputOnly;

impl Input for OutputOnly {
    fn line(&mut self, _: &str) -> io::Result<crate::prompt::Line> {
        unreachable!("Steam review has no prompts")
    }
}

pub fn run(paths: &Paths, args: Args, first_question: bool) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        paths.portable_root().is_none(),
        "Steam games can't be imported into a portable folder."
    );
    let global = GlobalConfig::load(paths)?;
    let result = (|| {
        if args.list {
            list(paths, &global)
        } else if args.all {
            bulk(paths, &global, &args)
        } else if let Some(app_id) = &args.game {
            match steam_import::plan(paths, &global, app_id, args.name.as_deref())? {
                PlanOutcome::AlreadyImported(name) => {
                    println!("Already imported as {name}.");
                    if args.dry_run {
                        println!("Dry run, nothing was imported.");
                    }
                    Ok(ExitCode::SUCCESS)
                }
                PlanOutcome::Ready(plan) => {
                    let mut out = Prompt::new(OutputOnly, crate::output::stdout());
                    let key = art_key(paths, &global, args.art);
                    review(&plan, key.is_some(), &mut out)?;
                    if args.dry_run {
                        println!("Dry run, nothing was imported.");
                    } else {
                        let cancellation = Cancellation::new()?;
                        apply(paths, &global, &plan, key.as_ref(), &cancellation.token)?;
                    }
                    Ok(ExitCode::SUCCESS)
                }
            }
        } else {
            anyhow::ensure!(
                !args.dry_run && prompt::interactive(),
                "guided Steam import needs a terminal; pass --game <appid> or --all --yes"
            );
            let mut out = Prompt::new(Terminal::open()?, crate::output::stdout());
            guided(paths, &global, &args, &mut out, first_question)
        }
    })();
    match result {
        Err(error) if canceled(&error) => {
            eprintln!("Import canceled.");
            Ok(ExitCode::from(130))
        }
        other => other,
    }
}

fn canceled(error: &anyhow::Error) -> bool {
    error.is::<Canceled>()
        || error.is::<wineo_core::import::Cancelled>()
        || matches!(
            error.downcast_ref::<wineo_core::Error>(),
            Some(wineo_core::Error::Canceled(_))
        )
}

fn list(paths: &Paths, global: &GlobalConfig) -> anyhow::Result<ExitCode> {
    for entry in steam_import::listing(paths, global)? {
        let state = match &entry.state {
            EntryState::Installed => "installed",
            EntryState::Updating => "updating",
            EntryState::Unreadable(_) => "unreadable manifest",
        };
        let size = entry
            .size_on_disk
            .map(style::size)
            .unwrap_or_else(|| "-".to_owned());
        let imported = entry
            .imported_as
            .map(|name| format!("  Already imported as {name}."))
            .unwrap_or_default();
        println!(
            "{}  {}  {size}  {state}{imported}",
            entry.app_id,
            entry.title.as_deref().unwrap_or("-")
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn review<I: Input, W: Write>(plan: &Plan, fetch: bool, out: &mut Prompt<I, W>) -> io::Result<()> {
    out.say(format!("Ready to import {}:", plan.name()))?;
    out.say(format!("  game      {} ({})", plan.title(), plan.app_id()))?;
    out.say(format!("  size      {}", style::size(plan.size_on_disk())))?;
    out.say("  launcher  Steam")?;
    out.say("  tags      game")?;
    out.say("  desktop   off")?;
    out.say(if fetch {
        "  art       Steam cache, then SteamGridDB"
    } else {
        "  art       Steam cache"
    })
}

fn art_key(paths: &Paths, global: &GlobalConfig, wanted: bool) -> Option<Key> {
    if !wanted || global.art.fetch.kinds().is_empty() {
        return None;
    }
    let check = art::check_key(paths);
    check.print_skip();
    match check {
        KeyCheck::Found(key) => Some(key),
        _ => None,
    }
}

fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &Plan,
    key: Option<&Key>,
    cancel: &Token,
) -> anyhow::Result<ApplyOutcome> {
    let outcome = steam_import::apply(paths, global, plan, cancel)?;
    let applied = match &outcome {
        ApplyOutcome::Imported(applied) => applied,
        ApplyOutcome::AlreadyImported(name) => {
            println!("Already imported as {name}.");
            return Ok(outcome);
        }
    };
    for warning in &applied.warnings {
        eprintln!("warning: {warning}");
    }
    println!("Imported {}.", applied.environment.name);
    if cancel.is_canceled() {
        return Err(Canceled.into());
    }
    if let Some(key) = key {
        fetch_art(paths, global, plan, key, cancel)?;
    }
    Ok(outcome)
}

fn fetch_art(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &Plan,
    key: &Key,
    cancel: &Token,
) -> anyhow::Result<()> {
    if cancel.is_canceled() {
        return Err(Canceled.into());
    }
    let result = (|| {
        let transport = wineo_core::download::Https::new()?;
        let api = wineo_core::steamgriddb::Api::new(&transport, &key.value);
        let language = wineo_core::steamgriddb::language(|var| std::env::var_os(var));
        wineo_core::art_fetch::fetch_steam(
            paths,
            plan.name(),
            plan.app_id()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid app_id: not a number."))?,
            &api,
            &language,
            &global.art.fetch.kinds(),
        )
    })();
    match result {
        Ok(results) => {
            for (kind, fetched) in results {
                if let Err(error) = fetched {
                    eprintln!("warning: {kind} art couldn't be fetched: {error:#}.");
                }
            }
        }
        Err(error) => eprintln!("warning: SteamGridDB art couldn't be fetched: {error:#}."),
    }
    if cancel.is_canceled() {
        return Err(Canceled.into());
    }
    Ok(())
}

fn bulk(paths: &Paths, global: &GlobalConfig, args: &Args) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        args.dry_run || args.yes || prompt::interactive(),
        "bulk Steam import needs a terminal; pass --yes to skip the confirmation"
    );
    let key = art_key(paths, global, args.art);
    let mut ready = Vec::new();
    let mut failures = Vec::new();
    let mut done = 0;
    let mut already_imported = Vec::new();
    let mut names = HashSet::new();
    let mut out = Prompt::new(OutputOnly, crate::output::stdout());
    for entry in steam_import::listing(paths, global)? {
        if let Some(name) = entry.imported_as {
            println!("Already imported as {name}.");
            continue;
        }
        match steam_import::plan(paths, global, &entry.app_id, None) {
            Ok(PlanOutcome::Ready(plan)) => {
                if names.insert(plan.name().to_owned()) {
                    review(&plan, key.is_some(), &mut out)?;
                    ready.push(plan);
                } else {
                    failures.push((
                        entry.app_id,
                        plan.name().to_owned(),
                        wineo_core::Error::EnvironmentExists(plan.name().to_owned()).to_string(),
                    ));
                }
            }
            Ok(PlanOutcome::AlreadyImported(name)) => println!("Already imported as {name}."),
            Err(error) => {
                let name = entry
                    .title
                    .as_deref()
                    .map(|title| wineo_core::heroic_import::default_name(title, None))
                    .unwrap_or_else(|| entry.app_id.clone());
                failures.push((entry.app_id, name, format!("{error:#}")));
            }
        }
    }
    let mut total = ready.len();
    if args.dry_run {
        println!("Dry run, nothing was imported.");
    } else if !ready.is_empty() {
        if !args.yes {
            let mut out =
                Prompt::new(Terminal::open()?, crate::output::stdout()).with_first_question(true);
            match out.confirm(
                &format!(
                    "Import {} {}?",
                    ready.len(),
                    if ready.len() == 1 { "game" } else { "games" }
                ),
                true,
            )? {
                Reply::Value(true) => {}
                _ => return Err(Canceled.into()),
            }
        }
        let cancellation = Cancellation::new()?;
        for plan in ready {
            if cancellation.token.is_canceled() {
                return Err(Canceled.into());
            }
            match apply(paths, global, &plan, key.as_ref(), &cancellation.token) {
                Ok(ApplyOutcome::Imported(_)) => done += 1,
                Ok(ApplyOutcome::AlreadyImported(name)) => {
                    total -= 1;
                    already_imported.push(name);
                }
                Err(error) if canceled(&error) => return Err(error),
                Err(error) => failures.push((
                    plan.app_id().to_owned(),
                    plan.name().to_owned(),
                    format!("{error:#}"),
                )),
            }
        }
    }
    for (app_id, _, reason) in &failures {
        eprintln!("Failed {app_id}: {reason}");
    }
    if !args.dry_run {
        let noun = if total == 1 { "game" } else { "games" };
        println!("Imported {done} of {total} {noun}.");
        if !already_imported.is_empty() {
            println!(
                "{} already imported: {}",
                already_imported.len(),
                already_imported.join(", ")
            );
        }
        if !failures.is_empty() {
            let names: Vec<_> = failures.iter().map(|(_, name, _)| name.as_str()).collect();
            println!("{} failed: {}", failures.len(), names.join(", "));
        }
    }
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn guided<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    args: &Args,
    out: &mut Prompt<I, W>,
    first_question: bool,
) -> anyhow::Result<ExitCode> {
    let entries: Vec<_> = steam_import::listing(paths, global)?
        .into_iter()
        .filter(|entry| {
            entry.imported_as.is_none() && !matches!(entry.state, EntryState::Unreadable(_))
        })
        .collect();
    anyhow::ensure!(!entries.is_empty(), "Steam has no games to import.");
    let choices = numbered(entries.iter().map(|entry| {
        (
            format!(
                "{} ({})",
                entry.title.as_deref().unwrap_or("-"),
                entry.app_id
            ),
            entry.app_id.clone(),
        )
    }));
    let mut step = 0;
    let mut game = String::new();
    let mut name = args.name.clone();
    loop {
        out.set_first_question(step == 0 && first_question);
        match step {
            0 => match out.choose("Which Steam game?", "Game", &choices, None, true)? {
                Reply::Value(value) => {
                    game = value;
                    name = args.name.clone();
                    step = 1;
                }
                Reply::Back if first_question => return Err(Canceled.into()),
                Reply::Back => return Err(ReturnToSourceMenu.into()),
            },
            1 => {
                let entry = entries.iter().find(|entry| entry.app_id == game).unwrap();
                let default = wineo_core::heroic_import::default_name(
                    entry.title.as_deref().unwrap_or(""),
                    None,
                );
                match out.text(
                    "Environment name",
                    Some(name.as_deref().unwrap_or(&default)),
                    |value| {
                        steam_import::plan(paths, global, &game, Some(value))
                            .map(|_| value.to_owned())
                            .map_err(|error| error.to_string())
                    },
                )? {
                    Reply::Value(value) => {
                        name = Some(value);
                        step = 2;
                    }
                    Reply::Back => step = 0,
                }
            }
            _ => {
                let plan = match steam_import::plan(paths, global, &game, name.as_deref())? {
                    PlanOutcome::Ready(plan) => plan,
                    PlanOutcome::AlreadyImported(name) => {
                        out.say(format!("Already imported as {name}."))?;
                        return Ok(ExitCode::SUCCESS);
                    }
                };
                let key = art_key(paths, global, args.art);
                review(&plan, key.is_some(), out)?;
                out.say("Press Enter to import it, Esc to go back, Ctrl+C to cancel.")?;
                match out.confirm("Import it?", true)? {
                    Reply::Back => step = 1,
                    Reply::Value(false) => return Err(Canceled.into()),
                    Reply::Value(true) => {
                        let cancellation = Cancellation::new()?;
                        apply(paths, global, &plan, key.as_ref(), &cancellation.token)?;
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            }
        }
    }
}
