use std::cell::OnceCell;
use std::collections::HashMap;

use jiff::Timestamp;
use serde::Serialize;

use crate::components::{Builds, Component, LATEST};
use crate::config::{GlobalConfig, Kind, Launcher};
use crate::desktop;
use crate::environment::{self, Environment};
use crate::error::{IoContext, Result};
use crate::install::{self, Wanted};
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::runners::Runners;
use crate::runtime::{RunStatus, Runtime, State, Status};
use crate::stats;
use crate::time::LocalZone;
use crate::winetricks;

/// Everything `wineo status <name>` shows, gathered here so `--json` stays
/// cheap to add later.
#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentReport {
    pub name: String,
    pub tags: Vec<String>,
    pub winetricks: Vec<String>,
    pub winetricks_missing: Vec<String>,
    pub status: Status,
    pub kind: KindReport,
    /// None for a native or proxied environment, which has no runner or prefix.
    pub runner: Option<String>,
    pub runner_resolved: Option<String>,
    pub runner_version: Option<String>,
    /// The prefix as stored in the config, e.g. `~/.local/share/wineo/prefixes/notepad`.
    pub prefix: Option<String>,
    /// The exe of the run's target in the current config. None if there's no
    /// run, or its target was removed from the config since.
    pub target_exe: Option<String>,
    /// An operation holding the environment right now, like a winetricks install.
    pub busy: Option<Operation>,
    /// An installer never confirmed that everything it started had stopped,
    /// so other work is refused until `wineo stop` cleans up.
    pub unresolved: bool,
    /// Whether the last run's target, or the environment itself, asks for gamemode.
    pub gamemode: bool,
    /// Whether the same target, or the environment, asks for these.
    pub gamescope: bool,
    #[serde(skip)]
    pub hooks: Option<crate::config::Hooks>,
    pub mangohud: bool,
    /// Whether Wine's Wayland driver is asked for. Always false for native.
    pub wayland: bool,
    /// What umu is told to look protonfixes up by, when this environment asked
    /// for them.
    pub gameid: Option<String>,
    pub store: Option<String>,
    /// The Direct3D builds this environment asks for, when it asks for any.
    pub dxvk: Option<ComponentReport>,
    pub vkd3d: Option<ComponentReport>,
    /// Targets that have a desktop entry on disk right now.
    pub desktop: Vec<String>,
    /// None if the environment has never been played.
    pub played: Option<Played>,
}

/// What an environment runs on, for `status` and `list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum KindReport {
    Wine,
    Native,
    Proxied {
        launcher: Launcher,
        app_id: String,
        title: Option<String>,
        install: InstallReport,
    },
}

impl KindReport {
    pub fn new(kind: &Kind) -> Self {
        match kind {
            Kind::Wine(_) => Self::Wine,
            Kind::Native => Self::Native,
            Kind::Proxied(proxied) => Self::Proxied {
                launcher: proxied.launcher,
                app_id: proxied.app_id.clone(),
                title: proxied.title.clone(),
                install: InstallReport::from(crate::steam::AppState::Unknown(
                    "Steam isn't found".to_owned(),
                )),
            },
        }
    }

    /// The `kind` value as written in a config.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wine => "wine",
            Self::Native => "native",
            Self::Proxied { .. } => "proxied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallReport {
    pub state: &'static str,
    pub problem: Option<String>,
    /// How far an install is, from 0 to 100. None when there's no percent or
    /// the game isn't installing.
    pub progress: Option<u8>,
}

impl InstallReport {
    pub fn display(&self) -> &'static str {
        match self.state {
            "not_installed" => "not installed",
            "install_queued" => "install queued",
            other => other,
        }
    }

    /// `display` with the percent, like `installing 42%`.
    pub fn label(&self) -> String {
        match (self.state, self.progress) {
            ("installing", Some(percent)) => format!("installing {percent}%"),
            _ => self.display().to_owned(),
        }
    }
}

