use super::*;
use crate::launch::SteamRequest;
use crate::steam;

const REQUEST_COMMAND: &str = "__steam_request";

/// How long a launch waits on a client that is coming up but not ready before
/// sending the launch anyway. A client in its own pid namespace never looks
/// ready, so this is well inside `start`'s own 60 s.
const CLIENT_STARTING_LIMIT: Duration = Duration::from_secs(15);
/// How long Steam's startup update may leave its log unwritten before it
/// stops counting, as stuck or killed. It logs about every 300 ms while
/// downloading.
const CLIENT_UPDATE_SILENCE: Duration = Duration::from_secs(120);
/// How long a client that was coming up must stay gone before it counts as
/// quit, when there's no silent start of ours to watch. Steam's client
/// processes come and go for a moment or two while it restarts itself.
const CLIENT_ABSENT_LIMIT: Duration = Duration::from_secs(10);
/// The longest a launch waits on a client, so an update whose log never
/// stops changing can't hold the launch forever.
const CLIENT_WAIT_CAP: Duration = Duration::from_secs(30 * 60);
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
            let report = |report: String| {
                io::stdout()
                    .write_all(report.as_bytes())
                    .context(|| "failed to report Steam request".to_owned())
            };
            // The launch follows the report, so `start`'s own clock covers the
            // client's boot as it did when both went out together. Nothing
            // is left to report to if the launch's spawn fails.
            match first_step(&launch) {
                First::Send => {
                    report(match spawn_quiet(launch.command()) {
                        Ok(_) => "spawned\n".to_owned(),
                        Err(err) => format!("failed {err}\n"),
                    })?;
                }
                First::Start(command) => {
                    let silent = spawn_quiet(command);
                    report(match &silent {
                        Ok(_) => "spawned\n".to_owned(),
                        Err(err) => format!("failed {err}\n"),
                    })?;
                    if let (Ok(silent), Host::Proxied { install, .. }) = (silent, &launch.host) {
                        wait_for_client(&launch, install, Some(silent));
                        let _ = spawn_quiet(launch.command());
                    }
                }
                // A client is already coming up, so `steam` resolved moments
                // ago. As after a silent start, the launch's own spawn goes
                // unreported.
                First::Wait => {
                    report("spawned\n".to_owned())?;
                    if let Host::Proxied { install, .. } = &launch.host {
                        wait_for_client(&launch, install, None);
                        let _ = spawn_quiet(launch.command());
                    }
                }
            }
            Ok(())
        })
}

/// What the helper does before a launch reaches Steam.
enum First {
    /// Send the request now.
    Send,
    /// Start the client silently, then send the launch once it's ready.
    /// Steam ignores `-silent` when `-applaunch` arrives in the same cold
    /// start and opens its library window.
    Start(Command),
    /// A client is coming up: send the launch once it's ready, with no
    /// second start.
    Wait,
}

/// Only a launch waits for the client. When the check can't tell, the
/// request is sent as it is.
fn first_step(launch: &Launch) -> First {
    let Host::Proxied {
        install,
        request: SteamRequest::Launch,
        ..
    } = &launch.host
    else {
        return First::Send;
    };
    match steam::client_state(install) {
        Ok(steam::ClientState::Absent) => launch
            .silent_start_command()
            .map_or(First::Send, First::Start),
        Ok(steam::ClientState::Starting) => First::Wait,
        Ok(steam::ClientState::Ready) | Err(_) => First::Send,
    }
}

