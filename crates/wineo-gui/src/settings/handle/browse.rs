//! Choosing a path or folder with the file picker.

use iced::Task;
use wineo_core::paths::Paths;

use crate::Message;
use crate::settings::{Event, Page, Step, TargetEdit, View, global};

impl Page {
    pub(super) fn browse_path(&mut self, target: Option<String>, cwd: bool) -> Step {
        if self.locked() || self.global {
            return Step::none();
        }
        let Some(editor) = self.editor() else {
            return Step::none();
        };
        let previous = match &target {
            Some(target) => match editor.form.draft.targets.get(target) {
                Some(values) if cwd => values.cwd.clone(),
                Some(values) => values.exe.clone(),
                None => return Step::none(),
            },
            None => editor.new_target.path.clone(),
        };
        self.resolve_sequence = self.resolve_sequence.wrapping_add(1);
        let sequence = self.resolve_sequence;
        let generation = self.generation;
        Step::task(Task::perform(
            crate::picker::pick(
                if cwd {
                    "Select working directory"
                } else {
                    "Select executable"
                }
                .to_owned(),
                cwd,
            ),
            move |result| {
                Message::Settings(Event::PickedPath(
                    generation,
                    sequence,
                    target.clone(),
                    cwd,
                    previous.clone(),
                    result,
                ))
            },
        ))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn picked_path(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        target: Option<String>,
        cwd: bool,
        previous: String,
        result: Result<Option<std::path::PathBuf>, String>,
    ) -> Step {
        if generation != self.generation || sequence != self.resolve_sequence || self.locked() {
            return Step::none();
        }
        let Some(editor) = self.editor() else {
            return Step::none();
        };
        let current = match &target {
            Some(target) => match editor.form.draft.targets.get(target) {
                Some(values) if cwd => &values.cwd,
                Some(values) => &values.exe,
                None => return Step::none(),
            },
            None => &editor.new_target.path,
        };
        if *current != previous {
            return Step::none();
        }
        match result {
            Ok(Some(path)) => {
                let value = path.to_string_lossy().into_owned();
                let edit = match target {
                    Some(target) if cwd => Event::Target(target, TargetEdit::Cwd(value)),
                    Some(target) => Event::Target(target, TargetEdit::Exe(value)),
                    None => Event::NewTargetPath(value),
                };
                self.handle(paths, edit)
            }
            Ok(None) => Step::none(),
            Err(error) => {
                self.error = Some(error);
                self.error_checkup = false;
                Step::none()
            }
        }
    }
    pub(super) fn browse_games_root(&mut self, paths: &Paths) -> Step {
        let View::Global(editor) = &self.view else {
            return Step::none();
        };
        if self.locked() || paths.portable_root().is_some() {
            return Step::none();
        }
        let previous = editor.form.draft.games_root.clone();
        let generation = self.generation;
        Step::task(Task::perform(
            crate::picker::pick("Select games folder".to_owned(), true),
            move |result| {
                Message::Settings(Event::PickedGamesRoot(generation, previous.clone(), result))
            },
        ))
    }
    pub(super) fn picked_games_root(
        &mut self,
        paths: &Paths,
        generation: u64,
        previous: String,
        result: Result<Option<std::path::PathBuf>, String>,
    ) -> Step {
        let View::Global(editor) = &self.view else {
            return Step::none();
        };
        // A field typed in meanwhile wins over a late pick.
        if generation != self.generation || editor.form.draft.games_root != previous {
            return Step::none();
        }
        match result {
            Ok(Some(path)) => self.update(
                paths,
                Event::GlobalField(
                    global::Field::GamesRoot,
                    path.to_string_lossy().into_owned(),
                ),
            ),
            Ok(None) => Step::none(),
            Err(error) => {
                self.error = Some(error);
                self.error_checkup = false;
                Step::none()
            }
        }
    }
}
