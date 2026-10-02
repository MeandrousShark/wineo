use std::sync::Arc;

use anyhow::Context;
use std::time::Duration;

use iced::Task;
use iced::futures::channel::mpsc::{UnboundedSender, unbounded};
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{self, Provider};
use wineo_core::error::Error as CoreError;
use wineo_core::paths::Paths;
use wineo_core::runtime::Runtime;
use wineo_core::validate::{self, Fix, Note};
use wineo_core::winetricks::{EnsureOutput, WinetricksEvent};

use crate::Message;
use crate::checkup::{Event, Outcome, Scope};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) fn check(scope: Scope, generation: u64, paths: Paths) -> Task<Message> {
    let event_scope = scope.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let result = check_report(&paths, &scope);
                result.map_err(|error| paths.contract_text(&format!("{error:#}")))
            })
            .await
            .map_err(|error| error.to_string())?
        },
        move |result| Message::Checkup(Event::Checked(event_scope, generation, result)),
    )
}

pub(crate) fn check_report(paths: &Paths, scope: &Scope) -> wineo_core::Result<validate::Report> {
    match scope {
        Scope::Global => validate::check_all(paths),
        Scope::Environment(name) => validate::check_environment(paths, name),
    }
}

pub(crate) fn apply(generation: u64, paths: Paths, fixes: Vec<Fix>, token: Token) -> Task<Message> {
    let (sender, receiver) = unbounded();
    let work = Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let mut outcome = run_fixes(&fixes, &token, |fix, token| {
                    apply_one(&paths, fix, token, generation, &sender)
                });
                for (_, message) in &mut outcome.failed {
                    *message = paths.contract_text(message);
                }
                outcome
            })
            .await
            .map_err(|error| error.to_string())
        },
        move |result| Message::Checkup(Event::Finished(generation, result)),
    );
    Task::batch([Task::run(receiver, Message::Checkup), work])
}

pub(crate) fn recover_snapshot(generation: u64, paths: Paths, fix: Fix) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let result = validate::apply_single(
                    &paths,
                    &fix,
                    &Token::new(),
                    EnsureOutput::Stderr,
                    |_| {},
                    |_, _| unreachable!("snapshot recovery does not download builds"),
                );
                let outcome = match result {
                    Ok(notes) => Outcome {
                        applied: vec![fix],
                        warnings: notes
                            .into_iter()
                            .map(|note| match note {
                                Note::Warning(message) => message,
                            })
                            .collect(),
                        ..Outcome::default()
                    },
                    Err(error) => Outcome {
                        failed: vec![(fix, paths.contract_text(&format!("{error:#}")))],
                        ..Outcome::default()
                    },
                };
                Ok::<_, String>(outcome)
            })
            .await
            .map_err(|error| error.to_string())?
        },
        move |result| Message::Checkup(Event::Finished(generation, result)),
    )
}

pub(crate) fn run_fixes(
    fixes: &[Fix],
    token: &Token,
    mut apply_one: impl FnMut(&Fix, &Token) -> anyhow::Result<Vec<Note>>,
) -> Outcome {
    let mut outcome = Outcome {
        applied: Vec::new(),
        failed: Vec::new(),
        warnings: Vec::new(),
        canceled: false,
        skipped: 0,
    };

    let mut unique = Vec::new();
    for fix in fixes {
        if !unique.contains(fix) {
            unique.push(fix.clone());
        }
    }

    for (index, fix) in unique.iter().enumerate() {
        if token.is_canceled() {
            outcome.canceled = true;
            outcome.skipped = unique.len() - index;
            break;
        }

        match apply_one(fix, token) {
            Ok(notes) => {
                outcome.applied.push(fix.clone());
                outcome
                    .warnings
                    .extend(notes.into_iter().map(|note| match note {
                        Note::Warning(warning) => warning,
                    }));
            }
            Err(error) if is_canceled(&error) => {
                outcome.canceled = true;
                outcome.skipped = unique.len() - index - 1;
                break;
            }
            Err(error) => outcome.failed.push((fix.clone(), format!("{error:#}"))),
        }
    }

    if !unique.is_empty() && token.is_canceled() {
        outcome.canceled = true;
    }

    outcome
}

fn is_canceled(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<CoreError>()
        .is_some_and(|error| matches!(error, CoreError::Canceled(_)))
}

