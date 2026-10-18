//! The environment variable rows.

use crate::settings::{Page, Step, View};

impl Page {
    pub(super) fn env_key(&mut self, index: usize, key: String) -> Step {
        if let Some(row) = self.draft().and_then(|draft| draft.env.get_mut(index)) {
            row.key = key;
            self.edited();
        } else if let View::Global(editor) = &mut self.view
            && let Some(row) = editor.form.draft.env.get_mut(index)
        {
            row.key = key;
            self.edited();
        }
        Step::none()
    }
    pub(super) fn env_value(&mut self, index: usize, value: String) -> Step {
        if let Some(row) = self.draft().and_then(|draft| draft.env.get_mut(index)) {
            row.value = value;
            self.edited();
        } else if let View::Global(editor) = &mut self.view
            && let Some(row) = editor.form.draft.env.get_mut(index)
        {
            row.value = value;
            self.edited();
        }
        Step::none()
    }
    pub(super) fn env_remove(&mut self, index: usize) -> Step {
        if let Some(draft) = self.draft()
            && index < draft.env.len()
        {
            draft.env.remove(index);
            self.edited();
        }
        if let View::Global(editor) = &mut self.view
            && index < editor.form.draft.env.len()
        {
            editor.form.draft.env.remove(index);
            self.edited();
        }
        Step::none()
    }
    pub(super) fn new_env_key(&mut self, key: String) -> Step {
        if let Some(editor) = self.editor_mut() {
            editor.new_env.key = key;
        } else if let View::Global(editor) = &mut self.view {
            editor.new_env.key = key;
        }
        Step::none()
    }
    pub(super) fn new_env_value(&mut self, value: String) -> Step {
        if let Some(editor) = self.editor_mut() {
            editor.new_env.value = value;
        } else if let View::Global(editor) = &mut self.view {
            editor.new_env.value = value;
        }
        Step::none()
    }
    pub(super) fn env_add(&mut self) -> Step {
        if let Some(editor) = self.editor_mut() {
            let row = &editor.new_env;
            if editor
                .form
                .draft
                .new_env_problem(&row.key, &row.value)
                .is_some()
            {
                return Step::none();
            }
            let row = std::mem::take(&mut editor.new_env);
            editor.form.draft.env.push(row);
            self.edited();
        }
        if let View::Global(editor) = &mut self.view {
            let row = &editor.new_env;
            if editor
                .form
                .draft
                .new_env_problem(&row.key, &row.value)
                .is_none()
            {
                let row = std::mem::take(&mut editor.new_env);
                editor.form.draft.env.push(row);
                self.edited();
            }
        }
        Step::none()
    }
}
