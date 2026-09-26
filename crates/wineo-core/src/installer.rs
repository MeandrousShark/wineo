//! Running an installer or updater in an environment's prefix.
//!
//! Unlike `exec`, an installer gets the environment to itself: it holds an
//! exclusive lock until everything it started has exited, runs in a process
//! group of its own so a cancellation reaches all of it, and leaves a record
//! behind until then, so whatever a crash leaves running can still be found
//! and stopped.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use jiff::Timestamp;

use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::launch::{Host, Launch};
use crate::operation::{Guard, Locks, Operation};
use crate::paths::Paths;
use crate::process;
use crate::runtime::{OperationState, Runtime};
use crate::supervisor::{Notice, Settle, Supervisor, Watched};

/// How long kills repeat after a cancellation before wineo stops waiting and
/// reports what it could not confirm, rather than holding on for good.
const GIVE_UP: Duration = Duration::from_secs(10);

/// Where an installer's output goes.
#[derive(Debug, Clone, Copy)]
pub enum Output<'a> {
    /// Wherever wineo's own output goes.
    Inherit,
    /// A log file, replaced by each run.
    Log(&'a Path),
}

/// How an installer run ended.
#[derive(Debug)]
pub enum Outcome {
    /// The installer exited on its own, and so did everything it started.
    Finished { exit_code: Option<i32> },
    /// It was canceled, and everything it started has stopped.
    Canceled { exit_code: Option<i32> },
    /// wineo could not confirm that everything the installer started has
    /// stopped. The record stays, so the environment is refused until
    /// `wineo stop` cleans up.
    Unresolved {
        exit_code: Option<i32>,
        canceled: bool,
        error: Error,
    },
}

/// An installer ready to run, holding the environment and its prefix.
#[derive(Debug)]
pub struct Installer {
    guard: Guard,
    launch: Launch,
    runtime: Runtime,
    program: String,
}

impl Installer {
    /// Takes environment `name` and its prefix for `program`, resolved the way
    /// `exec` resolves it, `.msi` files included. Refused while a game runs,
    /// after a lost run, and while an earlier installer is unresolved.
    pub fn prepare(
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
    ) -> Result<Self> {
        // Resolved once to know which prefix to lock, then again under it.
        let locked = launch(paths, global, &Environment::load(paths, name)?, program)?
            .host
            .prefix()
            .ok_or_else(|| Error::NoPrefix(name.to_owned()))?
            .to_owned();
        let guard = Locks::new(paths).acquire(Operation::Install, name, &[&locked])?;
        Self::claim(guard, paths, global, name, program, Some(&locked), None)
    }

    /// The same for a caller already holding the environment and its prefix
    /// exclusively, like `new` right after creating them. The locks are kept
    /// and relabeled, so nothing can start in between.
    pub fn prepare_with(
        mut guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
    ) -> Result<Self> {
        guard.relabel(Operation::Install)?;
        Self::claim(guard, paths, global, name, program, None, None)
    }

    /// Keeps creation and its installer on the same resolved runner.
    pub fn prepare_with_runner(
        mut guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        runner: &crate::runner::Runner,
    ) -> Result<Self> {
        guard.relabel(Operation::Install)?;
        Self::claim(guard, paths, global, name, program, None, Some(runner))
    }

    /// Reads the environment again under the lock, so what runs is what its
    /// config says now rather than when someone started answering questions.
    fn claim(
        guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        locked: Option<&Path>,
        fixed_runner: Option<&crate::runner::Runner>,
    ) -> Result<Self> {
        let mut environment = Environment::load(paths, name)?;
        if let Some(runner) = fixed_runner {
            let wine = environment
                .config
                .wine_mut()
                .ok_or_else(|| Error::NoPrefix(name.to_owned()))?;
            wine.runner = runner.name().to_owned();
        }
        let mut launch = launch(paths, global, &environment, program)?;
        let Host::Wine {
            runner: launch_runner,
            prefix,
        } = &mut launch.host
        else {
            return Err(Error::NoPrefix(name.to_owned()));
        };
        if let Some(runner) = fixed_runner {
            *launch_runner = runner.clone();
        }
        if locked
            .is_some_and(|locked| crate::paths::resolve(locked) != crate::paths::resolve(prefix))
        {
            return Err(Error::Changed(name.to_owned()));
        }
        let runtime = Runtime::new(paths, name);
        runtime.check_quiet(name)?;
        Ok(Self {
            guard,
            launch,
            runtime,
            program: program.to_owned(),
        })
    }