impl From<crate::steam::AppState> for InstallReport {
    fn from(state: crate::steam::AppState) -> Self {
        use crate::steam::AppState;
        let mut progress = None;
        let (state, problem) = match state {
            AppState::Installed => ("installed", None),
            AppState::Updating => ("updating", None),
            AppState::NotInstalled => ("not_installed", None),
            AppState::Installing(percent) => {
                progress = percent;
                ("installing", None)
            }
            AppState::InstallQueued => ("install_queued", None),
            AppState::Uninstalling => ("uninstalling", None),
            AppState::Unavailable(path) => (
                "unavailable",
                Some(format!(
                    "Steam's library at {} isn't available",
                    path.display()
                )),
            ),
            AppState::Unknown(reason) => ("unknown", Some(reason)),
        };
        Self {
            state,
            problem,
            progress,
        }
    }
}

fn kind_report(paths: &Paths, global: &GlobalConfig, kind: &Kind) -> KindReport {
    let steam = matches!(kind, Kind::Proxied(_))
        .then(|| steam_overview(crate::steam::active_for(paths, global)));
    kind_report_with_steam(kind, steam.as_ref())
}

fn kind_report_with_steam(kind: &Kind, steam: Option<&SteamOverview>) -> KindReport {
    let mut report = KindReport::new(kind);
    if let KindReport::Proxied {
        app_id,
        title,
        install,
        ..
    } = &mut report
    {
        let active = steam.and_then(|steam| steam.install.as_ref());
        *install = crate::steam::app_state(active, app_id).into();
        if let Some(manifest) = steam
            .into_iter()
            .flat_map(|steam| &steam.manifests)
            .find(|entry| entry.app_id == *app_id)
            .and_then(|entry| entry.manifest.as_ref().ok())
        {
            *title = Some(manifest.name.clone());
        }
    }
    report
}

fn steam_overview(install: Option<crate::steam::Install>) -> SteamOverview {
    let manifests = install
        .as_ref()
        .map(crate::steam::manifests)
        .unwrap_or_default();
    SteamOverview { install, manifests }
}

/// The little of `stats::Summary` that `status` shows: enough to say how much
/// was played and when, without the per-target and longest-session detail
/// `wineo stats` has room for.
#[derive(Debug, Clone, Serialize)]
pub struct Played {
    pub seconds: i64,
    pub sessions: usize,
    pub last_played: Option<Timestamp>,
    #[serde(skip)]
    pub source: stats::Source,
    #[serde(skip)]
    pub playtime_unknown: bool,
}

/// What `wineo list` and `wineo status` without a name need from every
/// environment.
#[derive(Debug, Clone)]
pub struct OverviewEnvironment {
    pub name: String,
    /// None when the config can't be read.
    pub kind: Option<KindReport>,
    pub status: Status,
    pub busy: Option<Operation>,
    pub unresolved: bool,
    pub tags: Vec<String>,
    pub hooks: Option<crate::view::HooksOutput>,
}

/// An environment that could not be included in an overview.
#[derive(Debug, Clone)]
pub struct OverviewError {
    pub name: String,
    pub message: String,
}

/// Environments and per-environment errors from an overview query.
#[derive(Debug, Default, Clone)]
pub struct Overview {
    pub environments: Vec<OverviewEnvironment>,
    pub errors: Vec<OverviewError>,
    /// Steam data shared by proxied environments in this overview.
    pub steam: Option<SteamOverview>,
}

/// The active Steam install and its manifests, scanned at most once per overview.
#[derive(Debug, Clone)]
pub struct SteamOverview {
    pub install: Option<crate::steam::Install>,
    pub manifests: Vec<crate::steam::ManifestEntry>,
}

/// Optional filters shared by the list and running-status queries.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverviewFilter<'a> {
    pub tags: Option<&'a [String]>,
    pub running_only: bool,
}

/// What a config asks for against what its prefix actually has.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentReport {
    /// The config value, which may be `latest`.
    pub wanted: String,
    /// What `wanted` names right now. None when nothing is installed to pick.
    pub resolved: Option<String>,
    /// What the prefix's manifest records.
    pub installed: Option<String>,
    /// Whether the prefix already has what the config asks for.
    pub current: bool,
}

