use super::*;
use crate::launch::SteamRequest;
use crate::steam;

const REQUEST_COMMAND: &str = "__steam_request";

/// How long a launch waits for a client it started to appear before sending
/// the launch anyway. Steam may update itself on start.
const CLIENT_START_WAIT: Duration = Duration::from_secs(60);
const CLIENT_POLL: Duration = Duration::from_millis(250);

pub(super) fn dispatch_request_entry() -> Option<Result<()>> {
    std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == REQUEST_COMMAND)
        .then(|| {
            setsid()
                .map_err(io::Error::from)
                .context(|| "failed to isolate Steam request".to_owned())?;
            let mut input = String::new();
            io::stdin()
                .read_to_string(&mut input)
                .context(|| "failed to read Steam request".to_owned())?;
            let launch: Launch =
                toml::from_str(&input).map_err(|err| Error::StartFailed(err.to_string()))?;
            if !matches!(launch.host, Host::Proxied { .. }) {
                return Err(Error::StartFailed("invalid Steam request".to_owned()));
            }
            let (first, cold) = match silent_start(&launch) {
                Some(command) => (command, true),
                None => (launch.command(), false),
            };
            let (started, report) = match spawn_quiet(first) {
                Ok(child) => (Some(child), "spawned\n".to_owned()),
                Err(err) => (None, format!("failed {err}\n")),
            };
            io::stdout()
                .write_all(report.as_bytes())
                .context(|| "failed to report Steam request".to_owned())?;
            // The launch follows the report, so `start`'s own clock covers the
            // client's boot as it did when both went out together. Nothing
            // is left to report to if this spawn fails.
            if cold
                && let Some(mut silent) = started
                && let Host::Proxied { install, .. } = &launch.host
            {
                wait_for_client(install, &mut silent);
                let _ = spawn_quiet(launch.command());
            }
            Ok(())
        })
}

/// The silent start for a launch when no Steam client is running. Steam
/// ignores `-silent` when `-applaunch` arrives in the same cold start and
/// opens its library window, so the launch is sent once a client is up.
/// None when a client is running or the check can't tell, and the launch is
/// sent as it is.
fn silent_start(launch: &Launch) -> Option<Command> {
    let Host::Proxied {
        install,
        request: SteamRequest::Launch,
        ..
    } = &launch.host
    else {
        return None;
    };
    if !matches!(steam::client_running(install), Ok(false)) {
        return None;
    }
    launch.silent_start_command()
}

/// Waits for the client a silent start began. Steam may update itself first,
/// so this is long, and it ends early when the check fails, since the launch
/// is better sent than lost. The start command is reaped if it exits early,
/// so it doesn't linger as a zombie for the wait.
fn wait_for_client(install: &steam::Install, silent: &mut Child) {
    let deadline =
        Instant::now() + test_timeout("WINEO_TEST_STEAM_CLIENT_WAIT_MS", CLIENT_START_WAIT);
    wait_until(deadline, CLIENT_POLL, || {
        let _ = silent.try_wait();
        steam::client_running(install)
    });
}

/// Polls until `running` says yes, fails, or the deadline passes. A failed
/// check ends the wait the same way a yes does.
fn wait_until(deadline: Instant, poll: Duration, mut running: impl FnMut() -> io::Result<bool>) {
    loop {
        if !matches!(running(), Ok(false)) || Instant::now() >= deadline {
            return;
        }
        thread::sleep(poll);
    }
}

fn spawn_quiet(mut command: Command) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Hands a request to the Steam client from a process of its own, so Steam is
/// never in wineo's process group. Returns once Steam's command was spawned.
/// When that was a silent start of the client, the launch itself follows from
/// the helper once the client is up.
pub(crate) fn request(launch: &Launch) -> Result<()> {
    let exe = std::env::current_exe().context(|| "failed to locate wineo".to_owned())?;
    let mut child = Command::new(exe)
        .arg(REQUEST_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context(|| "failed to dispatch Steam request".to_owned())?;
    let text = toml::to_string(launch).map_err(|err| Error::StartFailed(err.to_string()))?;
    let mut input = child.stdin.take().expect("piped input");
    input
        .write_all(text.as_bytes())
        .context(|| "failed to send Steam request".to_owned())?;
    drop(input);
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("piped output"))
        .read_line(&mut line)
        .context(|| "failed to read Steam handoff".to_owned())?;
    // The helper may outlive its report to send a launch, so it's reaped aside.
    thread::spawn(move || child.wait());
    if line == "spawned\n" {
        Ok(())
    } else {
        Err(Error::StartFailed(format!(
            "failed to start Steam: {}",
            line.strip_prefix("failed ")
                .unwrap_or("the request exited without reporting back")
                .trim()
        )))
    }
}

