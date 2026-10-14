//! Shell hooks, separate from the program's environment and run state.

use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use rustix::process::Signal;
use serde::{Deserialize, Serialize};

use crate::cancel::Token;
use crate::error::{Error, IoContext, Result};
use crate::operation::Guard;
use crate::paths::{PORTABLE_VAR, Paths};
use crate::process::{self, Identity};
use crate::runner::Runner;

const VARIABLES: [&str; 11] = [
    "WINEO_LAUNCHER",
    "WINEO_APP_ID",
    "WINEPREFIX",
    "WINEO_ENVIRONMENT",
    "WINEO_TARGET",
    "WINEO_EXE",
    "WINEO_PREFIX",
    "WINEO_HOOK",
    "WINEO_END",
    "WINEO_EXIT_CODE",
    "WINEO_SECONDS",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Before,
    After,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Exited,
    Failed,
    Stopped,
}

impl End {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ending {
    pub end: End,
    pub exit_code: Option<i32>,
    pub seconds: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum Hook {
    Before,
    After(Ending),
}

impl Hook {
    pub fn phase(self) -> Phase {
        match self {
            Self::Before => Phase::Before,
            Self::After(_) => Phase::After,
        }
    }
}

/// Uses the configured exe and prefix, before target path checks have run.
#[derive(Debug)]
pub struct Context<'a> {
    pub environment: &'a str,
    pub target: &'a str,
    pub exe: &'a str,
    pub wine: Option<(&'a Runner, &'a Path)>,
    pub steam: Option<&'a Steam>,
    pub hook: Hook,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Steam {
    pub app_id: String,
    pub prefix: Option<std::path::PathBuf>,
}

impl Steam {
    pub fn new(install: &crate::steam::Install, app_id: &str) -> Self {
        let prefix = crate::steam::manifests(install)
            .into_iter()
            .find(|entry| entry.app_id == app_id)
            .map(|entry| {
                entry
                    .library
                    .join("steamapps/compatdata")
                    .join(app_id)
                    .join("pfx")
            });
        Self {
            app_id: app_id.to_owned(),
            prefix,
        }
    }
}

#[derive(Debug)]
pub enum Output {
    Inherit,
    File(File),
}

#[derive(Debug)]
pub enum Outcome {
    Finished(ExitStatus),
    /// A TERM trap exiting successfully must still cancel the launch.
    Canceled(ExitStatus),
}

fn command(paths: &Paths, context: &Context<'_>, text: &str) -> Command {
    let mut command = Command::new("sh");
    configure(&mut command, paths, context, text);
    command
}

fn configure(command: &mut Command, paths: &Paths, context: &Context<'_>, text: &str) {
    command
        .args(["-c", text])
        .current_dir(paths.home())
        .process_group(0)
        .stdin(Stdio::null());
    for variable in VARIABLES {
        command.env_remove(variable);
    }
    command.env_remove(PORTABLE_VAR);
    command
        .env("WINEO_ENVIRONMENT", context.environment)
        .env("WINEO_TARGET", context.target)
        .env("WINEO_HOOK", context.hook.phase().as_str());
    if let Some(steam) = context.steam {
        command
            .env("WINEO_LAUNCHER", "steam")
            .env("WINEO_APP_ID", &steam.app_id);
        if let Some(prefix) = &steam.prefix
            && prefix.is_dir()
        {
            command.env("WINEO_PREFIX", prefix);
        }
    } else {
        command.env("WINEO_EXE", context.exe);
    }
    if let Some((runner, prefix)) = context.wine {
        command.env("WINEO_PREFIX", runner.wine_prefix(prefix));
    }
    if let Some(root) = paths.portable_root() {
        command.env(PORTABLE_VAR, root);
    }
    if let Hook::After(ending) = context.hook {
        command
            .env("WINEO_END", ending.end.as_str())
            .env(
                "WINEO_EXIT_CODE",
                ending
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_default(),
            )
            .env("WINEO_SECONDS", ending.seconds.to_string());
    }
}

/// Runs under the caller's exclusive Hook guard. A forced token kills at once.
pub fn run(
    text: &str,
    paths: &Paths,
    context: &Context<'_>,
    guard: &Guard,
    token: &Token,
    stop_timeout: Duration,
    output: Output,
) -> Result<Outcome> {
    if token.is_canceled() {
        return Err(Error::Canceled("hook"));
    }
    guard.record_hook(context.hook.phase(), None)?;
    let mut command = command(paths, context, text);
    match output {
        Output::Inherit => {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        }
        Output::File(file) => {
            let stderr = file
                .try_clone()
                .context(|| "failed to clone hook output".to_owned())?;
            command.stdout(file).stderr(stderr);
        }
    }
    let child = command
        .spawn()
        .context(|| "failed to start hook".to_owned())?;
    process::quiet_core_dumps(&child);
    let mut spawned = Spawned {
        child,
        finished: false,
    };
    let group = spawned.child.id();
    let running = Identity {
        group,
        boot_id: process::boot_id()
            .ok_or_else(|| Error::Unsettled("could not read hook boot identity".to_owned()))?,
        leader_start: process::start_time(group)
            .ok_or_else(|| Error::Unsettled("could not read hook process identity".to_owned()))?,
    };
    guard.record_hook(context.hook.phase(), Some(&running))?;
    let stopped = wait_for_exit_requested(
        &spawned.child,
        token,
        stop_timeout,
        process::group_has_live_members,
        || guard.hook_request(token),
    )?;
    let status = spawned
        .child
        .wait()
        .context(|| "failed to wait for hook".to_owned())?;
    spawned.finished = true;
    guard.record_hook(context.hook.phase(), None)?;
    Ok(if stopped {
        Outcome::Canceled(status)
    } else {
        Outcome::Finished(status)
    })
}

#[cfg(test)]
fn wait_for_exit(
    child: &Child,
    token: &Token,
    stop_timeout: Duration,
    group_members: impl Fn(u32) -> Option<bool>,
) -> Result<bool> {
    wait_for_exit_requested(child, token, stop_timeout, group_members, || Ok(()))
}

const REQUEST_POLL: Duration = Duration::from_millis(20);
const UNCONFIRMED: &str = "the hook's processes could not be confirmed stopped";

/// The unreaped child pins its group number until the whole group settles,
/// so stopping it needs no `/proc` identity check.
fn wait_for_exit_requested(
    child: &Child,
    token: &Token,
    stop_timeout: Duration,
    group_members: impl Fn(u32) -> Option<bool>,
    mut request: impl FnMut() -> Result<()>,
) -> Result<bool> {
    let mut escalation = process::Escalation::new(stop_timeout, None);
    let ended = process::stop_group_probed(
        child,
        Signal::TERM,
        &mut escalation,
        token,
        // A stop can also be asked for through the lock file.
        Some(REQUEST_POLL),
        |step| step.map_or_else(&mut request, |_| Ok(())),
        group_members,
    )
    .context(|| "failed to stop the hook process group".to_owned())?
    // `Spawned` kills and reaps the group as `run` returns an error.
    .map_err(|(err, ended)| {
        process::failed_stop(err, ended == process::Ended::Stopped, UNCONFIRMED)
    })?;
    match ended {
        process::Ended::Exited => Ok(false),
        process::Ended::Stopped => Ok(true),
        // The leader is still unreaped here, so `Spawned` can kill the group
        // again safely as `run` returns this.
        process::Ended::Unconfirmed => Err(Error::Unsettled(UNCONFIRMED.to_owned())),
    }
}

struct Spawned {
    child: Child,
    finished: bool,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if !self.finished {
            // This child is never reaped before cleanup, so its group number
            // cannot have been reused even if saving its identity failed.
            // Bounded, so a leader stuck in uninterruptible sleep can't hang.
            process::kill_and_reap(&mut self.child);
        }
    }
}

#[cfg(test)]
mod tests;
