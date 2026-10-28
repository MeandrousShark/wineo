//! Leaving, saving, reloading and the results that come back.

use iced::Task;
use wineo_core::paths::Paths;

use crate::Message;
use crate::settings::form::EnvRow;
use crate::settings::load::{self, Report, Scan, Snapshot, Texts};
use crate::settings::{Event, Mode, Page, Step, View, unfocus, view};

impl Page {
    pub(super) fn go_back(&mut self) -> Step {
        if self.saving {
            Step::none()
        } else if self.is_dirty() {
            self.leave_prompt = true;
            // Typing would otherwise reach a field behind the prompt.
            Step::task(unfocus())
        } else {
            Step::close()
        }
    }
    pub(super) fn save_edits(&mut self, paths: &Paths) -> Step {
        if !self.can_save() {
            return Step::none();
        }
        let (edits, appearance) = match &self.view {
            View::Form(editor) => (editor.form.edits(), None),
            View::Global(editor) => (editor.form.edits(), editor.appearance.edits()),
            _ => return Step::none(),
        };
        if edits.is_empty() && appearance.is_none() {
            return Step::none();
        }
        self.saving = true;
        self.drop_checks();
        self.leave_prompt = false;
        self.edited();
        Step::task(load::save_task(
            paths.clone(),
            self.name.clone(),
            self.global,
            edits,
            appearance,
            self.generation,
        ))
    }
    pub(super) fn reload_from_disk(&mut self, paths: &Paths) -> Step {
        if !self.can_reload() {
            return Step::none();
        }
        self.edited();
        if let Some(editor) = self.editor_mut() {
            editor.clear_inputs();
        }
        if let View::Global(editor) = &mut self.view {
            editor.new_env = EnvRow::default();
        }
        Step::task(self.reload(paths, Mode::Replace))
    }
    pub(super) fn revert_edits(&mut self, paths: &Paths) -> Step {
        if self.locked() {
            return Step::none();
        }
        self.edited();
        if let Some(editor) = self.editor_mut() {
            editor.form.revert();
            editor.clear_inputs();
            editor.refusal = None;
            editor.build_refusal = None;
        }
        if let View::Global(editor) = &mut self.view {
            editor.form.revert();
            editor.appearance.revert();
            editor.new_env = EnvRow::default();
        }
        if self.changed_on_disk {
            return Step::task(self.reload(paths, Mode::Replace));
        }
        Step::none()
    }
    pub(super) fn loaded(&mut self, paths: &Paths, generation: u64, snapshot: Snapshot) -> Step {
        if generation != self.generation {
            return Step::none();
        }
        if let Some(pending) = self.pending.take() {
            let after_save = pending.after_save;
            self.land(pending, snapshot);
            if self.global && matches!(self.view, View::Global(_)) {
                let reveal = if std::mem::take(&mut self.reveal_steam_key) {
                    let generation = self.generation;
                    view::locate_steam_key().map(move |bounds| {
                        Message::Settings(Event::SteamKeyLocated(generation, bounds))
                    })
                } else {
                    Task::none()
                };
                return Step::task(Task::batch([
                    self.refresh_steamgriddb(paths),
                    self.refresh_steam_key(paths),
                    self.refresh_gog(paths),
                    reveal,
                ]));
            }
            if self.open_add_after_load && self.editor().is_some() {
                self.open_add_after_load = false;
                return self.handle(paths, Event::AddOpen(true));
            }
            if after_save {
                self.scan_sequence = self.scan_sequence.wrapping_add(1);
                let sequence = self.scan_sequence;
                if let Some(editor) = self.editor_mut() {
                    editor.scan = None;
                    if editor.add_open && !editor.info.native {
                        editor.scan = Some(Scan::Scanning);
                        return Step::task(load::scan_task(
                            paths.clone(),
                            self.name.clone(),
                            self.generation,
                            sequence,
                        ));
                    }
                }
            }
        }
        Step::none()
    }
    pub(super) fn checked(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        texts: Texts,
    ) -> Step {
        if generation != self.generation || sequence != self.check_sequence {
            return Step::none();
        }
        self.checking = false;
        let Some(disk) = &self.disk else {
            return Step::none();
        };
        if self.loading() || self.saving {
            return Step::none();
        }
        if *disk == texts {
            // Changed back to what the page holds, so its edits are
            // against the file again.
            self.changed_on_disk = false;
            return Step::none();
        }
        // Only `[appearance]` changed. Like the global file under an
        // environment page, it rebases, so General's edits stay; edits on
        // the Appearance tab ask for Reload instead.
        if self.global
            && disk.global == texts.global
            && !self.appearance().is_some_and(|form| form.is_dirty())
        {
            return Step::task(self.reload(paths, Mode::Rebase));
        }
        if self.global || disk.environment != texts.environment {
            if self.is_dirty() {
                self.changed_on_disk = true;
                Step::none()
            } else {
                Step::task(self.reload(paths, Mode::Replace))
            }
        } else {
            // Only the global file changed under an environment page.
            Step::task(self.reload(paths, Mode::Rebase))
        }
    }
    pub(super) fn saved(
        &mut self,
        paths: &Paths,
        generation: u64,
        result: Result<Report, String>,
    ) -> Step {
        if generation != self.generation {
            return Step::none();
        }
        self.saving = false;
        match result {
            Ok(report) => {
                self.notes = report.lines;
                match report.failed {
                    // The file may or may not have been written before
                    // the failure, so read it back as the new base and
                    // keep the edits, which drops whatever did land.
                    Some(error) => {
                        self.error = Some(error);
                        self.error_checkup = report.checkup;
                        Step::task(self.reload_after(paths, Mode::Rebase, true))
                    }
                    None => Step::task(self.reload_after(paths, Mode::AfterSave, true)),
                }
            }
            Err(error) => {
                self.error = Some(error);
                self.error_checkup = false;
                Step::none()
            }
        }
    }
}
