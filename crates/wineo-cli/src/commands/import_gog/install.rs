//! Installing a GOG game wineo doesn't have yet, from GOG's offline
//! installers: a review, progress while it downloads and installs, and GOG's
//! playtime and art after, like a migration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::create::CreateEvent;
use wineo_core::environment;
use wineo_core::gog::account::Session;
use wineo_core::gog::folder::{self, JournalKind, Platform, ProductStatus};
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::product;
use wineo_core::gog::update::install::{self, Installed, Plan, Request};
use wineo_core::gog::update::windows::{Event, SetupError};
use wineo_core::gog::update::{self, Progress, UpdateError};
use wineo_core::paths::Paths;
use wineo_core::targets::Batch;

use super::Args;
use crate::cancel;
use crate::commands::gog_support::{
    Account, Reporter, ask_wizard, offers_wizard, usage_error, warn,
};
use crate::commands::setup::{self, watched_message};
use crate::commands::table;
use crate::commands::transfer::Cancellation;
use crate::commands::{components, desktop};
use crate::picker::{self, Place};
use crate::prompt::{self, Ask, Reply, terminal};
use crate::style;

const NEEDS_YES: &str = "Installing without a terminal needs --yes.";

/// Whether wineo already has the game, and under which name.
enum Existing {
    None,
    /// An install that stopped, which installing again finishes.
    Unfinished(String),
    Installed {
        name: String,
        folder: Option<PathBuf>,
    },
}

/// The environment that has GOG game `id`: one whose config records it, or
/// the owner of an install that stopped before its config was written.
fn existing(paths: &Paths, id: u64) -> anyhow::Result<Existing> {
    let text = id.to_string();
    for name in environment::names(paths)? {
        // A config that can't be read doesn't say, as in `--list`.
        if update::recorded_gog_id(paths, &name)
            .ok()
            .flatten()
            .as_deref()
            != Some(text.as_str())
        {
            continue;
        }
        let state = folder::find(paths, &name, &text)?;
        if state
            .as_ref()
            .and_then(|state| state.journal.as_ref())
            .is_some_and(|journal| journal.kind == JournalKind::Install)
        {
            return Ok(Existing::Unfinished(name));
        }
        return Ok(Existing::Installed {
            name,
            folder: state.map(|state| state.folder),
        });
    }
    for (_, state) in folder::list(paths)? {
        let Ok(state) = state else { continue };
        let unfinished = state.committed.is_none()
            && state
                .journal
                .as_ref()
                .is_some_and(|journal| journal.kind == JournalKind::Install);
        if let Some(owner) = &state.owner
            && unfinished
            && state.gog_id() == Some(id)
            && !paths.environment_config(owner).exists()
        {
            return Ok(Existing::Unfinished(owner.clone()));
        }
    }
    Ok(Existing::None)
}

/// The new environment's name, from the game's title on GOG.
fn new_name(paths: &Paths, session: &Session, id: u64, dry_run: bool) -> anyhow::Result<String> {
    // A dry run caches nothing, so the details are only read.
    let details = product::load_with(paths, session.client(), id, false, !dry_run)?;
    let title = details.product.title.trim();
    let title = if title.is_empty() {
        format!("GOG game {id}")
    } else {
        title.to_owned()
    };
    Ok(wineo_core::name::from_title(&title))
}

