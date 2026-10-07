use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use iced::Task;
use iced::futures::channel::mpsc::{UnboundedSender, unbounded};
use wineo_core::cancel::Token;
use wineo_core::installer::Output;
use wineo_core::launch::{self, Overrides};
use wineo_core::log::{Log, Tail};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, Runtime, State};
use wineo_core::winetricks::WinetricksEvent;

use crate::Message;
use crate::panel::{PanelError, event_status};

pub(crate) const AFTER_HOOK_ENDED_NOTE: &str = "The after hook already ended.";

/// Which of an environment's logs the panel follows.
#[derive(Debug, Clone)]
pub(crate) enum LogSource {
    /// The running game's output.log.
    Output,
    /// install.log while winetricks installs. The token is canceled once the
    /// install phase is over, which is when the tail stops following.
    Install(Token),
}

pub(crate) fn start(
    paths: Paths,
    name: String,
    target: String,
    token: Token,
    install_done: Token,
) -> Task<Message> {
    let (sender, receiver) = unbounded();
    let progress_name = name.clone();
    let finish_name = name.clone();
    let work = Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let work = || -> anyhow::Result<_> {
                    let exe = std::env::current_exe().context("failed to find wineo-gui binary")?;
                    let log = Runtime::new(&paths, &name).install_log_path();
                    if let Some(parent) = log.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let capture = hook_capture(&paths, &name, &target)?;
                    let hook_output = match &capture {
                        Some(capture) => wineo_core::hooks::Output::File(
                            capture
                                .try_clone()
                                .context("failed to capture hook output")?,
                        ),
                        None => wineo_core::hooks::Output::Inherit,
                    };
                    let mut after_output_start = None;
                    let result = launch::start(
                        &paths,
                        &name,
                        Overrides {
                            target: Some(target),
                            ..Overrides::default()
                        },
                        &exe,
                        &token,
                        hook_output,
                        Output::Log(&log),
                        |event| {
                            if let Some(status) = event_status(&event) {
                                let _ = sender.unbounded_send(Message::LaunchProgress(
                                    progress_name.clone(),
                                    status,
                                ));
                            }
                            if matches!(event, launch::LaunchEvent::NoWaylandSession) {
                                let _ = sender.unbounded_send(Message::LaunchNote(
                                    progress_name.clone(),
                                    "No Wayland session is available".to_owned(),
                                ));
                            }
                            // LogStarted comes after install.log was emptied, so
                            // the panel never tails a previous run's text.
                            match &event {
                                launch::LaunchEvent::Winetricks(WinetricksEvent::LogStarted) => {
                                    let _ = sender.unbounded_send(Message::InstallPhase(
                                        progress_name.clone(),
                                        true,
                                    ));
                                }
                                launch::LaunchEvent::Winetricks(WinetricksEvent::Done(_)) => {
                                    install_done.cancel();
                                    let _ = sender.unbounded_send(Message::InstallPhase(
                                        progress_name.clone(),
                                        false,
                                    ));
                                }
                                launch::LaunchEvent::AfterHook(token) => {
                                    after_output_start = capture
                                        .as_ref()
                                        .and_then(|capture| capture.position().ok());
                                    let _ = sender.unbounded_send(Message::AfterHookPhase(
                                        progress_name.clone(),
                                        token.clone(),
                                    ));
                                }
                                _ => {}
                            }
                        },
                    );
                    // An error before Done still ends the install phase.
                    install_done.cancel();
                    let capture = capture.and_then(|capture| capture.finish().ok());
                    Ok((result, capture, after_output_start))
                };
                match work() {
                    Ok((result, mut capture, after_output_start)) => result.map_err(|error| {
                        launch_error(&error, capture.as_mut(), after_output_start)
                    }),
                    Err(error) => Err(PanelError::from_error(&error)),
                }
            })
            .await
            .map_err(|error| PanelError::from_error(&anyhow::Error::from(error)))
            .and_then(|result| result)
        },
        move |result| Message::StartFinished(finish_name, result),
    );
    Task::batch([Task::run(receiver, |message| message), work])
}

pub(crate) fn stop(paths: Paths, name: String, force: bool) -> Task<Message> {
    let finish_name = name.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || launch::stop(&paths, &name, force))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
                .map(|outcome| outcome.messages().to_vec())
                .map_err(|error| PanelError::from_error(&error))
        },
        move |result| Message::StopFinished(finish_name, result),
    )
}

