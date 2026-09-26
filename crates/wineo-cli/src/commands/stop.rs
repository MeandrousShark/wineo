use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use wineo_core::config::{DEFAULT_STOP_TIMEOUT, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::installer::{self, Cleaned};
use wineo_core::operation::Locks;
use wineo_core::paths::Paths;
use wineo_core::runtime::{LostRunCleanup, Runtime};
use wineo_core::supervisor;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to stop
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Kill everything in the prefix right away instead of asking programs to close
    #[arg(long)]
    force: bool,
}

pub enum Outcome {
    Stopped,
    NotRunning,
    /// The supervisor or an installer had died, so what it left behind was
    /// killed directly.
    CleanedUp,
    /// Cleanup could not be confirmed. The environment remains blocked.
    Unresolved,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let name = &args.name;
    // Only this command cleans up after an installer, as the one the refusals
    // point at. `restart` stops a run without it and is then refused.
    let outcome = match stop(paths, name, args.force)? {
        Outcome::NotRunning => match clean_up_installer(paths, name)? {
            Some(Cleaned::Stopped) => {
                eprintln!(
                    "wineo could not confirm that {name}'s last installer had finished, so it stopped anything left behind"
                );
                Outcome::CleanedUp
            }
            Some(Cleaned::Gone) => {
                eprintln!(
                    "wineo could not confirm that {name}'s last installer had finished, and what it started had already exited"
                );
                Outcome::CleanedUp
            }
            Some(Cleaned::Untracked | Cleaned::Unresolved) => {
                report_unresolved_installer(paths, name);
                Outcome::Unresolved
            }
            None => Outcome::NotRunning,
        },
        outcome => outcome,
    };
    match outcome {
        Outcome::Stopped => println!("Stopped {name}"),
        Outcome::NotRunning => eprintln!("{name} is not running"),
        Outcome::CleanedUp => {}
        Outcome::Unresolved => return Ok(ExitCode::FAILURE),
    }
    Ok(ExitCode::SUCCESS)
}

/// Stops an environment and waits until its run has ended.
pub fn stop(paths: &Paths, name: &str, force: bool) -> anyhow::Result<Outcome> {
    Environment::ensure_exists(paths, name)?;
    // An operation is not an app run, so it is never something to signal.
    if let Some(operation) = Locks::new(paths).busy_with(name)? {
        bail!(
            "{name} is busy: {}, so there is no run to stop. Finish or cancel it in its terminal",
            operation.describe()
        );
    }
    // A broken global config must not keep a running environment from stopping.
    let stop_timeout = GlobalConfig::load(paths).map_or_else(
        |err| {
            warn(err.into());
            DEFAULT_STOP_TIMEOUT
        },
        |global| global.stop_timeout,
    );
    let runtime = Runtime::new(paths, name);
    // Room for the clean stop, the kill after it, and the prefix shutting down.
    let grace = if force { 0 } else { stop_timeout };
    let timeout = Duration::from_secs(u64::from(grace) + 10);
    if supervisor::stop(&runtime, force, timeout)
        .with_context(|| format!("failed to stop {name}"))?
    {
        return Ok(Outcome::Stopped);
    }
    clean_up_lost_run(paths, name, &runtime, timeout)
}

/// Stops whatever an installer left running when wineo could not confirm it
/// was done, which is what other commands are refused over until then.
fn clean_up_installer(paths: &Paths, name: &str) -> anyhow::Result<Option<Cleaned>> {
    let global = GlobalConfig::load_or_default(paths)?;
    let timeout = installer::clean_up_timeout(&global);
    let record = Runtime::new(paths, name).operation_path();
    let cleaned = installer::clean_up(paths, &global, name, timeout).with_context(|| {
        format!(
            "failed to clean up after {name}'s last installer. If nothing it started is running any more, delete {} to clear this",
            record.display()
        )
    })?;
    Ok(cleaned)
}

fn report_unresolved_installer(paths: &Paths, name: &str) {
    eprintln!("{}", installer::unresolved_message(paths, name));
}

/// Cleans up after a lost run and says what happened.
fn clean_up_lost_run(
    paths: &Paths,
    name: &str,
    runtime: &Runtime,
    settle_timeout: Duration,
) -> anyhow::Result<Outcome> {
    let outcome = match runtime.clean_up_lost_run(settle_timeout, &paths.stats_log(name))? {
        LostRunCleanup::NotLost => return Ok(Outcome::NotRunning),
        LostRunCleanup::Unresolved { warning } => {
            if let Some(warning) = warning {
                eprintln!("warning: {warning}");
            }
            report_unresolved_run(paths, name);
            return Ok(Outcome::Unresolved);
        }
        LostRunCleanup::Gone { warning } => {
            if let Some(warning) = warning {
                eprintln!("warning: {warning}");
            }
            "and what it started had already exited"
        }
        LostRunCleanup::Killed { warning } => {
            if let Some(warning) = warning {
                eprintln!("warning: {warning}");
            }
            "so it killed anything left behind"
        }
    };
    eprintln!("wineo lost track of {name}'s last run, {outcome}");
    Ok(Outcome::CleanedUp)
}

fn report_unresolved_run(paths: &Paths, name: &str) {
    eprintln!("{}", unresolved_run_message(paths, name));
}

fn unresolved_run_message(paths: &Paths, name: &str) -> String {
    Runtime::new(paths, name).unresolved_run_message(name)
}

fn warn(err: anyhow::Error) {
    eprintln!("warning: {err:#}");
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
    use wineo_core::paths::Paths;
    use wineo_core::process;
    use wineo_core::runners::Runners;
    use wineo_core::runtime::{RunMode, RunState, RunStatus};

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
        let outcome =
            clean_up_lost_run(&paths, "app", &runtime, Duration::from_millis(30)).unwrap();
        let elapsed = started.elapsed();
        stop_child(&mut child);
        thread::sleep(Duration::from_millis(250));

        assert!(matches!(outcome, Outcome::Unresolved));
        assert!(
            elapsed < Duration::from_millis(150),
            "settling took {elapsed:?}"
        );
        assert_eq!(
            runtime.read_state().unwrap().unwrap().status,
            RunStatus::Running
        );
        assert!(runtime.status().unwrap().lost);
        let guidance = unresolved_run_message(&paths, "app");
        assert!(guidance.contains("Verify its processes have stopped"));
        assert!(guidance.contains("state.toml"));
    }
}
