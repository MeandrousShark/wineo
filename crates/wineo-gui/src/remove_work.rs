use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use iced::Task;
use iced::futures::channel::mpsc::unbounded;
use wineo_core::cancel::Token;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Entry, RemoveEvent};

use crate::Message;

pub(crate) fn measure(generation: u64, path: PathBuf, token: Token) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || measure_size(&path, &token))
                .await
                .map_err(|error| error.to_string())?
        },
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
) -> Result<Entry, String> {
    tokio::task::spawn_blocking(move || {
        let result = trash::remove_with(&paths, &name, include_prefix, &token, events)
            .map_err(|error| error.to_string());
        match result {
            Err(error)
                if trash::pending_entries(&paths)
                    .is_ok_and(|entries| entries.iter().any(|entry| entry.name == name)) =>
            {
                Err(format!(
                    "Removing {name} didn't finish. Recover it from Trash.\n{error}"
                ))
            }
            result => result,
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn restore_worker(paths: Paths, id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        trash::restore(&paths, &id).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn measure_size(root: &Path, token: &Token) -> Result<u64, String> {
    if token.is_canceled() {
        return Err("measurement canceled".to_owned());
    }

    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?;
    if root_metadata.file_type().is_symlink() {
        return Ok(0);
    }

    let root_device = root_metadata.dev();
    let mut size = 0_u64;
    let mut pending = vec![(root.to_path_buf(), root_metadata)];

    while let Some((path, metadata)) = pending.pop() {
        if token.is_canceled() {
            return Err("measurement canceled".to_owned());
        }
        if metadata.dev() != root_device {
            continue;
        }

        if metadata.file_type().is_file() {
            size = size.saturating_add(metadata.len());
            continue;
        }
        if !metadata.file_type().is_dir() {
            continue;
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
            pending.push((entry_path, metadata));
        }
    }

    Ok(size)
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
