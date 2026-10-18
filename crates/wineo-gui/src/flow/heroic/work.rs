//! The Heroic import's worker side: preparing, planning, applying, and copy progress.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Provider};
use wineo_core::heroic::{
    HeroicCandidate, HeroicGameKind, HeroicListing, HeroicSource, SettingsReview,
};
use wineo_core::heroic_import::{
    self, Game, LibraryOptions, Options, Playtime, Policy, Registered, RunnerCopy,
    RunnerSuggestion, Warning,
};
use wineo_core::import;
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;

use crate::flow::posters;
use crate::runner_label::{self, Choice};

use super::{BatchOptions, Draft, Prepared, Refreshed, Review};

pub(super) fn prepare_game(
    paths: &Paths,
    source: &HeroicSource,
    candidate: HeroicCandidate,
) -> Result<Prepared, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let (runners, names) = runner_choices(paths, &global, true)?;
    let (
        suggested,
        runner,
        raw_arguments,
        working_directory,
        environment,
        gamemode,
        wayland,
        mangohud,
        gamescope,
        hook_paths,
        hdr,
    ) = if candidate.kind == HeroicGameKind::Windows {
        let imported = source.inspect(&candidate.id).map_err(|e| e.to_string())?;
        let (suggestion, runner) = suggest(paths, &global, &runners, &names, &imported)?;
        let hook_paths = hook_paths(&imported.settings_review);
        (
            Some(suggestion),
            runner,
            imported.launcher_args,
            imported
                .cwd
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            imported
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            imported.gamemode,
            Some(imported.enable_wine_wayland),
            imported.mangohud,
            imported.gamescope,
            hook_paths,
            imported.enable_hdr,
        )
    } else {
        let imported = source
            .inspect_native(&candidate.id)
            .map_err(|e| e.to_string())?;
        let hook_paths = hook_paths(&imported.settings_review);
        (
            None,
            String::new(),
            imported.launcher_args,
            String::new(),
            imported
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            imported.gamemode,
            None,
            imported.mangohud,
            imported.gamescope,
            hook_paths,
            false,
        )
    };
    let (arguments, unparsed_arguments) =
        match wineo_core::heroic::parse_launcher_args(&raw_arguments) {
            Ok(arguments) => (arguments, None),
            Err(_) => (Vec::new(), Some(raw_arguments)),
        };
    Ok(Prepared {
        candidate,
        runners: names,
        suggested,
        runner,
        offer: runner_label::offer(paths, &global),
        arguments,
        unparsed_arguments,
        working_directory,
        environment,
        gamemode,
        wayland,
        mangohud,
        gamescope,
        hook_paths,
        hdr,
    })
}

/// Installed runners as the pickers show them, with the system wine first
/// when it's wanted and there is one.
pub(crate) fn runner_choices(
    paths: &Paths,
    global: &GlobalConfig,
    system: bool,
) -> Result<(Runners, Vec<Choice>), String> {
    let runners = Runners::new(paths, global).map_err(|e| e.to_string())?;
    let mut choices: Vec<Choice> = runners
        .list()
        .map_err(|e| e.to_string())?
        .runners
        .iter()
        .map(|runner| Choice::of(paths, runner))
        .collect();
    if system && runners.resolve("wine").is_ok() {
        choices.insert(0, Choice::plain("wine"));
    }
    Ok((runners, choices))
}

/// What to do about a Windows game's runner, and the runner that starts out
/// picked for it.
pub(crate) fn suggest(
    paths: &Paths,
    global: &GlobalConfig,
    runners: &Runners,
    choices: &[Choice],
    imported: &wineo_core::heroic::HeroicImport,
) -> Result<(RunnerSuggestion, String), String> {
    let suggestion =
        heroic_import::suggest_runner(paths, global, imported, None).map_err(|e| e.to_string())?;
    let runner = match &suggestion {
        RunnerSuggestion::Use(name) => name.clone(),
        RunnerSuggestion::Copy(_) => String::new(),
        RunnerSuggestion::Choose { proton, .. } => choices
            .iter()
            .find(|choice| {
                runners
                    .resolve(&choice.name)
                    .is_ok_and(|runner| runner.is_proton() == *proton)
            })
            .map(|choice| choice.name.clone())
            .unwrap_or_default(),
    };
    Ok((suggestion, runner))
}

pub(super) fn batch_options(paths: &Paths) -> Result<BatchOptions, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let (runners, choices) = runner_choices(paths, &global, false)?;
    let runner = runners
        .newest_proton()
        .map_err(|e| e.to_string())?
        .map(|runner| runner.selection().to_owned())
        .unwrap_or_default();
    Ok(BatchOptions {
        runner,
        runners: choices,
        art_fetch_enabled: !global.art.fetch.kinds().is_empty(),
        offer: runner_label::offer(paths, &global),
    })
}

