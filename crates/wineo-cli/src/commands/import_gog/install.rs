//! Installing a GOG game wineo doesn't have yet, from Galaxy's builds or
//! GOG's offline installers: a review, progress while it downloads and
//! installs, and GOG's playtime and art after, like a migration.

use std::path::PathBuf;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::create::CreateEvent;
use wineo_core::environment;
use wineo_core::gog::account::Session;
use wineo_core::gog::folder::{self, JournalKind, Platform, ProductStatus, Track};
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::product;
use wineo_core::gog::update::galaxy::{self as core_galaxy, Choice, Planned};
use wineo_core::gog::update::install::{Creation, Installed, Request};
use wineo_core::gog::update::windows::{Event, SetupError};
use wineo_core::gog::update::{self, Progress, UpdateError};
use wineo_core::paths::Paths;
use wineo_core::targets::Batch;

use super::Args;
use crate::cancel;
use crate::commands::galaxy::{self, GogTrack};
use crate::commands::gog_support::{
    Account, Reporter, ask_password, ask_wizard, finish_command, journal_for, needs_password,
    offers_wizard, password_from_stdin, usage_error, warn, with_finish_hint,
};
use crate::commands::setup::{self, watched_message};
use crate::commands::table;
use crate::commands::transfer::Cancellation;
use crate::commands::{components, desktop};
use crate::picker::{self, Place};
use crate::prompt::{self, Ask, Reply, terminal};
use crate::style;

const NEEDS_YES: &str = "Installing without a terminal needs --yes.";

/// An install planned on the Galaxy track or from offline installers, with
/// what the two share.
struct Plan(Planned);

impl Plan {
    fn galaxy(&self) -> Option<&core_galaxy::install::Plan> {
        match &self.0 {
            Planned::Galaxy(plan) => Some(plan),
            Planned::Installer(_) => None,
        }
    }

    fn environment(&self) -> &str {
        match &self.0 {
            Planned::Installer(plan) => plan.environment(),
            Planned::Galaxy(plan) => plan.environment(),
        }
    }

    fn title(&self) -> &str {
        match &self.0 {
            Planned::Installer(plan) => plan.title(),
            Planned::Galaxy(plan) => plan.title(),
        }
    }

    fn gog_id(&self) -> u64 {
        match &self.0 {
            Planned::Installer(plan) => plan.gog_id(),
            Planned::Galaxy(plan) => plan.gog_id(),
        }
    }

    fn folder(&self) -> &std::path::Path {
        match &self.0 {
            Planned::Installer(plan) => plan.folder(),
            Planned::Galaxy(plan) => plan.folder(),
        }
    }

    /// Galaxy builds are only for Windows games.
    fn platform(&self) -> Platform {
        match &self.0 {
            Planned::Installer(plan) => plan.platform(),
            Planned::Galaxy(_) => Platform::Windows,
        }
    }

    fn resumes(&self) -> bool {
        match &self.0 {
            Planned::Installer(plan) => plan.resumes(),
            Planned::Galaxy(plan) => plan.resumes(),
        }
    }

    fn has_config(&self) -> bool {
        match &self.0 {
            Planned::Installer(plan) => plan.has_config(),
            Planned::Galaxy(plan) => plan.has_config(),
        }
    }

    fn creation(&self) -> Option<&Creation> {
        match &self.0 {
            Planned::Installer(plan) => plan.creation(),
            Planned::Galaxy(plan) => plan.creation(),
        }
    }

    fn requested_language(&self) -> &str {
        match &self.0 {
            Planned::Installer(plan) => plan.requested_language(),
            Planned::Galaxy(plan) => plan.requested_language(),
        }
    }

    fn warnings(&self) -> &[String] {
        match &self.0 {
            Planned::Installer(plan) => plan.warnings(),
            Planned::Galaxy(plan) => plan.warnings(),
        }
    }

    fn product_labels(&self) -> std::collections::HashMap<u64, String> {
        match &self.0 {
            Planned::Installer(plan) => plan.product_labels(),
            Planned::Galaxy(plan) => plan.product_labels(),
        }
    }

