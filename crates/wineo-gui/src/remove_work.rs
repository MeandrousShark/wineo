use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use iced::Task;
use iced::futures::channel::mpsc::unbounded;
use wineo_core::cancel::Token;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Entry, RemoveEvent};

use crate::Message;
use crate::task;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoveError {
    Canceled,
    Failed(String),
}

impl From<String> for RemoveError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<&str> for RemoveError {
    fn from(error: &str) -> Self {
        Self::Failed(error.to_owned())
    }
}

impl fmt::Display for RemoveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canceled => formatter.write_str("removing environment canceled"),
            Self::Failed(error) => formatter.write_str(error),
        }
    }
}

pub(crate) fn measure(generation: u64, path: PathBuf, token: Token) -> Task<Message> {
    task::blocking(
        move || measure_size(&path, &token),
        move |result| Message::RemoveSize(generation, result),
    )
}

pub(crate) fn remove(
    generation: u64,
    paths: Paths,
    name: String,
    include_prefix: bool,
    token: Token,
) -> Task<Message> {
    let (sender, receiver) = unbounded();
    let work = Task::perform(
        remove_worker(paths, name, include_prefix, token, move |event| {
            let _ = sender.unbounded_send(event);
        }),
        move |result| Message::RemoveFinished(generation, result),
    );
    Task::batch([
        Task::run(receiver, move |event| {
            Message::RemoveProgress(generation, event)
        }),
        work,
    ])
}

pub(crate) fn restore(generation: u64, paths: Paths, id: String) -> Task<Message> {
    Task::perform(restore_worker(paths, id), move |result| {
        Message::UndoFinished(generation, result)
    })
}

async fn remove_worker(
    paths: Paths,
    name: String,
    include_prefix: bool,
    token: Token,
    events: impl Fn(RemoveEvent) + Send + 'static,
) -> Result<Entry, RemoveError> {
    task::join(
        move || {
            let mut previous_ids = None;
            let result = trash::remove_with(&paths, &name, include_prefix, &token, |event| {
                if event == RemoveEvent::Moving {
                    // Core holds the removal locks here, before publishing this attempt's entry.
                    previous_ids = trash::pending_entries(&paths)
                        .ok()
                        .map(|entries| entries.into_iter().map(|entry| entry.id).collect());
                }
                events(event);
            });
            match result {
                Ok(entry) => Ok(entry),
                Err(error) => {
                    if matches!(
                        error.downcast_ref::<wineo_core::error::Error>(),
                        Some(wineo_core::error::Error::Canceled(_))
                    ) {
                        return Err(RemoveError::Canceled);
                    }

                    let message = error.to_string();
                    let pending_created = trash::pending_entries(&paths).is_ok_and(|entries| {
                        has_new_pending_entry(&name, previous_ids.as_ref(), &entries)
                    });
                    Err(RemoveError::Failed(pending_error(
                        &name,
                        pending_created,
                        message,
                    )))
                }
            }
        },
        |error| RemoveError::Failed(error.to_string()),
    )
    .await
}

async fn restore_worker(paths: Paths, id: String) -> Result<(), String> {
    task::join_string(move || trash::restore(&paths, &id).map_err(|error| error.to_string())).await
}