    /// What will run, for a caller that reconciles the prefix under the same
    /// lock before the installer starts.
    pub fn launch(&self) -> &Launch {
        &self.launch
    }

    /// Runs the installer until it and everything it started have exited. The
    /// first SIGINT, SIGTERM, or SIGHUP asks programs to close, and another one
    /// or `stop_timeout` kills them. An error means nothing was started.
    pub fn run(
        self,
        stop_timeout: Duration,
        output: Output,
        watched: impl FnMut(Watched),
    ) -> Result<Outcome> {
        self.run_with_writer(stop_timeout, output, watched, |runtime, state| {
            runtime.write_operation(state)
        })
    }

    fn run_with_writer(
        self,
        stop_timeout: Duration,
        output: Output,
        mut watched: impl FnMut(Watched),
        mut write_operation: impl FnMut(&Runtime, &OperationState) -> Result<()>,
    ) -> Result<Outcome> {
        let Self {
            guard,
            launch,
            runtime,
            program,
        } = self;
        let Host::Wine { runner, prefix } = &launch.host else {
            unreachable!("an installer is only claimed for a launch in a prefix");
        };
        let mut record = OperationState {
            operation: Operation::Install,
            program,
            runner: runner.clone(),
            prefix: prefix.clone(),
            pid: None,
            group: None,
            boot_id: None,
            leader_start: None,
            started_at: Timestamp::now(),
        };
        // Listening starts first, so a Ctrl+C that comes early is still held
        // for the loop below rather than killing wineo with a record written.
        let supervisor = Supervisor::listen()?;
        // Written before anything starts, so a crash can never leave something
        // running that nothing records.
        write_operation(&runtime, &record)?;
        let mut child = match command(&launch, output).and_then(|mut command| {
            command
                .spawn()
                .context(|| format!("failed to start {}", runner.launcher()))
        }) {
            Ok(child) => child,
            Err(err) => {
                // Nothing is running, so there is nothing left to protect.
                let _ = runtime.clear_operation();
                return Err(err);
            }
        };
        record.pid = Some(child.id());
        record.group = Some(child.id());
        record.boot_id = process::boot_id();
        record.leader_start = process::start_time(child.id());
        if let Err(err) = write_operation(&runtime, &record) {
            let cleanup = runner.kill_owned(
                prefix,
                record.group,
                record.boot_id.as_deref(),
                record.leader_start,
            );
            let _ = child.kill();
            let exit_code = child.wait().ok().map(crate::runtime::exit_code);
            let settled = match cleanup {
                Ok(process::GroupOwnership::Owned | process::GroupOwnership::Gone) => {
                    settle(&record, GIVE_UP)
                }
                Ok(process::GroupOwnership::Unknown) => Err(Error::Unsettled(
                    "installer process ownership could not be confirmed".to_owned(),
                )),
                Err(cleanup) => Err(cleanup),
            };
            return match settled {
                Ok(()) => match runtime.clear_operation() {
                    Ok(()) => Err(err),
                    Err(cleanup) => Ok(Outcome::Unresolved {
                        exit_code,
                        canceled: false,
                        error: cleanup,
                    }),
                },
                Err(cleanup) => Ok(Outcome::Unresolved {
                    exit_code,
                    canceled: false,
                    error: Error::Unsettled(format!(
                        "failed to save installer process identity: {err}; {cleanup}"
                    )),
                }),
            };
        }

        let ending = supervisor.watch(
            &launch.host,
            record.group,
            record.boot_id.clone(),
            record.leader_start,
            child,
            stop_timeout,
            Settle::Confirmed,
            Some(GIVE_UP),
            &mut watched,
        );
        drop(supervisor);

        let outcome = match ending.settled {
            Ok(()) => {
                // Everything has stopped either way. A record that can't be
                // removed only makes the next command point at `wineo stop`.
                if let Err(err) = runtime.clear_operation() {
                    watched(Watched::Notice(Notice::Warning(err)));
                }
                if ending.stopped {
                    Outcome::Canceled {
                        exit_code: ending.exit_code,
                    }
                } else {
                    Outcome::Finished {
                        exit_code: ending.exit_code,
                    }
                }
            }
            Err(error) => Outcome::Unresolved {
                exit_code: ending.exit_code,
                canceled: ending.stopped,
                error,
            },
        };
        // Only now, once everything is known to have stopped or is recorded as
        // unresolved, can anything else have the prefix.
        drop(guard);
        Ok(outcome)
    }
}

