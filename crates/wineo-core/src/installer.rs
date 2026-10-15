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

use crate::build_lock;
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::launch::{ExecResolution, Host, Launch};
use crate::operation::{Guard, Locks, Operation};
use crate::paths::Paths;
use crate::process;
use crate::runners::Runners;
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

/// What an installer runs with besides its program. Empty by default, as for
/// an installer someone picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub args: Vec<String>,
    /// Set on top of the environment's own variables.
    pub env: BTreeMap<String, String>,
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
    build_guard: Option<build_lock::Guard>,
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
        Self::prepare_args(paths, global, name, program, &Args::default())
    }

    /// `prepare`, for a program run with arguments and extra environment
    /// variables, like an installer run silently.
    pub fn prepare_args(
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        args: &Args,
    ) -> Result<Self> {
        // Resolved once to know which prefix to lock, then again under it.
        let environment = Environment::load(paths, name)?;
        let locked = launch(
            paths,
            global,
            &environment,
            program,
            args,
            ExecResolution::CurrentDirectory,
        )?
        .host
        .prefix()
        .ok_or_else(|| environment.config.no_prefix(name))?
        .to_owned();
        let guard = Locks::new(paths).acquire(Operation::Install, name, &[&locked])?;
        Self::claim(
            guard,
            paths,
            global,
            name,
            program,
            args,
            Some(&locked),
            None,
            ExecResolution::CurrentDirectory,
        )
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
        Self::claim(
            guard,
            paths,
            global,
            name,
            program,
            &Args::default(),
            None,
            None,
            ExecResolution::CurrentDirectory,
        )
    }

    /// `prepare_args`, for a caller already holding the environment and
    /// `locked`, its prefix, exclusively. The locks are kept and relabeled.
    pub fn prepare_args_with(
        mut guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        args: &Args,
        locked: &Path,
    ) -> Result<Self> {
        guard.relabel(Operation::Install)?;
        Self::claim(
            guard,
            paths,
            global,
            name,
            program,
            args,
            Some(locked),
            None,
            ExecResolution::CurrentDirectory,
        )
    }

    /// `prepare_args_with` for a program in the prefix named by itself, like
    /// `cmd`, which Wine looks up there whatever files the current directory
    /// holds.
    pub fn prepare_prefix_program_with(
        mut guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        args: &Args,
        locked: &Path,
    ) -> Result<Self> {
        guard.relabel(Operation::Install)?;
        Self::claim(
            guard,
            paths,
            global,
            name,
            program,
            args,
            Some(locked),
            None,
            ExecResolution::ExplicitPaths,
        )
    }

    /// Keeps creation and its installer on the same resolved runner.
    pub fn prepare_with_runner(
        guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        runner: &crate::runner::Runner,
    ) -> Result<Self> {
        Self::prepare_with_runner_args(
            guard,
            paths,
            global,
            name,
            program,
            &Args::default(),
            runner,
        )
    }

    /// `prepare_with_runner`, with arguments and extra environment variables.
    pub fn prepare_with_runner_args(
        mut guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        args: &Args,
        runner: &crate::runner::Runner,
    ) -> Result<Self> {
        guard.relabel(Operation::Install)?;
        Self::claim(
            guard,
            paths,
            global,
            name,
            program,
            args,
            None,
            Some(runner),
            ExecResolution::CurrentDirectory,
        )
    }

    /// Reads the environment again under the lock, so what runs is what its
    /// config says now rather than when someone started answering questions.
    #[allow(clippy::too_many_arguments)]
    fn claim(
        guard: Guard,
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        program: &str,
        args: &Args,
        locked: Option<&Path>,
        fixed_runner: Option<&crate::runner::Runner>,
        resolution: ExecResolution,
    ) -> Result<Self> {
        let mut environment = Environment::load(paths, name)?;
        if let Some(runner) = fixed_runner {
            let wine = environment.config.require_wine_mut(name)?;
            wine.runner = runner.name().to_owned();
        }
        let (runner, build_guard) = if let Some(runner) = fixed_runner {
            if crate::runner_alias::is_alias(runner.selection()) {
                Runners::new(paths, global)?.lock_resolved(
                    paths,
                    runner.selection(),
                    runner.clone(),
                )?
            } else {
                (runner.clone(), build_lock::runner(paths, runner)?)
            }
        } else {
            Runners::new(paths, global)?.resolve_locked(paths, &environment.wine()?.runner)?
        };
        environment.config.require_wine_mut(name)?.runner = runner.name().to_owned();
        let mut launch = launch(paths, global, &environment, program, args, resolution)?;
        let Host::Wine {
            runner: launch_runner,
            prefix,
        } = &mut launch.host
        else {
            return Err(environment.config.no_prefix(name));
        };
        *launch_runner = runner;
        if locked
            .is_some_and(|locked| crate::paths::resolve(locked) != crate::paths::resolve(prefix))
        {
            return Err(Error::Changed(name.to_owned()));
        }
        let runtime = Runtime::new(paths, name);
        runtime.check_quiet(name)?;
        Ok(Self {
            guard,
            build_guard,
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
    /// first cancel request asks programs to close, and a force request or
    /// `stop_timeout` kills them. An error means nothing was started.
    pub fn run(
        self,
        token: &Token,
        stop_timeout: Duration,
        output: Output,
        watched: impl FnMut(Watched),
    ) -> Result<Outcome> {
        self.run_with_writer(token, stop_timeout, output, watched, |runtime, state| {
            runtime.write_operation(state)
        })
    }

    /// `run`, handing back the environment's and prefix's locks instead of
    /// letting go of them, for a caller that runs more in the prefix and has
    /// to keep everything else out until it's done.
    pub fn run_keeping(
        self,
        token: &Token,
        stop_timeout: Duration,
        output: Output,
        watched: impl FnMut(Watched),
    ) -> (Result<Outcome>, Guard) {
        self.run_keeping_with(token, stop_timeout, output, watched, |runtime, state| {
            runtime.write_operation(state)
        })
    }

    fn run_with_writer(
        self,
        token: &Token,
        stop_timeout: Duration,
        output: Output,
        watched: impl FnMut(Watched),
        write_operation: impl FnMut(&Runtime, &OperationState) -> Result<()>,
    ) -> Result<Outcome> {
        // Only once everything is known to have stopped or is recorded as
        // unresolved can anything else have the prefix.
        self.run_keeping_with(token, stop_timeout, output, watched, write_operation)
            .0
    }

    fn run_keeping_with(
        self,
        token: &Token,
        stop_timeout: Duration,
        output: Output,
        watched: impl FnMut(Watched),
        write_operation: impl FnMut(&Runtime, &OperationState) -> Result<()>,
    ) -> (Result<Outcome>, Guard) {
        let Self {
            guard,
            build_guard,
            launch,
            runtime,
            program,
        } = self;
        let outcome = run_parts(
            build_guard,
            launch,
            runtime,
            program,
            (token, stop_timeout, output),
            watched,
            write_operation,
        );
        (outcome, guard)
    }
}

fn run_parts(
    build_guard: Option<build_lock::Guard>,
    launch: Launch,
    runtime: Runtime,
    program: String,
    (token, stop_timeout, output): (&Token, Duration, Output),
    mut watched: impl FnMut(Watched),
    mut write_operation: impl FnMut(&Runtime, &OperationState) -> Result<()>,
) -> Result<Outcome> {
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
    let supervisor = Supervisor::with_token(token);
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
    process::quiet_core_dumps(&child);
    record.pid = Some(child.id());
    record.group = Some(child.id());
    record.boot_id = process::boot_id();
    record.leader_start = process::start_time(child.id());
    if let Err(err) = write_operation(&runtime, &record) {
        let cleanup = runner.kill_owned(prefix, record.identity().as_ref());
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
        record.identity(),
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
    drop(build_guard);
    Ok(outcome)
}

fn launch(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    program: &str,
    args: &Args,
    resolution: ExecResolution,
) -> Result<Launch> {
    Launch::exec_with_resolution(
        paths,
        global,
        environment,
        program,
        &args.args,
        &args.env,
        resolution,
    )
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
    clean_up_with(paths, global, name, timeout, |_| Ok(()))
}

fn clean_up_with(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    timeout: Duration,
    locked: impl FnOnce(&OperationState) -> Result<()>,
) -> Result<Option<Cleaned>> {
    let runtime = Runtime::new(paths, name);
    let Some(target) = cleanup_target(&runtime)? else {
        return Ok(None);
    };
    let _guard = Locks::new(paths).acquire(Operation::Cleanup, name, &[&target.prefix])?;
    // Read again under the lock, in case another cleanup got there first.
    let Some(target) = cleanup_target(&runtime)? else {
        return Ok(None);
    };
    let _runner_guard = if target.runner.name() == crate::runner::Runner::SYSTEM {
        None
    } else {
        // Cleanup uses the recorded runner even after its build directory is gone.
        build_lock::shared(paths, build_lock::Kind::Runner, target.runner.name()).ok()
    };
    // Build discovery must not prevent cleanup of the recorded operation.
    let environment = Environment::load(paths, name).ok();
    let wine = environment
        .as_ref()
        .and_then(|environment| environment.config.wine());
    let _component_guards = build_lock::config(
        paths,
        global,
        None,
        wine.and_then(|wine| wine.dxvk.as_deref()),
        wine.and_then(|wine| wine.vkd3d.as_deref()),
    )
    .unwrap_or_default();
    locked(&target)?;
    let cleaned = match target.group_ownership() {
        process::GroupOwnership::Unknown => return Ok(Some(Cleaned::Unresolved)),
        process::GroupOwnership::Gone => {
            settle(&target, timeout)?;
            Cleaned::Gone
        }
        process::GroupOwnership::Owned => {
            match target
                .runner
                .kill_owned(&target.prefix, target.identity().as_ref())?
            {
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

/// Recorded runners can remain in use after an installer watcher releases its guards.
/// Components are copied into the prefix at install time, so leftover processes
/// do not use their build directories.
pub fn unresolved_builds(paths: &Paths) -> Result<Vec<(String, build_lock::Kind, String)>> {
    let root = paths.runtime_dir("");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", root.display())),
    };
    let mut references = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", root.display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !crate::name::is_valid(&name) || !entry.path().is_dir() {
            continue;
        }
        let Some(record) = Runtime::new(paths, &name).read_operation()? else {
            continue;
        };
        if record.operation != Operation::Install {
            continue;
        }
        if record.runner.name() != crate::runner::Runner::SYSTEM {
            references.push((
                name.clone(),
                build_lock::Kind::Runner,
                record.runner.name().to_owned(),
            ));
        }
    }
    references.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    Ok(references)
}

/// How long `clean_up` waits, the way `wineo stop` runs it: the stop timeout,
/// plus room for the kill and the prefix shutting down.
pub fn clean_up_timeout(global: &GlobalConfig) -> Duration {
    Duration::from_secs(u64::from(global.stop_timeout) + 10)
}

/// What to tell someone when cleanup after an installer could not be confirmed.
pub fn unresolved_message(paths: &Paths, name: &str) -> String {
    let record = Runtime::new(paths, name).operation_path();
    format!(
        "wineo could not confirm cleanup after {name}'s last installer. Check the prefix, then remove {} only after confirming nothing it started is still running",
        record.display()
    )
}

/// The record cleanup acts on. An unreadable record cannot identify what is
/// safe to stop, so it remains unresolved.
fn cleanup_target(runtime: &Runtime) -> Result<Option<OperationState>> {
    runtime.read_operation()
}

/// Waits for a kill to take, giving up after `timeout` rather than hanging.
fn settle(record: &OperationState, timeout: Duration) -> Result<()> {
    let (sender, settled) = mpsc::channel();
    let (runner, prefix, identity) = (
        record.runner.clone(),
        record.prefix.clone(),
        record.identity(),
    );
    thread::spawn(move || {
        let _ = sender.send(runner.wait_settled_owned(&prefix, identity.as_ref()));
    });
    settled
        .recv_timeout(timeout)
        .unwrap_or(Err(Error::StillRunning(timeout.as_secs())))
}

#[cfg(test)]
mod tests;