    fn product_label(&self, id: u64) -> String {
        match &self.0 {
            Planned::Installer(plan) => plan.product_label(id),
            Planned::Galaxy(plan) => plan
                .product_labels()
                .remove(&id)
                .unwrap_or_else(|| format!("GOG product {id}")),
        }
    }
}

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
    // clap lets this through beside --build, which conflicts with --branch.
    if args.password_stdin && args.branch.is_none() {
        return Ok(usage_error("--password-stdin needs --branch."));
    }
    // A branch or build is Galaxy's, so asking for one picks the track.
    let (choice, branch_flag) = match (&args.branch, &args.build) {
        (Some(branch), _) => (Choice::Branch(branch.clone()), Some("--branch")),
        (_, Some(build)) => (Choice::Build(build.clone()), Some("--build")),
        _ => (Choice::Keep, None),
    };
    if let (Some(flag), Some(GogTrack::Installer)) = (branch_flag, args.track) {
        return Ok(usage_error(format!(
            "{flag} is for Galaxy games, so leave out --track installer."
        )));
    }
    let track: Option<Track> = match (args.track, branch_flag) {
        (Some(track), _) => Some(track.into()),
        (None, Some(_)) => Some(Track::Galaxy),
        (None, None) => None,
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
        builds: choice,
        // Read before anything else, so a script feeding it isn't left waiting.
        password: if args.password_stdin {
            Some(password_from_stdin()?)
        } else {
            None
        },
    };
    let plan = match plan_asking(
        paths,
        &mut session,
        &mut request,
        track,
        unfinished,
        args.branch.is_some(),
    )? {
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
            (args.track.is_some(), "--track"),
            (args.branch.is_some(), "--branch"),
            (args.build.is_some(), "--build"),
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
                return stopped(paths, error, &plan, &plan);
            };
            match ask_wizard(prompt, &label) {
                Ok(true) => {}
                Ok(false) => return stopped(paths, error, &plan, &plan),
                Err(asked) if cancel::prompted(&asked) => {
                    eprintln!("Install canceled.");
                    say_finish(paths, plan.environment(), plan.gog_id());
                    return Ok(cancel::exit_code());
                }
                Err(asked) => return Err(asked),
            }
            // The first attempt left its journal, so this carries it on.
            request.wizard = true;
            let again = match plan_install(paths, &mut session, &request, None, true)? {
                Ok(again) => again,
                Err(code) => return Ok(code),
            };
            // A plan resumed from cached packages may only know the game
            // by its id, so the first plan names things.
            match apply(paths, &mut session, &again, &plan) {
                Ok(installed) => installed,
                Err(error) => return stopped(paths, error, &again, &plan),
            }
        }
    };
    report(paths, &plan, &installed)?;
    if plan.platform() == Platform::Windows && installed.target.is_none() {
        return pick_target(paths, &plan, prompt.as_mut());
    }
    Ok(ExitCode::SUCCESS)
}

/// `plan_install`, asking for a private branch's password once when the plan
/// needs one that wasn't given.
fn plan_asking(
    paths: &Paths,
    session: &mut Session,
    request: &mut Request,
    track: Option<Track>,
    unfinished: bool,
    branch_named: bool,
) -> anyhow::Result<Result<Plan, ExitCode>> {
    loop {
        match plan_install(paths, session, request, track, unfinished) {
            Err(error) if request.password.is_none() => match needs_password(&error) {
                Some(needed) => request.password = Some(ask_password(&needed, branch_named)?),
                None => return Err(error),
            },
            other => return other,
        }
    }
}

/// Plans the install, telling Ctrl+C apart: an unfinished install keeps its
/// journal, which `wineo update` finishes.
fn plan_install(
    paths: &Paths,
    session: &mut Session,
    request: &Request,
    track: Option<Track>,
    unfinished: bool,
) -> anyhow::Result<Result<Plan, ExitCode>> {
    let cancellation = Cancellation::new()?;
    #[cfg(debug_assertions)]
    pause_for_test(&cancellation.token);
    let planned = core_galaxy::plan(paths, session, request, track, &cancellation.token);
    match planned {
        // Ctrl+C while planning cancels whatever planning made of it.
        _ if cancellation.token.is_canceled() => {
            if unfinished {
                eprintln!("Install interrupted.");
                say_finish(paths, &request.name, request.gog_id);
            } else {
                eprintln!("Install canceled.");
            }
            Ok(Err(cancel::exit_code()))
        }
        Ok(planned) => Ok(Ok(Plan(planned))),
        Err(error) if is_cancel(&error) => {
            if unfinished {
                eprintln!("Install interrupted.");
                say_finish(paths, &request.name, request.gog_id);
            } else {
                eprintln!("Install canceled.");
            }
            Ok(Err(cancel::exit_code()))
        }
        // The caller asks for the password and plans again.
        Err(error) if needs_password(&error).is_some() => Err(error),
        // A resumed install that can't be planned still has its journal.
        Err(error) => Err(with_finish_hint(
            paths,
            error,
            &request.name,
            Some(request.gog_id),
            false,
        )),
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
    ];
    // Only a Windows game has a choice of tracks.
    if plan.platform() == Platform::Windows {
        let track = if plan.galaxy().is_some() {
            "Galaxy"
        } else {
            "installer"
        };
        rows.push(("track".to_owned(), track.to_owned()));
    }
    rows.push((
        "platform".to_owned(),
        platform_name(plan.platform()).to_owned(),
    ));
    rows.push(("language".to_owned(), plan.requested_language().to_owned()));
    if let Some(creation) = plan.creation() {
        rows.push(("runner".to_owned(), creation.runner().name().to_owned()));
        rows.push(("arch".to_owned(), creation.resolved_arch().to_string()));
    }
    match &plan.0 {
        Planned::Installer(offline) => installer_rows(offline, &mut rows),
        Planned::Galaxy(galaxy_plan) => galaxy::install_rows(paths, galaxy_plan, &mut rows),
    }
    let action = if plan.resumes() {
        "Finish the interrupted install of"
    } else {
        "Install"
    };
    let mut lines = vec![format!("{action} {} ({})", plan.title(), plan.gog_id())];
    lines.extend(table::aligned_rows(rows));
    lines
}

