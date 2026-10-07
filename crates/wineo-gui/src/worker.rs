use std::fs::File;
use std::io::{self, Write};

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
                let exe = std::env::current_exe().context("failed to find wineo-gui binary")?;
                let log = Runtime::new(&paths, &name).install_log_path();
                if let Some(parent) = log.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let result = launch::start(
                    &paths,
                    &name,
                    Overrides {
                        target: Some(target),
                        ..Overrides::default()
                    },
                    &exe,
                    &token,
                    wineo_core::hooks::Output::Inherit,
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
                            _ => {}
                        }
                    },
                );
                // An error before Done still ends the install phase.
                install_done.cancel();
                result
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
            .map_err(|error| PanelError::from_error(&error))
        },
        move |result| Message::StartFinished(finish_name, result),
    );
    Task::batch([Task::run(receiver, |message| message), work])
}

pub(crate) fn stop(paths: Paths, name: String) -> Task<Message> {
    let finish_name = name.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || launch::stop(&paths, &name, false))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
                .map(|outcome| outcome.messages().to_vec())
                .map_err(|error| PanelError::from_error(&error))
        },
        move |result| Message::StopFinished(finish_name, result),
    )
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