fn launch(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    program: &str,
) -> Result<Launch> {
    Launch::exec(paths, global, environment, program, &[], &BTreeMap::new())
}

fn command(launch: &Launch, output: Output) -> Result<Command> {
    let mut command = launch.command();
    // A group of its own keeps the terminal's Ctrl+C away from the installer,
    // so wineo can ask it to close, and lets a stop reach everything it
    // started. It has no terminal to read from, so it can't stop for input.
    command.stdin(Stdio::null()).process_group(0);
    if let Output::Log(path) = output {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
        }
        let log = File::create(path).context(|| format!("failed to create {}", path.display()))?;
        let stderr = log
            .try_clone()
            .context(|| format!("failed to open {}", path.display()))?;
        command.stdout(log).stderr(stderr);
    }
    Ok(command)
}

/// What cleaning up after an unresolved installer found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleaned {
    /// Whatever it left running has been stopped.
    Stopped,
    /// Nothing it started was still running.
    Gone,
    /// It recorded no process group, so there was nothing to signal.
    Untracked,
    /// Its identity or cleanup could not be confirmed, so the record remains.
    Unresolved,
}

/// Stops whatever an installer left running when it could not confirm that
/// everything had stopped, then clears its record so the environment can be
/// used again. None when there was nothing to clean up.
pub fn clean_up(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    timeout: Duration,
) -> Result<Option<Cleaned>> {
    let runtime = Runtime::new(paths, name);
    let Some(target) = cleanup_target(paths, global, name, &runtime)? else {
        return Ok(None);
    };
    let _guard = Locks::new(paths).acquire(Operation::Cleanup, name, &[&target.prefix])?;
    // Read again under the lock, in case another cleanup got there first.
    let Some(target) = cleanup_target(paths, global, name, &runtime)? else {
        return Ok(None);
    };
    let cleaned = match target.group_ownership() {
        process::GroupOwnership::Unknown => return Ok(Some(Cleaned::Unresolved)),
        process::GroupOwnership::Gone => {
            settle(&target, timeout)?;
            Cleaned::Gone
        }
        process::GroupOwnership::Owned => {
            match target.runner.kill_owned(
                &target.prefix,
                target.group,
                target.boot_id.as_deref(),
                target.leader_start,
            )? {
                process::GroupOwnership::Unknown => return Ok(Some(Cleaned::Unresolved)),
                process::GroupOwnership::Gone => {
                    settle(&target, timeout)?;
                    Cleaned::Gone
                }
                process::GroupOwnership::Owned => {
                    settle(&target, timeout)?;
                    Cleaned::Stopped
                }
            }
        }
    };
    runtime.clear_operation()?;
    Ok(Some(cleaned))
}

/// The record cleanup acts on. An unreadable record cannot identify what is
/// safe to stop, so it remains unresolved.
fn cleanup_target(
    _paths: &Paths,
    _global: &GlobalConfig,
    _name: &str,
    runtime: &Runtime,
) -> Result<Option<OperationState>> {
    runtime.read_operation()
}