pub(crate) fn stop_after(
    paths: Paths,
    name: String,
    expected: Option<wineo_core::hooks::Running>,
) -> Task<Message> {
    let finish_name = name.clone();
    Task::perform(
        async move {
            let Some(expected) = expected else {
                return Ok(vec![AFTER_HOOK_ENDED_NOTE.to_owned()]);
            };
            tokio::task::spawn_blocking(move || launch::force_stop_after(&paths, &name, &expected))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
                .map(after_stop_messages)
                .map_err(|error| PanelError::from_error(&error))
        },
        move |result| Message::StopFinished(finish_name, result),
    )
}

fn after_stop_messages(outcome: launch::StopOutcome) -> Vec<String> {
    match outcome {
        launch::StopOutcome::NotRunning { .. } => vec![AFTER_HOOK_ENDED_NOTE.to_owned()],
        outcome => outcome.messages().to_vec(),
    }
}

const HOOK_CAPTURE_LIMIT: usize = 64 * 1024;

struct HookCapture {
    writer: Option<File>,
    state: Arc<Mutex<CaptureState>>,
    done: Arc<AtomicBool>,
    reader: Option<JoinHandle<io::Result<()>>>,
}

struct CaptureState {
    stream: UnixStream,
    bytes: VecDeque<u8>,
    total: u64,
}

struct CaptureSnapshot {
    bytes: Vec<u8>,
    total: u64,
}

