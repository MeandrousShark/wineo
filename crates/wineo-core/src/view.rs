use crate::operation::Operation;
use crate::report::{ComponentReport, EnvironmentReport, KindReport, Played};
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
    pub hooks: Option<HooksOutput>,
    pub run: Option<RunOutput<'a>>,
    /// `wine`, `native`, or `proxied`. None when the config can't be read.
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launcher: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_problem: Option<&'a str>,
}

impl<'a> StatusOutput<'a> {
    pub fn new(
        name: &'a str,
        status: &'a Status,
        busy: Option<Operation>,
        unresolved: bool,
        tags: &'a [String],
        kind: Option<&'a KindReport>,
    ) -> Self {
        let proxied = match kind {
            Some(KindReport::Proxied {
                launcher,
                app_id,
                install,
                ..
            }) => Some((launcher.as_str(), app_id.as_str(), install)),
            Some(KindReport::Wine | KindReport::Native) | None => None,
        };
        Self {
            name,
            tags,
            state: status.state,
            lost: status.lost || status.run.as_ref().is_some_and(|run| run.lost),
            busy,
            unresolved,
            hooks: None,
            run: status.run.as_ref().map(RunOutput::from),
            kind: kind.map(KindReport::as_str),
            launcher: proxied.map(|(launcher, _, _)| launcher),
            app_id: proxied.map(|(_, app_id, _)| app_id),
            install: proxied.map(|(_, _, install)| install.state),
            install_problem: proxied.and_then(|(_, _, install)| install.problem.as_deref()),
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

#[derive(Debug, Clone, Serialize)]
pub struct HooksOutput {
    pub before: bool,
    pub after: bool,
}

impl HooksOutput {
    pub fn presence(hooks: Option<&crate::config::Hooks>) -> Option<Self> {
        hooks
            .filter(|hooks| hooks.before.is_some() || hooks.after.is_some())
            .map(|hooks| Self {
                before: hooks.before.is_some(),
                after: hooks.after.is_some(),
            })
    }
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
            status: StatusOutput {
                hooks: HooksOutput::presence(report.hooks.as_ref()),
                ..StatusOutput::new(
                    &report.name,
                    &report.status,
                    report.busy,
                    report.unresolved,
                    &report.tags,
                    Some(&report.kind),
                )
            },
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
/// `environments`. Week and month fields stay omitted for Wineo stats and are
/// null for Steam, which cannot supply them.
#[derive(Serialize)]
pub struct StatsOutput<'a> {
    pub name: &'a str,
    pub seconds: Option<i64>,
    pub sessions: Option<usize>,
    pub average_seconds: Option<i64>,
    pub longest: Option<Longest>,
    pub first_played: Option<Timestamp>,
    pub last_played: Option<Timestamp>,
    pub running: bool,
    pub imported_seconds: i64,
    pub targets: Vec<TargetSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_log: Option<&'a [Session]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub this_week_seconds: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub this_month_seconds: Option<Option<i64>>,
    pub source: &'static str,
}

impl<'a> StatsOutput<'a> {
    pub fn new(name: &'a str, summary: &Summary, sessions_log: Option<&'a [Session]>) -> Self {
        let steam = summary.source == crate::stats::Source::Steam;
        Self {
            name,
            seconds: (!summary.playtime_unknown).then_some(summary.seconds),
            sessions: (!steam).then_some(summary.sessions),
            average_seconds: summary.average_seconds,
            longest: summary.longest,
            first_played: summary.first_played,
            last_played: summary.last_played,
            running: summary.running,
            imported_seconds: summary.imported_seconds,
            targets: summary.targets.clone(),
            sessions_log: (!steam).then_some(sessions_log).flatten(),
            this_week_seconds: steam.then_some(None),
            this_month_seconds: steam.then_some(None),
            source: summary.source.as_str(),
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