impl Supervisor {
    pub fn begin_steam(
        &self,
        runtime: &Runtime,
        launch: &Launch,
        mode: RunMode,
        overrides: &Overrides,
        name: &str,
        mut notify: impl FnMut(Notice),
    ) -> Result<(RunState, bool)> {
        let Host::Proxied {
            install,
            app_id,
            baseline: Some(baseline),
            ..
        } = &launch.host
        else {
            unreachable!()
        };
        let candidates = steam::eligible_reapers(install, app_id, baseline)
            .context(|| "failed to inspect Steam processes".to_owned())?;
        if candidates.len() > 1 {
            return Err(ambiguous(name));
        }
        let adopted = !candidates.is_empty();
        let mut state = RunState::running(mode, "steam".to_owned(), 0, None, overrides);
        state.record_host(&launch.host);
        if let Some(reaper) = candidates.first() {
            claim(&mut state, reaper);
        } else {
            if self.steam_stop().is_some() {
                return Err(Error::Canceled("launch"));
            }
            request(launch)?;
            state.phase = Some(crate::runtime::RunPhase::Starting);
        }
        write_state(runtime, &state, &mut notify);
        Ok((state, adopted))
    }

    pub fn wait_steam(
        &self,
        runtime: &Runtime,
        launch: &Launch,
        state: &mut RunState,
        name: &str,
        timeout: Duration,
        mut notify: impl FnMut(Notice),
    ) -> Result<()> {
        let result = self.watch_steam(runtime, launch, state, name, timeout, &mut notify);
        match result {
            Ok(status) => {
                state.finish(status);
                write_state(runtime, state, &mut notify);
                Ok(())
            }
            Err(err) => {
                // The after hook releases the run lock before CLI error reporting.
                if state.mode == RunMode::Detached {
                    let path = runtime.log_path();
                    if let Err(log_err) = OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .and_then(|mut log| writeln!(log, "error: {}", error::chain(&err)))
                        .context(|| format!("failed to write {}", path.display()))
                    {
                        notify(Notice::Warning(log_err));
                    }
                }
                state.finish(RunStatus::Failed);
                write_state(runtime, state, &mut notify);
                Err(err)
            }
        }
    }

    fn steam_stop(&self) -> Option<bool> {
        self.stop_request(None)
    }

    /// Whether a stop was asked for, and whether it is a force. `received` is
    /// an event the caller already took off the channel while waiting, so it
    /// isn't lost.
    fn stop_request(&self, received: Option<Event>) -> Option<bool> {
        if self.token.as_ref().is_some_and(Token::is_forced) {
            return Some(true);
        }
        if self.token.as_ref().is_some_and(Token::is_canceled) {
            return Some(false);
        }
        // A wakeup that isn't a stop must not hide one queued behind it, so
        // the whole queue is read. A second stop is the escalation to a
        // force, which would otherwise be swallowed here.
        let mut stop = None;
        for event in received.into_iter().chain(self.events.try_iter()) {
            match event {
                Event::Force => return Some(true),
                Event::Stop if stop.is_some() => return Some(true),
                Event::Stop => stop = Some(false),
                _ => {}
            }
        }
        stop
    }