fn apply_one(
    paths: &Paths,
    fix: &Fix,
    token: &Token,
    generation: u64,
    sender: &UnboundedSender<Event>,
) -> anyhow::Result<Vec<Note>> {
    let _ = sender.unbounded_send(Event::Progress(generation, fix.label()));
    let install_log = match fix {
        Fix::InstallWinetricks { environment } => {
            Some(Runtime::new(paths, environment).install_log_path())
        }
        _ => None,
    };
    let output = match install_log.as_deref() {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            EnsureOutput::Log(path)
        }
        None => EnsureOutput::Stderr,
    };
    let install_environment = match fix {
        Fix::InstallWinetricks { environment } => Some(environment.clone()),
        _ => None,
    };
    let install_done = Token::new();
    let result = validate::apply(
        paths,
        fix,
        token,
        output,
        |event| {
            forward_winetricks(
                sender,
                generation,
                install_environment.as_deref(),
                &install_done,
                event,
            );
        },
        |component, version| download_build(paths, component, version, token, generation, sender),
    );
    if install_environment.is_some() {
        install_done.cancel();
        let _ = sender.unbounded_send(Event::InstallEnded(generation));
    }
    result
}

fn forward_winetricks(
    sender: &UnboundedSender<Event>,
    generation: u64,
    environment: Option<&str>,
    done: &Token,
    event: WinetricksEvent,
) {
    if matches!(event, WinetricksEvent::LogStarted)
        && let Some(environment) = environment
    {
        let _ = sender.unbounded_send(Event::InstallStarted(
            generation,
            environment.to_owned(),
            done.clone(),
        ));
    }
    if crate::panel::winetricks_status(&event).is_some()
        || matches!(event, WinetricksEvent::Stopping | WinetricksEvent::Killing)
    {
        let _ = sender.unbounded_send(Event::Winetricks(generation, Arc::new(event)));
    }
}

