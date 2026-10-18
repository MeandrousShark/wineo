//! Message handlers for a running start or stop and the Steam request.

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::launch::StartOutcome;

use crate::panel::PanelError;
use crate::{App, Message, OperationKind};

impl App {
    pub(crate) fn steam_requested(
        &mut self,
        name: String,
        result: Result<String, PanelError>,
    ) -> Task<Message> {
        if self.steam_request.as_deref() == Some(&name) {
            self.steam_request = None;
        }
        if self.selected.as_deref() == Some(&name) {
            match result {
                Ok(note) => self.panel_note = Some(note),
                Err(error) => self.panel_error = Some(error),
            }
        }
        self.reload_library_once()
    }

    pub(crate) fn after_hook_phase(&mut self, name: String, token: Token) -> Task<Message> {
        if let Some(operation) = &mut self.operation
            && operation.name == name
            && operation.kind == OperationKind::Start
        {
            operation.token = Some(token);
            operation.status = "Running its after hook".to_owned();
            operation.after_hook = true;
        }
        Task::none()
    }

    pub(crate) fn cancel_start(&mut self) -> Task<Message> {
        if let Some(operation) = &mut self.operation
            && operation.kind == OperationKind::Start
        {
            if let Some(token) = &operation.token {
                if operation.cancelled {
                    token.force();
                    operation.status = "Force stopping...".to_owned();
                } else {
                    token.cancel();
                    operation.status = "Canceling...".to_owned();
                }
            }
            operation.cancelled = true;
        }
        Task::none()
    }

    pub(crate) fn launch_progress(&mut self, name: String, status: String) -> Task<Message> {
        if let Some(operation) = &mut self.operation
            && operation.name == name
            && operation.kind == OperationKind::Start
            && !operation.cancelled
        {
            operation.status = status;
        }
        Task::none()
    }

    pub(crate) fn install_phase(&mut self, name: String, installing: bool) -> Task<Message> {
        if let Some(operation) = &mut self.operation
            && operation.name == name
            && operation.kind == OperationKind::Start
        {
            operation.installing = installing;
        }
        Task::none()
    }

    pub(crate) fn start_finished(
        &mut self,
        name: String,
        result: Result<StartOutcome, PanelError>,
    ) -> Task<Message> {
        let cancelled = self
            .operation
            .as_ref()
            .is_some_and(|operation| operation.name == name && operation.cancelled);
        if self.operation.as_ref().is_some_and(|operation| {
            operation.name == name && operation.kind == OperationKind::Start
        }) {
            self.operation = None;
        }
        if self.selected.as_deref() == Some(&name)
            && let Err(error) = result
            && !cancelled
        {
            self.panel_error = Some(error);
        }
        self.reload_library_once()
    }

    pub(crate) fn stop_finished(
        &mut self,
        name: String,
        result: Result<Vec<String>, PanelError>,
    ) -> Task<Message> {
        if self.operation.as_ref().is_some_and(|operation| {
            operation.name == name && operation.kind == OperationKind::Stop
        }) {
            self.operation = None;
        }
        if self
            .force_stop_at
            .as_ref()
            .is_some_and(|(stopping, _)| *stopping == name)
        {
            self.force_stop_at = None;
        }
        if self.selected.as_deref() == Some(&name) {
            match result {
                Ok(messages) => {
                    self.panel_note = (!messages.is_empty()).then(|| messages.join("\n"))
                }
                Err(error) => self.panel_error = Some(error),
            }
        }
        self.reload_library_once()
    }
}