    fn watch_steam(
        &self,
        runtime: &Runtime,
        launch: &Launch,
        state: &mut RunState,
        name: &str,
        timeout: Duration,
        notify: &mut impl FnMut(Notice),
    ) -> Result<RunStatus> {
        let Host::Proxied {
            install,
            app_id,
            baseline: Some(baseline),
            ..
        } = &launch.host
        else {
            unreachable!()
        };
        let dispatched = Instant::now();
        let mut waiting = StartingWait::new(dispatched);
        let mut next_scan = dispatched;
        let mut next_manifest = dispatched;
        let mut heartbeat = Instant::now();
        let fast_scan = test_timeout(
            "WINEO_TEST_STEAM_SCAN_INTERVAL_MS",
            Duration::from_millis(250),
        );
        let start_limit =
            test_timeout("WINEO_TEST_STEAM_START_TIMEOUT_MS", Duration::from_secs(60));
        let stall_limit = test_timeout(
            "WINEO_TEST_STEAM_STALL_TIMEOUT_MS",
            Duration::from_secs(600),
        );
        while state.phase.is_some() {
            if let Some(force) = self.steam_stop() {
                notify(stop_notice(force));
                return Ok(RunStatus::Stopped);
            }
            let now = Instant::now();
            if now < next_scan {
                // Stop requests and token changes arrive as events, so there
                // is nothing to poll for between scans.
                if let Ok(event) = self.events.recv_timeout(next_scan - now)
                    && let Some(force) = self.stop_request(Some(event))
                {
                    notify(stop_notice(force));
                    return Ok(RunStatus::Stopped);
                }
                continue;
            }
            next_scan = now + scan_interval(now - dispatched, fast_scan);
            let candidates = steam::eligible_reapers(install, app_id, baseline)
                .context(|| "failed to inspect Steam processes".to_owned())?;
            if candidates.len() > 1 {
                return Err(ambiguous(name));
            }
            if let Some(reaper) = candidates.first() {
                claim(state, reaper);
                write_state(runtime, state, notify);
                break;
            }
            if now >= next_manifest {
                let (app_state, progress) = steam::app_snapshot(Some(install), app_id);
                waiting.observe(now, &app_state, progress);
                next_manifest = now + Duration::from_secs(1);
            }
            if waiting.expired(now, start_limit, stall_limit) {
                return Err(Error::StartFailed(format!("Steam didn't start {name}.")));
            }
            heartbeat_state(runtime, state, &mut heartbeat, notify);
        }
        let reaper = steam::Reaper {
            pid: state.pid,
            start_time: state.leader_start.expect("claimed start time"),
            boot_id: state.boot_id.clone().expect("claimed boot"),
        };
        let Some(fd) =
            steam::open_reaper(&reaper).context(|| "failed to watch Steam process".to_owned())?
        else {
            return Ok(RunStatus::Exited);
        };
        // The thread stays blocked in poll if this returns while the reaper is
        // still alive, which costs nothing until it exits or the process ends.
        let watcher = fd
            .try_clone()
            .context(|| "failed to watch Steam process".to_owned())?;
        let exited = self.sender.clone();
        thread::spawn(move || {
            let _ = steam::wait_pidfd_exit(&watcher);
            let _ = exited.send(Event::ReaperExited);
        });
        let mut received = None;
        loop {
            if let Some(force) = self.stop_request(received.take()) {
                notify(stop_notice(force));
                let mut announced = force;
                steam::stop_tree(&reaper, timeout, force, || {
                    let forced = self.token.as_ref().is_some_and(Token::is_forced)
                        || matches!(self.events.try_recv(), Ok(Event::Stop | Event::Force));
                    if forced && !announced {
                        notify(Notice::Killing);
                        announced = true;
                    }
                    forced
                })
                .context(|| "failed to stop Steam's game processes".to_owned())?;
                return Ok(RunStatus::Stopped);
            }
            if crate::process::pidfd_exited(&fd)
                .context(|| "failed to watch Steam process".to_owned())?
            {
                return Ok(RunStatus::Exited);
            }
            heartbeat_state(runtime, state, &mut heartbeat, notify);
            let due = heartbeat + HEARTBEAT_INTERVAL;
            // An exit event only prompts the pidfd check above, so a stop that
            // raced it is still seen first.
            received = self
                .events
                .recv_timeout(due.saturating_duration_since(Instant::now()))
                .ok();
        }
    }
}

fn stop_notice(force: bool) -> Notice {
    if force {
        Notice::Killing
    } else {
        Notice::Stopping
    }
}

fn claim(state: &mut RunState, reaper: &steam::Reaper) {
    state.pid = reaper.pid;
    state.boot_id = Some(reaper.boot_id.clone());
    state.leader_start = Some(reaper.start_time);
    state.phase = None;
    state.running_at = Some(jiff::Timestamp::now());
}

struct StartingWait {
    updating: bool,
    start_window: Instant,
    progressed_at: Instant,
    previous_progress: Option<steam::Progress>,
}

impl StartingWait {
    fn new(now: Instant) -> Self {
        Self {
            updating: false,
            start_window: now,
            progressed_at: now,
            previous_progress: None,
        }
    }