fn measure_size(root: &Path, token: &Token) -> Result<u64, String> {
    if token.is_canceled() {
        return Err("measurement canceled".to_owned());
    }

    let resolved_root = fs::canonicalize(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?;
    let root_metadata = fs::symlink_metadata(&resolved_root)
        .map_err(|error| format!("failed to read {}: {error}", resolved_root.display()))?;

    let root_device = root_metadata.dev();
    let mut size = 0_u64;
    let mut pending = Vec::new();
    if root_metadata.file_type().is_file() {
        return Ok(root_metadata.len());
    }
    if root_metadata.file_type().is_dir() {
        pending.push(resolved_root);
    }

    while let Some(path) = pending.pop() {
        if token.is_canceled() {
            return Err("measurement canceled".to_owned());
        }

        let entries = fs::read_dir(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        for entry in entries {
            if token.is_canceled() {
                return Err("measurement canceled".to_owned());
            }
            let entry =
                entry.map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let entry_path = entry.path();
            let metadata = fs::symlink_metadata(&entry_path)
                .map_err(|error| format!("failed to read {}: {error}", entry_path.display()))?;
            if metadata.file_type().is_symlink() || metadata.dev() != root_device {
                continue;
            }
            if metadata.file_type().is_file() {
                size = size.saturating_add(metadata.len());
            } else if metadata.file_type().is_dir() {
                pending.push(entry_path);
            }
        }
    }

    Ok(size)
}

fn has_new_pending_entry(
    name: &str,
    previous_ids: Option<&HashSet<String>>,
    pending: &[trash::PendingEntry],
) -> bool {
    previous_ids.is_some_and(|previous_ids| {
        pending
            .iter()
            .any(|entry| entry.name == name && !previous_ids.contains(&entry.id))
    })
}

fn pending_error(name: &str, pending_created: bool, error: String) -> String {
    if pending_created {
        format!("Removing {name} didn't finish. Recover it from Trash.\n{error}")
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;

    use super::*;

    fn fixture() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        fs::create_dir_all(paths.environments_dir()).unwrap();
        (root, paths)
    }

    fn write_config(paths: &Paths, name: &str, contents: &str) {
        fs::write(paths.environment_config(name), contents).unwrap();
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    #[test]
    fn measurement_stops_when_canceled() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("large-file"), [0; 16]).unwrap();
        let token = Token::new();
        token.cancel();

        assert_eq!(
            measure_size(root.path(), &token).unwrap_err(),
            "measurement canceled"
        );
    }

    #[test]
    fn measurement_counts_files_without_following_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(root.path().join("inside"), [0; 7]).unwrap();
        fs::write(outside.path().join("outside"), [0; 19]).unwrap();
        symlink(
            outside.path().join("outside"),
            root.path().join("linked-file"),
        )
        .unwrap();
        symlink(outside.path(), root.path().join("linked-dir")).unwrap();

        assert_eq!(measure_size(root.path(), &Token::new()).unwrap(), 7);
    }

    #[test]
    fn measurement_accumulates_many_files() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..256 {
            fs::write(root.path().join(index.to_string()), [0; 3]).unwrap();
        }

        assert_eq!(measure_size(root.path(), &Token::new()).unwrap(), 256 * 3);
    }

    #[test]
    fn measurement_resolves_a_symlinked_root_once() {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        fs::write(target.path().join("inside"), [0; 7]).unwrap();
        let linked_root = root.path().join("prefix");
        symlink(target.path(), &linked_root).unwrap();

        assert_eq!(measure_size(&linked_root, &Token::new()).unwrap(), 7);
    }

    #[test]
    fn pending_error_does_not_wrap_an_older_entry_with_the_same_name() {
        let previous_ids = HashSet::from(["older-id".to_owned()]);
        let pending = [trash::PendingEntry {
            id: "older-id".to_owned(),
            name: "game".to_owned(),
            finished: false,
        }];

        let pending_created = has_new_pending_entry("game", Some(&previous_ids), &pending);
        assert_eq!(
            pending_error("game", pending_created, "unrelated failure".to_owned()),
            "unrelated failure"
        );
    }

    #[test]
    fn pending_error_wraps_a_new_entry_created_by_this_remove() {
        let previous_ids = HashSet::from(["older-id".to_owned()]);
        let pending = [trash::PendingEntry {
            id: "new-id".to_owned(),
            name: "game".to_owned(),
            finished: false,
        }];

        let pending_created = has_new_pending_entry("game", Some(&previous_ids), &pending);
        assert_eq!(
            pending_error("game", pending_created, "interrupted".to_owned()),
            "Removing game didn't finish. Recover it from Trash.\ninterrupted"
        );
    }

    #[test]
    fn failure_before_moving_cannot_claim_another_attempts_pending_entry() {
        let pending = [trash::PendingEntry {
            id: "another-attempt-id".into(),
            name: "game".into(),
            finished: false,
        }];
        let pending_created = has_new_pending_entry("game", None, &pending);
        assert_eq!(
            pending_error("game", pending_created, "lock failure".into()),
            "lock failure"
        );
    }

    #[test]
    fn remove_worker_preserves_typed_cancellation_and_config() {
        let (_root, paths) = fixture();
        write_config(&paths, "game", "kind = 'native'\n");
        let token = Token::new();
        token.cancel();
        let error = runtime()
            .block_on(remove_worker(
                paths.clone(),
                "game".into(),
                false,
                token,
                |_| {},
            ))
            .unwrap_err();
        assert_eq!(error, RemoveError::Canceled);
        assert!(paths.environment_config("game").exists());
        assert!(trash::pending_entries(&paths).unwrap().is_empty());
    }

    #[test]
    fn remove_worker_forwards_prefix_inclusion_and_progress() {
        let (_root, paths) = fixture();
        let prefix = paths.default_prefix_root().join("game");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        write_config(
            &paths,
            "game",
            &format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                prefix.display()
            ),
        );
        let (sender, receiver) = mpsc::channel();
        let entry = runtime()
            .block_on(remove_worker(
                paths.clone(),
                "game".to_owned(),
                true,
                Token::new(),
                move |event| sender.send(event).unwrap(),
            ))
            .unwrap();

        assert!(entry.include_prefix);
        assert!(!prefix.exists());
        assert!(trash::inspect(&paths, &entry.id).unwrap().include_prefix);
        assert!(
            receiver
                .try_iter()
                .any(|event| event == RemoveEvent::Moving)
        );
    }

    #[test]
    fn restore_worker_uses_the_entry_id_and_preserves_collisions() {
        let (_root, paths) = fixture();
        let original = "kind = 'native'\n# original\n";
        write_config(&paths, "game", original);
        let entry = runtime()
            .block_on(remove_worker(
                paths.clone(),
                "game".to_owned(),
                false,
                Token::new(),
                |_| {},
            ))
            .unwrap();

        let replacement = "kind = 'native'\n# replacement\n";
        write_config(&paths, "game", replacement);
        let error = runtime()
            .block_on(restore_worker(paths.clone(), entry.id.clone()))
            .unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            replacement
        );

        fs::remove_file(paths.environment_config("game")).unwrap();
        runtime()
            .block_on(restore_worker(paths.clone(), entry.id))
            .unwrap();
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            original
        );
    }
}
