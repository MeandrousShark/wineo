//! Adding, scanning and editing the environment's targets.

use wineo_core::paths::Paths;

use crate::settings::load::{self, ProgramChoice, Scan};
use crate::settings::{NewTarget, Page, Step, TargetEdit, form};

impl Page {
    pub(super) fn add_open(&mut self, paths: &Paths, open: bool) -> Step {
        let generation = self.generation;
        self.scan_sequence = self.scan_sequence.wrapping_add(1);
        let sequence = self.scan_sequence;
        let Some(editor) = self.editor_mut() else {
            return Step::none();
        };
        editor.add_open = open;
        // A native environment has no prefix to scan.
        if !open || editor.info.native {
            return Step::none();
        }
        editor.scan = Some(Scan::Scanning);
        Step::task(load::scan_task(
            paths.clone(),
            self.name.clone(),
            generation,
            sequence,
        ))
    }
    pub(super) fn program_picked(&mut self, program: ProgramChoice) -> Step {
        if let Some(editor) = self.editor_mut() {
            editor.candidate_exe = Some(program.exe.clone());
            editor.new_target = NewTarget {
                name: editor.form.suggest_target(&program.name, &program.exe),
                path: program.exe,
                label: Some(program.name),
            };
            editor.add_error = None;
        }
        Step::none()
    }
    pub(super) fn new_target_path(&mut self, paths: &Paths, path: String) -> Step {
        if let Some(editor) = self.editor_mut() {
            editor.candidate_exe = wineo_core::target_path::candidate_exe(
                paths,
                editor.info.wine_prefix.as_deref(),
                &path,
            );
            editor.new_target.path = path;
            // The picked program's name no longer fits a typed path.
            editor.new_target.label = None;
            editor.add_error = None;
        }
        Step::none()
    }
    pub(super) fn target_add(&mut self, paths: &Paths) -> Step {
        let Some(editor) = self.editor() else {
            return Step::none();
        };
        if !editor.can_add_target() || self.resolving.is_some() {
            return Step::none();
        }
        let request = editor.new_target.clone();
        self.resolve_sequence = self.resolve_sequence.wrapping_add(1);
        self.resolving = Some(request.clone());
        Step::task(load::resolve_task(
            paths.clone(),
            self.name.clone(),
            request.path,
            self.generation,
            self.resolve_sequence,
        ))
    }
    pub(super) fn resolved(
        &mut self,
        generation: u64,
        sequence: u64,
        result: Result<String, String>,
    ) -> Step {
        if generation != self.generation || sequence != self.resolve_sequence {
            return Step::none();
        }
        let Some(request) = self.resolving.take() else {
            return Step::none();
        };
        // A save started since, or the form changed under the check,
        // so this no longer answers what's on the page.
        if self.locked() {
            return Step::none();
        }
        let Some(editor) = self.editor_mut() else {
            return Step::none();
        };
        if editor.new_target != request {
            return Step::none();
        }
        match result.and_then(|exe| editor.add_resolved(&request, exe)) {
            Ok(()) => self.edited(),
            Err(error) => editor.add_error = Some(error),
        }
        Step::none()
    }
    /// Applies one target's edit. Returns whether the draft changed.
    pub(super) fn target_edit(&mut self, target: &str, edit: TargetEdit) -> bool {
        let Some(editor) = self.editor_mut() else {
            return false;
        };
        let form = &mut editor.form;
        let Some(values) = form.draft.targets.get_mut(target) else {
            return false;
        };
        match edit {
            TargetEdit::Exe(exe) => values.exe = wineo_core::target_path::clean_path(&exe),
            TargetEdit::Cwd(cwd) => values.cwd = wineo_core::target_path::clean_path(&cwd),
            TargetEdit::Name(name) => values.name = name,
            TargetEdit::Desktop(on) => values.desktop = on,
            TargetEdit::Switch(switch, on) => *values.switch_mut(switch) = Some(on),
            TargetEdit::Reset(switch) => *values.switch_mut(switch) = None,
            TargetEdit::GamescopeReset => values.gamescope = form::TargetGamescope::Inherit,
            TargetEdit::ArgsAdd => {
                let Some(arg) = editor
                    .target_args_input
                    .remove(target)
                    .filter(|arg| !arg.is_empty())
                else {
                    return false;
                };
                values.args.push(arg);
            }
            TargetEdit::ArgsRemove(index) => {
                if index >= values.args.len() {
                    return false;
                }
                values.args.remove(index);
            }
            TargetEdit::Default(on) => form.set_default(target, on),
            TargetEdit::Gamescope(on) => return form.set_target_gamescope(target, on),
            TargetEdit::GamescopeHere => return form.override_target_gamescope(target),
            TargetEdit::FieldText(field, value) => {
                return form.edit_target_fields(target, |fields| fields.set_text(field, value));
            }
            TargetEdit::FieldChoice(field, choice) => {
                return form.edit_target_fields(target, |fields| {
                    fields.set_choice(field, choice);
                    true
                });
            }
            TargetEdit::GrabCursor(on) => {
                return form.edit_target_fields(target, |fields| {
                    fields.grab_cursor = on;
                    true
                });
            }
            TargetEdit::GamescopeArgsAdd => {
                let arg = editor.target_gamescope_args_input(target).to_owned();
                if arg.is_empty()
                    || !editor.form.edit_target_fields(target, |fields| {
                        fields.args.push(arg);
                        true
                    })
                {
                    return false;
                }
                editor.target_gamescope_args_input.remove(target);
            }
            TargetEdit::GamescopeArgsRemove(index) => {
                return form.edit_target_fields(target, |fields| {
                    if index >= fields.args.len() {
                        return false;
                    }
                    fields.args.remove(index);
                    true
                });
            }
            TargetEdit::ArgsInput(value) => {
                editor.target_args_input.insert(target.to_owned(), value);
                return false;
            }
            TargetEdit::GamescopeArgsInput(value) => {
                editor
                    .target_gamescope_args_input
                    .insert(target.to_owned(), value);
                return false;
            }
            TargetEdit::AskRemove => {
                editor.removing = Some(target.to_owned());
                return false;
            }
            TargetEdit::Remove => {
                if editor.removing.as_deref() != Some(target) {
                    return false;
                }
                editor.removing = None;
                editor.target_args_input.remove(target);
                editor.target_gamescope_args_input.remove(target);
                editor.open_cards.remove(target);
                return editor.form.remove_target(target);
            }
        }
        true
    }
}