    fn observe(
        &mut self,
        now: Instant,
        state: &steam::AppState,
        progress: Option<steam::Progress>,
    ) {
        // A partial install is waited on like an update, so a state that
        // briefly drops FullyInstalled doesn't start the launch clock.
        let updating = matches!(
            state,
            steam::AppState::Updating | steam::AppState::Installing(_)
        );
        if self.updating && !updating {
            // Steam still needs time to launch after an update or a transient bad read.
            self.start_window = now;
        }
        self.updating = updating;
        if let Some(next) = progress {
            if self
                .previous_progress
                .is_none_or(|old| next.progressed_since(&old))
            {
                self.progressed_at = now;
            }
            self.previous_progress = Some(next);
        }
    }

    fn expired(&self, now: Instant, start_limit: Duration, stall_limit: Duration) -> bool {
        if self.updating {
            now.duration_since(self.progressed_at) >= stall_limit
        } else {
            now.duration_since(self.start_window) >= start_limit
        }
    }
}

fn write_state(runtime: &Runtime, state: &RunState, notify: &mut impl FnMut(Notice)) {
    if let Err(err) = runtime.write_state(state) {
        notify(Notice::Warning(err));
    }
}

fn heartbeat_state(
    runtime: &Runtime,
    state: &mut RunState,
    last: &mut Instant,
    notify: &mut impl FnMut(Notice),
) {
    if last.elapsed() >= HEARTBEAT_INTERVAL {
        state.heartbeat();
        write_state(runtime, state, notify);
        *last = Instant::now();
        notify(Notice::Heartbeat);
    }
}

fn ambiguous(name: &str) -> Error {
    Error::StartFailed(format!("More than one Steam process matches {name}."))
}

/// A launch usually lands within seconds, but Steam can spend minutes
/// updating the game first, so scans slow down after the first 10 s.
fn scan_interval(waited: Duration, fast: Duration) -> Duration {
    if waited < Duration::from_secs(10) {
        fast
    } else {
        fast.max(Duration::from_secs(1))
    }
}