/// Waits for the client to be ready for a launch, giving a startup update of
/// the client as long as it counts, and starting the client again once if it
/// quits on the way. Started commands are reaped as they exit, so they don't
/// linger as zombies for the wait.
fn wait_for_client(launch: &Launch, install: &steam::Install, silent: Option<Child>) {
    let mut started: Vec<Child> = silent.into_iter().collect();
    let begun = Instant::now();
    let silence = client_update_silence();
    let mut wait = ClientWait::new(
        begun,
        test_timeout("WINEO_TEST_STEAM_CLIENT_WAIT_MS", CLIENT_STARTING_LIMIT),
        test_timeout("WINEO_TEST_STEAM_CLIENT_ABSENT_MS", CLIENT_ABSENT_LIMIT),
    );
    while begun.elapsed() < CLIENT_WAIT_CAP {
        let mut ours = Ours::None;
        for child in &mut started {
            // A check that fails can't show the command is still there.
            ours = match child.try_wait() {
                Ok(None) => Ours::Running,
                Ok(Some(_)) | Err(_) => Ours::Exited,
            };
        }
        let client = steam::client_state(install);
        let update = match client {
            Ok(steam::ClientState::Starting | steam::ClientState::Absent) => {
                steam::client_update(install, silence)
            }
            _ => None,
        };
        match wait.step(Instant::now(), client, update.as_ref(), ours) {
            Next::Wait => thread::sleep(CLIENT_POLL),
            Next::Send => return,
            Next::Restart => match launch.silent_start_command().map(spawn_quiet) {
                Some(Ok(child)) => started.push(child),
                _ => return,
            },
        }
    }
}

/// The helper and the supervisor read the log with the same limit, so they
/// agree on when an update stops counting.
fn client_update_silence() -> Duration {
    test_timeout("WINEO_TEST_STEAM_CLIENT_STALL_MS", CLIENT_UPDATE_SILENCE)
}

/// What the wait for a client does after one look at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Wait,
    Send,
    /// Start the client silently again, then keep waiting.
    Restart,
}

/// The newest silent start command of ours. It's `steam.sh`, which lives as
/// long as the client does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ours {
    /// The client was already coming up, and nothing of ours started it.
    None,
    Running,
    Exited,
}

/// The rules of the wait for a client, apart from the clock and `/proc`.
struct ClientWait {
    starting_limit: Duration,
    absent_limit: Duration,
    /// When the client began its way up: the wait's start, a restart, or the
    /// end of a startup update.
    starting_since: Instant,
    /// A startup update counted at the last look.
    updating: bool,
    /// When the client was last seen going, if it's gone now.
    absent_since: Option<Instant>,
    /// A client process was seen since the wait began or restarted. Until
    /// then a silent start is still on its way.
    seen: bool,
    restarted: bool,
}

impl ClientWait {
    fn new(now: Instant, starting_limit: Duration, absent_limit: Duration) -> Self {
        Self {
            starting_limit,
            absent_limit,
            starting_since: now,
            updating: false,
            absent_since: None,
            seen: false,
            restarted: false,
        }
    }