pub(super) fn one(paths: &Paths, args: &Args, id: &str) -> anyhow::Result<ExitCode> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.into());
    }
    let Ok(gog_id) = id.parse::<u64>() else {
        return Ok(usage_error(format!("{id} isn't a GOG game id.")));
    };
    // Whether wineo already has it is known without GOG or a question.
    let existing = existing(paths, gog_id)?;
    if let Existing::Installed { name, folder } = &existing {
        match folder {
            Some(folder) => println!(
                "Already installed. {name} owns {}.",
                style::path(paths, folder)
            ),
            None => println!("Already installed as {name}."),
        }
        if args.dry_run {
            println!("Dry run, nothing was installed.");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    let account = Account::new();
    let mut session = account.session(paths)?;
    let unfinished = matches!(existing, Existing::Unfinished(_));
    let name = match existing {
        Existing::Installed { .. } => unreachable!("said above"),
        Existing::Unfinished(name) => name,
        Existing::None => new_name(paths, &session, gog_id, args.dry_run)?,
    };
    let dir = args
        .dir
        .as_deref()
        .map(|dir| paths.absolute(&picker::clean_path(dir)))
        .transpose()?;
    let mut request = Request {
        name,
        gog_id,
        dir,
        language: args.language.clone(),
        // New environments get desktop entries, as a migration's do.
        desktop: true,
        refresh: false,
        read_only: args.dry_run,
        platform: args.platform.map(Into::into),
        runner: args.runner.clone(),
        arch: args.arch,
        wizard: false,
    };
    let plan = match plan_install(paths, &mut session, &request, unfinished)? {
        Ok(plan) => plan,
        Err(code) => return Ok(code),
    };
    if plan.platform() == Platform::Linux && (args.runner.is_some() || args.arch.is_some()) {
        let flags = match (args.runner.is_some(), args.arch.is_some()) {
            (true, true) => "--runner and --arch",
            (true, false) => "--runner",
            _ => "--arch",
        };
        return Ok(usage_error(format!(
            "{} installs natively, so leave out {flags}.",
            plan.title()
        )));
    }
    // An unfinished install carries on as it began.
    if plan.resumes() {
        let has_environment = plan.creation().is_none();
        let ignored: Vec<&str> = [
            (args.platform.is_some(), "--platform"),
            (args.language.is_some(), "--language"),
            (has_environment && args.runner.is_some(), "--runner"),
            (has_environment && args.arch.is_some(), "--arch"),
        ]
        .into_iter()
        .filter_map(|(given, flag)| given.then_some(flag))
        .collect();
        if !ignored.is_empty() {
            return Ok(usage_error(format!(
                "{}'s install didn't finish, so --game finishes it. Leave out {}.",
                plan.title(),
                super::listed(&ignored)
            )));
        }
    }
    print_review(paths, &plan);
    if args.dry_run {
        println!("Dry run, nothing was installed.");
        return Ok(ExitCode::SUCCESS);
    }
    let mut prompt = if asking { Some(terminal()?) } else { None };
    if let Some(prompt) = prompt.as_mut()
        && !confirm(prompt, &plan)?
    {
        eprintln!("Install canceled.");
        return Ok(cancel::exit_code());
    }
    let installed = match apply(paths, &mut session, &plan, &plan) {
        Ok(installed) => installed,
        Err(error) => {
            let label = offers_wizard(&error);
            let Some((prompt, label)) = prompt.as_mut().zip(label) else {
                return stopped(error, &plan, &plan);
            };
            match ask_wizard(prompt, &label) {
                Ok(true) => {}
                Ok(false) => return stopped(error, &plan, &plan),
                Err(asked) if cancel::prompted(&asked) => {
                    eprintln!("Install canceled.");
                    say_finish(plan.environment());
                    return Ok(cancel::exit_code());
                }
                Err(asked) => return Err(asked),
            }
            // The first attempt left its journal, so this carries it on.
            request.wizard = true;
            let again = match plan_install(paths, &mut session, &request, true)? {
                Ok(again) => again,
                Err(code) => return Ok(code),
            };
            // A plan resumed from cached packages may only know the game
            // by its id, so the first plan names things.
            match apply(paths, &mut session, &again, &plan) {
                Ok(installed) => installed,
                Err(error) => return stopped(error, &again, &plan),
            }
        }
    };
    report(paths, &plan, &installed)?;
    if plan.platform() == Platform::Windows && installed.target.is_none() {
        return pick_target(paths, &plan, prompt.as_mut());
    }
    Ok(ExitCode::SUCCESS)
}

/// Plans the install, telling Ctrl+C apart: an unfinished install keeps its
/// journal, which `wineo update` finishes.
fn plan_install(
    paths: &Paths,
    session: &mut Session,
    request: &Request,
    unfinished: bool,
) -> anyhow::Result<Result<Plan, ExitCode>> {
    let cancellation = Cancellation::new()?;
    #[cfg(debug_assertions)]
    pause_for_test(&cancellation.token);
    let planned = install::plan(paths, session, request, &cancellation.token);
    match planned {
        // Ctrl+C while planning cancels whatever planning made of it.
        _ if cancellation.token.is_canceled() => {
            if unfinished {
                eprintln!("Install interrupted.");
                say_finish(&request.name);
            } else {
                eprintln!("Install canceled.");
            }
            Ok(Err(cancel::exit_code()))
        }
        Ok(plan) => Ok(Ok(plan)),
        Err(error) if is_cancel(&error) => {
            if unfinished {
                eprintln!("Install interrupted.");
                say_finish(&request.name);
            } else {
                eprintln!("Install canceled.");
            }
            Ok(Err(cancel::exit_code()))
        }
        Err(error) => Err(error),
    }
}

/// Lets a test press Ctrl+C while an install is planned: with
/// `WINEO_TEST_UPDATE_PAUSE=plan`, planning waits for it first. Release
/// builds never look at it.
#[cfg(debug_assertions)]
fn pause_for_test(token: &wineo_core::cancel::Token) {
    if std::env::var("WINEO_TEST_UPDATE_PAUSE").as_deref() == Ok("plan") {
        eprintln!("Planning");
        while !token.is_canceled() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

fn is_cancel(error: &anyhow::Error) -> bool {
    cancel::prompted(error)
        || matches!(
            error.downcast_ref::<UpdateError>(),
            Some(UpdateError::Canceled { .. })
        )
        || matches!(
            error.downcast_ref::<folder::FolderError>(),
            Some(folder::FolderError::Canceled)
        )
}

/// A Windows game whose info file names no program to start: at a
/// terminal, the programs it installed are offered, as after `new`'s
/// installer; otherwise it says how to pick one later.
fn pick_target(paths: &Paths, plan: &Plan, prompt: Option<&mut Ask>) -> anyhow::Result<ExitCode> {
    let name = plan.environment();
    let environment = environment::Environment::load(paths, name)?;
    if !environment.config.targets.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let Some(prompt) = prompt else {
        println!(
            "{} names no program to start. Pick one with: wineo setup {name}",
            plan.title()
        );
        return Ok(ExitCode::SUCCESS);
    };
    let global = GlobalConfig::load(paths)?;
    let dir = environment.prefix(paths)?;
    let place = Place {
        paths,
        name,
        wine_prefix: wineo_core::prefix::wine_prefix_in(&dir),
        native_env: None,
        game_folder: Some(plan.folder().to_owned()),
    };
    let mut batch = Batch::new(environment.config.targets.clone());
    prompt.set_first_question(false);
    picker::pick(prompt, &place, &mut batch)?;
    if batch.is_empty() {
        return setup::finish(paths, &global, name, prompt, &batch);
    }
    match setup::save(paths, &global, name, prompt, &mut batch, true)? {
        Reply::Value(()) => setup::finish(paths, &global, name, prompt, &batch),
        // Keeping on editing is what `setup` is for.
        Reply::Back => setup::menu(paths, &global, name, prompt, &place, batch),
    }
}

/// Whether the install goes ahead.
fn confirm(prompt: &mut Ask, plan: &Plan) -> anyhow::Result<bool> {
    let question = if plan.resumes() {
        "Finish it?"
    } else {
        "Install it?"
    };
    prompt.confirm_hint(false)?;
    Ok(matches!(
        prompt.confirm(question, true)?,
        Reply::Value(true)
    ))
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "Linux",
        Platform::Windows => "Windows",
    }
}

fn review_lines(paths: &Paths, plan: &Plan) -> Vec<String> {
    let kind = if plan.has_config() {
        String::new()
    } else {
        match plan.platform() {
            Platform::Linux => " (new native environment)".to_owned(),
            Platform::Windows => " (new Windows environment)".to_owned(),
        }
    };
    let mut rows = vec![
        (
            "environment".to_owned(),
            format!("{}{kind}", plan.environment()),
        ),
        ("folder".to_owned(), style::path(paths, plan.folder())),
        (
            "platform".to_owned(),
            platform_name(plan.platform()).to_owned(),
        ),
        ("language".to_owned(), plan.requested_language().to_owned()),
    ];
    if let Some(creation) = plan.creation() {
        rows.push(("runner".to_owned(), creation.runner().name().to_owned()));
        rows.push(("arch".to_owned(), creation.resolved_arch().to_string()));
    }
    let mut dlc = Vec::new();
    for product in plan.products() {
        let version = match (product.status, &product.version) {
            (ProductStatus::Installed, Some(version)) => version.clone(),
            (ProductStatus::Installed, None) => "unknown version".to_owned(),
            _ => "unavailable".to_owned(),
        };
        if product.id == plan.gog_id() {
            rows.push(("version".to_owned(), version));
        } else {
            dlc.push(format!("{} {version}", plan.dlc_label(product.id)));
        }
    }
    // One DLC per line; the rest sit under the first one's value.
    for (index, line) in dlc.into_iter().enumerate() {
        let label = if index == 0 { "dlc" } else { "" };
        rows.push((label.to_owned(), line));
    }
    if plan.is_offline() {
        rows.push((
            "download".to_owned(),
            "none, uses the packages already downloaded".to_owned(),
        ));
    } else {
        rows.push(("download".to_owned(), style::size(plan.download_size())));
    }
    rows.push((
        "installed".to_owned(),
        format!(
            "about {} (an estimate)",
            style::size(plan.installed_estimate())
        ),
    ));
    let action = if plan.resumes() {
        "Finish the interrupted install of"
    } else {
        "Install"
    };
    let mut lines = vec![format!("{action} {} ({})", plan.title(), plan.gog_id())];
    lines.extend(table::aligned_rows(rows));
    lines
}

fn print_review(paths: &Paths, plan: &Plan) {
    for line in review_lines(paths, plan) {
        println!("{line}");
    }
    for warning in plan.warnings() {
        warn(warning);
    }
    for product in plan.products() {
        if let Some(note) = &product.note {
            warn(note);
        }
    }
}

/// Runs the install with progress for its downloads, unpacking, and each
/// Windows installer, with products named as `names` names them.
fn apply(
    paths: &Paths,
    session: &mut Session,
    plan: &Plan,
    names: &Plan,
) -> anyhow::Result<Installed> {
    let labels: HashMap<u64, String> = names
        .products()
        .iter()
        .map(|product| {
            let label = if product.id == names.gog_id() {
                names.title().to_owned()
            } else {
                names.dlc_label(product.id)
            };
            (product.id, label)
        })
        .collect();
    let progress = std::cell::RefCell::new(Reporter::new(labels));
    let cancellation = Cancellation::new()?;
    let name = plan.environment();
    install::apply_with_events(
        paths,
        session,
        plan,
        &cancellation.token,
        |step| {
            progress.borrow_mut().show(&step);
            #[cfg(debug_assertions)]
            crate::commands::gog_support::pause_for_test(name, &step, &cancellation.token);
        },
        |event| match event {
            Event::Installing {
                product,
                interactive,
            } => progress.borrow_mut().show(&Progress::Installing {
                product,
                interactive,
            }),
            Event::Watched(watched) => {
                if let Some(line) = watched_message(watched) {
                    eprintln!("{line}");
                }
            }
            Event::Create(event) => created(paths, name, event),
        },
    )
}

/// Says what creating a Windows game's environment did, like `new`.
fn created(paths: &Paths, name: &str, event: CreateEvent) {
    match event {
        CreateEvent::BuildingPrefix { dir, runner } => {
            let dir = style::path(paths, &dir);
            if runner.is_proton() {
                eprintln!(
                    "Creating a Proton prefix in {dir}. The first umu run also downloads the Steam Linux Runtime, which takes a while"
                );
            } else {
                eprintln!("Creating a wine prefix in {dir}, this takes a few seconds");
            }
        }
        CreateEvent::TemplateComponentsOmitted { runner } => eprintln!(
            "{runner} bundles its own DXVK and vkd3d-proton, so the template's are left out"
        ),
        CreateEvent::Components(changes) => {
            for change in changes {
                eprintln!("{}", components::describe(&change, name));
            }
        }
        CreateEvent::Desktop(changes) => {
            for change in changes {
                eprintln!("{}", desktop::describe(&change, name));
            }
        }
        CreateEvent::Saved | CreateEvent::Registered { .. } => {}
    }
}

/// Says how an install that didn't finish left things, and exits with the
/// matching code.
fn stopped(error: anyhow::Error, plan: &Plan, names: &Plan) -> anyhow::Result<ExitCode> {
    let name = plan.environment();
    let finish = || say_finish(name);
    if let Some(UpdateError::Canceled { journal }) = error.downcast_ref() {
        if *journal {
            eprintln!("Install interrupted.");
            finish();
        } else {
            eprintln!("Install canceled.");
        }
        return Ok(cancel::exit_code());
    }
    match error.downcast_ref::<SetupError>() {
        // The game's own installer was canceled before it changed anything,
        // but the install it belongs to is still there to finish.
        Some(SetupError::Canceled { .. }) => {
            eprintln!("Install canceled.");
            finish();
            Ok(cancel::exit_code())
        }
        // A plan resumed from cached packages may only know the game by its
        // id, so it is named as `names` names it.
        Some(SetupError::NotStarted { product, code, .. }) => {
            let label = if *product == names.gog_id() {
                names.title().to_owned()
            } else {
                names.dlc_label(*product)
            };
            eprintln!(
                "{}",
                style::error_stderr(format!(
                    "error: {label}'s installer stopped before installing anything (exit code {code})."
                ))
            );
            finish();
            Ok(ExitCode::FAILURE)
        }
        _ => Err(error),
    }
}

fn say_finish(name: &str) {
    eprintln!("Finish the interrupted install with: wineo update {name}");
}

/// Says what the install did, then imports GOG's playtime and art and
/// writes the desktop entries, as after a migration.
fn report(paths: &Paths, plan: &Plan, installed: &Installed) -> anyhow::Result<()> {
    let name = installed.environment.as_str();
    if !installed.saved.is_empty() {
        println!("Moved your files aside where the game has files:");
        for saved in &installed.saved {
            println!("  {} -> {}", saved.path, saved.saved_as);
        }
    }
    if !installed.kept.is_empty() {
        println!("Kept these files you changed, which the game no longer ships:");
        for path in &installed.kept {
            println!("  {path}");
        }
    }
    if !installed.overwritten.is_empty() {
        println!("Overwrote these files that differed from the game's:");
        for path in &installed.overwritten {
            println!("  {path}");
        }
    }
    // The plan's warnings came first, and the review already showed them.
    for warning in installed.warnings.iter().skip(plan.warnings().len()) {
        warn(warning);
    }
    let global = GlobalConfig::load(paths)?;
    let title = plan.title();
    super::extras::import(paths, &global, name, installed.gog_id, title);
    if let Err(error) = crate::commands::art::apply_desktop(paths, name) {
        warn(format!("Couldn't update desktop entries: {error:#}"));
    }
    // A plan carried on after the wizard question is still the same install.
    if plan.resumes() {
        println!("Finished the interrupted install of {name}.");
    } else {
        println!("Installed {title} as {name}.");
    }
    Ok(())
}