impl EnvironmentReport {
    pub fn load(paths: &Paths, global: &GlobalConfig, environment: &Environment) -> Result<Self> {
        Self::load_with_warnings(paths, global, environment).0
    }

    pub fn load_with_warnings(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
    ) -> (Result<Self>, Vec<String>) {
        let mut warnings = Vec::new();
        let report = Self::load_inner(paths, global, environment, &mut warnings);
        (report, warnings)
    }

    fn load_inner(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        warnings: &mut Vec<String>,
    ) -> Result<Self> {
        let busy = Locks::new(paths).busy_with(&environment.name)?;
        let app_ids = match &environment.config.kind {
            Kind::Proxied(proxied) => vec![proxied.app_id.clone()],
            Kind::Wine(_) | Kind::Native => Vec::new(),
        };
        let probe = SteamProbe::new(paths, Some(global), app_ids, &crate::steam::reapers_by_app);
        let status = status(paths, &environment.name, busy, Some(environment), &probe)?;
        let wine = environment.config.wine();
        // An unresolvable runner is worth showing the rest of the status for.
        let operation_runner = Runtime::new(paths, &environment.name)
            .read_operation()
            .ok()
            .flatten()
            .map(|operation| operation.runner);
        let resolved = operation_runner
            .or_else(|| status.run.as_ref().and_then(|run| run.runner.clone()))
            .or_else(|| {
                let wine = wine?;
                Runners::new(paths, global)
                    .and_then(|runners| runners.resolve(&wine.runner))
                    .ok()
            });
        let runner_version = resolved.as_ref().and_then(|runner| runner.version());
        let runner_resolved = resolved.as_ref().map(|runner| runner.name().to_owned());
        let winetricks_missing = if environment.config.winetricks.is_empty() {
            Vec::new()
        } else {
            match resolved.as_ref().zip(environment.prefix(paths).ok()) {
                Some((runner, prefix)) => {
                    match winetricks::missing_for(runner, &prefix, &environment.config.winetricks) {
                        Ok(missing) => missing,
                        Err(err) => {
                            warnings.push(format!(
                                "warning: cannot read winetricks log for {}: {err}",
                                environment.name
                            ));
                            environment.config.winetricks.clone()
                        }
                    }
                }
                None => environment.config.winetricks.clone(),
            }
        };
        let runner_name = wine.map(|wine| match &resolved {
            Some(runner) if runner.name() != wine.runner => {
                format!("{} -> {}", wine.runner, runner.name())
            }
            _ => wine.runner.clone(),
        });
        let target = status
            .run
            .as_ref()
            .and_then(|run| environment.config.targets.get(&run.target));
        let target_exe = target.map(|target| target.exe.clone());
        let components = components(paths, global, environment);
        let (summary, _) = stats::summary(
            paths,
            &environment.name,
            Timestamp::now(),
            &LocalZone::new(),
        )?;
        let played = (summary.source == stats::Source::Steam
            || summary.sessions > 0
            || summary.imported_seconds > 0)
            .then_some(Played {
                seconds: summary.seconds,
                sessions: summary.sessions,
                last_played: summary.last_played,
                source: summary.source,
                playtime_unknown: summary.playtime_unknown,
            });
        Ok(Self {
            dxvk: components.0,
            vkd3d: components.1,
            unresolved: busy.is_none() && unresolved(paths, &environment.name),
            busy,
            gamemode: !matches!(environment.config.kind, Kind::Proxied(_))
                && crate::launch::gamemode(global, environment, target),
            gamescope: !matches!(environment.config.kind, Kind::Proxied(_))
                && crate::launch::gamescope(environment, target).is_some(),
            hooks: crate::launch::hooks(environment, target).cloned(),
            mangohud: !matches!(environment.config.kind, Kind::Proxied(_))
                && crate::launch::mangohud(global, environment, target),
            wayland: !matches!(environment.config.kind, Kind::Proxied(_))
                && crate::launch::wayland(global, environment, target),
            name: environment.name.clone(),
            tags: environment.config.tags.clone(),
            winetricks: environment.config.winetricks.clone(),
            winetricks_missing,
            kind: kind_report(paths, global, &environment.config.kind),
            runner: runner_name,
            runner_resolved,
            runner_version,
            prefix: wine.map(|wine| paths.shown(&wine.prefix)),
            gameid: wine.and_then(|wine| wine.gameid.clone()),
            store: wine.and_then(|wine| wine.store.clone()),
            target_exe,
            status,
            desktop: desktop::supported(paths)
                .and_then(|()| desktop::list(paths))
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry.environment == environment.name)
                .map(|entry| entry.target)
                .collect(),
            played,
        })
    }
}

