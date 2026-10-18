//! Edits to the environment's runner, builds, lists and proxied entry.

use wineo_core::components::Component;

use super::refused;
use crate::settings::Page;
use crate::settings::Step;
use crate::settings::form::{Build, List};
use crate::settings::load::{self, RunnerChoice};

impl Page {
    pub(super) fn runner_picked(&mut self, choice: RunnerChoice) -> Step {
        self.edited();
        if let Some(editor) = self.editor_mut() {
            if refused(editor.info.current.as_ref(), &choice) {
                editor.refusal = Some(load::KIND_MISMATCH.to_owned());
            } else {
                editor.refusal = None;
                editor.form.draft.runner = Some(choice.name);
            }
        }
        Step::none()
    }
    pub(super) fn args_add(&mut self) -> Step {
        if let Some(editor) = self.editor_mut()
            && !editor.args_input.is_empty()
        {
            let arg = std::mem::take(&mut editor.args_input);
            editor.form.draft.fields.args.push(arg);
            self.edited();
        }
        Step::none()
    }
    pub(super) fn build_picked(&mut self, component: Component, build: Build) -> Step {
        self.edited();
        if let Some(editor) = self.editor_mut() {
            editor.build_refusal = editor
                .form
                .pick_build(component, build.0)
                .err()
                .map(str::to_owned);
        }
        Step::none()
    }
    pub(super) fn build_removed(&mut self, component: Component) -> Step {
        self.edited();
        if let Some(editor) = self.editor_mut() {
            editor.build_refusal = editor.form.remove_build(component).err().map(str::to_owned);
        }
        Step::none()
    }
    pub(super) fn proxied_title(&mut self, title: String) -> Step {
        if let Some(proxied) = self.draft().and_then(|draft| draft.proxied.as_mut()) {
            let title = (!title.is_empty()).then_some(title);
            if proxied.title != title {
                proxied.title = title;
                self.edited();
            }
        }
        Step::none()
    }
    pub(super) fn proxied_desktop(&mut self, desktop: bool) -> Step {
        if let Some(proxied) = self.draft().and_then(|draft| draft.proxied.as_mut())
            && proxied.desktop != desktop
        {
            proxied.desktop = desktop;
            self.edited();
        }
        Step::none()
    }
    pub(super) fn list_add(&mut self, list: List) -> Step {
        if let Some(editor) = self.editor_mut() {
            let input = editor.list_input(list);
            let items = editor.form.draft.list(list);
            if input.is_empty() || list.problem(items, input).is_some() {
                return Step::none();
            }
            let item = std::mem::take(editor.list_input_mut(list));
            editor.form.draft.list_mut(list).push(item);
            self.edited();
        }
        Step::none()
    }
}
