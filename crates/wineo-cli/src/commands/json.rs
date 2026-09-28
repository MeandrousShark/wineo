use jiff::Timestamp;
use serde::Serialize;
use wineo_core::operation::Operation;
use wineo_core::report::{ComponentReport, EnvironmentReport, Played};
use wineo_core::runtime::{RunMode, RunState, State, Status};
use wineo_core::stats::{Longest, Session, Summary, TargetSummary};

#[derive(Serialize)]
pub struct RunOutput<'a> {
    target: &'a str,
    mode: RunMode,
    pid: u32,
    started_at: Timestamp,
    program_exited_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
    exit_code: Option<i32>,
}

impl<'a> From<&'a RunState> for RunOutput<'a> {
    fn from(run: &'a RunState) -> Self {
        Self {
            target: &run.target,
            mode: run.mode,
            pid: run.pid,
            started_at: run.started_at,
            program_exited_at: run.program_exited_at,
            ended_at: run.ended_at,
            exit_code: run.exit_code,
        }
    }
}

#[derive(Serialize)]
pub struct StatusOutput<'a> {
    name: &'a str,
    tags: &'a [String],
    state: State,
    lost: bool,
    busy: Option<Operation>,
    unresolved: bool,
    run: Option<RunOutput<'a>>,
}

impl<'a> StatusOutput<'a> {
    pub fn new(
        name: &'a str,
        status: &'a Status,
        busy: Option<Operation>,
        unresolved: bool,
        tags: &'a [String],
    ) -> Self {
        Self {
            name,
            tags,
            state: status.state,
            lost: status.lost,
            busy,
            unresolved,
            run: status.run.as_ref().map(RunOutput::from),
        }
    }
}

#[derive(Serialize)]
struct RunnerOutput<'a> {
    configured: &'a str,
    resolved: Option<&'a str>,
    version: Option<&'a str>,
}

#[derive(Serialize)]
pub struct NamedOutput<'a> {
    #[serde(flatten)]
    status: StatusOutput<'a>,
    kind: &'static str,
    runner: Option<RunnerOutput<'a>>,
    prefix: Option<&'a str>,
    target_exe: Option<&'a str>,
    gamemode: bool,
    gamescope: bool,
    mangohud: bool,
    wayland: bool,
    gameid: Option<&'a str>,
    store: Option<&'a str>,
    dxvk: Option<&'a ComponentReport>,
    vkd3d: Option<&'a ComponentReport>,
    desktop: &'a [String],
    played: Option<PlayedOutput>,
}

#[derive(Serialize)]
pub struct PlayedOutput {
    seconds: i64,
    sessions: usize,
    last_played: Option<Timestamp>,
}

impl From<&Played> for PlayedOutput {
    fn from(played: &Played) -> Self {
        Self {
            seconds: played.seconds,
            sessions: played.sessions,
            last_played: played.last_played,
        }
    }
}

impl<'a> NamedOutput<'a> {
    pub fn new(report: &'a EnvironmentReport, configured_runner: Option<&'a str>) -> Self {
        Self {
            status: StatusOutput::new(
                &report.name,
                &report.status,
                report.busy,
                report.unresolved,
                &report.tags,
            ),
            kind: if report.native { "native" } else { "wine" },
            runner: configured_runner.map(|configured| RunnerOutput {
                configured,
                resolved: report.runner_resolved.as_deref(),
                version: report.runner_version.as_deref(),
            }),
            prefix: report.prefix.as_deref(),
            target_exe: report.target_exe.as_deref(),
            gamemode: report.gamemode,
            gamescope: report.gamescope,
            mangohud: report.mangohud,
            wayland: report.wayland,
            gameid: report.gameid.as_deref(),
            store: report.store.as_deref(),
            dxvk: report.dxvk.as_ref(),
            vkd3d: report.vkd3d.as_ref(),
            desktop: &report.desktop,
            played: report.played.as_ref().map(PlayedOutput::from),
        }
    }
}

#[derive(Serialize)]
pub struct CollectionOutput<'a> {
    pub environments: Vec<StatusOutput<'a>>,
    pub errors: &'a [CollectionError],
}

#[derive(Serialize)]
pub struct CollectionError {
    pub name: String,
    pub message: String,
}

/// `wineo stats [NAME] --json`'s shape, named or as one of a collection's
/// `environments`. `this_week_seconds` and `this_month_seconds` are text-only,
/// so `Summary` isn't serialized as is.
#[derive(Serialize)]
pub struct StatsOutput<'a> {
    name: &'a str,
    seconds: i64,
    sessions: usize,
    average_seconds: Option<i64>,
    longest: Option<Longest>,
    first_played: Option<Timestamp>,
    last_played: Option<Timestamp>,
    running: bool,
    imported_seconds: i64,
    targets: Vec<TargetSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions_log: Option<&'a [Session]>,
}

impl<'a> StatsOutput<'a> {
    pub fn new(name: &'a str, summary: &Summary, sessions_log: Option<&'a [Session]>) -> Self {
        Self {
            name,
            seconds: summary.seconds,
            sessions: summary.sessions,
            average_seconds: summary.average_seconds,
            longest: summary.longest,
            first_played: summary.first_played,
            last_played: summary.last_played,
            running: summary.running,
            imported_seconds: summary.imported_seconds,
            targets: summary.targets.clone(),
            sessions_log,
        }
    }
}

#[derive(Serialize)]
pub struct StatsCollectionOutput<'a> {
    pub environments: Vec<StatsOutput<'a>>,
    pub errors: &'a [CollectionError],
}

pub fn print(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
