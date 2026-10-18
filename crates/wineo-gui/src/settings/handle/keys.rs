//! The SteamGridDB and Steam key rows.

use iced::Task;
use wineo_core::paths::Paths;

use crate::Message;
use crate::settings::{Page, Step, steam_key, steamgriddb};

impl Page {
    pub(super) fn sgdb_key_set_open(&mut self) -> Step {
        if self.global && !self.locked() && !self.loading() && self.steamgriddb.busy.is_none() {
            self.steamgriddb.input_open = true;
            self.steamgriddb.input = steamgriddb::Secret::default();
            self.steamgriddb_input_session = Some(self.bump_steamgriddb_input_session());
            self.steamgriddb_input_session_touched = true;
            self.steamgriddb.error = None;
            self.steamgriddb.result = None;
        }
        Step::none()
    }
    pub(super) fn sgdb_key_set_cancel(&mut self) -> Step {
        if self.global && self.steamgriddb.busy.is_none() {
            self.steamgriddb.input_open = false;
            self.steamgriddb.input = steamgriddb::Secret::default();
            self.steamgriddb_input_session = None;
            self.steamgriddb_input_session_touched = true;
            self.steamgriddb.error = None;
        }
        Step::none()
    }
    pub(super) fn sgdb_key_text(&mut self, value: steamgriddb::Secret) -> Step {
        if self.global && self.steamgriddb.input_open && self.steamgriddb.busy.is_none() {
            self.steamgriddb.input = value;
            self.steamgriddb.error = None;
        }
        Step::none()
    }
    pub(super) fn sgdb_key_set(&mut self, paths: &Paths) -> Step {
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
    pub(super) fn sgdb_key_remove(&mut self, paths: &Paths) -> Step {
        if !self.global || self.locked() || self.loading() || self.steamgriddb.busy.is_some() {
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
    pub(super) fn sgdb_key_status_loaded(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        result: Result<steamgriddb::Status, String>,
    ) -> Step {
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
    pub(super) fn sgdb_key_saved(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        input_session: Option<u64>,
        result: Result<steamgriddb::Saved, String>,
    ) -> Step {
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
                if input_session.is_some() && input_session == self.steamgriddb_input_session {
                    self.steamgriddb.error = None;
                    self.steamgriddb.input_open = false;
                    self.steamgriddb.input = steamgriddb::Secret::default();
                    self.steamgriddb_input_session = None;
                }
                Step::task(self.refresh_steamgriddb(paths))
            }
            Err(error) => {
                self.steamgriddb.error = Some(error);
                if input_session.is_some() && input_session == self.steamgriddb_input_session {
                    self.steamgriddb.input_open = true;
                    self.steamgriddb.input = steamgriddb::Secret::default();
                    self.steamgriddb_input_session = Some(self.bump_steamgriddb_input_session());
                }
                Step::task(self.refresh_steamgriddb(paths))
            }
        }
    }
    pub(super) fn sgdb_key_removed(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        input_session: Option<u64>,
        result: Result<steamgriddb::Removed, String>,
    ) -> Step {
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
                if input_session.is_some() && input_session == self.steamgriddb_input_session {
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
    pub(super) fn steam_key_set_open(&mut self) -> Step {
        if self.steam_key_idle() {
            self.steam_key.input_open = true;
            self.steam_key.input = steamgriddb::Secret::default();
            self.steam_key_input_session = Some(self.bump_steam_key_sequence());
            self.steam_key.error = None;
            self.steam_key.result = None;
        }
        Step::none()
    }
    pub(super) fn steam_key_set_cancel(&mut self) -> Step {
        if self.global && self.steam_key.busy.is_none() {
            self.steam_key.input_open = false;
            self.steam_key.input = steamgriddb::Secret::default();
            self.steam_key_input_session = None;
            self.steam_key.error = None;
        }
        Step::none()
    }
    pub(super) fn steam_key_text(&mut self, value: steamgriddb::Secret) -> Step {
        if self.global && self.steam_key.input_open && self.steam_key.busy.is_none() {
            self.steam_key.input = value;
            self.steam_key.error = None;
        }
        Step::none()
    }
    pub(super) fn steam_key_set(&mut self, paths: &Paths) -> Step {
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
    pub(super) fn steam_key_remove(&mut self, paths: &Paths) -> Step {
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
    pub(super) fn steam_key_status_loaded(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        result: Result<steam_key::Status, String>,
    ) -> Step {
        if generation != self.generation || !self.global || sequence != self.steam_key_sequence {
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
    pub(super) fn steam_key_saved(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        input_session: Option<u64>,
        result: Result<steam_key::Saved, String>,
    ) -> Step {
        if generation != self.generation
            || !self.global
            || self.steam_key_busy_sequence != Some(sequence)
        {
            return Step::none();
        }
        self.steam_key.busy = None;
        self.steam_key_busy_sequence = None;
        let same_input = input_session.is_some() && input_session == self.steam_key_input_session;
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
    pub(super) fn steam_key_removed(
        &mut self,
        paths: &Paths,
        generation: u64,
        sequence: u64,
        input_session: Option<u64>,
        result: Result<steam_key::Removed, String>,
    ) -> Step {
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
                if input_session.is_some() && input_session == self.steam_key_input_session {
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
}
