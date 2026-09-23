use jiff::Timestamp;
use serde::Serialize;
use wineo_core::operation::Operation;
use wineo_core::report::{ComponentReport, EnvironmentReport};
use wineo_core::runtime::{RunMode, RunState, State, Status};

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
    ) -> Self {
        Self {
            name,
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
    runner: RunnerOutput<'a>,
    prefix: &'a str,
    target_exe: Option<&'a str>,
    gamemode: bool,
    gameid: Option<&'a str>,
    store: Option<&'a str>,
    dxvk: Option<&'a ComponentReport>,
    vkd3d: Option<&'a ComponentReport>,
    desktop: &'a [String],
}

impl<'a> NamedOutput<'a> {
    pub fn new(report: &'a EnvironmentReport, configured_runner: &'a str) -> Self {
        Self {
            status: StatusOutput::new(&report.name, &report.status, report.busy, report.unresolved),
            runner: RunnerOutput {
                configured: configured_runner,
                resolved: report.runner_resolved.as_deref(),
                version: report.runner_version.as_deref(),
            },
            prefix: &report.prefix,
            target_exe: report.target_exe.as_deref(),
            gamemode: report.gamemode,
            gameid: report.gameid.as_deref(),
            store: report.store.as_deref(),
            dxvk: report.dxvk.as_ref(),
            vkd3d: report.vkd3d.as_ref(),
            desktop: &report.desktop,
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

pub fn print(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
