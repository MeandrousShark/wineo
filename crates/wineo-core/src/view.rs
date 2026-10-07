use crate::operation::Operation;
use crate::report::{ComponentReport, EnvironmentReport, Played};
use crate::runtime::{RunMode, RunState, State, Status};
use crate::stats::{Longest, Session, Summary, TagBreakdown, TagTotals, TargetSummary};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct RunOutput<'a> {
    pub target: &'a str,
    pub mode: RunMode,
    pub pid: u32,
    pub started_at: Timestamp,
    pub program_exited_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub exit_code: Option<i32>,
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
    pub name: &'a str,
    pub tags: &'a [String],
    pub state: State,
    pub lost: bool,
    pub busy: Option<Operation>,
    pub unresolved: bool,
    pub run: Option<RunOutput<'a>>,
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
pub struct RunnerOutput<'a> {
    pub configured: &'a str,
    pub resolved: Option<&'a str>,
    pub version: Option<&'a str>,
}

#[derive(Serialize)]
pub struct NamedOutput<'a> {
    #[serde(flatten)]
    pub status: StatusOutput<'a>,
    pub kind: &'static str,
    pub runner: Option<RunnerOutput<'a>>,
    pub prefix: Option<&'a str>,
    pub winetricks: &'a [String],
    pub winetricks_missing: &'a [String],
    pub target_exe: Option<&'a str>,
    pub gamemode: bool,
    pub gamescope: bool,
    pub mangohud: bool,
    pub wayland: bool,
    pub gameid: Option<&'a str>,
    pub store: Option<&'a str>,
    pub dxvk: Option<&'a ComponentReport>,
    vkd3d: Option<&'a ComponentReport>,
    pub desktop: &'a [String],
    pub played: Option<PlayedOutput>,
}

#[derive(Serialize)]
pub struct PlayedOutput {
    pub seconds: i64,
    pub sessions: usize,
    pub last_played: Option<Timestamp>,
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
            winetricks: &report.winetricks,
            winetricks_missing: &report.winetricks_missing,
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
    pub name: &'a str,
    pub seconds: i64,
    pub sessions: usize,
    pub average_seconds: Option<i64>,
    pub longest: Option<Longest>,
    pub first_played: Option<Timestamp>,
    pub last_played: Option<Timestamp>,
    pub running: bool,
    pub imported_seconds: i64,
    pub targets: Vec<TargetSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_log: Option<&'a [Session]>,
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

#[derive(Serialize)]
pub struct TagTotalsOutput {
    pub seconds: i64,
    pub sessions: usize,
    pub environments: usize,
    pub played_environments: usize,
    pub last_played: Option<Timestamp>,
}

impl From<&TagTotals> for TagTotalsOutput {
    fn from(totals: &TagTotals) -> Self {
        Self {
            seconds: totals.seconds,
            sessions: totals.sessions,
            environments: totals.environments,
            played_environments: totals.played_environments,
            last_played: totals.last_played,
        }
    }
}

#[derive(Serialize)]
pub struct TagStatsOutput<'a> {
    pub tag: &'a str,
    #[serde(flatten)]
    pub totals: TagTotalsOutput,
}

#[derive(Serialize)]
pub struct TagBreakdownOutput<'a> {
    pub tags: Vec<TagStatsOutput<'a>>,
    pub untagged: Option<TagTotalsOutput>,
    pub errors: &'a [CollectionError],
}

impl<'a> TagBreakdownOutput<'a> {
    pub fn new(breakdown: &'a TagBreakdown, errors: &'a [CollectionError]) -> Self {
        Self {
            tags: breakdown
                .tags
                .iter()
                .map(|(tag, totals)| TagStatsOutput {
                    tag,
                    totals: totals.into(),
                })
                .collect(),
            untagged: breakdown.untagged.as_ref().map(Into::into),
            errors,
        }
    }
}
