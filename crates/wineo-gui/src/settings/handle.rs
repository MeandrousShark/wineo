//! The settings page's event handling and how a reload lands.

use iced::Task;
use wineo_core::paths::Paths;

use super::form::{EnvRow, Form};
use super::load::{Content, Current, RunnerChoice, Scan, Snapshot};
use super::{
    Editor, Event, Mode, NewTarget, Page, Pending, Step, TargetEdit, View, form, global, load,
    steam_key, steamgriddb, unfocus, view,
};
use crate::Message;

impl Page {
    pub(super) fn handle(&mut self, paths: &Paths, event: Event) -> Step {
        if self.locked() && event.edits() {
            return Step::none();
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
                    | Event::EnvKey(..)
                    | Event::EnvValue(..)
                    | Event::EnvRemove(_)
                    | Event::NewEnvKey(_)
                    | Event::NewEnvValue(_)
                    | Event::EnvAdd
            )
        {
            return Step::none();
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
            )
        {
            return Step::none();
        }
        if let Some(editor) = self.editor()
            && !editor.accepts(&self.name, &event)
        {
            return Step::none();
        }
        match event {
            Event::BrowsePath(target, cwd) => {
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
            Event::PickedPath(generation, sequence, target, cwd, previous, result) => {
                if generation != self.generation
                    || sequence != self.resolve_sequence
                    || self.locked()
                {
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
                        Step::none()
                    }
                }
            }
            Event::Back => {
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
            Event::Discard if self.saving => Step::none(),
            Event::Discard => Step::close(),
            Event::KeepEditing => {
                self.leave_prompt = false;
                Step::none()
            }
            Event::Save => {
                if !self.can_save() {
                    return Step::none();
                }
                let edits = match &self.view {
                    View::Form(editor) => editor.form.edits(),
                    View::Global(editor) => editor.form.edits(),
                    _ => return Step::none(),
                };
                if edits.is_empty() {
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
                    self.generation,
                ))
            }
            Event::Reload => {
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
            Event::Revert => {
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
                    editor.new_env = EnvRow::default();
                }
                if self.changed_on_disk {
                    return Step::task(self.reload(paths, Mode::Replace));
                }
                Step::none()
            }
            Event::RunnerPicked(choice) => {
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
            Event::ArgsAdd => {
                if let Some(editor) = self.editor_mut()
                    && !editor.args_input.is_empty()
                {
                    let arg = std::mem::take(&mut editor.args_input);
                    editor.form.draft.fields.args.push(arg);
                    self.edited();
                }
                Step::none()
            }
            Event::ArgsRemove(index) => {
                if let Some(draft) = self.draft()
                    && index < draft.fields.args.len()
                {
                    draft.fields.args.remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::BuildPicked(component, build) => {
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
            Event::BuildRemoved(component) => {
                self.edited();
                if let Some(editor) = self.editor_mut() {
                    editor.build_refusal =
                        editor.form.remove_build(component).err().map(str::to_owned);
                }
                Step::none()
            }
            Event::ListInput(list, value) => {
                if let Some(editor) = self.editor_mut() {
                    *editor.list_input_mut(list) = value;
                }
                Step::none()
            }
            Event::ProxiedTitle(title) => {
                if let Some(proxied) = self.draft().and_then(|draft| draft.proxied.as_mut()) {
                    let title = (!title.is_empty()).then_some(title);
                    if proxied.title != title {
                        proxied.title = title;
                        self.edited();
                    }
                }
                Step::none()
            }
            Event::ProxiedDesktop(desktop) => {
                if let Some(proxied) = self.draft().and_then(|draft| draft.proxied.as_mut())
                    && proxied.desktop != desktop
                {
                    proxied.desktop = desktop;
                    self.edited();
                }
                Step::none()
            }
            Event::ListAdd(list) => {
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
            Event::ListRemove(list, index) => {
                if let Some(draft) = self.draft()
                    && index < draft.list(list).len()
                {
                    draft.list_mut(list).remove(index);
                    self.edited();
                }
                Step::none()
            }
            Event::EnvKey(index, key) => {
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
            Event::EnvValue(index, value) => {
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
            Event::EnvRemove(index) => {
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
            Event::NewEnvKey(key) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_env.key = key;
                } else if let View::Global(editor) = &mut self.view {
                    editor.new_env.key = key;
                }
                Step::none()
            }
            Event::NewEnvValue(value) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_env.value = value;
                } else if let View::Global(editor) = &mut self.view {
                    editor.new_env.value = value;
                }
                Step::none()
            }
            Event::EnvAdd => {
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
            Event::BrowseGamesRoot => {
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
                        Message::Settings(Event::PickedGamesRoot(
                            generation,
                            previous.clone(),
                            result,
                        ))
                    },
                ))
            }
            Event::PickedGamesRoot(generation, previous, result) => {
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
                        Step::none()
                    }
                }
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
            Event::SteamGridDbKeySetOpen => {
                if self.global
                    && !self.locked()
                    && !self.loading()
                    && self.steamgriddb.busy.is_none()
                {
                    self.steamgriddb.input_open = true;
                    self.steamgriddb.input = steamgriddb::Secret::default();
                    self.steamgriddb_input_session = Some(self.bump_steamgriddb_input_session());
                    self.steamgriddb_input_session_touched = true;
                    self.steamgriddb.error = None;
                    self.steamgriddb.result = None;
                }
                Step::none()
            }
            Event::SteamGridDbKeySetCancel => {
                if self.global && self.steamgriddb.busy.is_none() {
                    self.steamgriddb.input_open = false;
                    self.steamgriddb.input = steamgriddb::Secret::default();
                    self.steamgriddb_input_session = None;
                    self.steamgriddb_input_session_touched = true;
                    self.steamgriddb.error = None;
                }
                Step::none()
            }
            Event::SteamGridDbKeyText(value) => {
                if self.global && self.steamgriddb.input_open && self.steamgriddb.busy.is_none() {
                    self.steamgriddb.input = value;
                    self.steamgriddb.error = None;
                }
                Step::none()
            }
            Event::SteamGridDbKeySet => {
                if !self.global
                    || self.locked()
                    || self.loading()
                    || self.steamgriddb.busy.is_some()
                    || self.steamgriddb.input.as_str().trim().is_empty()
                {
                    return Step::none();
                }
                self.steamgriddb.busy = Some(steamgriddb::Busy::Saving);
                self.steamgriddb.error = None;
                self.steamgriddb.result = None;
                let input_session = self.steamgriddb_input_session;
                let key = std::mem::take(&mut self.steamgriddb.input);
                let sequence = self.bump_steamgriddb_sequence();
                self.steamgriddb_busy_sequence = Some(sequence);
                Step::task(steamgriddb::save_task(
                    paths.clone(),
                    self.generation,
                    sequence,
                    input_session,
                    key,
                ))
            }
            Event::SteamGridDbKeyRemove => {
                if !self.global
                    || self.locked()
                    || self.loading()
                    || self.steamgriddb.busy.is_some()
                {
                    return Step::none();
                }
                self.steamgriddb.busy = Some(steamgriddb::Busy::Removing);
                self.steamgriddb.error = None;
                self.steamgriddb.result = None;
                let input_session = self.steamgriddb_input_session;
                let sequence = self.bump_steamgriddb_sequence();
                self.steamgriddb_busy_sequence = Some(sequence);
                Step::task(steamgriddb::remove_task(
                    paths.clone(),
                    self.generation,
                    sequence,
                    input_session,
                ))
            }
            Event::SteamGridDbKeyStatusLoaded(generation, sequence, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                if !self.global {
                    return Step::none();
                }
                let available = match result {
                    Ok(status) => {
                        let available = status.available();
                        self.steamgriddb.warning = status.warning(paths);
                        self.steamgriddb.status = Some(status);
                        self.steamgriddb.status_error = None;
                        available
                    }
                    Err(error) => {
                        self.steamgriddb.status_error = Some(error);
                        false
                    }
                };
                Step::task(Task::done(Message::SteamGridDbKeyChangedAt(
                    sequence, available,
                )))
            }
            Event::SteamGridDbKeySaved(generation, sequence, input_session, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                if !self.global {
                    return Step::none();
                }
                if self.steamgriddb_busy_sequence == Some(sequence) {
                    self.steamgriddb.busy = None;
                    self.steamgriddb_busy_sequence = None;
                }
                match result {
                    Ok(saved) => {
                        self.steamgriddb.warning = saved.warning;
                        self.steamgriddb.result = saved.result;
                        if input_session.is_some()
                            && input_session == self.steamgriddb_input_session
                        {
                            self.steamgriddb.error = None;
                            self.steamgriddb.input_open = false;
                            self.steamgriddb.input = steamgriddb::Secret::default();
                            self.steamgriddb_input_session = None;
                        }
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                    Err(error) => {
                        self.steamgriddb.error = Some(error);
                        if input_session.is_some()
                            && input_session == self.steamgriddb_input_session
                        {
                            self.steamgriddb.input_open = true;
                            self.steamgriddb.input = steamgriddb::Secret::default();
                            self.steamgriddb_input_session =
                                Some(self.bump_steamgriddb_input_session());
                        }
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                }
            }
            Event::SteamGridDbKeyRemoved(generation, sequence, input_session, result) => {
                if generation != self.generation {
                    return Step::none();
                }
                if !self.global {
                    return Step::none();
                }
                if self.steamgriddb_busy_sequence == Some(sequence) {
                    self.steamgriddb.busy = None;
                    self.steamgriddb_busy_sequence = None;
                }
                match result {
                    Ok(removed) => {
                        self.steamgriddb.warning = None;
                        self.steamgriddb.result = Some(removed.result);
                        if input_session.is_some()
                            && input_session == self.steamgriddb_input_session
                        {
                            self.steamgriddb.error = None;
                            self.steamgriddb.input_open = false;
                            self.steamgriddb.input = steamgriddb::Secret::default();
                            self.steamgriddb_input_session = None;
                        }
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                    Err(error) => {
                        self.steamgriddb.error = Some(error);
                        Step::task(self.refresh_steamgriddb(paths))
                    }
                }
            }
            Event::Gog(event) => {
                if !self.global {
                    return Step::none();
                }
                let context = self.gog_context();
                Step::task(self.gog.update(paths, context, event))
            }
            Event::SteamKeySetOpen => {
                if self.steam_key_idle() {
                    self.steam_key.input_open = true;
                    self.steam_key.input = steamgriddb::Secret::default();
                    self.steam_key_input_session = Some(self.bump_steam_key_sequence());
                    self.steam_key.error = None;
                    self.steam_key.result = None;
                }
                Step::none()
            }
            Event::SteamKeySetCancel => {
                if self.global && self.steam_key.busy.is_none() {
                    self.steam_key.input_open = false;
                    self.steam_key.input = steamgriddb::Secret::default();
                    self.steam_key_input_session = None;
                    self.steam_key.error = None;
                }
                Step::none()
            }
            Event::SteamKeyText(value) => {
                if self.global && self.steam_key.input_open && self.steam_key.busy.is_none() {
                    self.steam_key.input = value;
                    self.steam_key.error = None;
                }
                Step::none()
            }
            Event::SteamKeySet => {
                if !self.steam_key_idle() || self.steam_key.input.as_str().trim().is_empty() {
                    return Step::none();
                }
                self.steam_key.busy = Some(steamgriddb::Busy::Saving);
                self.steam_key.error = None;
                self.steam_key.result = None;
                let key = std::mem::take(&mut self.steam_key.input);
                let sequence = self.bump_steam_key_sequence();
                self.steam_key_busy_sequence = Some(sequence);
                Step::task(steam_key::save_task(
                    paths.clone(),
                    self.generation,
                    sequence,
                    self.steam_key_input_session,
                    key,
                ))
            }
            Event::SteamKeyRemove => {
                if !self.steam_key_idle() {
                    return Step::none();
                }
                self.steam_key.busy = Some(steamgriddb::Busy::Removing);
                self.steam_key.error = None;
                self.steam_key.result = None;
                let sequence = self.bump_steam_key_sequence();
                self.steam_key_busy_sequence = Some(sequence);
                Step::task(steam_key::remove_task(
                    paths.clone(),
                    self.generation,
                    sequence,
                    self.steam_key_input_session,
                ))
            }
            Event::SteamKeyStatusLoaded(generation, sequence, result) => {
                if generation != self.generation
                    || !self.global
                    || sequence != self.steam_key_sequence
                {
                    return Step::none();
                }
                match result {
                    Ok(status) => {
                        self.steam_key.warning = status.warning(paths);
                        self.steam_key.status = Some(status);
                        self.steam_key.status_error = None;
                    }
                    Err(error) => self.steam_key.status_error = Some(error),
                }
                Step::none()
            }
            Event::SteamKeySaved(generation, sequence, input_session, result) => {
                if generation != self.generation
                    || !self.global
                    || self.steam_key_busy_sequence != Some(sequence)
                {
                    return Step::none();
                }
                self.steam_key.busy = None;
                self.steam_key_busy_sequence = None;
                let same_input =
                    input_session.is_some() && input_session == self.steam_key_input_session;
                match result {
                    Ok(saved) => {
                        self.steam_key.warning = saved.warning;
                        self.steam_key.result = saved.result;
                        self.steam_key.status = Some(saved.status);
                        if same_input {
                            self.steam_key.error = None;
                            self.steam_key.input_open = false;
                            self.steam_key_input_session = None;
                        }
                    }
                    Err(error) => {
                        self.steam_key.error = Some(error);
                        if same_input {
                            self.steam_key.input_open = true;
                            self.steam_key_input_session = Some(self.bump_steam_key_sequence());
                        }
                    }
                }
                self.steam_key.input = steamgriddb::Secret::default();
                Step::task(self.refresh_steam_key(paths))
            }
            Event::SteamKeyRemoved(generation, sequence, input_session, result) => {
                if generation != self.generation
                    || !self.global
                    || self.steam_key_busy_sequence != Some(sequence)
                {
                    return Step::none();
                }
                self.steam_key.busy = None;
                self.steam_key_busy_sequence = None;
                match result {
                    Ok(removed) => {
                        self.steam_key.warning = None;
                        self.steam_key.result = Some(removed.result);
                        self.steam_key.status = Some(removed.status);
                        if input_session.is_some() && input_session == self.steam_key_input_session
                        {
                            self.steam_key.error = None;
                            self.steam_key.input_open = false;
                            self.steam_key.input = steamgriddb::Secret::default();
                            self.steam_key_input_session = None;
                        }
                    }
                    Err(error) => self.steam_key.error = Some(error),
                }
                Step::task(self.refresh_steam_key(paths))
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
            Event::AddOpen(open) => {
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
            Event::ProgramPicked(program) => {
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
            Event::NewTargetName(name) => {
                if let Some(editor) = self.editor_mut() {
                    editor.new_target.name = name;
                    editor.add_error = None;
                }
                Step::none()
            }
            Event::NewTargetPath(path) => {
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
            Event::TargetAdd => {
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
            Event::Resolved(generation, sequence, result) => {
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
                Step::none()
            }
            Event::Loaded(generation, snapshot) => {
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
            Event::Checked(generation, sequence, texts) => {
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
            Event::CheckFailed(generation, sequence) => {
                if generation == self.generation && sequence == self.check_sequence {
                    self.checking = false;
                }
                Step::none()
            }
            Event::Saved(generation, result) => {
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
                                Step::task(self.reload_after(paths, Mode::Rebase, true))
                            }
                            None => Step::task(self.reload_after(paths, Mode::AfterSave, true)),
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Step::none()
                    }
                }
            }
        }
    }
    /// Applies one target's edit. Returns whether the draft changed.
    fn target_edit(&mut self, target: &str, edit: TargetEdit) -> bool {
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
    fn land(&mut self, pending: Pending, snapshot: Snapshot) {
        self.menu_open = false;
        let draft = self.editor().map(|editor| &editor.form.draft);
        let global_draft = match &self.view {
            View::Global(editor) => Some(&editor.form.draft),
            _ => None,
        };
        let edited_since = pending.mode == Mode::Replace
            && (draft != pending.draft.as_ref() || global_draft != pending.global_draft.as_ref());
        match (pending.mode, snapshot.content) {
            (Mode::Rebase, Content::Global(info)) if matches!(self.view, View::Global(_)) => {
                let editor = match &mut self.view {
                    View::Global(editor) => editor,
                    _ => unreachable!("checked above"),
                };
                editor.form.rebase(info.values.clone());
                editor.runners = info.runners;
                self.disk = Some(snapshot.texts);
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
                        let mut editor = global::Editor::new(info.values, info.runners);
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