impl CaptureState {
    fn drain(&mut self) -> io::Result<()> {
        let mut chunk = [0; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    self.total += count as u64;
                    self.bytes
                        .drain(..(self.bytes.len() + count).saturating_sub(HOOK_CAPTURE_LIMIT));
                    self.bytes.extend(&chunk[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

impl HookCapture {
    fn new() -> io::Result<Self> {
        let (stream, writer) = UnixStream::pair()?;
        stream.set_nonblocking(true)?;
        let state = Arc::new(Mutex::new(CaptureState {
            stream,
            bytes: VecDeque::with_capacity(HOOK_CAPTURE_LIMIT),
            total: 0,
        }));
        let done = Arc::new(AtomicBool::new(false));
        let read_state = state.clone();
        let read_done = done.clone();
        let reader = thread::spawn(move || {
            loop {
                let finished = read_done.load(Ordering::Acquire);
                read_state
                    .lock()
                    .map_err(|_| io::Error::other("hook capture lock poisoned"))?
                    .drain()?;
                if finished {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(Self {
            writer: Some(OwnedFd::from(writer).into()),
            state,
            done,
            reader: Some(reader),
        })
    }

    fn try_clone(&self) -> io::Result<File> {
        self.writer.as_ref().expect("capture is active").try_clone()
    }

    fn position(&self) -> io::Result<u64> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("hook capture lock poisoned"))?;
        state.drain()?;
        Ok(state.total)
    }

    fn finish(mut self) -> io::Result<CaptureSnapshot> {
        drop(self.writer.take());
        self.done.store(true, Ordering::Release);
        self.reader
            .take()
            .expect("capture reader is active")
            .join()
            .map_err(|_| io::Error::other("hook capture thread panicked"))??;
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("hook capture lock poisoned"))?;
        Ok(CaptureSnapshot {
            bytes: state.bytes.iter().copied().collect(),
            total: state.total,
        })
    }
}

impl Drop for HookCapture {
    fn drop(&mut self) {
        drop(self.writer.take());
        self.done.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn hook_capture(paths: &Paths, name: &str, target: &str) -> anyhow::Result<Option<HookCapture>> {
    let Ok(environment) = wineo_core::environment::Environment::load(paths, name) else {
        return Ok(None);
    };
    let hooks = launch::hooks(&environment, environment.config.targets.get(target))
        .filter(|hooks| hooks.before.is_some() || hooks.after.is_some());
    hooks
        .map(|_| HookCapture::new().context("failed to capture hook output"))
        .transpose()
}

trait Captured {
    fn size(&mut self) -> io::Result<u64>;
    fn tail(&mut self) -> io::Result<String>;
}

impl Captured for CaptureSnapshot {
    fn size(&mut self) -> io::Result<u64> {
        Ok(self.total)
    }

    fn tail(&mut self) -> io::Result<String> {
        Ok(captured_tail_bytes(&self.bytes))
    }
}

#[cfg(test)]
impl Captured for File {
    fn size(&mut self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn tail(&mut self) -> io::Result<String> {
        captured_tail(self)
    }
}

fn launch_error<C: Captured>(
    error: &anyhow::Error,
    mut capture: Option<&mut C>,
    after_output_start: Option<u64>,
) -> PanelError {
    let mut mapped = PanelError::from_error(error);
    let cleanup_output = after_output_start.is_some_and(|start| {
        capture
            .as_mut()
            .and_then(|capture| capture.size().ok())
            .is_some_and(|size| size > start)
    });
    if cleanup_output
        || matches!(
            error.downcast_ref::<wineo_core::Error>(),
            Some(wineo_core::Error::BeforeHookFailed(_))
        )
    {
        mapped.output = capture
            .and_then(|capture| capture.tail().ok())
            .filter(|output| !output.is_empty());
    }
    mapped
}

#[cfg(test)]
fn captured_tail(capture: &mut File) -> io::Result<String> {
    use std::io::{Seek, SeekFrom};
    let length = capture.metadata()?.len();
    capture.seek(SeekFrom::Start(
        length.saturating_sub(HOOK_CAPTURE_LIMIT as u64),
    ))?;
    let mut bytes = Vec::new();
    capture
        .take(HOOK_CAPTURE_LIMIT as u64)
        .read_to_end(&mut bytes)?;
    Ok(captured_tail_bytes(&bytes))
}

fn captured_tail_bytes(bytes: &[u8]) -> String {
    let output = String::from_utf8_lossy(bytes);
    let output = crate::logview::normalize_captured(&output);
    let lines: Vec<_> = output.lines().rev().take(20).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

struct ChannelWriter(UnboundedSender<Message>, u64);

impl Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .unbounded_send(Message::LogChunk(
                self.1,
                String::from_utf8_lossy(bytes).into_owned(),
            ))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "log panel closed"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn log(
    paths: Paths,
    name: String,
    generation: u64,
    token: Token,
    source: LogSource,
) -> Task<Message> {
    let (sender, receiver) = unbounded();
    let work = Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let runtime = Runtime::new(&paths, &name);
                let path = match &source {
                    LogSource::Output => runtime.log_path(),
                    LogSource::Install(_) => runtime.install_log_path(),
                };
                let file = File::open(&path)
                    .with_context(|| format!("failed to open {}", path.display()))?;
                // Only output.log is capped and cleared by a supervisor, so
                // only it has a reset counter to honor.
                let session = matches!(source, LogSource::Output).then(|| Log::new(path.clone()));
                let mut tail = Tail::new(file, session);
                tail.seek_to_last_lines(2000)?;
                let mut writer = ChannelWriter(sender, generation);
                if let LogSource::Install(done) = &source {
                    if tail.stream(&mut writer)? && !done.is_canceled() {
                        tail.follow_until(&mut writer, &token, |_| Ok(done.is_canceled()))?;
                    }
                    return Ok(());
                }
                let attached = runtime.status().is_ok_and(|status| {
                    status.state == State::Running
                        && status.run.is_some_and(|run| run.mode == RunMode::Attached)
                });
                if tail.stream(&mut writer)? && !attached && runtime.is_running()? {
                    tail.follow(&runtime, &path, &mut writer, &token)?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
            .map_err(|error| error.to_string())
        },
        move |result| Message::LogFinished(generation, result),
    );
    Task::batch([Task::run(receiver, |message| message), work])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_after_hook_reports_a_panel_note() {
        let messages = after_stop_messages(launch::StopOutcome::NotRunning {
            messages: Vec::new(),
        });
        assert_eq!(messages, vec![AFTER_HOOK_ENDED_NOTE]);
    }

    #[test]
    fn capture_is_only_created_for_the_selected_before_hook() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("app");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            "schema_version = 1\nkind = 'native'\n[targets.app]\nexe = '/missing'\n",
        )
        .unwrap();
        assert!(hook_capture(&paths, "app", "app").unwrap().is_none());
        std::fs::write(&config, "schema_version = 1\nkind = 'native'\n[hooks]\nbefore = 'echo output; echo error >&2; exit 3'\n[targets.app]\nexe = '/missing'\n[targets.disabled]\nexe = '/missing'\nhooks = false\n").unwrap();
        assert!(hook_capture(&paths, "app", "disabled").unwrap().is_none());
        let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
        let error = launch::prepare(
            &paths,
            "app",
            &Overrides {
                target: Some("app".into()),
                ..Overrides::default()
            },
            &Token::new(),
            wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
            |_| {},
        )
        .err()
        .expect("before hook should fail");
        let mut capture = capture.finish().unwrap();
        let mapped = launch_error(&error, Some(&mut capture), None);
        assert_eq!(
            mapped.text,
            "Launch canceled: the before hook exited with code 3."
        );
        assert_eq!(mapped.output.as_deref(), Some("output\nerror"));
    }

    #[test]
    fn failed_before_shows_only_last_twenty_output_lines() {
        let mut capture = tempfile::tempfile().unwrap();
        for index in 0..30 {
            writeln!(capture, "line {index}\r").unwrap();
        }
        write!(capture, "partial").unwrap();
        let error = anyhow::Error::new(wineo_core::Error::BeforeHookFailed(7));
        let mapped = launch_error(&error, Some(&mut capture), None);
        assert_eq!(
            mapped.text,
            "Launch canceled: the before hook exited with code 7."
        );
        let output = mapped.output.unwrap();
        assert_eq!(output.lines().count(), 20);
        assert!(output.starts_with("line 11\n"));
        assert!(output.ends_with("line 29\npartial"));
        assert!(!output.contains('\r'));
    }

    #[test]
    fn failed_launch_shows_cleanup_output_after_before_and_keeps_twenty_lines() {
        for before in [None, Some("echo before")] {
            for lines in [1, 25] {
                let root = tempfile::tempdir().unwrap();
                let paths = Paths::from_lookup(|key| {
                    (key == "HOME").then(|| root.path().as_os_str().to_owned())
                })
                .unwrap();
                let config = paths.environment_config("app");
                std::fs::create_dir_all(config.parent().unwrap()).unwrap();
                let before_config = before
                    .map(|command| format!("before = '{command}'\n"))
                    .unwrap_or_default();
                std::fs::write(
                    config,
                    format!(
                        "kind = 'native'\n[hooks]\n{before_config}after = 'i=0; while [ $i -lt {lines} ]; do echo cleanup-$i >&2; i=$((i+1)); done'\n[targets.app]\nexe = '{}'\n",
                        root.path().join("missing").display()
                    ),
                )
                .unwrap();
                let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
                let mut after_output_start = None;
                let error = launch::start(
                    &paths,
                    "app",
                    Overrides::default(),
                    &root.path().join("unused-supervisor"),
                    &Token::new(),
                    wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
                    Output::Inherit,
                    |event| {
                        if matches!(event, launch::LaunchEvent::AfterHook(_)) {
                            after_output_start = Some(capture.position().unwrap());
                        }
                    },
                )
                .unwrap_err();
                let mut capture = capture.finish().unwrap();
                let mapped = launch_error(&error, Some(&mut capture), after_output_start);
                assert_eq!(mapped.text, PanelError::from_error(&error).text);
                let mut expected = Vec::new();
                if before.is_some() {
                    expected.push("before".to_owned());
                }
                expected.extend((0..lines).map(|index| format!("cleanup-{index}")));
                let expected = expected[expected.len().saturating_sub(20)..].join("\n");
                assert_eq!(mapped.output.as_deref(), Some(expected.as_str()));
            }
        }
    }

    #[test]
    fn silent_cleanup_keeps_successful_before_output_hidden() {
        let mut capture = tempfile::tempfile().unwrap();
        writeln!(capture, "successful before output").unwrap();
        let after_output_start = Some(capture.metadata().unwrap().len());
        let error = anyhow::anyhow!("missing executable");
        assert!(
            launch_error(&error, Some(&mut capture), after_output_start)
                .output
                .is_none()
        );
    }

    #[test]
    fn empty_capture_and_other_errors_have_no_hook_output() {
        let mut capture = tempfile::tempfile().unwrap();
        let failure = anyhow::Error::new(wineo_core::Error::BeforeHookFailed(1));
        assert!(
            launch_error(&failure, Some(&mut capture), None)
                .output
                .is_none()
        );
        writeln!(capture, "successful before output").unwrap();
        let error = anyhow::anyhow!("missing executable");
        assert!(
            launch_error(&error, Some(&mut capture), None)
                .output
                .is_none()
        );
    }

    #[test]
    fn capture_bounds_long_and_invalid_utf8_output() {
        let mut capture = tempfile::tempfile().unwrap();
        capture.write_all(&vec![0xff; 100_000]).unwrap();
        writeln!(capture, "\nlast line").unwrap();
        let output = captured_tail(&mut capture).unwrap();
        assert!(output.ends_with("last line"));
        assert!(output.chars().count() <= 64 * 1024);
    }

    #[test]
    fn hook_capture_stays_bounded_while_a_chatty_hook_runs() {
        use std::os::unix::fs::FileTypeExt;

        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("app");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            "kind = 'native'\n[hooks]\nbefore = 'yes x | head -c 4194304; echo last line; exit 8'\n[targets.app]\nexe = '/missing'\n",
        )
        .unwrap();
        let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
        assert!(
            capture
                .writer
                .as_ref()
                .unwrap()
                .metadata()
                .unwrap()
                .file_type()
                .is_socket()
        );
        let error = launch::prepare(
            &paths,
            "app",
            &Overrides {
                target: Some("app".into()),
                ..Overrides::default()
            },
            &Token::new(),
            wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
            |_| {},
        )
        .err()
        .expect("before hook should fail");
        capture.position().unwrap();
        let state = capture.state.lock().unwrap();
        assert!(state.bytes.len() <= HOOK_CAPTURE_LIMIT);
        assert!(state.bytes.capacity() <= HOOK_CAPTURE_LIMIT);
        assert!(state.total > 4 * 1024 * 1024);
        drop(state);
        let mut snapshot = capture.finish().unwrap();
        assert_eq!(snapshot.bytes.len(), HOOK_CAPTURE_LIMIT);
        assert!(snapshot.tail().unwrap().ends_with("last line"));
        assert!(
            launch_error(&error, Some(&mut snapshot), None)
                .output
                .unwrap()
                .ends_with("last line")
        );
    }

    #[test]
    fn capture_finish_drains_pending_output() {
        for _ in 0..32 {
            let capture = HookCapture::new().unwrap();
            let mut writer = capture.try_clone().unwrap();
            writer.write_all(b"before\n").unwrap();
            drop(writer);
            let mut snapshot = capture.finish().unwrap();
            assert_eq!(snapshot.size().unwrap(), 7);
            assert_eq!(snapshot.tail().unwrap(), "before");
        }
    }

    #[test]
    fn silent_cleanup_after_chatty_before_hides_before_output() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("app");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "kind = 'native'\n[hooks]\nbefore = 'yes x | head -c 4194304; echo before-final'\nafter = 'true'\n[targets.app]\nexe = '{}'\n",
                root.path().join("missing").display()
            ),
        )
        .unwrap();
        let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
        let mut after_output_start = None;
        let error = launch::start(
            &paths,
            "app",
            Overrides::default(),
            &root.path().join("unused-supervisor"),
            &Token::new(),
            wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
            Output::Inherit,
            |event| {
                if matches!(event, launch::LaunchEvent::AfterHook(_)) {
                    after_output_start = Some(capture.position().unwrap());
                }
            },
        )
        .unwrap_err();
        let mut snapshot = capture.finish().unwrap();
        let before_end = after_output_start.expect("cleanup should run");
        assert!(before_end > 4 * 1024 * 1024);
        assert_eq!(snapshot.total, before_end);
        assert!(snapshot.tail().unwrap().ends_with("before-final"));
        assert!(
            launch_error(&error, Some(&mut snapshot), after_output_start)
                .output
                .is_none()
        );
    }

    #[test]
    fn capture_load_failure_defers_to_launch() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("app");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(config, "not valid toml = [").unwrap();
        assert!(hook_capture(&paths, "app", "app").unwrap().is_none());
        assert!(
            launch::prepare(
                &paths,
                "app",
                &Overrides::default(),
                &Token::new(),
                wineo_core::hooks::Output::Inherit,
                |_| {},
            )
            .is_err()
        );
    }
}
