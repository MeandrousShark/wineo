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
    ) -> Result<(RunState, bool)> {
        let Host::Proxied {
            install,
            app_id,
            baseline,
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
        runtime.write_state(&state)?;
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
                runtime.write_state(state)?;
                Ok(())
            }
            Err(err) => {
                state.finish(RunStatus::Failed);
                runtime.write_state(state)?;
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
            baseline,
        } = &launch.host
        else {
            unreachable!()
        };
        let dispatched = Instant::now();
        let mut previous_progress = progress(install, app_id);
        let mut progressed_at = dispatched;
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
            let candidates = steam::eligible_reapers(install, app_id, baseline)
                .context(|| "failed to inspect Steam processes".to_owned())?;
            if candidates.len() > 1 {
                return Err(ambiguous(name));
            }
            if let Some(reaper) = candidates.first() {
                claim(state, reaper);
                runtime.write_state(state)?;
                break;
            }
            let next = progress(install, app_id);
            if let Some(next) = next {
                if previous_progress.is_some_and(|old| next.progressed_since(&old)) {
                    progressed_at = Instant::now();
                }
                previous_progress = Some(next);
            }
            let updating = matches!(
                steam::app_state(Some(install), app_id),
                steam::AppState::Updating
            );
            if (updating && progressed_at.elapsed() >= stall_limit)
                || (!updating && dispatched.elapsed() >= start_limit)
            {
                return Err(Error::StartFailed(format!("Steam didn't start {name}.")));
            }
            heartbeat_state(runtime, state, &mut heartbeat, notify)?;
            thread::sleep(Duration::from_millis(20));
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
            if steam::pidfd_exited(&fd).context(|| "failed to watch Steam process".to_owned())? {
                return Ok(RunStatus::Exited);
            }
            heartbeat_state(runtime, state, &mut heartbeat, notify)?;
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

fn progress(install: &steam::Install, app_id: &str) -> Option<steam::Progress> {
    steam::manifests(install)
        .into_iter()
        .find(|entry| entry.app_id == app_id)?
        .manifest
        .ok()
        .map(|manifest| manifest.progress())
}

fn heartbeat_state(
    runtime: &Runtime,
    state: &mut RunState,
    last: &mut Instant,
    notify: &mut impl FnMut(Notice),
) -> Result<()> {
    if last.elapsed() >= HEARTBEAT_INTERVAL {
        state.heartbeat();
        runtime.write_state(state)?;
        *last = Instant::now();
        notify(Notice::Heartbeat);
    }
    Ok(())
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