/// Waits for a kill to take, giving up after `timeout` rather than hanging.
fn settle(record: &OperationState, timeout: Duration) -> Result<()> {
    let (sender, settled) = mpsc::channel();
    let (runner, prefix, group, boot_id, leader_start) = (
        record.runner.clone(),
        record.prefix.clone(),
        record.group,
        record.boot_id.clone(),
        record.leader_start,
    );
    thread::spawn(move || {
        let _ = sender.send(runner.wait_settled_owned(
            &prefix,
            group,
            boot_id.as_deref(),
            leader_start,
        ));
    });
    settled
        .recv_timeout(timeout)
        .unwrap_or(Err(Error::StillRunning(timeout.as_secs())))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;
    use crate::runner::Runner;

    fn paths(root: &Path) -> Paths {
        let home = root.to_owned();
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap()
    }

    fn runtime(root: &Path) -> Runtime {
        Runtime::new(&paths(root), "hades")
    }

    fn installer(root: &Path, wine: &str) -> Installer {
        let locks = Locks::at(root.join("locks"));
        let prefix = root.join("prefix");
        Installer {
            guard: locks
                .acquire(Operation::Install, "hades", &[&prefix])
                .unwrap(),
            launch: Launch {
                host: Host::Wine {
                    runner: Runner::wine_build(
                        "fake".to_owned(),
                        PathBuf::from(wine),
                        PathBuf::from("/bin/true"),
                    ),
                    prefix,
                },
                target: None,
                args: vec!["setup.exe".to_owned()],
                cwd: root.to_owned(),
                env: BTreeMap::new(),
                gamemode: None,
                gameid: None,
                store: None,
            },
            runtime: runtime(root),
            program: "setup.exe".to_owned(),
        }
    }

    #[test]
    fn an_installer_that_cannot_start_leaves_no_record() {
        let root = tempfile::tempdir().unwrap();
        let err = installer(root.path(), "/nonexistent/wine")
            .run(Duration::from_secs(1), Output::Inherit, |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("failed to start wine"), "{err}");
        assert!(runtime(root.path()).read_operation().unwrap().is_none());
    }

    #[test]
    fn a_finished_installer_clears_its_record() {
        let root = tempfile::tempdir().unwrap();
        let outcome = installer(root.path(), "/bin/true")
            .run(Duration::from_secs(1), Output::Inherit, |_| {})
            .unwrap();
        assert!(
            matches!(outcome, Outcome::Finished { exit_code: Some(0) }),
            "{outcome:?}"
        );
        assert!(runtime(root.path()).read_operation().unwrap().is_none());
    }

    #[test]
    fn a_post_spawn_identity_write_failure_stops_the_live_child_before_returning() {
        let root = tempfile::tempdir().unwrap();
        let mut installer = installer(root.path(), "/bin/sleep");
        installer.launch.args = vec!["30".to_owned()];
        let mut writes = 0;
        let mut identity = None;
        let error = installer
            .run_with_writer(
                Duration::from_secs(1),
                Output::Inherit,
                |_| {},
                |runtime, state| {
                    writes += 1;
                    if writes == 1 {
                        runtime.write_operation(state)
                    } else {
                        identity = Some((state.group, state.boot_id.clone(), state.leader_start));
                        Err(Error::Unsettled(
                            "injected identity write failure".to_owned(),
                        ))
                    }
                },
            )
            .unwrap_err();

        assert_eq!(error.to_string(), "injected identity write failure");
        assert_eq!(writes, 2);
        assert!(runtime(root.path()).read_operation().unwrap().is_none());
        let (Some(group), Some(boot), Some(leader_start)) = identity.unwrap() else {
            panic!("the launched child's full identity should be available");
        };
        assert_eq!(
            process::group_ownership(Some(group), Some(&boot), Some(leader_start)),
            process::GroupOwnership::Gone
        );
    }

    #[test]
    fn a_post_spawn_identity_write_failure_keeps_unsettled_work_blocked() {
        let root = tempfile::tempdir().unwrap();
        let mut installer = installer(root.path(), "/bin/sh");
        let started = root.path().join("child-started");
        installer.launch.args = vec![
            "-c".to_owned(),
            format!("sleep 1 & echo ready > '{}'; wait", started.display()),
        ];
        let mut writes = 0;
        let mut group = None;
        let outcome = installer
            .run_with_writer(
                Duration::from_secs(1),
                Output::Inherit,
                |_| {},
                |runtime, state| {
                    writes += 1;
                    if writes == 1 {
                        runtime.write_operation(state)
                    } else {
                        group = state.group;
                        let deadline = std::time::Instant::now() + Duration::from_secs(2);
                        while !started.exists() && std::time::Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(Error::Unsettled(
                            "injected identity write failure".to_owned(),
                        ))
                    }
                },
            )
            .unwrap();

        assert!(matches!(outcome, Outcome::Unresolved { .. }), "{outcome:?}");
        let runtime = runtime(root.path());
        assert!(runtime.read_operation().unwrap().is_some());
        assert!(matches!(
            runtime.check_resolved("hades"),
            Err(Error::Unresolved(_))
        ));
        let group = group.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while process::group_has_live_members(group) != Some(false)
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(process::group_has_live_members(group), Some(false));
    }

    #[test]
    fn output_can_go_to_a_log_that_each_run_replaces() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("state/install.log");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(&log, "the previous run\n").unwrap();
        // `echo` stands in for wine, so what it prints is the arguments.
        installer(root.path(), "/bin/echo")
            .run(Duration::from_secs(1), Output::Log(&log), |_| {})
            .unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "setup.exe\n");
    }

    #[test]
    fn records_round_trip_and_are_what_refuses_other_work() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        assert!(runtime.check_resolved("hades").is_ok());
        let record = OperationState {
            operation: Operation::Install,
            program: "setup.exe".to_owned(),
            runner: Runner::fake("wine"),
            prefix: root.path().join("prefix"),
            pid: Some(42),
            group: Some(42),
            boot_id: None,
            leader_start: None,
            started_at: Timestamp::now(),
        };
        runtime.write_operation(&record).unwrap();
        let read = runtime.read_operation().unwrap().unwrap();
        assert_eq!(read.group, Some(42));
        assert_eq!(read.runner.name(), "wine");
        assert!(matches!(
            runtime.check_resolved("hades"),
            Err(Error::Unresolved(_))
        ));

        // Garbage still counts, since it says something was left unresolved.
        fs::write(runtime.dir().join("operation.toml"), "not toml [").unwrap();
        assert!(matches!(
            runtime.check_resolved("hades"),
            Err(Error::Unresolved(_))
        ));
        runtime.clear_operation().unwrap();
        runtime.clear_operation().unwrap();
        assert!(runtime.check_resolved("hades").is_ok());
    }

    #[test]
    fn cleanup_keeps_an_old_identity_unresolved_and_blocked() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runtime = Runtime::new(&paths, "hades");
        let record = OperationState {
            operation: Operation::Install,
            program: "setup.exe".to_owned(),
            runner: Runner::fake("wine"),
            prefix: root.path().join("prefix-alias"),
            pid: Some(1234),
            group: Some(1234),
            boot_id: None,
            leader_start: None,
            started_at: Timestamp::now(),
        };
        runtime.write_operation(&record).unwrap();

        let result = clean_up(
            &paths,
            &GlobalConfig::default(),
            "hades",
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(result, Some(Cleaned::Unresolved));
        assert!(runtime.read_operation().unwrap().is_some());
        assert!(matches!(
            runtime.check_resolved("hades"),
            Err(Error::Unresolved(_))
        ));
    }

    #[test]
    fn cleanup_does_not_remove_an_unreadable_operation_record() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runtime = Runtime::new(&paths, "hades");
        fs::create_dir_all(runtime.dir()).unwrap();
        let text = "not toml [";
        fs::write(runtime.dir().join("operation.toml"), text).unwrap();

        let result = clean_up(
            &paths,
            &GlobalConfig::default(),
            "hades",
            Duration::from_secs(1),
        );

        assert!(matches!(result, Err(Error::StateSyntax { .. })));
        assert_eq!(
            fs::read_to_string(runtime.dir().join("operation.toml")).unwrap(),
            text
        );
    }
}