/// Whether an installer left a record behind without anything holding the
/// environment. Only a momentary answer, like `busy`.
pub fn unresolved(paths: &Paths, name: &str) -> bool {
    Runtime::new(paths, name).check_resolved(name).is_err()
}

/// Reapers for several apps from one Steam install, or the failure to find them.
type ReaperScan<'a> =
    dyn Fn(&crate::steam::Install, &[String]) -> std::io::Result<ReapersByApp> + 'a;
type ReapersByApp = HashMap<String, Vec<crate::steam::Reaper>>;

/// What `status` needs to know about Steam. Finding the install and sweeping
/// the process table cost far more than a single environment's share, so both
/// happen at most once, on the first environment that needs them, for every
/// proxied environment in the batch.
struct SteamProbe<'a> {
    paths: &'a Paths,
    global: Option<&'a GlobalConfig>,
    app_ids: Vec<String>,
    scan: &'a ReaperScan<'a>,
    install: OnceCell<Option<crate::steam::Install>>,
    // io::Error can't be cloned, and every environment that asks after a
    // failed scan should report it.
    reapers: OnceCell<std::result::Result<ReapersByApp, (std::io::ErrorKind, String)>>,
}

impl<'a> SteamProbe<'a> {
    fn new(
        paths: &'a Paths,
        global: Option<&'a GlobalConfig>,
        app_ids: Vec<String>,
        scan: &'a ReaperScan<'a>,
    ) -> Self {
        Self {
            paths,
            global,
            app_ids,
            scan,
            install: OnceCell::new(),
            reapers: OnceCell::new(),
        }
    }

    /// None when the global config can't be read or Steam isn't installed.
    fn install(&self) -> Option<&crate::steam::Install> {
        self.install
            .get_or_init(|| {
                self.global
                    .and_then(|global| crate::steam::active_for(self.paths, global))
            })
            .as_ref()
    }

    fn game_running(&self, install: &crate::steam::Install, app_id: &str) -> std::io::Result<bool> {
        let reapers = self.reapers.get_or_init(|| {
            (self.scan)(install, &self.app_ids).map_err(|error| (error.kind(), error.to_string()))
        });
        match reapers {
            Ok(reapers) => Ok(reapers.get(app_id).is_some_and(|found| !found.is_empty())),
            Err((kind, message)) => Err(std::io::Error::new(*kind, message.clone())),
        }
    }
}

fn status(
    paths: &Paths,
    name: &str,
    busy: Option<Operation>,
    environment: Option<&Environment>,
    steam: &SteamProbe<'_>,
) -> Result<Status> {
    let runtime = Runtime::new(paths, name);
    let mut status = runtime.status()?;
    let proxied = environment.and_then(|environment| match &environment.config.kind {
        Kind::Proxied(config) => Some(config),
        Kind::Wine(_) | Kind::Native => None,
    });
    if proxied.is_some() && status.lost {
        runtime.clean_up_lost_run(std::time::Duration::ZERO, &paths.stats_log(name))?;
        status = runtime.status()?;
    }
    if busy == Some(Operation::Hook(crate::hooks::Phase::After))
        && let Some(run) = runtime.read_state()?
    {
        let state = match run.status {
            RunStatus::Exited => State::Exited,
            RunStatus::Failed => State::Failed,
            RunStatus::Stopped => State::Stopped,
            RunStatus::Running => return Ok(status),
        };
        return Ok(Status {
            state,
            run: Some(run),
            lost: false,
        });
    }
    if let Some(proxied) = proxied
        && busy.is_none()
        && !matches!(status.state, State::Running | State::Starting)
        && let Some(install) = steam.install()
        && steam
            .game_running(install, &proxied.app_id)
            .context(|| format!("failed to scan Steam processes for {name}"))?
    {
        status.state = State::SteamRunning;
        status.run = None;
    }
    Ok(status)
}