pub(super) fn refresh(
    paths: &Paths,
    single: Option<(HeroicSource, HeroicCandidate)>,
) -> Result<Refreshed, String> {
    let Some((source, candidate)) = single else {
        return batch_options(paths).map(Refreshed::Batch);
    };
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let (runners, choices) = runner_choices(paths, &global, true)?;
    let imported = source.inspect(&candidate.id).map_err(|e| e.to_string())?;
    let (suggested, runner) = suggest(paths, &global, &runners, &choices, &imported)?;
    Ok(Refreshed::Single {
        runners: choices,
        suggested: Some(suggested),
        runner,
    })
}

/// The review's runner line, naming the launcher when the build isn't
/// wineo's, like `Runner: GE-Proton-latest (Heroic)`.
pub(super) fn runner_line(paths: &Paths, runner: &wineo_core::runner::Runner) -> String {
    format!(
        "Runner: {}",
        runner_label::labeled(runner.selection(), runner_label::launcher(paths, runner))
    )
}

pub(super) fn quote_args(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| format!("'{}'", argument.replace('\'', "'\"'\"'")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn plan_review(
    paths: &Paths,
    listing: &HeroicListing,
    selected: Option<usize>,
    draft: &Draft,
) -> Result<Review, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    if draft.desktop {
        wineo_core::desktop::supported(paths).map_err(|e| e.to_string())?;
    }
    if draft.batch {
        let has_windows = listing.candidates.iter().any(|candidate| {
            draft.games.contains(&candidate.id) && candidate.kind == HeroicGameKind::Windows
        });
        let runner = if has_windows {
            if draft.runner.trim().is_empty() {
                return Err("Choose or download a runner for Windows games".to_owned());
            }
            Some(draft.runner.as_str())
        } else {
            None
        };
        let mut library = heroic_import::plan_selection(
            paths,
            &global,
            listing,
            &draft.games,
            runner,
            &LibraryOptions {
                adopt: draft.adopt,
                arch: None,
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        library.recheck(paths, &global);
        return Ok(Review::Batch(library));
    }
    let candidate = selected
        .and_then(|index| listing.candidates.get(index))
        .ok_or("Choose a Heroic game")?;
    if draft.name.trim().is_empty() {
        return Err("Enter an environment name".to_owned());
    }
    if let Some(problem) = wineo_core::name::problem(&draft.name) {
        return Err(format!("Invalid environment name: {problem}"));
    }
    if paths.environment_config(&draft.name).exists() {
        return Err(format!("{} already exists", draft.name));
    }
    let source = &listing.source;
    let mut missing_build = None;
    let game = if candidate.kind == HeroicGameKind::Native {
        let mut plan = heroic_import::plan_native(source, &candidate.id, Some(draft.name.clone()))
            .map_err(|e| e.to_string())?;
        plan.set_executable(PathBuf::from(&draft.executable))
            .map_err(|e| e.to_string())?;
        plan.set_launcher_args(&quote_args(&draft.arguments))
            .map_err(|e| e.to_string())?;
        plan.set_working_directory(
            (!draft.working_directory.trim().is_empty())
                .then(|| PathBuf::from(&draft.working_directory)),
        );
        if draft.environment_dirty {
            plan.set_environment(&draft.environment)
                .map_err(|e| e.to_string())?;
        }
        if draft.gamemode_dirty {
            plan.set_gamemode(draft.gamemode);
        }
        if draft.mangohud_dirty {
            plan.set_mangohud(draft.mangohud);
        }
        if draft.gamescope.dirty {
            plan.set_gamescope(draft.gamescope.resolved()?);
        }
        plan.set_desktop_enabled(draft.desktop);
        plan.reject_invalid().map_err(|e| e.to_string())?;
        plan.check_exe(paths, &global).map_err(|e| e.to_string())?;
        Game::Native(plan)
    } else {
        if draft.runner.trim().is_empty() {
            return Err("Choose or download a runner".to_owned());
        }
        let mut plan = heroic_import::plan_game(
            paths,
            &global,
            source,
            &candidate.id,
            &draft.runner,
            Options {
                name: Some(draft.name.clone()),
                adopt: draft.adopt,
                ..Options::default()
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        let exe = plan
            .resolve_path(paths, &draft.executable)
            .map_err(|e| e.to_string())?;
        plan.set_executable(exe).map_err(|e| e.to_string())?;
        let cwd = if draft.working_directory.trim().is_empty() {
            None
        } else {
            Some(
                plan.resolve_path(paths, &draft.working_directory)
                    .map_err(|e| e.to_string())?,
            )
        };
        plan.set_working_directory(cwd);
        plan.set_launcher_args(&quote_args(&draft.arguments))
            .map_err(|e| e.to_string())?;
        if draft.environment_dirty {
            let mut entries = draft.environment.clone();
            let hdr_key = if plan.runner().is_proton() {
                "PROTON_ENABLE_HDR"
            } else {
                "DXVK_HDR"
            };
            if plan.resolved().enable_hdr
                && !entries
                    .iter()
                    .any(|entry| entry.starts_with(&format!("{hdr_key}=")))
                && let Some(value) = plan.resolved().env.get(hdr_key)
            {
                entries.push(format!("{hdr_key}={value}"));
            }
            plan.set_environment(&entries).map_err(|e| e.to_string())?;
        }
        if draft.gamemode_dirty {
            plan.set_gamemode(draft.gamemode);
        }
        if draft.mangohud_dirty {
            plan.set_mangohud(draft.mangohud);
        }
        if draft.gamescope.dirty {
            plan.set_gamescope(draft.gamescope.resolved()?);
        }
        if draft.wayland_dirty {
            plan.set_wayland(draft.wayland).map_err(|e| e.to_string())?;
        }
        if draft.hdr_dirty {
            plan.set_hdr(draft.hdr).map_err(|e| e.to_string())?;
        }
        plan.set_desktop_enabled(draft.desktop);
        plan.resolve(Policy::Defaults);
        if let Some(finding) = plan.blocking().next() {
            return Err(finding.issue.message.clone());
        }
        missing_build = plan
            .missing_build(paths, &global)
            .map_err(|e| e.to_string())?
            .map(|missing| (missing.component, missing.version));
        Game::Windows(plan)
    };
    let runner_line = match &game {
        Game::Windows(plan) => Some(runner_line(paths, plan.runner())),
        Game::Native(_) => None,
    };
    Ok(Review::Single {
        game: Box::new(game),
        missing_build,
        runner_line,
    })
}

pub(super) fn apply_game(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    game: &Game,
    token: &Token,
    mut progress: impl FnMut(import::ImportProgress),
) -> anyhow::Result<Registered> {
    if token.is_canceled() {
        anyhow::bail!("Import canceled");
    }
    match game {
        Game::Windows(plan) => {
            let mut warnings = Vec::new();
            let mut registered =
                heroic_import::apply(paths, global, source, plan, token, |event| match event {
                    import::ImportProgress::Warning(warning) => warnings.push(warning),
                    event => progress(event),
                })?;
            registered.warnings.extend(warnings);
            Ok(registered)
        }
        Game::Native(plan) => heroic_import::apply_native(paths, source, plan),
    }
}

pub(super) fn heroic_id(game: &Game) -> &str {
    match game {
        Game::Windows(plan) => &plan.heroic().id,
        Game::Native(plan) => &plan.heroic().id,
    }
}

/// A Heroic poster a batch may store, and whether the grid decoded it.
#[derive(Debug, Clone)]
pub(super) struct BatchPoster {
    pub(super) path: PathBuf,
    pub(super) decoded: bool,
}

/// Stores Heroic's cached poster as a batch-imported environment's Grid art.
/// It runs before SteamGridDB, which then only fills what's still missing. Art
/// never undoes an import, so a failure is only a note. Runs on a worker,
/// since a poster the grid hadn't decoded yet is decoded here.
pub(super) fn store_heroic_poster(
    paths: &Paths,
    name: &str,
    app: &str,
    poster: Option<&BatchPoster>,
) -> Option<String> {
    let poster = poster?;
    // Core only checks the signature, so the image has to decode first.
    if !poster.decoded
        && !posters::decode_waiting(&poster.path, &paths.art_cache_dir(), None)
            .is_some_and(|decoded| decoded.is_ok())
    {
        return None;
    }
    heroic_import::store_poster(paths, name, app, &poster.path)
        .err()
        .map(|error| format!("Heroic poster: {error}"))
}

/// How often copy progress reaches the UI. Core reports every copied file,
/// and a message per file floods the channel on a large prefix.
pub(super) const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Turns core's import progress into a readable phase, throttled.
#[derive(Debug, Default)]
pub(super) struct CopyProgress {
    total: u64,
    copied: u64,
    sent: Option<Instant>,
}

impl CopyProgress {
    /// The phase to show for `progress`, or None when it's too soon after
    /// the last one.
    pub(super) fn record(
        &mut self,
        progress: &import::ImportProgress,
        now: Instant,
    ) -> Option<String> {
        self.due(progress, now).then(|| self.phase(progress))
    }

    /// Counts `progress` and says whether it should show. The last copied
    /// file always shows.
    fn due(&mut self, progress: &import::ImportProgress, now: Instant) -> bool {
        if let import::ImportProgress::Started { entries } = progress {
            self.total = *entries;
            self.copied = 0;
        }
        if let import::ImportProgress::Copied { .. } = progress {
            self.copied += 1;
            let due = self.copied >= self.total
                || self
                    .sent
                    .is_none_or(|sent| now.duration_since(sent) >= PROGRESS_INTERVAL);
            if !due {
                return false;
            }
        }
        self.sent = Some(now);
        true
    }

    fn counts(&self) -> String {
        format!("{} of {} files", grouped(self.copied), grouped(self.total))
    }

    fn phase(&self, progress: &import::ImportProgress) -> String {
        match progress {
            import::ImportProgress::Started { .. } | import::ImportProgress::Copied { .. } => {
                format!("copying prefix ({})", self.counts())
            }
            import::ImportProgress::Checking { entries } => {
                format!("checking prefix ({} files)", grouped(*entries))
            }
            import::ImportProgress::Finished { .. } => "finishing".to_owned(),
            import::ImportProgress::Warning(warning) => warning.clone(),
        }
    }
}

/// "(2 of 5) Importing Title: phase", without the position for a single game
/// and with an ellipsis before the first phase.
pub(super) fn import_status(
    title: &str,
    position: Option<(usize, usize)>,
    phase: Option<&str>,
) -> String {
    let position = position
        .map(|(index, count)| format!("({index} of {count}) "))
        .unwrap_or_default();
    match phase {
        Some(phase) => format!("{position}Importing {title}: {phase}"),
        None => format!("{position}Importing {title}..."),
    }
}

/// `n` with comma thousands separators.
pub(super) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub(super) fn hook_paths(review: &SettingsReview) -> Vec<(&'static str, String)> {
    review
        .hooks()
        .into_iter()
        .map(|(field, path)| {
            let label = if field == "before hook" {
                "Before hook"
            } else {
                "After hook"
            };
            (label, path)
        })
        .collect()
}

/// The review only calls out anti-cheat, once even when both EAC and
/// BattlEye are on, since the game may not run without it.
pub(crate) fn review_warnings(warnings: &[Warning]) -> Vec<&'static str> {
    if warnings.contains(&Warning::AntiCheatNotImported) {
        vec!["Heroic-managed anti-cheat runtime is not imported"]
    } else {
        Vec::new()
    }
}

pub(crate) fn playtime_note(playtime: &Playtime) -> Option<String> {
    match playtime {
        Playtime::None | Playtime::Imported => None,
        Playtime::Unreadable(reason) => {
            Some(format!("Heroic playtime could not be read: {reason}"))
        }
        Playtime::NotRecorded(reason) => {
            Some(format!("Heroic playtime could not be recorded: {reason}"))
        }
    }
}

#[derive(Clone)]
pub(crate) enum Prerequisite {
    CopyRunner(RunnerCopy),
    Download(Provider, Option<String>),
}

impl Prerequisite {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::CopyRunner(copy) => format!("Copying Heroic runner {}...", copy.name),
            Self::Download(provider, _) => format!("Downloading {provider}..."),
        }
    }
}

pub(crate) fn install_prerequisite(
    paths: &Paths,
    operation: Prerequisite,
    token: &Token,
    report: &dyn Fn(String),
) -> Result<String, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    match operation {
        Prerequisite::CopyRunner(copy) => {
            let root = global.runner_root(paths).map_err(|e| e.to_string())?;
            let _source_guard = copy
                .lock_source(paths, &global)
                .map_err(|e| e.to_string())?;
            let mut files = CopyProgress::default();
            let receipt = import::copy_runner_reviewed(
                &copy.inspection,
                root,
                &copy.name,
                token,
                |progress| {
                    if files.due(&progress, Instant::now()) {
                        let line = match progress {
                            import::ImportProgress::Started { .. }
                            | import::ImportProgress::Copied { .. } => {
                                format!("Copying {} ({})", copy.name, files.counts())
                            }
                            import::ImportProgress::Checking { entries } => {
                                format!("Checking {} ({} files)", copy.name, grouped(entries))
                            }
                            import::ImportProgress::Finished { .. } => {
                                format!("Finishing copy of {}", copy.name)
                            }
                            import::ImportProgress::Warning(warning) => warning,
                        };
                        report(line);
                    }
                },
            )
            .map_err(|e| format!("{e:#}"))?;
            Ok(receipt.name)
        }
        Prerequisite::Download(provider, version) => {
            let client = Client::new().map_err(|e| e.to_string())?;
            let release = match version {
                Some(name) => client.select_pinned(provider, &name),
                None => client.select(provider, None),
            }
            .map_err(|e| format!("{e:#}"))?;
            crate::download_progress::install(
                &client,
                paths,
                &global,
                &release,
                token,
                PROGRESS_INTERVAL,
                report,
            )
            .map_err(|e| format!("{e:#}"))?;
            Ok(release.installed_name)
        }
    }
}