fn test_timeout(_variable: &str, fallback: Duration) -> Duration {
    #[cfg(debug_assertions)]
    if let Some(millis) = std::env::var(_variable)
        .ok()
        .and_then(|value| value.parse().ok())
    {
        return Duration::from_millis(millis);
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_wait_ends_on_a_yes_an_error_or_the_deadline() {
        let poll = Duration::from_millis(5);
        let far = Instant::now() + Duration::from_secs(30);
        let mut checks = 0;
        wait_until(far, poll, || {
            checks += 1;
            Ok(checks == 3)
        });
        assert_eq!(checks, 3);

        // A check that can't tell sends the launch rather than waiting.
        let mut checks = 0;
        wait_until(far, poll, || {
            checks += 1;
            Err(io::Error::other("no /proc"))
        });
        assert_eq!(checks, 1);

        let started = Instant::now();
        wait_until(started + Duration::from_millis(60), poll, || Ok(false));
        assert!(started.elapsed() >= Duration::from_millis(60));
    }

    #[test]
    fn a_stop_queued_behind_a_wakeup_is_not_missed() {
        let token = Token::new();
        let supervisor = Supervisor::with_token(&token);
        supervisor.sender.send(Event::ReaperExited).unwrap();
        supervisor.sender.send(Event::Pulse).unwrap();
        supervisor.sender.send(Event::Stop).unwrap();
        let received = supervisor.events.recv().unwrap();
        assert!(matches!(received, Event::ReaperExited));
        assert_eq!(supervisor.stop_request(Some(received)), Some(false));
        assert_eq!(supervisor.stop_request(None), None);
    }

    #[test]
    fn a_second_queued_stop_is_a_force() {
        let token = Token::new();
        let supervisor = Supervisor::with_token(&token);
        supervisor.sender.send(Event::Stop).unwrap();
        supervisor.sender.send(Event::Pulse).unwrap();
        supervisor.sender.send(Event::Stop).unwrap();
        assert_eq!(supervisor.stop_request(None), Some(true));
        supervisor.sender.send(Event::Pulse).unwrap();
        supervisor.sender.send(Event::Stop).unwrap();
        assert_eq!(supervisor.stop_request(Some(Event::Stop)), Some(true));
    }

    #[test]
    fn a_queued_force_outranks_an_earlier_stop() {
        let token = Token::new();
        let supervisor = Supervisor::with_token(&token);
        supervisor.sender.send(Event::Stop).unwrap();
        supervisor.sender.send(Event::Pulse).unwrap();
        supervisor.sender.send(Event::Force).unwrap();
        assert_eq!(supervisor.stop_request(Some(Event::Pulse)), Some(true));
    }

    #[test]
    fn scans_slow_to_once_a_second_after_ten_seconds() {
        let fast = Duration::from_millis(250);
        assert_eq!(scan_interval(Duration::ZERO, fast), fast);
        assert_eq!(scan_interval(Duration::from_millis(9_999), fast), fast);
        assert_eq!(
            scan_interval(Duration::from_secs(10), fast),
            Duration::from_secs(1)
        );
        let slow = Duration::from_secs(2);
        assert_eq!(scan_interval(Duration::from_secs(30), slow), slow);
    }

    fn progress(flags: u32, staged: u64) -> steam::Progress {
        steam::Progress {
            state_flags: flags,
            bytes_downloaded: 100,
            bytes_staged: staged,
        }
    }

    #[test]
    fn finishing_an_update_restarts_the_launch_window() {
        let now = Instant::now();
        let start = Duration::from_secs(60);
        let stall = Duration::from_secs(600);
        let mut wait = StartingWait::new(now);
        wait.observe(now, &steam::AppState::Updating, Some(progress(1030, 0)));
        let finished = now + Duration::from_secs(120);
        wait.observe(finished, &steam::AppState::Installed, Some(progress(4, 10)));
        assert!(!wait.expired(finished + Duration::from_secs(59), start, stall));
        assert!(wait.expired(finished + start, start, stall));
    }

    #[test]
    fn unknown_manifest_after_updating_gets_a_launch_window() {
        let now = Instant::now();
        let mut wait = StartingWait::new(now);
        wait.observe(now, &steam::AppState::Updating, Some(progress(1030, 0)));
        let unreadable = now + Duration::from_secs(120);
        wait.observe(
            unreadable,
            &steam::AppState::Unknown("rewriting".to_owned()),
            None,
        );
        assert!(!wait.expired(
            unreadable,
            Duration::from_secs(60),
            Duration::from_secs(600)
        ));
        wait.observe(
            unreadable + Duration::from_secs(1),
            &steam::AppState::Installed,
            Some(progress(4, 1)),
        );
        assert!(!wait.expired(
            unreadable + Duration::from_secs(59),
            Duration::from_secs(60),
            Duration::from_secs(600)
        ));
    }

    #[test]
    fn progress_recovers_after_an_unreadable_first_snapshot() {
        let now = Instant::now();
        let mut wait = StartingWait::new(now);
        wait.observe(now, &steam::AppState::Unknown("rewriting".to_owned()), None);
        let updating = now + Duration::from_secs(1);
        wait.observe(updating, &steam::AppState::Updating, None);
        wait.observe(
            updating + Duration::from_secs(9),
            &steam::AppState::Updating,
            Some(progress(1030, 1)),
        );
        wait.observe(
            updating + Duration::from_secs(18),
            &steam::AppState::Updating,
            Some(progress(1030, 2)),
        );
        assert!(!wait.expired(
            updating + Duration::from_secs(20),
            Duration::from_secs(60),
            Duration::from_secs(10)
        ));
        assert!(wait.expired(
            updating + Duration::from_secs(28),
            Duration::from_secs(60),
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn transient_unknown_does_not_restart_the_update_stall_window() {
        let now = Instant::now();
        let mut wait = StartingWait::new(now);
        wait.observe(now, &steam::AppState::Updating, Some(progress(1030, 0)));
        wait.observe(
            now + Duration::from_secs(540),
            &steam::AppState::Unknown("rewriting".to_owned()),
            None,
        );
        wait.observe(
            now + Duration::from_secs(541),
            &steam::AppState::Updating,
            Some(progress(1030, 0)),
        );
        assert!(wait.expired(
            now + Duration::from_secs(600),
            Duration::from_secs(60),
            Duration::from_secs(600)
        ));
    }

    #[test]
    fn heartbeat_write_failure_warns_and_keeps_the_run_live() {
        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let runtime = Runtime::at(blocked);
        let mut state = RunState::running(
            RunMode::Attached,
            "steam".to_owned(),
            0,
            None,
            &Overrides::default(),
        );
        let mut last = Instant::now() - HEARTBEAT_INTERVAL;
        let mut warnings = 0;
        heartbeat_state(&runtime, &mut state, &mut last, &mut |notice| {
            if matches!(notice, Notice::Warning(_)) {
                warnings += 1;
            }
        });
        assert_eq!(warnings, 1);
        assert_eq!(state.status, RunStatus::Running);
        assert!(state.heartbeat_at.is_some());
    }
}
