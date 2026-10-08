use super::*;
use crate::steam;

const REQUEST_COMMAND: &str = "__steam_request";

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
            let report = match launch
                .command()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(_) => "spawned\n".to_owned(),
                Err(err) => format!("failed {err}\n"),
            };
            io::stdout()
                .write_all(report.as_bytes())
                .context(|| "failed to report Steam request".to_owned())
        })
}

fn request(launch: &Launch) -> Result<()> {
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
    let _ = child.wait();
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
        if self.token.as_ref().is_some_and(Token::is_forced) {
            return Some(true);
        }
        if self.token.as_ref().is_some_and(Token::is_canceled) {
            return Some(false);
        }
        match self.events.try_recv() {
            Ok(Event::Force) => Some(true),
            Ok(Event::Stop) => Some(false),
            _ => None,
        }
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
        } = &launch.host
        else {
            unreachable!()
        };
        let dispatched = Instant::now();
        let mut waiting = StartingWait::new(dispatched);
        let mut next_scan = dispatched;
        let mut next_manifest = dispatched;
        let mut heartbeat = Instant::now();
        let start_limit =
            test_timeout("WINEO_TEST_STEAM_START_TIMEOUT_MS", Duration::from_secs(60));
        let stall_limit = test_timeout(
            "WINEO_TEST_STEAM_STALL_TIMEOUT_MS",
            Duration::from_secs(600),
        );
        while state.phase.is_some() {
            if let Some(force) = self.steam_stop() {
                notify(if force {
                    Notice::Killing
                } else {
                    Notice::Stopping
                });
                return Ok(RunStatus::Stopped);
            }
            let now = Instant::now();
            if now < next_scan {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            next_scan = now + Duration::from_millis(250);
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
        loop {
            if let Some(force) = self.steam_stop() {
                notify(if force {
                    Notice::Killing
                } else {
                    Notice::Stopping
                });
                let mut announced = force;
                steam::stop_tree(&reaper, timeout, force, || {
                    let forced = self.token.as_ref().is_some_and(Token::is_forced)
                        || matches!(self.events.try_recv(), Ok(Event::Force));
                    if forced && !announced {
                        notify(Notice::Killing);
                        announced = true;
                    }
                    forced
                })
                .context(|| "failed to stop Steam's game processes".to_owned())?;
                return Ok(RunStatus::Stopped);
            }
            if steam::pidfd_exited(&fd).context(|| "failed to watch Steam process".to_owned())? {
                return Ok(RunStatus::Exited);
            }
            heartbeat_state(runtime, state, &mut heartbeat, notify);
            thread::sleep(Duration::from_millis(20));
        }
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
        let updating = matches!(state, steam::AppState::Updating);
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

fn test_timeout(variable: &str, fallback: Duration) -> Duration {
    if cfg!(debug_assertions) {
        std::env::var(variable)
            .ok()
            .and_then(|value| value.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(fallback)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