/// The rows of an offline installer install: versions, DLC, and sizes.
fn installer_rows(plan: &wineo_core::gog::update::install::Plan, rows: &mut Vec<(String, String)>) {
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
}

fn print_review(paths: &Paths, plan: &Plan) {
    for line in review_lines(paths, plan) {
        println!("{line}");
    }
    for warning in plan.warnings() {
        warn(warning);
    }
    let notes: Vec<&Option<String>> = match &plan.0 {
        Planned::Installer(offline) => offline.products().iter().map(|p| &p.note).collect(),
        Planned::Galaxy(galaxy_plan) => galaxy_plan.products().iter().map(|p| &p.note).collect(),
    };
    for note in notes.into_iter().flatten() {
        warn(note);
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
    let cancellation = Cancellation::new()?;
    let name = plan.environment();
    let offline = match &plan.0 {
        Planned::Installer(offline) => offline,
        Planned::Galaxy(galaxy_plan) => {
            let mut progress = galaxy::Reporter::new(std::time::Duration::from_secs(1));
            return core_galaxy::install::apply_with_events(
                paths,
                session,
                galaxy_plan,
                &cancellation.token,
                |step| {
                    if let Some(line) = progress.line(&step) {
                        eprintln!("{line}");
                    }
                    #[cfg(debug_assertions)]
                    galaxy::pause_for_test(name, &step, &cancellation.token);
                },
                |event| match event {
                    Event::Watched(watched) => {
                        if let Some(line) = watched_message(watched) {
                            eprintln!("{line}");
                        }
                    }
                    Event::Create(event) => created(paths, name, event),
                    Event::Installing { .. } => {}
                },
            );
        }
    };
    let progress = std::cell::RefCell::new(Reporter::new(names.product_labels()));
    wineo_core::gog::update::install::apply_with_events(
        paths,
        session,
        offline,
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
        CreateEvent::Warning(warning) => eprintln!("warning: {warning}"),
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
fn stopped(
    paths: &Paths,
    error: anyhow::Error,
    plan: &Plan,
    names: &Plan,
) -> anyhow::Result<ExitCode> {
    let name = plan.environment();
    let finish = || say_finish(paths, name, plan.gog_id());
    if let Some(UpdateError::Canceled { journal }) = error.downcast_ref() {
        if *journal {
            eprintln!("Install interrupted.");
            finish();
        } else {
            eprintln!("Install canceled.");
        }
        return Ok(cancel::exit_code());
    }
    // A setup step in the prefix failed. The journal stays, and installing
    // again carries on from that step.
    if let Some(core_galaxy::GalaxyError::Setup {
        title,
        label,
        detail,
    }) = error.downcast_ref()
    {
        eprintln!(
            "{}",
            style::error_stderr(format!(
                "error: Setting up {title} stopped: {label} failed ({detail})."
            ))
        );
        finish();
        return Ok(ExitCode::FAILURE);
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
            let label = names.product_label(*product);
            eprintln!(
                "{}",
                style::error_stderr(format!(
                    "error: {label}'s installer stopped before installing anything (exit code {code})."
                ))
            );
            finish();
            Ok(ExitCode::FAILURE)
        }
        // Whatever else stopped it, a journal still in the folder is there
        // to finish.
        _ if journal_for(paths, name, Some(plan.gog_id())).is_some() => {
            eprintln!("{}", style::error_stderr(format!("error: {error:#}")));
            finish();
            Ok(ExitCode::FAILURE)
        }
        _ => Err(error),
    }
}

/// How an install that stopped is finished, by its journal.
fn say_finish(paths: &Paths, name: &str, gog_id: u64) {
    let command = journal_for(paths, name, Some(gog_id)).map_or_else(
        || format!("wineo update {name}"),
        |journal| finish_command(&journal, name),
    );
    eprintln!("Finish the interrupted install with: {command}");
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
    for line in galaxy::restart_lines(&installed.needs_restart) {
        println!("{line}");
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