/// Reads the state, lock, installer record, and tags needed for every
/// environment shown by `list` or `status` without a name.
pub fn overview(paths: &Paths, filter: OverviewFilter<'_>) -> Result<Overview> {
    overview_with(paths, filter, &crate::steam::reapers_by_app)
}

/// `overview` against a trash scan the caller also uses for its own loads of
/// the same environments, so one batch reads the trash once.
pub fn overview_with_pending(
    paths: &Paths,
    filter: OverviewFilter<'_>,
    pending: &crate::trash::PendingScan,
) -> Result<Overview> {
    overview_from(paths, filter, &crate::steam::reapers_by_app, Some(pending))
}

fn overview_with(paths: &Paths, filter: OverviewFilter<'_>, scan: &ReaperScan) -> Result<Overview> {
    overview_from(paths, filter, scan, None)
}

fn overview_from(
    paths: &Paths,
    filter: OverviewFilter<'_>,
    scan: &ReaperScan,
    pending: Option<&crate::trash::PendingScan>,
) -> Result<Overview> {
    let locks = Locks::new(paths);
    let global = GlobalConfig::load(paths).ok();
    let mut overview = Overview::default();
    let names = environment::names(paths)?;
    // Nothing here trashes or restores, so one trash scan serves every load.
    let own;
    let pending = match pending {
        Some(pending) => pending,
        None => {
            own = crate::trash::PendingScan::new(paths);
            &own
        }
    };
    // Loaded up front so one process sweep can look for every Steam game.
    let environments: Vec<_> = names
        .into_iter()
        .map(|name| {
            let environment = Environment::load_with_pending(paths, &name, pending);
            (name, environment)
        })
        .collect();
    let mut app_ids: Vec<String> = environments
        .iter()
        .filter_map(
            |(_, environment)| match &environment.as_ref().ok()?.config.kind {
                Kind::Proxied(proxied) => Some(proxied.app_id.clone()),
                Kind::Wine(_) | Kind::Native => None,
            },
        )
        .collect();
    app_ids.sort();
    app_ids.dedup();
    let probe = SteamProbe::new(paths, global.as_ref(), app_ids, scan);
    for (name, environment) in environments {
        let busy = locks.busy_with(&name).unwrap_or(None);
        let current = status(paths, &name, busy, environment.as_ref().ok(), &probe);
        let unresolved = busy.is_none() && unresolved(paths, &name);
        match current {
            Ok(status)
                if !filter.running_only
                    || matches!(
                        status.state,
                        State::Running | State::Starting | State::SteamRunning
                    )
                    || busy.is_some()
                    || unresolved =>
            {
                match environment {
                    Ok(environment) => {
                        let tags = &environment.config.tags;
                        if filter
                            .tags
                            .is_none_or(|filter| filter.iter().all(|tag| tags.contains(tag)))
                        {
                            overview.environments.push(OverviewEnvironment {
                                hooks: crate::view::HooksOutput::presence(crate::launch::hooks(
                                    &environment,
                                    status.run.as_ref().and_then(|run| {
                                        environment.config.targets.get(&run.target)
                                    }),
                                )),
                                kind: Some(match &global {
                                    Some(_) => match &environment.config.kind {
                                        Kind::Proxied(_) => {
                                            let steam = overview.steam.get_or_insert_with(|| {
                                                steam_overview(probe.install().cloned())
                                            });
                                            kind_report_with_steam(
                                                &environment.config.kind,
                                                Some(steam),
                                            )
                                        }
                                        _ => KindReport::new(&environment.config.kind),
                                    },
                                    None => KindReport::new(&environment.config.kind),
                                }),
                                name,
                                status,
                                busy,
                                unresolved,
                                tags: tags.clone(),
                            });
                        }
                    }
                    Err(err) if filter.tags.is_some() => overview.errors.push(OverviewError {
                        name,
                        message: format!("{:#}", anyhow::Error::from(err)),
                    }),
                    Err(_) => overview.environments.push(OverviewEnvironment {
                        name,
                        kind: None,
                        status,
                        busy,
                        unresolved,
                        tags: Vec::new(),
                        hooks: None,
                    }),
                }
            }
            Ok(_) => {}
            Err(err) => overview.errors.push(OverviewError {
                name,
                message: format!("{:#}", anyhow::Error::from(err)),
            }),
        }
    }
    Ok(overview)
}

