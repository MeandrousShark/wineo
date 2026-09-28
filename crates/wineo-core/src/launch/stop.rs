use std::time::Duration;

use anyhow::{Context, bail};

use crate::config::{DEFAULT_STOP_TIMEOUT, GlobalConfig};
use crate::environment::Environment;
use crate::error::Result;
use crate::installer::{self, Cleaned};
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::runtime::{LostRunCleanup, Runtime};
use crate::supervisor;

/// The result of stopping an environment.
#[derive(Debug)]
pub enum StopOutcome {
    Stopped { messages: Vec<String> },
    NotRunning { messages: Vec<String> },
    CleanedUp { messages: Vec<String> },
    Unresolved { messages: Vec<String> },
}

impl StopOutcome {
    pub fn messages(&self) -> &[String] {
        match self {
            StopOutcome::Stopped { messages }
            | StopOutcome::NotRunning { messages }
            | StopOutcome::CleanedUp { messages }
            | StopOutcome::Unresolved { messages } => messages,
        }
    }
}

/// Stops an environment, cleaning up a lost run or installer when needed.
pub fn stop(paths: &Paths, name: &str, force: bool) -> anyhow::Result<StopOutcome> {
    let outcome = stop_run(paths, name, force)?;
    let StopOutcome::NotRunning { mut messages } = outcome else {
        return Ok(outcome);
    };

    let cleaned = clean_up_installer(paths, name).with_context(|| {
        let record = Runtime::new(paths, name).operation_path();
        format!(
            "failed to clean up after {name}'s last installer. If nothing it started is running any more, delete {} to clear this",
            record.display()
        )
    })?;
    match cleaned {
        Some(Cleaned::Stopped) => {
            messages.push(format!(
                "wineo could not confirm that {name}'s last installer had finished, so it stopped anything left behind"
            ));
            Ok(StopOutcome::CleanedUp { messages })
        }
        Some(Cleaned::Gone) => {
            messages.push(format!(
                "wineo could not confirm that {name}'s last installer had finished, and what it started had already exited"
            ));
            Ok(StopOutcome::CleanedUp { messages })
        }
        Some(Cleaned::Untracked | Cleaned::Unresolved) => {
            messages.push(installer::unresolved_message(paths, name));
            Ok(StopOutcome::Unresolved { messages })
        }
        None => Ok(StopOutcome::NotRunning { messages }),
    }
}

/// Stops an app run or cleans up after one that wineo lost track of.
pub fn stop_run(paths: &Paths, name: &str, force: bool) -> anyhow::Result<StopOutcome> {
    Environment::ensure_exists(paths, name)?;
    if let Some(operation) = busy_with(paths, name)? {
        bail!(
            "{name} is busy: {}, so there is no run to stop. Finish or cancel it in its terminal",
            operation.describe()
        );
    }

    let (stop_timeout, mut messages) = stop_timeout(paths);
    let runtime = Runtime::new(paths, name);
    let grace = if force { 0 } else { stop_timeout };
    let timeout = Duration::from_secs(u64::from(grace) + 10);
    if supervisor::stop(&runtime, force, timeout)
        .with_context(|| format!("failed to stop {name}"))?
    {
        return Ok(StopOutcome::Stopped { messages });
    }

    match clean_up_lost_run(paths, name, timeout)? {
        LostRunOutcome::NotLost => {}
        LostRunOutcome::Unresolved { warning, message } => {
            append_warning(&mut messages, warning);
            messages.push(message);
            return Ok(StopOutcome::Unresolved { messages });
        }
        LostRunOutcome::Gone { warning } => {
            append_warning(&mut messages, warning);
            messages.push(format!(
                "wineo lost track of {name}'s last run, and what it started had already exited"
            ));
            return Ok(StopOutcome::CleanedUp { messages });
        }
        LostRunOutcome::Killed { warning } => {
            append_warning(&mut messages, warning);
            messages.push(format!(
                "wineo lost track of {name}'s last run, so it killed anything left behind"
            ));
            return Ok(StopOutcome::CleanedUp { messages });
        }
    }
    Ok(StopOutcome::NotRunning { messages })
}

/// Result of cleaning up state left behind by a lost app run.
#[derive(Debug)]
pub(crate) enum LostRunOutcome {
    NotLost,
    Unresolved {
        warning: Option<String>,
        message: String,
    },
    Gone {
        warning: Option<String>,
    },
    Killed {
        warning: Option<String>,
    },
}

pub(crate) fn busy_with(paths: &Paths, name: &str) -> Result<Option<Operation>> {
    Locks::new(paths).busy_with(name)
}