    /// A failed check sends the launch, which is better sent than lost. A
    /// client that was seen has quit once our silent start has exited too,
    /// or with none of ours, once it stays gone for `absent_limit`. Shorter
    /// gaps change nothing.
    fn step(
        &mut self,
        now: Instant,
        client: io::Result<steam::ClientState>,
        update: Option<&steam::ClientUpdate>,
        ours: Ours,
    ) -> Next {
        match client {
            Err(_) | Ok(steam::ClientState::Ready) => return Next::Send,
            Ok(steam::ClientState::Starting) => {
                self.seen = true;
                self.absent_since = None;
            }
            Ok(steam::ClientState::Absent) => {
                let gone_since = *self.absent_since.get_or_insert(now);
                let quit = self.seen
                    && match ours {
                        Ours::Exited => true,
                        Ours::Running => false,
                        Ours::None => now.duration_since(gone_since) >= self.absent_limit,
                    };
                if quit {
                    if self.restarted {
                        return Next::Send;
                    }
                    self.restarted = true;
                    self.seen = false;
                    self.starting_since = now;
                    self.updating = false;
                    self.absent_since = None;
                    return Next::Restart;
                }
            }
        }
        if update.is_some() {
            self.updating = true;
            return Next::Wait;
        }
        if std::mem::take(&mut self.updating) {
            self.starting_since = now;
        }
        if now.duration_since(self.starting_since) >= self.starting_limit {
            Next::Send
        } else {
            Next::Wait
        }
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
        let client_silence = client_update_silence();
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
                waiting.observe_client(now, steam::client_update(install, client_silence).as_ref());
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

/// The clocks for a launch Steam hasn't started yet. While the game updates,
/// the start window waits and a stall clock runs instead. While a startup
/// update of the client counts, both wait and only `CLIENT_WAIT_CAP` can end
/// the run. The window starts over when an update ends.
struct StartingWait {
    updating: bool,
    start_window: Instant,
    progressed_at: Instant,
    previous_progress: Option<steam::Progress>,
    began: Instant,
    client_updating: bool,
}

impl StartingWait {
    fn new(now: Instant) -> Self {
        Self {
            updating: false,
            start_window: now,
            progressed_at: now,
            previous_progress: None,
            began: now,
            client_updating: false,
        }
    }

    /// The end of a client update also restarts the game's stall clock,
    /// which stood still while the client updated.
    fn observe_client(&mut self, now: Instant, update: Option<&steam::ClientUpdate>) {
        if self.client_updating && update.is_none() {
            self.start_window = now;
            self.progressed_at = now;
        }
        self.client_updating = update.is_some();
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

    /// The helper gives up on the client after `CLIENT_WAIT_CAP`, and the
    /// launch then gets the usual window.
    fn expired(&self, now: Instant, start_limit: Duration, stall_limit: Duration) -> bool {
        if self.client_updating {
            now.duration_since(self.began) >= CLIENT_WAIT_CAP + start_limit
        } else if self.updating {
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

    const UPDATE: steam::ClientUpdate = steam::ClientUpdate {
        phase: steam::ClientUpdatePhase::Installing,
    };

    #[test]
    fn a_client_update_pauses_the_start_window_until_it_ends() {
        let now = Instant::now();
        let start = Duration::from_secs(60);
        let stall = Duration::from_secs(600);
        let mut wait = StartingWait::new(now);
        for second in 0..=100 {
            let at = now + Duration::from_secs(second);
            wait.observe_client(at, Some(&UPDATE));
            assert!(!wait.expired(at, start, stall), "expired at {second} s");
        }
        let ended = now + Duration::from_secs(101);
        wait.observe_client(ended, None);
        assert!(!wait.expired(ended + Duration::from_secs(59), start, stall));
        assert!(wait.expired(ended + start, start, stall));
    }

    #[test]
    fn a_counting_client_update_ends_the_run_only_at_the_cap() {
        let now = Instant::now();
        let start = Duration::from_secs(60);
        let stall = Duration::from_secs(600);
        let mut wait = StartingWait::new(now);
        let cap = now + CLIENT_WAIT_CAP + start;
        wait.observe_client(cap - Duration::from_secs(1), Some(&UPDATE));
        assert!(!wait.expired(cap - Duration::from_secs(1), start, stall));
        assert!(wait.expired(cap, start, stall));
    }

    #[test]
    fn the_start_window_outlasts_the_helpers_send_after_a_client_update() {
        let now = Instant::now();
        let start = Duration::from_secs(60);
        let stall = Duration::from_secs(600);
        let mut supervisor = StartingWait::new(now);
        let mut helper = limits(now);
        let ended = 400;
        let mut sent = None;
        // Ticks of the helper's 250 ms poll. The supervisor reads the log
        // once a second, so it can see the end up to a second later or a
        // poll sooner.
        for tick in 0..4 * 200 {
            let at = now + CLIENT_POLL * tick;
            let update = (tick < ended).then_some(&UPDATE);
            if tick % 4 == 1 {
                supervisor.observe_client(at, update);
            }
            if sent.is_none() && helper.step(at, starting(), update, Ours::Running) == Next::Send {
                sent = Some(at);
            }
            if let Some(sent) = sent
                && at <= sent + Duration::from_secs(5)
            {
                assert!(!supervisor.expired(at, start, stall), "expired at {tick}");
            }
        }
        let sent = sent.expect("the helper sends");
        let end = now + CLIENT_POLL * ended;
        assert!(sent >= end + CLIENT_STARTING_LIMIT, "sent early");
    }

    #[test]
    fn the_game_stall_clock_waits_while_the_client_updates() {
        let now = Instant::now();
        let start = Duration::from_secs(60);
        let stall = Duration::from_secs(600);
        let mut wait = StartingWait::new(now);
        wait.observe(now, &steam::AppState::Updating, Some(progress(1030, 0)));
        wait.observe_client(now, Some(&UPDATE));
        for second in 1..=700 {
            let at = now + Duration::from_secs(second);
            wait.observe(at, &steam::AppState::Updating, Some(progress(1030, 0)));
            wait.observe_client(at, Some(&UPDATE));
            assert!(!wait.expired(at, start, stall), "expired at {second} s");
        }
        let ended = now + Duration::from_secs(701);
        wait.observe_client(ended, None);
        assert!(!wait.expired(ended + Duration::from_secs(599), start, stall));
        assert!(wait.expired(ended + stall, start, stall));
    }

    fn limits(now: Instant) -> ClientWait {
        ClientWait::new(now, CLIENT_STARTING_LIMIT, CLIENT_ABSENT_LIMIT)
    }

    fn starting() -> io::Result<steam::ClientState> {
        Ok(steam::ClientState::Starting)
    }

    fn absent() -> io::Result<steam::ClientState> {
        Ok(steam::ClientState::Absent)
    }

    fn after(now: Instant, millis: u64) -> Instant {
        now + Duration::from_millis(millis)
    }

    #[test]
    fn a_ready_client_gets_the_launch() {
        let now = Instant::now();
        let mut wait = limits(now);
        assert_eq!(wait.step(now, starting(), None, Ours::None), Next::Wait);
        let ready = Ok(steam::ClientState::Ready);
        // Ready wins even over a log that still says updating.
        assert_eq!(
            wait.step(after(now, 250), ready, Some(&UPDATE), Ours::None),
            Next::Send
        );
    }

    #[test]
    fn a_failed_client_check_sends_the_launch() {
        let now = Instant::now();
        let mut wait = limits(now);
        assert_eq!(wait.step(now, starting(), None, Ours::None), Next::Wait);
        let failed = Err(io::Error::other("no /proc"));
        assert_eq!(
            wait.step(after(now, 250), failed, None, Ours::None),
            Next::Send
        );
    }

    #[test]
    fn a_client_starting_with_no_update_gets_the_launch_after_15_s() {
        let now = Instant::now();
        let mut wait = limits(now);
        assert_eq!(wait.step(now, starting(), None, Ours::None), Next::Wait);
        assert_eq!(
            wait.step(after(now, 14_999), starting(), None, Ours::None),
            Next::Wait
        );
        assert_eq!(
            wait.step(after(now, 15_000), starting(), None, Ours::None),
            Next::Send
        );
    }

    #[test]
    fn a_silent_start_not_up_yet_counts_as_starting() {
        let now = Instant::now();
        let mut wait = limits(now);
        // Even with our command gone, a client never seen hasn't quit.
        for ours in [Ours::Running, Ours::Exited] {
            assert_eq!(wait.step(now, absent(), None, ours), Next::Wait);
            assert_eq!(
                wait.step(after(now, 14_999), absent(), None, ours),
                Next::Wait
            );
        }
        assert_eq!(
            wait.step(after(now, 15_000), absent(), None, Ours::Exited),
            Next::Send
        );
    }

    #[test]
    fn a_client_update_keeps_the_launch_waiting_while_it_counts() {
        let now = Instant::now();
        let mut wait = limits(now);
        for second in 0..=600 {
            let at = now + Duration::from_secs(second);
            assert_eq!(
                wait.step(at, starting(), Some(&UPDATE), Ours::Running),
                Next::Wait,
                "sent at {second} s"
            );
        }
    }

    #[test]
    fn the_end_of_a_client_update_starts_the_15_s_over() {
        let now = Instant::now();
        let mut wait = limits(now);
        for second in 0..100 {
            let at = now + Duration::from_secs(second);
            assert_eq!(
                wait.step(at, starting(), Some(&UPDATE), Ours::Running),
                Next::Wait
            );
        }
        let ended = now + Duration::from_secs(100);
        assert_eq!(
            wait.step(ended, starting(), None, Ours::Running),
            Next::Wait
        );
        let limit = ended + CLIENT_STARTING_LIMIT;
        assert_eq!(
            wait.step(
                limit - Duration::from_millis(1),
                starting(),
                None,
                Ours::Running
            ),
            Next::Wait
        );
        assert_eq!(
            wait.step(limit, starting(), None, Ours::Running),
            Next::Send
        );
    }

    #[test]
    fn a_relaunch_gap_with_our_start_running_changes_nothing() {
        let now = Instant::now();
        let mut wait = limits(now);
        assert_eq!(wait.step(now, starting(), None, Ours::Running), Next::Wait);
        // Steam's verify-only block, then the real one a couple of seconds
        // later, with no client process between.
        for millis in (250..=2_250).step_by(250) {
            assert_eq!(
                wait.step(after(now, millis), absent(), None, Ours::Running),
                Next::Wait
            );
        }
        assert_eq!(
            wait.step(after(now, 2_500), starting(), None, Ours::Running),
            Next::Wait
        );
        assert_eq!(
            wait.step(after(now, 15_000), starting(), None, Ours::Running),
            Next::Send
        );
    }

    #[test]
    fn a_client_we_didnt_start_quits_only_after_10_s_gone() {
        let now = Instant::now();
        let mut wait = limits(now);
        assert_eq!(
            wait.step(now, starting(), Some(&UPDATE), Ours::None),
            Next::Wait
        );
        // Gaps shorter than the limit, even back to back, change nothing.
        for gap in 0..2 {
            let from = 20_000 * gap;
            for millis in (from + 250..from + 10_000).step_by(250) {
                assert_eq!(
                    wait.step(after(now, millis), absent(), Some(&UPDATE), Ours::None),
                    Next::Wait,
                    "at {millis} ms"
                );
            }
            assert_eq!(
                wait.step(
                    after(now, from + 10_000),
                    starting(),
                    Some(&UPDATE),
                    Ours::None
                ),
                Next::Wait
            );
        }
        let gone = after(now, 50_000);
        assert_eq!(
            wait.step(gone, absent(), Some(&UPDATE), Ours::None),
            Next::Wait
        );
        assert_eq!(
            wait.step(gone + CLIENT_ABSENT_LIMIT, absent(), None, Ours::None),
            Next::Restart
        );
    }

    #[test]
    fn a_client_that_quits_is_started_once_more_then_the_launch_is_sent() {
        let now = Instant::now();
        let mut wait = limits(now);
        assert_eq!(
            wait.step(now, starting(), Some(&UPDATE), Ours::Running),
            Next::Wait
        );
        let quit = after(now, 10_000);
        assert_eq!(wait.step(quit, absent(), None, Ours::Running), Next::Wait);
        assert_eq!(
            wait.step(after(quit, 250), absent(), None, Ours::Exited),
            Next::Restart
        );
        let restart = after(quit, 250);
        // The new start is on its way, and its 15 s run from the restart.
        assert_eq!(
            wait.step(after(restart, 250), absent(), None, Ours::Running),
            Next::Wait
        );
        assert_eq!(
            wait.step(after(restart, 5_000), starting(), None, Ours::Running),
            Next::Wait
        );
        assert_eq!(
            wait.step(after(restart, 14_999), starting(), None, Ours::Running),
            Next::Wait
        );
        assert_eq!(
            wait.step(after(restart, 14_999), absent(), None, Ours::Exited),
            Next::Send
        );
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