/// Reading the manifest needs the runner and the prefix, and neither is
/// worth failing a whole status over, so an unreadable one reports nothing.
fn components(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> (Option<ComponentReport>, Option<ComponentReport>) {
    let wanted = Wanted::from_config(&environment.config);
    let resolved = environment
        .wine()
        .and_then(|wine| Runners::new(paths, global)?.resolve(&wine.runner))
        .ok()
        .zip(environment.prefix(paths).ok());

    let report = |component: Component| {
        let asked = wanted.get(component)?.to_owned();
        let build = Builds::new(paths, global, component)
            .and_then(|builds| builds.resolve(&asked))
            .ok()
            .map(|build| build.name().to_owned());
        let installed = resolved
            .as_ref()
            .and_then(|(runner, prefix)| install::installed(runner, prefix, component).ok())
            .flatten();
        // A pinned build whose directory was removed still counts as current,
        // since its DLLs are in the prefix either way.
        let current = installed.as_deref() == build.as_deref()
            || installed.as_deref() == Some(asked.as_str());
        Some(ComponentReport {
            wanted: asked,
            resolved: build,
            installed,
            current,
        })
    };
    (report(Component::Dxvk), report(Component::Vkd3d))
}

/// The spelling `status` uses for a component line.
pub fn component_value(report: &ComponentReport) -> String {
    let mut value = if report.wanted == LATEST {
        match &report.resolved {
            Some(resolved) => format!("{LATEST} ({resolved})"),
            None => format!("{LATEST} (nothing installed)"),
        }
    } else {
        report.wanted.clone()
    };
    if !report.current {
        value.push_str(", will be installed on the next launch");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::config::EnvironmentConfig;
    use crate::runtime::{RunMode, RunState};

    fn test_paths(root: &Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(root.join("home"))),
            _ => None,
        })
        .unwrap()
    }

    fn environment(text: &str) -> Environment {
        let text =
            format!("prefix = \"~/prefix\"\nrunner = \"proton-ge\"\narch = \"win64\"\n{text}");
        Environment {
            name: "notepad".to_owned(),
            path: Path::new("/notepad.toml").to_owned(),
            config: EnvironmentConfig::parse(Path::new("/notepad.toml"), &text).unwrap(),
        }
    }

    // An unsupported runner name resolves deterministically to no version,
    // without needing wine installed to run this test.
    #[test]
    fn runner_version_is_none_when_the_runner_cant_be_resolved() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let env = environment("");
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.runner.as_deref(), Some("proton-ge"));
        assert_eq!(report.runner_version, None);
        assert_eq!(report.prefix.as_deref(), Some("~/prefix"));
    }

    #[test]
    fn live_manifest_title_wins_over_imported_title() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let steamapps = paths.home().join(".steam/root/steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(
            steamapps.join("appmanifest_289070.acf"),
            "\"AppState\" { \"appid\" \"289070\" \"name\" \"Civilization VI\" \"StateFlags\" \"4\" }",
        )
        .unwrap();
        let config = EnvironmentConfig::parse(
            Path::new("/civ.toml"),
            "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"289070\"\ntitle = \"Old title\"\n",
        )
        .unwrap();
        let kind = kind_report(&paths, &GlobalConfig::default(), &config.kind);
        assert!(
            matches!(kind, KindReport::Proxied { title: Some(title), .. } if title == "Civilization VI")
        );
    }

    #[test]
    fn overview_does_not_scan_steam_without_proxied_environments() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let steamapps = paths.home().join(".local/share/Steam/steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(
            steamapps.join("appmanifest_289070.acf"),
            "\"AppState\" { \"appid\" \"289070\" \"name\" \"Civilization VI\" \"StateFlags\" \"4\" }",
        )
        .unwrap();
        assert!(crate::steam::active_for(&paths, &GlobalConfig::default()).is_some());

        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(
            paths.environment_config("notepad"),
            "prefix = \"~/prefix\"\nrunner = \"proton-ge\"\narch = \"win64\"\n",
        )
        .unwrap();

        let overview = overview(&paths, OverviewFilter::default()).unwrap();
        assert_eq!(overview.environments.len(), 1);
        assert!(overview.steam.is_none());
    }

    /// One proxied environment per `(name, app id)`, with a Steam install.
    fn steam_library(paths: &Paths, proxied: &[(&str, &str)]) {
        fs::create_dir_all(paths.home().join(".local/share/Steam/steamapps")).unwrap();
        fs::create_dir_all(paths.environments_dir()).unwrap();
        for (name, app_id) in proxied {
            fs::write(
                paths.environment_config(name),
                format!("kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"{app_id}\"\n"),
            )
            .unwrap();
        }
    }

    fn reaper(pid: u32) -> crate::steam::Reaper {
        crate::steam::Reaper {
            pid,
            start_time: 1,
            boot_id: "boot".to_owned(),
        }
    }

    #[test]
    fn overview_sweeps_processes_once_for_every_proxied_environment() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        steam_library(
            &paths,
            &[
                ("game-a", "111"),
                ("game-b", "222"),
                ("game-c", "333"),
                ("game-d", "222"),
            ],
        );

        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let scan = |_: &crate::steam::Install, app_ids: &[String]| {
            calls.borrow_mut().push(app_ids.to_vec());
            Ok(app_ids
                .iter()
                .map(|app_id| {
                    let reapers = if app_id == "222" {
                        vec![reaper(7), reaper(8)]
                    } else {
                        Vec::new()
                    };
                    (app_id.clone(), reapers)
                })
                .collect())
        };
        let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

        // Two environments share app 222, which is asked about once.
        assert_eq!(*calls.borrow(), [["111", "222", "333"]]);
        let states: Vec<_> = overview
            .environments
            .iter()
            .map(|environment| (environment.name.as_str(), environment.status.state))
            .collect();
        assert_eq!(
            states,
            [
                ("game-a", State::Inactive),
                ("game-b", State::SteamRunning),
                ("game-c", State::Inactive),
                ("game-d", State::SteamRunning),
            ]
        );
    }

    #[test]
    fn environments_that_are_busy_or_running_never_cause_a_sweep() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        steam_library(
            &paths,
            &[
                ("busy", "111"),
                ("running", "222"),
                ("starting", "333"),
                ("hook", "444"),
            ],
        );
        let locks = Locks::new(&paths);
        let _busy = locks.acquire(Operation::Winetricks, "busy", &[]).unwrap();

        let state = |name: &str, phase: Option<crate::runtime::RunPhase>| {
            let mut state = RunState::running(
                RunMode::Detached,
                "steam".to_owned(),
                1,
                None,
                &Default::default(),
            );
            state.phase = phase;
            Runtime::new(&paths, name).write_state(&state).unwrap();
        };
        state("running", None);
        let _running = Runtime::new(&paths, "running").try_lock().unwrap().unwrap();
        state("starting", Some(crate::runtime::RunPhase::Starting));
        let _starting = Runtime::new(&paths, "starting")
            .try_lock()
            .unwrap()
            .unwrap();
        // The after hook runs once the game has already exited.
        let mut exited = RunState::running(
            RunMode::Detached,
            "steam".to_owned(),
            1,
            None,
            &Default::default(),
        );
        exited.status = RunStatus::Exited;
        Runtime::new(&paths, "hook").write_state(&exited).unwrap();
        let _hook = locks
            .acquire(Operation::Hook(crate::hooks::Phase::After), "hook", &[])
            .unwrap();

        let calls = std::cell::Cell::new(0);
        let scan = |_: &crate::steam::Install, _: &[String]| {
            calls.set(calls.get() + 1);
            Ok(HashMap::new())
        };
        let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

        assert_eq!(calls.get(), 0);
        let states: Vec<_> = overview
            .environments
            .iter()
            .map(|environment| (environment.name.as_str(), environment.status.state))
            .collect();
        assert_eq!(
            states,
            [
                ("busy", State::Inactive),
                ("hook", State::Exited),
                ("running", State::Running),
                ("starting", State::Starting),
            ]
        );

        // One idle game among them is enough to sweep, once.
        steam_library(&paths, &[("idle", "555")]);
        overview_with(&paths, OverviewFilter::default(), &scan).unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn overview_reads_each_trash_manifest_once() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        fs::create_dir_all(paths.environments_dir()).unwrap();
        for index in 0..5 {
            let name = format!("gone-{index}");
            fs::write(paths.environment_config(&name), "kind = 'native'\n").unwrap();
            crate::trash::remove(&paths, &name, false).unwrap();
        }
        for index in 0..4 {
            fs::write(
                paths.environment_config(&format!("kept-{index}")),
                "kind = 'native'\n",
            )
            .unwrap();
        }
        let scan = |_: &crate::steam::Install, _: &[String]| Ok(HashMap::new());

        crate::trash::take_manifest_reads();
        let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();
        assert_eq!(overview.environments.len(), 4);
        assert_eq!(crate::trash::take_manifest_reads(), 5);
    }

    #[test]
    fn a_failed_sweep_is_reported_for_each_environment_that_needed_it() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        steam_library(&paths, &[("game-111", "111"), ("game-222", "222")]);

        let calls = std::cell::Cell::new(0);
        let scan = |_: &crate::steam::Install, _: &[String]| {
            calls.set(calls.get() + 1);
            Err(std::io::Error::other("no /proc"))
        };
        let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

        assert_eq!(calls.get(), 1);
        assert!(overview.environments.is_empty());
        let messages: Vec<_> = overview
            .errors
            .iter()
            .map(|error| (error.name.as_str(), error.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                (
                    "game-111",
                    "failed to scan Steam processes for game-111: no /proc"
                ),
                (
                    "game-222",
                    "failed to scan Steam processes for game-222: no /proc"
                ),
            ]
        );
    }

    #[test]
    fn overview_skips_the_sweep_without_a_steam_install() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        steam_library(&paths, &[("game-111", "111")]);
        fs::remove_dir_all(paths.home().join(".local/share/Steam")).unwrap();

        let calls = std::cell::Cell::new(0);
        let scan = |_: &crate::steam::Install, _: &[String]| {
            calls.set(calls.get() + 1);
            Ok(HashMap::new())
        };
        let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

        assert_eq!(calls.get(), 0);
        assert_eq!(overview.environments.len(), 1);
    }

    #[test]
    fn target_exe_is_looked_up_from_the_current_config() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let env = environment("[targets.notepad]\nexe = \"notepad\"\n");
        let runtime = Runtime::new(&paths, &env.name);

        // No run yet: no target to report.
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.target_exe, None);

        let state = RunState::running(
            RunMode::Detached,
            "notepad".to_owned(),
            1,
            Some(1),
            &Default::default(),
        );
        runtime.write_state(&state).unwrap();
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.target_exe.as_deref(), Some("notepad"));

        let mut state = state;
        state.target = "gone".to_owned();
        runtime.write_state(&state).unwrap();
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.target_exe, None);
    }
}