fn download_build(
    paths: &Paths,
    component: Component,
    version: Option<&str>,
    token: &Token,
    generation: u64,
    sender: &UnboundedSender<Event>,
) -> anyhow::Result<()> {
    let provider = match component {
        Component::Dxvk => Provider::Dxvk,
        Component::Vkd3d => Provider::Vkd3d,
    };
    let client = download::Client::new()?;
    let global = GlobalConfig::load_or_default(paths)?;
    let release = match version {
        Some(version) => client.select_pinned(provider, version),
        None => client.select(provider, None),
    }?;
    let _ = sender.unbounded_send(Event::Progress(
        generation,
        format!("Downloading {} ({})", release.tag, release.verification()),
    ));

    crate::download_progress::install(
        &client,
        paths,
        &global,
        &release,
        token,
        PROGRESS_INTERVAL,
        |line| {
            let _ = sender.unbounded_send(Event::Progress(generation, line));
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wineo_core::download::Progress;

    #[test]
    fn winetricks_only_forwards_status_events_and_log_start() {
        use std::os::unix::process::ExitStatusExt;
        use wineo_core::winetricks::Step;

        let (sender, mut receiver) = unbounded();
        let done = Token::new();
        for event in [
            WinetricksEvent::OutputLine("log line".into()),
            WinetricksEvent::Done(std::process::ExitStatus::from_raw(0)),
            WinetricksEvent::Warning("warning".into()),
        ] {
            forward_winetricks(&sender, 7, Some("game"), &done, event);
        }
        assert!(receiver.try_recv().is_err());
        forward_winetricks(&sender, 7, Some("game"), &done, WinetricksEvent::LogStarted);
        assert!(
            matches!(receiver.try_recv().unwrap(), Event::InstallStarted(7, name, _) if name == "game")
        );
        assert!(receiver.try_recv().is_err());
        for event in [
            WinetricksEvent::Progress(Step {
                verb: "gdiplus".into(),
                position: 1,
                total: 1,
            }),
            WinetricksEvent::Installing(vec!["gdiplus".into()]),
            WinetricksEvent::Stopping,
            WinetricksEvent::Killing,
        ] {
            let expected = std::mem::discriminant(&event);
            forward_winetricks(&sender, 7, Some("game"), &done, event);
            let Event::Winetricks(7, event) = receiver.try_recv().unwrap() else {
                panic!("missing status event");
            };
            assert_eq!(std::mem::discriminant(event.as_ref()), expected);
        }
        assert!(receiver.try_recv().is_err());
    }

    fn fix(name: &str) -> Fix {
        Fix::CleanUpLostRun {
            environment: name.to_owned(),
        }
    }

    #[test]
    fn failed_fix_does_not_stop_the_rest() {
        let fixes = [fix("first"), fix("second"), fix("third")];
        let mut attempted = Vec::new();
        let outcome = run_fixes(&fixes, &Token::new(), |fix, _| {
            let Fix::CleanUpLostRun { environment } = fix else {
                unreachable!()
            };
            attempted.push(environment.clone());
            if environment == "second" {
                anyhow::bail!("second failed");
            }
            Ok(Vec::new())
        });

        assert_eq!(attempted, ["first", "second", "third"]);
        assert_eq!(outcome.applied, [fix("first"), fix("third")]);
        assert_eq!(
            outcome.failed,
            [(fix("second"), "second failed".to_owned())]
        );
        assert!(!outcome.canceled);
        assert_eq!(outcome.skipped, 0);
    }

    #[test]
    fn canceled_fix_keeps_completed_fixes_and_skips_the_rest() {
        let fixes = [fix("first"), fix("current"), fix("last")];
        let token = Token::new();
        let mut attempted = Vec::new();
        let outcome = run_fixes(&fixes, &token, |fix, token| {
            let Fix::CleanUpLostRun { environment } = fix else {
                unreachable!()
            };
            attempted.push(environment.clone());
            if environment == "current" {
                token.cancel();
                anyhow::bail!(CoreError::Canceled("download"));
            }
            Ok(Vec::new())
        });

        assert_eq!(attempted, ["first", "current"]);
        assert_eq!(outcome.applied, [fix("first")]);
        assert!(outcome.failed.is_empty());
        assert!(outcome.canceled);
        assert_eq!(outcome.skipped, 1);
    }

    #[test]
    fn warnings_from_applied_fixes_are_retained() {
        let outcome = run_fixes(&[fix("first")], &Token::new(), |_, _| {
            Ok(vec![Note::Warning("left something behind".to_owned())])
        });

        assert_eq!(outcome.warnings, ["left something behind"]);
        assert_eq!(outcome.applied, [fix("first")]);
    }

    #[test]
    fn cancellation_before_the_batch_skips_every_fix() {
        let fixes = [fix("first"), fix("second")];
        let token = Token::new();
        token.cancel();
        let outcome = run_fixes(&fixes, &token, |_, _| panic!("canceled work must not run"));

        assert!(outcome.applied.is_empty());
        assert!(outcome.canceled);
        assert_eq!(outcome.skipped, 2);
    }

    #[test]
    fn shared_fixes_are_applied_once_in_first_occurrence_order() {
        let fixes = [fix("shared"), fix("other"), fix("shared")];
        let mut attempted = Vec::new();
        let outcome = run_fixes(&fixes, &Token::new(), |fix, _| {
            let Fix::CleanUpLostRun { environment } = fix else {
                unreachable!()
            };
            attempted.push(environment.clone());
            Ok(Vec::new())
        });

        assert_eq!(attempted, ["shared", "other"]);
        assert_eq!(outcome.applied, [fix("shared"), fix("other")]);
    }

    #[test]
    fn canceled_quick_fix_is_kept_and_the_remaining_fix_is_skipped() {
        let fixes = [fix("current"), fix("last")];
        let token = Token::new();
        let outcome = run_fixes(&fixes, &token, |_, token| {
            token.cancel();
            Ok(Vec::new())
        });

        assert_eq!(outcome.applied, [fix("current")]);
        assert!(outcome.canceled);
        assert_eq!(outcome.skipped, 1);
    }

    #[test]
    fn canceled_last_quick_fix_is_kept_and_marks_the_batch_canceled() {
        let token = Token::new();
        let outcome = run_fixes(&[fix("last")], &token, |_, token| {
            token.cancel();
            Ok(Vec::new())
        });

        assert_eq!(outcome.applied, [fix("last")]);
        assert!(outcome.canceled);
        assert_eq!(outcome.skipped, 0);
    }

    #[test]
    fn download_progress_matches_the_install_flows() {
        assert_eq!(
            crate::download_progress::download_line(
                Provider::Dxvk,
                Progress::Downloading {
                    received: 4,
                    total: 12,
                }
            ),
            "Downloading dxvk: 4 B of 12 B"
        );
    }
}
