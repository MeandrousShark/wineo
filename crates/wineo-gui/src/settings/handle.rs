//! The settings page's event handling and how a reload lands.

mod browse;
mod env;
mod fields;
mod keys;
mod lifecycle;
mod targets;

use wineo_core::paths::Paths;

use super::form::Form;
use super::load::{Content, Current, RunnerChoice, Snapshot};
use super::{ChangedFile, Editor, Event, Mode, Page, Pending, Step, View, global, load, view};

impl Page {
    pub(super) fn handle(&mut self, paths: &Paths, event: Event) -> Step {
        if self.ignores(&event) {
            return Step::none();
        }
        match event {
            Event::BrowsePath(target, cwd) => self.browse_path(target, cwd),
            Event::PickedPath(generation, sequence, target, cwd, previous, result) => {
                self.picked_path(paths, generation, sequence, target, cwd, previous, result)
            }
            Event::Back => self.go_back(),
            Event::Tab(tab) => self.select_tab(tab),
            Event::Scrolled(tab, offset) => {
                self.scrolled(tab, offset);
                Step::none()
            }
            Event::Discard if self.saving => Step::none(),
            Event::Discard => Step::close(),
            Event::KeepEditing => {
                self.leave_prompt = false;
                Step::none()
            }
            Event::Save => self.save_edits(paths),
            Event::Reload => self.reload_from_disk(paths),
            Event::Revert => self.revert_edits(paths),
            Event::RunnerPicked(choice) => self.runner_picked(choice),
            Event::Switch(switch, on) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    *draft.switch_mut(switch) = Some(on);
                }
                Step::none()
            }
            Event::Reset(switch) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    *draft.switch_mut(switch) = None;
                }
                Step::none()
            }
            Event::Gamescope(on) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.gamescope = on;
                }
                Step::none()
            }
            Event::FieldText(field, value) => {
                if let Some(draft) = self.draft()
                    && draft.fields.set_text(field, value)
                {
                    self.edited();
                }
                Step::none()
            }
            Event::FieldChoice(field, choice) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.fields.set_choice(field, choice);
                }
                Step::none()
            }
            Event::GrabCursor(on) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.fields.grab_cursor = on;
                }
                Step::none()
            }
            Event::ArgsInput(value) => {
                if let Some(editor) = self.editor_mut() {
                    editor.args_input = value;
                }
                Step::none()
            }
            Event::ArgsAdd => self.args_add(),
            Event::ArgsRemove(index) => {
                if let Some(draft) = self.draft()
                    && index < draft.fields.args.len()
                {
                    draft.fields.args.remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::BuildPicked(component, build) => self.build_picked(component, build),
            Event::BuildRemoved(component) => self.build_removed(component),
            Event::ListInput(list, value) => {
                if let Some(editor) = self.editor_mut() {
                    *editor.list_input_mut(list) = value;
                }
                Step::none()
            }
            Event::Title(title) => self.title(title),
            Event::ProxiedDesktop(desktop) => self.proxied_desktop(desktop),
            Event::ListAdd(list) => self.list_add(list),
            Event::ListRemove(list, index) => {
                if let Some(draft) = self.draft()
                    && index < draft.list(list).len()
                {
                    draft.list_mut(list).remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::EnvKey(index, key) => self.env_key(index, key),
            Event::EnvValue(index, value) => self.env_value(index, value),
            Event::EnvRemove(index) => self.env_remove(index),
            Event::NewEnvKey(key) => self.new_env_key(key),
            Event::NewEnvValue(value) => self.new_env_value(value),
            Event::EnvAdd => self.env_add(),
            Event::GlobalSwitch(key, on) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.set_switch(key, on);
                }
                Step::none()
            }
            Event::GlobalRunner(choice) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.draft.runner = choice.name;
                }
                Step::none()
            }
            Event::GlobalArch(arch) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.draft.arch = arch;
                }
                Step::none()
            }
            // GOG games live in the host library, so a portable folder's
            // values would do nothing.
            Event::GlobalField(global::Field::GamesRoot | global::Field::GogLanguage, _)
                if paths.portable_root().is_some() =>
            {
                Step::none()
            }
            Event::GlobalField(field, value) => {
                if let View::Global(editor) = &mut self.view
                    && editor.form.draft.set_field(field, value)
                {
                    self.edited();
                }
                Step::none()
            }
            Event::Appearance(edit) => {
                self.edited();
                let mode = self.system_mode;
                if let View::Global(editor) = &mut self.view {
                    editor.appearance.apply(edit, mode);
                }
                Step::none()
            }
            Event::BrowseGamesRoot => self.browse_games_root(paths),
            Event::PickedGamesRoot(generation, previous, result) => {
                self.picked_games_root(paths, generation, previous, result)
            }
            // A portable folder never uses other launchers' builds, so the
            // switch there would do nothing.
            Event::GlobalExternalRunners(_) if paths.portable_root().is_some() => Step::none(),
            Event::GlobalExternalRunners(on) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.set_external_runners(on);
                }
                Step::none()
            }
            Event::GlobalArtKind(kind, on) => {
                if let View::Global(editor) = &self.view {
                    let fetch = editor.form.draft.art_fetch.with(kind, on);
                    return self.update(paths, Event::GlobalArt(fetch));
                }
                Step::none()
            }
            Event::GlobalArt(fetch) => {
                self.edited();
                if let View::Global(editor) = &mut self.view {
                    editor.form.draft.art_fetch = fetch;
                }
                Step::none()
            }
            Event::SteamGridDbKeySetOpen => self.sgdb_key_set_open(),
            Event::SteamGridDbKeySetCancel => self.sgdb_key_set_cancel(),
            Event::SteamGridDbKeyText(value) => self.sgdb_key_text(value),
            Event::SteamGridDbKeySet => self.sgdb_key_set(paths),
            Event::SteamGridDbKeyRemove => self.sgdb_key_remove(paths),
            Event::SteamGridDbKeyStatusLoaded(generation, sequence, result) => {
                self.sgdb_key_status_loaded(paths, generation, sequence, result)
            }
            Event::SteamGridDbKeySaved(generation, sequence, input_session, result) => {
                self.sgdb_key_saved(paths, generation, sequence, input_session, result)
            }
            Event::SteamGridDbKeyRemoved(generation, sequence, input_session, result) => {
                self.sgdb_key_removed(paths, generation, sequence, input_session, result)
            }
            Event::Gog(event) => {
                if !self.global {
                    return Step::none();
                }
                let context = self.gog_context();
                Step::task(self.gog.update(paths, context, event))
            }
            Event::SteamKeySetOpen => self.steam_key_set_open(),
            Event::SteamKeySetCancel => self.steam_key_set_cancel(),
            Event::SteamKeyText(value) => self.steam_key_text(value),
            Event::SteamKeySet => self.steam_key_set(paths),
            Event::SteamKeyRemove => self.steam_key_remove(paths),
            Event::SteamKeyStatusLoaded(generation, sequence, result) => {
                self.steam_key_status_loaded(paths, generation, sequence, result)
            }
            Event::SteamKeySaved(generation, sequence, input_session, result) => {
                self.steam_key_saved(paths, generation, sequence, input_session, result)
            }
            Event::SteamKeyRemoved(generation, sequence, input_session, result) => {
                self.steam_key_removed(paths, generation, sequence, input_session, result)
            }
            Event::SteamKeyLocated(generation, bounds) => {
                if generation != self.generation {
                    return Step::none();
                }
                Step::task(view::scroll_to_row(bounds))
            }
            Event::Prefix(value) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.prefix = value;
                }
                Step::none()
            }
            Event::GameId(value) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.gameid = value;
                }
                Step::none()
            }
            Event::Store(value) => {
                self.edited();
                if let Some(draft) = self.draft() {
                    draft.store = value;
                }
                Step::none()
            }
            Event::Target(target, edit) => {
                if self.target_edit(&target, edit) {
                    self.edited();
                }
                Step::none()
            }
            Event::Card(target, open) => {
                if let Some(editor) = self.editor_mut() {
                    if open {
                        editor.open_cards.insert(target);
                    } else {
                        editor.open_cards.remove(&target);
                    }
                }
                Step::none()
            }
            Event::KeepTarget => {
                if let Some(editor) = self.editor_mut() {
                    editor.removing = None;
                }
                Step::none()
            }
            Event::AddOpen(open) => self.add_open(paths, open),
            Event::ShowHeldBack(show) => {
                if let Some(editor) = self.editor_mut() {
                    editor.show_held_back = show;
                }
                Step::none()
            }
            Event::Scanned(generation, sequence, scan) => {
                if generation == self.generation
                    && sequence == self.scan_sequence
                    && !self.locked()
                    && let Some(editor) = self.editor_mut()
                {
                    editor.scan = Some(scan);
                }
                Step::none()
            }
            Event::ProgramPicked(program) => self.program_picked(program),
            Event::NewTargetName(name) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_target.name = name;
                    editor.add_error = None;
                }
                Step::none()
            }
            Event::NewTargetPath(path) => self.new_target_path(paths, path),
            Event::TargetAdd => self.target_add(paths),
            Event::Resolved(generation, sequence, result) => {
                self.resolved(generation, sequence, result)
            }
            Event::MenuOpened => {
                self.menu_open = true;
                Step::none()
            }
            Event::MenuClosed => {
                self.menu_open = false;
                Step::none()
            }
            Event::OpenConfig => Step::task(load::open_task(if self.global {
                paths.global_config()
            } else {
                paths.environment_config(&self.name)
            })),
            Event::Opened(result) => {
                self.error = result.err();
                self.error_checkup = false;
                Step::none()
            }
            Event::Loaded(generation, snapshot) => self.loaded(paths, generation, snapshot),
            Event::Checked(generation, sequence, texts) => {
                self.checked(paths, generation, sequence, texts)
            }
            Event::CheckFailed(generation, sequence) => {
                if generation == self.generation && sequence == self.check_sequence {
                    self.checking = false;
                }
                Step::none()
            }
            Event::Saved(generation, result) => self.saved(paths, generation, result),
        }
    }
    /// Whether the page drops the event: it edits while the page is locked, it
    /// edits something the global page doesn't have, or it targets the other
    /// kind of page or an editor that's gone.
    fn ignores(&self, event: &Event) -> bool {
        if self.locked() && event.edits() {
            return true;
        }
        if self.global
            && event.edits()
            && !matches!(
                event,
                Event::GlobalSwitch(..)
                    | Event::GlobalRunner(_)
                    | Event::GlobalArch(_)
                    | Event::GlobalField(..)
                    | Event::GlobalArt(_)
                    | Event::GlobalArtKind(_, _)
                    | Event::GlobalExternalRunners(_)
                    | Event::Appearance(_)
                    | Event::EnvKey(..)
                    | Event::EnvValue(..)
                    | Event::EnvRemove(_)
                    | Event::NewEnvKey(_)
                    | Event::NewEnvValue(_)
                    | Event::EnvAdd
            )
        {
            return true;
        }
        if !self.global
            && matches!(
                event,
                Event::GlobalSwitch(..)
                    | Event::GlobalRunner(_)
                    | Event::GlobalArch(_)
                    | Event::GlobalField(..)
                    | Event::GlobalArt(_)
                    | Event::GlobalArtKind(_, _)
                    | Event::GlobalExternalRunners(_)
                    | Event::Appearance(_)
            )
        {
            return true;
        }
        if let Some(editor) = self.editor()
            && !editor.accepts(&self.name, event)
        {
            return true;
        }
        false
    }
    fn land(&mut self, pending: Pending, snapshot: Snapshot) {
        self.menu_open = false;
        let draft = self.editor().map(|editor| &editor.form.draft);
        let global_draft = match &self.view {
            View::Global(editor) => Some(&editor.form.draft),
            _ => None,
        };
        let appearance_draft = self.appearance().map(super::appearance::Form::drafts);
        let edited_since = pending.mode == Mode::Replace
            && (draft != pending.draft.as_ref()
                || global_draft != pending.global_draft.as_ref()
                || appearance_draft != pending.appearance_draft);
        match (pending.mode, snapshot.content) {
            (Mode::Rebase, Content::Global(info)) if matches!(self.view, View::Global(_)) => {
                // A rebase for `[appearance]` alone can find the global file
                // changed too. That change is left for the next check, which
                // still compares it with what General's edits are against.
                let config_moved = !pending.after_save
                    && self.disk.as_ref().map(|disk| &disk.global) != Some(&snapshot.texts.global);
                let editor = match &mut self.view {
                    View::Global(editor) => editor,
                    _ => unreachable!("checked above"),
                };
                editor.appearance.rebase(info.appearance, info.themes);
                if config_moved {
                    if let Some(disk) = &mut self.disk {
                        disk.appearance = snapshot.texts.appearance;
                        disk.themes = snapshot.texts.themes;
                    }
                } else {
                    editor.form.rebase(info.values.clone());
                    editor.runners = info.runners;
                    self.disk = Some(snapshot.texts);
                }
            }
            (Mode::Rebase, Content::Form(info)) if self.editor().is_some() => {
                let keep_base = self.changed_on_disk;
                let editor = self.editor_mut().expect("checked above");
                let values = info.values.clone();
                editor.info = *info;
                if keep_base {
                    // The environment file changed under the edits too, so
                    // Reload stays the way forward.
                    if let Some(disk) = &mut self.disk {
                        disk.global = snapshot.texts.global;
                    }
                } else {
                    editor.form.rebase(values);
                    self.disk = Some(snapshot.texts);
                }
            }
            // Edits made while this ran are the user's, so they stay, and the
            // page says the file moved on under them.
            _ if edited_since || (pending.mode == Mode::Rebase && self.is_dirty()) => {
                self.changed_on_disk = true;
                self.changed_file = self.disk.as_ref().map_or(ChangedFile::Config, |disk| {
                    ChangedFile::between(disk, &snapshot.texts)
                });
            }
            (_, content) => {
                self.disk = Some(snapshot.texts);
                self.changed_on_disk = false;
                let previous = self.editor().map(|editor| &editor.form);
                self.view = match content {
                    Content::Form(info) => {
                        let form = Form::reloaded(info.values.clone(), previous);
                        let mut editor = Editor::new(*info, form);
                        if let Some(previous) = self.editor() {
                            editor.carry_view(previous);
                        }
                        // A quiet reload shouldn't eat what's being typed. Reload
                        // and Revert clear it first, and a save reload drops it.
                        if pending.mode == Mode::Replace
                            && let Some(previous) = self.editor()
                        {
                            editor.carry_inputs(previous);
                        }
                        View::Form(Box::new(editor))
                    }
                    Content::Global(info) => {
                        let mut editor = global::Editor::new(
                            info.values,
                            info.appearance,
                            info.themes,
                            info.runners,
                        );
                        if pending.mode == Mode::Replace
                            && let View::Global(previous) = &self.view
                        {
                            editor.new_env.clone_from(&previous.new_env);
                        }
                        View::Global(Box::new(editor))
                    }
                    Content::Error(error) => View::Error(error),
                };
            }
        }
    }
}

/// Wine and Proton prefixes are laid out differently, so `settings::apply`
/// refuses a switch between them. Refusing it here says so before Save.
pub(super) fn refused(current: Option<&Current>, choice: &RunnerChoice) -> bool {
    match (current, choice.proton) {
        (Some(current), Some(proton)) => current.proton != proton,
        _ => false,
    }
}