/// Cleans up after a lost run without deciding how its result is presented.
pub(crate) fn clean_up_lost_run(
    paths: &Paths,
    name: &str,
    settle_timeout: Duration,
) -> Result<LostRunOutcome> {
    let runtime = Runtime::new(paths, name);
    let cleanup = runtime.clean_up_lost_run(settle_timeout, &paths.stats_log(name))?;
    Ok(match cleanup {
        LostRunCleanup::NotLost => LostRunOutcome::NotLost,
        LostRunCleanup::Unresolved { warning } => LostRunOutcome::Unresolved {
            warning,
            message: runtime.unresolved_run_message(name),
        },
        LostRunCleanup::Gone { warning } => LostRunOutcome::Gone { warning },
        LostRunCleanup::Killed { warning } => LostRunOutcome::Killed { warning },
    })
}

/// Stops whatever an installer left running when wineo could not confirm it
/// was done.
pub(crate) fn clean_up_installer(paths: &Paths, name: &str) -> Result<Option<Cleaned>> {
    let global = GlobalConfig::load_or_default(paths)?;
    let timeout = installer::clean_up_timeout(&global);
    installer::clean_up(paths, &global, name, timeout)
}

pub(crate) fn stop_timeout(paths: &Paths) -> (u32, Vec<String>) {
    match GlobalConfig::load(paths) {
        Ok(global) => (global.stop_timeout, Vec::new()),
        Err(err) => {
            let err: anyhow::Error = err.into();
            (DEFAULT_STOP_TIMEOUT, vec![format!("warning: {err:#}")])
        }
    }
}

fn append_warning(messages: &mut Vec<String>, warning: Option<String>) {
    if let Some(warning) = warning {
        messages.push(format!("warning: {warning}"));
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::thread;

    use super::*;
    use crate::paths::Paths;
    use crate::process;
    use crate::runners::Runners;
    use crate::runtime::{RunMode, RunState, RunStatus};

    fn paths(root: &std::path::Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| {
            let path = match key {
                "HOME" => root.clone(),
                "XDG_CONFIG_HOME" => root.join("config"),
                "XDG_DATA_HOME" => root.join("data"),
                "XDG_STATE_HOME" => root.join("state"),
                _ => return None,
            };
            Some(OsString::from(path.as_os_str()))
        })
        .unwrap()
    }

    fn executable(path: &std::path::Path, content: &str) {
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn stop_child(child: &mut Child) {
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }

    #[test]
    fn stop_reports_an_environment_without_a_run() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let config = paths.environment_config("app");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, "kind = \"native\"\n").unwrap();

        let outcome = stop(&paths, "app", false).unwrap();

        assert!(matches!(outcome, StopOutcome::NotRunning { messages } if messages.is_empty()));
    }

    #[test]
    fn a_lost_run_that_does_not_settle_keeps_its_state() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("data/wineo/runners");
        let bin = runner_root.join("fake/bin");
        fs::create_dir_all(&bin).unwrap();
        executable(&bin.join("wine"), "#!/bin/sh\nexit 0\n");
        executable(
            &bin.join("wineserver"),
            "#!/bin/sh\nif [ \"$1\" = \"-w\" ]; then sleep 0.2; fi\nexit 0\n",
        );
        let runner = Runners::at(runner_root).resolve("fake").unwrap();

        let prefix = root.path().join("prefix");
        fs::create_dir_all(&prefix).unwrap();
        let prefix = fs::canonicalize(prefix).unwrap();
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut run = RunState::running(
            RunMode::Detached,
            "app".to_owned(),
            pid,
            Some(pid),
            &Default::default(),
        );
        run.runner = Some(runner);
        run.prefix = Some(prefix);
        run.boot_id = process::boot_id();
        run.leader_start = process::start_time(pid);
        assert!(run.boot_id.is_some());
        assert!(run.leader_start.is_some());

        let runtime = Runtime::new(&paths, "app");
        runtime.write_state(&run).unwrap();
        let started = std::time::Instant::now();
        let outcome = clean_up_lost_run(&paths, "app", Duration::from_millis(30)).unwrap();
        let elapsed = started.elapsed();
        stop_child(&mut child);
        thread::sleep(Duration::from_millis(250));

        assert!(matches!(outcome, LostRunOutcome::Unresolved { .. }));
        assert!(
            elapsed < Duration::from_millis(150),
            "settling took {elapsed:?}"
        );
        assert_eq!(
            runtime.read_state().unwrap().unwrap().status,
            RunStatus::Running
        );
        assert!(runtime.status().unwrap().lost);
        let guidance = runtime.unresolved_run_message("app");
        assert!(guidance.contains("Verify its processes have stopped"));
        assert!(guidance.contains("state.toml"));
    }
}
