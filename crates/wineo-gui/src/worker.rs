use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::Context;
use iced::Task;
use iced::futures::channel::mpsc::{UnboundedSender, unbounded};
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::installer::Output;
use wineo_core::launch::{self, Overrides};
use wineo_core::log::{Log, Tail};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, Runtime, State};
use wineo_core::steam_install::{self, Action};
use wineo_core::winetricks::WinetricksEvent;

use crate::Message;
use crate::panel::{PanelError, event_status};
use crate::task;

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
            task::join(
                move || {
                    let work = || -> anyhow::Result<_> {
                        let exe =
                            std::env::current_exe().context("failed to find wineo-gui binary")?;
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
                                    launch::LaunchEvent::Winetricks(
                                        WinetricksEvent::LogStarted,
                                    ) => {
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
                },
                |error| PanelError::from_error(&anyhow::Error::from(error)),
            )
            .await
        },
        move |result| Message::StartFinished(finish_name, result),
    );
    Task::batch([Task::run(receiver, |message| message), work])
}

pub(crate) fn stop(paths: Paths, name: String, force: bool) -> Task<Message> {
    let finish_name = name.clone();
    Task::perform(
        async move {
            task::join(
                move || launch::stop(&paths, &name, force),
                anyhow::Error::from,
            )
            .await
            .map(|outcome| outcome.messages().to_vec())
            .map_err(|error| PanelError::from_error(&error))
        },
        move |result| Message::StopFinished(finish_name, result),
    )
}

/// Asks Steam to install or uninstall a game, which opens Steam's own dialog.
pub(crate) fn steam_request(paths: Paths, name: String, action: Action) -> Task<Message> {
    let finish_name = name.clone();
    Task::perform(
        async move {
            task::join(
                move || steam_request_now(&paths, &name, action),
                anyhow::Error::from,
            )
            .await
            .map_err(|error| PanelError::from_error(&error))
        },
        move |result| Message::SteamRequested(finish_name, result),
    )
}

/// The line the panel shows once Steam has the request, like the CLI's.
pub(crate) fn steam_request_now(
    paths: &Paths,
    name: &str,
    action: Action,
) -> anyhow::Result<String> {
    let global = GlobalConfig::load(paths)?;
    let outcome = steam_install::request(paths, &global, name, action)?;
    Ok(outcome.note().unwrap_or_else(|| match action {
        Action::Install => format!("Asked Steam to install {name}."),
        Action::Uninstall => format!("Asked Steam to uninstall {name}."),
    }))
}

pub(crate) fn stop_after(
    paths: Paths,
    name: String,
    expected: Option<wineo_core::process::Identity>,
) -> Task<Message> {
    let finish_name = name.clone();
    Task::perform(
        async move {
            let Some(expected) = expected else {
                return Ok(vec![AFTER_HOOK_ENDED_NOTE.to_owned()]);
            };
            task::join(
                move || launch::force_stop_after(&paths, &name, &expected),
                anyhow::Error::from,
            )
            .await
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
const READER_NAME: &str = "hook-capture";

struct HookCapture {
    writer: Option<File>,
    state: Arc<Mutex<CaptureState>>,
    done: Arc<AtomicBool>,
    /// Shares the reader's socket, to cut its wait short at the end.
    wake: UnixStream,
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

fn stream_clone(state: &Mutex<CaptureState>) -> io::Result<UnixStream> {
    state
        .lock()
        .map_err(|_| io::Error::other("hook capture lock poisoned"))?
        .stream
        .try_clone()
}

/// Sleeps until the stream has bytes, has hit its end, or was shut down.
fn wait_readable(stream: &UnixStream) -> io::Result<()> {
    use rustix::event::{PollFd, PollFlags, poll};
    loop {
        let mut fds = [PollFd::new(stream, PollFlags::IN)];
        match poll(&mut fds, None) {
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

impl HookCapture {
    fn new() -> io::Result<Self> {
        Self::named(READER_NAME)
    }

    /// A distinct name lets a test find its own reader thread among the
    /// others running in parallel.
    fn named(name: &str) -> io::Result<Self> {
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
        let wake = stream_clone(&state)?;
        let watch = stream_clone(&state)?;
        let reader = thread::Builder::new().name(name.into()).spawn(move || {
            loop {
                // Read before draining: whatever was written before the
                // flag was set is then in the socket for the last drain.
                let finished = read_done.load(Ordering::Acquire);
                read_state
                    .lock()
                    .map_err(|_| io::Error::other("hook capture lock poisoned"))?
                    .drain()?;
                if finished {
                    return Ok(());
                }
                wait_readable(&watch)?;
            }
        })?;
        Ok(Self {
            writer: Some(OwnedFd::from(writer).into()),
            state,
            done,
            wake,
            reader: Some(reader),
        })
    }

    /// Ends the reader's wait. A hook's grandchild can keep the write end
    /// open long after the hook exits, so no end of file may be coming.
    fn stop_reader(&mut self) {
        self.done.store(true, Ordering::Release);
        drop(self.writer.take());
        // Shutting the read side makes the socket readable for good, which
        // wakes the reader. It fails only if the peer is already gone, and
        // then the reader was woken by that.
        let _ = self.wake.shutdown(Shutdown::Read);
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
        self.stop_reader();
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
        self.stop_reader();
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
            task::join(
                move || {
                    let runtime = Runtime::new(&paths, &name);
                    let path = match &source {
                        LogSource::Output => runtime.log_path(),
                        LogSource::Install(_) => runtime.install_log_path(),
                    };
                    let file = File::open(&path)
                        .with_context(|| format!("failed to open {}", path.display()))?;
                    // Only output.log is capped and cleared by a supervisor, so
                    // only it has a reset counter to honor.
                    let session =
                        matches!(source, LogSource::Output).then(|| Log::new(path.clone()));
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
                },
                anyhow::Error::from,
            )
            .await
            .map_err(|error| error.to_string())
        },
        move |result| Message::LogFinished(generation, result),
    );
    Task::batch([Task::run(receiver, |message| message), work])
}

#[cfg(test)]
mod tests;
