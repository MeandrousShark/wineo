use std::collections::VecDeque;

use iced::widget::{
    Space, button, column, container, image, pick_list, row, scrollable, stack, text, text_editor,
};
use iced::{Center, Color, ContentFit, Element, Fill, Font, Task, border, font};
use wineo_core::cancel::Token;
use wineo_core::error::Error;
use wineo_core::launch::LaunchEvent;
use wineo_core::runtime::State;
use wineo_core::winetricks::WinetricksEvent;

use crate::artwork;
use crate::library::Card;
use crate::logview::{self, LogView};
use crate::theme::Palette;
use crate::worker::LogSource;
use crate::{App, Message, Operation, OperationKind, worker};

const PANEL_WIDTH: f32 = 420.0;
pub(crate) const LOG_ID: &str = "panel-log";
const LOG_LINES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelError {
    pub(crate) text: String,
    pub(crate) cleanup: bool,
}

impl PanelError {
    pub(crate) fn from_error(error: &anyhow::Error) -> Self {
        match error.downcast_ref::<Error>() {
            Some(Error::LostRun(_)) => Self {
                text: format!("{error:#}"),
                cleanup: true,
            },
            Some(Error::Busy {
                holder,
                holder_label,
                ..
            }) => Self {
                text: holder.map_or_else(
                    || format!("Environment is busy: {holder_label}"),
                    |holder| format!("Environment is busy with {}", holder.as_str()),
                ),
                cleanup: false,
            },
            _ => Self {
                text: format!("{error:#}"),
                cleanup: false,
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct LogBuffer {
    lines: VecDeque<String>,
    pending: String,
    view: LogView,
}

const PENDING_CAP: usize = 64 * 1024;

impl LogBuffer {
    /// Returns true when the log view should snap to the end.
    #[must_use]
    pub(crate) fn push(&mut self, chunk: &str) -> bool {
        let chunk = logview::normalize(chunk);
        self.pending.push_str(&chunk);
        let mut dropped = String::new();
        while let Some(end) = self.pending.find('\n') {
            let line = self.pending[..end].to_owned();
            self.pending.drain(..=end);
            self.lines.push_back(line);
            if self.lines.len() > LOG_LINES
                && let Some(old) = self.lines.pop_front()
            {
                dropped.push_str(&old);
                dropped.push('\n');
            }
        }
        if self.pending.len() > PENDING_CAP {
            // The cut lands inside the partial line, which the view cannot
            // mirror as a front trim.
            let mut start = self.pending.len() - PENDING_CAP;
            // Output is arbitrary UTF-8, and draining mid-character panics.
            while !self.pending.is_char_boundary(start) {
                start += 1;
            }
            self.pending.drain(..start);
            self.view.resync(&Self::render(&self.lines, &self.pending))
        } else {
            self.view.push(&chunk, &dropped, || {
                Self::render(&self.lines, &self.pending)
            })
        }
    }
    fn render(lines: &VecDeque<String>, pending: &str) -> String {
        let mut text = lines.iter().cloned().collect::<Vec<_>>().join("\n");
        // Test the deque, not the text: a log of only blank lines is still
        // "\n" separated, and the view is a plain concatenation of chunks.
        if !lines.is_empty() {
            text.push('\n');
        }
        text.push_str(pending);
        text
    }
    pub(crate) fn display(&self) -> String {
        Self::render(&self.lines, &self.pending)
    }
    pub(crate) fn view(&self) -> &LogView {
        &self.view
    }
    #[must_use]
    pub(crate) fn act(&mut self, action: text_editor::Action) -> bool {
        self.view
            .perform(action, || Self::render(&self.lines, &self.pending))
    }
    #[must_use]
    pub(crate) fn follow(&mut self) -> bool {
        self.view
            .follow(|| Self::render(&self.lines, &self.pending))
    }
    #[must_use]
    pub(crate) fn scrolled(&mut self, viewport: scrollable::Viewport) -> bool {
        self.view.scrolled(
            viewport.absolute_offset().y,
            viewport.bounds().height,
            viewport.content_bounds().height,
            || Self::render(&self.lines, &self.pending),
        )
    }
    pub(crate) fn has_text(&self) -> bool {
        !self.lines.is_empty() || !self.pending.is_empty()
    }
    pub(crate) fn copy(&mut self) -> String {
        self.view.mark_copied();
        self.display()
    }
}

pub(crate) fn status(event: &LaunchEvent) -> Option<&'static str> {
    match event {
        LaunchEvent::BuildingPrefix { .. } => Some("Building prefix..."),
        LaunchEvent::Winetricks(WinetricksEvent::Installing(_)) => Some("Installing..."),
        LaunchEvent::Starting | LaunchEvent::Started => Some("Starting..."),
        _ => None,
    }
}

pub(crate) fn event_status(event: &LaunchEvent) -> Option<String> {
    match event {
        LaunchEvent::Winetricks(event) => winetricks_status(event),
        _ => status(event).map(str::to_owned),
    }
}

pub(crate) fn winetricks_status(event: &WinetricksEvent) -> Option<String> {
    match event {
        WinetricksEvent::Progress(step) => Some(match step.total {
            1 => format!("Installing {}...", step.verb),
            total => format!("Installing {} ({} of {total})...", step.verb, step.position),
        }),
        WinetricksEvent::Installing(verbs) => Some(format!("Installing {}...", verbs.join(", "))),
        _ => None,
    }
}

pub(crate) fn play_enabled(card: &Card, operation_busy: bool, target: Option<&str>) -> bool {
    card.error.is_none()
        && !card.busy
        && card.state != State::Running
        && !operation_busy
        && target.is_some()
}

pub(crate) fn stop_enabled(card: &Card, operation_busy: bool) -> bool {
    !card.busy && card.state == State::Running && !operation_busy
}

fn busy_notice(card: &Card, operation_busy: bool) -> Option<(String, bool)> {
    if let Some(holder) = &card.busy_holder {
        Some((format!("Busy with {holder}"), false))
    } else if card.unresolved {
        Some((
            "an installer did not finish cleanly".to_owned(),
            !operation_busy,
        ))
    } else {
        None
    }
}

impl App {
    pub(crate) fn chosen_target<'a>(&'a self, card: &'a Card) -> Option<&'a str> {
        self.target.as_deref().or(card.default_target.as_deref())
    }
    pub(crate) fn panel_log_visible(&self) -> bool {
        self.log_view && self.checkup_log_scope != Some(crate::checkup::Scope::Global)
    }
    pub(crate) fn close_log(&mut self) {
        if let Some(token) = self.log_token.take() {
            token.cancel();
        }
        self.log_view = false;
        self.log_error = None;
        self.checkup_log_scope = None;
        self.log_generation = self.log_generation.wrapping_add(1);
    }
    pub(crate) fn close_panel(&mut self) {
        self.close_log();
        self.close_checkup();
        self.panel_open = false;
        self.target = None;
        self.panel_error = None;
        self.panel_note = None;
    }
    pub(crate) fn start_selected(&mut self) -> Task<Message> {
        let Some(card) = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
        else {
            return Task::none();
        };
        let Some(target) = self.chosen_target(card).map(str::to_owned) else {
            return Task::none();
        };
        if !play_enabled(
            card,
            self.operation.is_some() || self.tool_blocks_launch(&card.name),
            Some(&target),
        ) {
            return Task::none();
        }
        let name = card.name.clone();
        let token = Token::new();
        let install_done = Token::new();
        self.operation = Some(Operation {
            name: name.clone(),
            kind: OperationKind::Start,
            token: Some(token.clone()),
            cancelled: false,
            status: "Starting...".to_owned(),
            install_done: Some(install_done.clone()),
            installing: false,
        });
        self.panel_error = None;
        self.panel_note = None;
        worker::start(self.paths.clone(), name, target, token, install_done)
    }
    pub(crate) fn stop_selected(&mut self) -> Task<Message> {
        let Some(card) = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
        else {
            return Task::none();
        };
        let cleanup = card.busy_holder.is_none()
            && (card.unresolved || self.panel_error.as_ref().is_some_and(|error| error.cleanup));
        if !cleanup && !stop_enabled(card, self.operation.is_some()) {
            return Task::none();
        }
        if self.operation.is_some() || self.tool_prefix_busy(&card.name) {
            return Task::none();
        }
        let name = card.name.clone();
        self.operation = Some(Operation {
            name: name.clone(),
            kind: OperationKind::Stop,
            token: None,
            cancelled: false,
            status: "Stopping...".to_owned(),
            install_done: None,
            installing: false,
        });
        self.panel_error = None;
        self.panel_note = None;
        worker::stop(self.paths.clone(), name)
    }
    pub(crate) fn open_log(&mut self) -> Task<Message> {
        let Some(card) = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
        else {
            return Task::none();
        };
        let Some(source) = self.log_source(card) else {
            return Task::none();
        };
        self.open_log_for(card.name.clone(), source)
    }

    pub(crate) fn open_log_for(&mut self, name: String, source: LogSource) -> Task<Message> {
        self.close_log();
        self.log_view = true;
        self.log_install = matches!(source, LogSource::Install(_));
        self.log_buffer = LogBuffer::default();
        let token = Token::new();
        self.log_token = Some(token.clone());
        worker::log(self.paths.clone(), name, self.log_generation, token, source)
    }
    pub(crate) fn log_content(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let mut body = column![].spacing(8).push(
            row![
                button("Back").on_press(Message::CloseLog).padding([6, 12]),
                logview::controls(
                    self.log_buffer.view(),
                    self.log_buffer.has_text(),
                    Message::CopyLog,
                    Message::LogFollow,
                ),
                text(if self.log_install {
                    "Install log"
                } else {
                    "Game output"
                })
                .size(14)
                .color(palette.muted)
            ]
            .spacing(8)
            .align_y(Center),
        );
        if let Some(error) = &self.log_error {
            body = body.push(text(error).color(palette.danger));
        }
        body.push(logview::editor(
            self.log_buffer.view(),
            palette,
            Fill,
            LOG_ID,
            Message::LogAction,
            Message::LogScrolled,
        ))
        .into()
    }
    /// The log the panel can show for a card right now. An install in progress
    /// wins over the state, because the environment is still Inactive then.
    pub(crate) fn log_source(&self, card: &Card) -> Option<LogSource> {
        if let Some(source) = self.checkup_log_source(&card.name) {
            return Some(source);
        }
        let installing = self.operation.as_ref().filter(|operation| {
            operation.name == card.name
                && operation.kind == OperationKind::Start
                && operation.installing
        });
        if let Some(done) = installing.and_then(|operation| operation.install_done.clone()) {
            Some(LogSource::Install(done))
        } else if card.state != State::Inactive {
            Some(LogSource::Output)
        } else {
            None
        }
    }
    pub(crate) fn load_panel_art(&mut self) -> Task<Message> {
        if !self.panel_open {
            return Task::none();
        }
        let paths = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
            .and_then(|card| {
                self.chosen_target(card)
                    .and_then(|target| card.panel_art.get(target))
            })
            .map(|art| {
                [
                    (art.hero.as_ref(), artwork::Kind::Hero),
                    (art.logo.as_ref(), artwork::Kind::Logo),
                ]
            })
            .into_iter()
            .flatten()
            .filter_map(|(art, kind)| art.map(|art| (art.clone(), kind)))
            .collect::<Vec<_>>();
        self.load_images(paths)
    }
    pub(crate) fn panel<'a>(&'a self, card: &'a Card) -> Element<'a, Message> {
        let palette = self.palette;
        let target = self.chosen_target(card);
        let art = target.and_then(|target| card.panel_art.get(target));
        let hero = art.and_then(|art| art.hero.as_ref());
        let logo = art.and_then(|art| art.logo.as_ref());
        let hero_image = hero.and_then(|art| self.images.get(&(art.clone(), artwork::Kind::Hero)));
        let logo_image = logo.and_then(|art| self.images.get(&(art.clone(), artwork::Kind::Logo)));
        let hero: Element<'_, Message> = match hero_image {
            Some(handle) => image(handle.clone())
                .width(Fill)
                .height(220)
                .content_fit(ContentFit::Cover)
                .into(),
            None => container(Space::new())
                .width(Fill)
                .height(220)
                .style(move |_| container::Style::default().background(palette.surface))
                .into(),
        };
        let banner = container(hero).width(Fill).height(220).clip(true);
        let close = button(text("x").size(16).color(Color::WHITE).center())
            .width(28)
            .height(28)
            .padding(0)
            .on_press(Message::ClosePanel)
            .style(|_, status| button::Style {
                background: Some(
                    Color::BLACK
                        .scale_alpha(if matches!(status, button::Status::Hovered) {
                            0.75
                        } else {
                            0.55
                        })
                        .into(),
                ),
                border: border::rounded(14),
                ..button::Style::default()
            });
        let mut banner_layers = stack![banner];
        if let Some(handle) = logo_image {
            let logo_layer = image(handle.clone())
                .height(72)
                .content_fit(ContentFit::Contain)
                .expand(true);
            let logo_overlay = container(logo_layer)
                .width(Fill)
                .height(112)
                .padding(iced::Padding {
                    top: 24.0,
                    right: 16.0,
                    bottom: 8.0,
                    left: 16.0,
                })
                .align_left(Fill)
                .align_bottom(Fill)
                .style(move |_| {
                    if hero_image.is_some() {
                        let gradient =
                            iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                                .add_stop(0.0, Color::BLACK.scale_alpha(0.0))
                                .add_stop(1.0, Color::BLACK.scale_alpha(0.78));
                        container::Style::default().background(gradient)
                    } else {
                        container::Style::default()
                    }
                });
            banner_layers = banner_layers.push(
                container(logo_overlay)
                    .width(Fill)
                    .height(220)
                    .align_bottom(Fill),
            );
        }
        banner_layers = banner_layers.push(
            container(close)
                .width(Fill)
                .height(220)
                .align_right(Fill)
                .align_top(Fill)
                .padding(12),
        );
        let mut body = column![banner_layers].spacing(0).width(Fill);
        if logo_image.is_none() {
            body = body.push(heading(&card.name, palette));
        }
        let operation_busy = self.operation.is_some() || self.tool_blocks_launch(&card.name);
        let mut actions = row![].spacing(8).align_y(Center);
        if card.error.is_none() {
            let play = action_button(
                "Play",
                palette.primary,
                play_enabled(card, operation_busy, target),
                Message::Play,
            );
            actions = actions.push(play);
            if card.targets.len() > 1 {
                actions = actions.push(
                    pick_list(
                        card.targets.clone(),
                        self.target.clone(),
                        Message::TargetPicked,
                    )
                    .width(Fill),
                );
            }
        }
        actions = actions.push(action_button(
            "Stop",
            palette.danger,
            stop_enabled(card, operation_busy),
            Message::Stop,
        ));
        let mut inner = column![].spacing(8).width(Fill);
        if self.panel_log_visible() {
            inner = inner.push(self.log_content());
        } else if self.checkup_visible() {
            inner = inner
                .push(self.checkup_view(crate::checkup::Scope::Environment(card.name.clone())));
        } else if let Some(error) = &card.error {
            inner = inner.push(error_box(error, palette));
        } else {
            inner = inner.push(detail_row("State", card.state.as_str(), palette));
            inner = inner.push(detail_row("Playtime", &card.playtime, palette));
            inner = inner.push(detail_row("Sessions", &card.sessions.to_string(), palette));
            inner = inner.push(detail_row("Last played", &card.last_played, palette));
            if !card.tags.is_empty() {
                inner = inner.push(detail_row("Tags", &card.tags.join(", "), palette));
            }
        }
        if !self.panel_log_visible() && !self.checkup_visible() {
            inner = inner.push(actions);
            let mut extra = row![].spacing(8);
            if self.log_source(card).is_some() {
                extra = extra.push(button("Log").on_press(Message::OpenLog).padding([6, 12]));
            }
            extra = extra.push(
                button("Settings")
                    .on_press(Message::OpenSettings)
                    .padding([6, 12]),
            );
            extra = extra.push(button("Art").on_press(Message::OpenArt).padding([6, 12]));
            extra = extra.push(
                button("Actions")
                    .on_press(Message::Context(card.name.clone()))
                    .padding([6, 12]),
            );
            inner = inner.push(extra);
        }
        inner =
            inner.push(self.checkup_status(&crate::checkup::Scope::Environment(card.name.clone())));
        if let Some(operation) = self.operation.as_ref().filter(|op| op.name == card.name) {
            inner = inner.push(
                row![
                    crate::spinner::spinner(palette, self.spinner_angle, 16.0),
                    text(&operation.status).size(14).color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
            );
            if operation.kind == OperationKind::Start {
                let label = if operation.cancelled {
                    "Force stop"
                } else {
                    "Cancel"
                };
                inner = inner.push(
                    button(label)
                        .on_press(Message::CancelStart)
                        .padding([6, 12]),
                );
            }
        }
        if let Some(note) = &self.panel_note {
            inner = inner.push(text(note).size(12).color(palette.muted));
        }
        if let Some((notice, cleanup)) = busy_notice(card, operation_busy) {
            inner = inner.push(text(notice).size(12).color(palette.muted));
            if cleanup {
                inner = inner.push(button("Clean up").on_press(Message::CleanUp));
            }
        }
        if let Some(error) = &self.panel_error {
            let mut box_content =
                column![text(&error.text).size(14).color(palette.danger)].spacing(8);
            if error.cleanup && !card.unresolved && card.busy_holder.is_none() && !operation_busy {
                box_content = box_content.push(button("Clean up").on_press(Message::CleanUp));
            }
            inner = inner.push(
                container(box_content)
                    .width(Fill)
                    .padding(8)
                    .style(move |_| {
                        container::Style::default()
                            .background(palette.danger.scale_alpha(0.15))
                            .border(border::rounded(6))
                    }),
            );
        }
        body = body.push(container(inner).padding(16).width(Fill).height(Fill));
        container(body)
            .width(PANEL_WIDTH)
            .height(Fill)
            .style(move |_| {
                container::Style::default()
                    .background(palette.background)
                    .border(border::rounded(0).width(1).color(palette.border))
            })
            .into()
    }
}

fn heading<'a>(name: &'a str, palette: Palette) -> Element<'a, Message> {
    container(
        text(name)
            .size(20)
            .font(Font {
                weight: font::Weight::Semibold,
                ..Font::DEFAULT
            })
            .color(palette.foreground),
    )
    .width(Fill)
    .padding([12, 16])
    .into()
}

fn detail_row(label: &str, value: &str, palette: Palette) -> Element<'static, Message> {
    row![
        text(label.to_owned()).color(palette.muted),
        Space::new().width(Fill),
        text(value.to_owned()).color(palette.foreground)
    ]
    .spacing(8)
    .width(Fill)
    .into()
}

pub(crate) fn error_box<'a>(error: &'a str, palette: Palette) -> Element<'a, Message> {
    container(text(error).size(14).color(palette.danger))
        .width(Fill)
        .padding(8)
        .style(move |_| {
            container::Style::default()
                .background(palette.danger.scale_alpha(0.15))
                .border(border::rounded(6))
        })
        .into()
}

pub(crate) fn action_button(
    label: &'static str,
    color: Color,
    enabled: bool,
    message: Message,
) -> Element<'static, Message> {
    button(
        text(label)
            .size(14)
            .color(Color::WHITE.scale_alpha(if enabled { 1.0 } else { 0.4 }))
            .center(),
    )
    .height(32)
    .padding([0, 16])
    .on_press_maybe(enabled.then_some(message))
    .style(move |_, status| button::Style {
        background: Some(
            color
                .scale_alpha(if matches!(status, button::Status::Disabled) {
                    0.4
                } else {
                    1.0
                })
                .into(),
        ),
        border: border::rounded(6),
        ..button::Style::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use wineo_core::operation::Operation;
    use wineo_core::winetricks::Step;

    fn card(state: State) -> Card {
        Card {
            name: "game".into(),
            state,
            lost: false,
            prefix: None,
            busy: false,
            busy_holder: None,
            unresolved: false,
            error: None,
            playtime: "never played".into(),
            sessions: 0,
            last_played: "never".into(),
            tags: Vec::new(),
            targets: vec!["game".into()],
            default_target: Some("game".into()),
            art: None,
            panel_art: HashMap::new(),
        }
    }

    #[test]
    fn button_rules_cover_running_busy_error_and_operations() {
        let idle = card(State::Inactive);
        assert!(play_enabled(&idle, false, Some("game")));
        assert!(!stop_enabled(&idle, false));
        assert!(!play_enabled(&idle, false, None));
        assert!(!play_enabled(&idle, true, Some("game")));
        let running = card(State::Running);
        assert!(!play_enabled(&running, false, Some("game")));
        assert!(stop_enabled(&running, false));
        assert!(!stop_enabled(&running, true));
        let mut busy = card(State::Running);
        busy.busy = true;
        assert!(!play_enabled(&busy, false, Some("game")));
        assert!(!stop_enabled(&busy, false));
        let mut broken = card(State::Inactive);
        broken.error = Some("broken".into());
        assert!(!play_enabled(&broken, false, Some("game")));
    }

    #[test]
    fn unresolved_installer_has_cleanup_and_busy_lock_does_not() {
        let mut unresolved = card(State::Inactive);
        unresolved.busy = true;
        unresolved.unresolved = true;
        assert_eq!(
            busy_notice(&unresolved, false),
            Some(("an installer did not finish cleanly".to_owned(), true))
        );
        assert_eq!(
            busy_notice(&unresolved, true),
            Some(("an installer did not finish cleanly".to_owned(), false))
        );
        let mut busy = card(State::Inactive);
        busy.busy = true;
        busy.busy_holder = Some("create".into());
        assert_eq!(
            busy_notice(&busy, false),
            Some(("Busy with create".into(), false))
        );
    }

    #[test]
    fn launch_events_map_to_short_status() {
        assert_eq!(
            event_status(&LaunchEvent::Starting),
            Some("Starting...".into())
        );
        assert_eq!(
            event_status(&LaunchEvent::Winetricks(WinetricksEvent::Installing(vec![
                "corefonts".into()
            ]))),
            Some("Installing corefonts...".into())
        );
        assert_eq!(event_status(&LaunchEvent::NoWaylandSession), None);
    }

    fn progress(verb: &str, position: usize, total: usize) -> LaunchEvent {
        LaunchEvent::Winetricks(WinetricksEvent::Progress(Step {
            verb: verb.into(),
            position,
            total,
        }))
    }

    #[test]
    fn install_progress_names_the_verb_and_count() {
        assert_eq!(
            event_status(&progress("gdiplus", 2, 3)),
            Some("Installing gdiplus (2 of 3)...".into())
        );
        assert_eq!(
            event_status(&progress("corefonts", 1, 1)),
            Some("Installing corefonts...".into())
        );
    }

    #[test]
    fn typed_errors_map_to_cleanup_and_busy_text() {
        let lost = anyhow::Error::new(Error::LostRun("game".into()));
        assert!(PanelError::from_error(&lost).cleanup);
        let busy = anyhow::Error::new(Error::Busy {
            subject: "game".into(),
            holder: Some(Operation::Create),
            holder_label: "create",
        });
        let mapped = PanelError::from_error(&busy);
        assert_eq!(mapped.text, "Environment is busy with create");
        assert!(!mapped.cleanup);
    }

    #[test]
    fn buffer_keeps_last_lines_and_partial_line() {
        let mut buffer = LogBuffer::default();
        for index in 0..2100 {
            let _ = buffer.push(&format!("{index}\n"));
        }
        let _ = buffer.push("tail");
        assert_eq!(buffer.lines.len(), 2000);
        assert_eq!(buffer.lines.front().unwrap(), "100");
        assert!(buffer.display().ends_with("2099\ntail"));
    }

    #[test]
    fn view_matches_the_capped_buffer() {
        let mut buffer = LogBuffer::default();
        for index in 0..2100 {
            let _ = buffer.push(&format!("{index}\r\n"));
        }
        let _ = buffer.push("tail");
        assert_eq!(buffer.view().content().text(), buffer.display());
        assert_eq!(buffer.view().content().line_count(), 2001);
    }

    #[test]
    fn copy_returns_the_whole_log_even_while_paused() {
        let mut buffer = LogBuffer::default();
        let _ = buffer.push("one\ntwo\n");
        let _ = buffer.act(text_editor::Action::SelectAll);
        let _ = buffer.push("three\nfour");
        assert!(buffer.view().paused());
        assert_eq!(buffer.view().content().text(), "one\ntwo\n");
        assert_eq!(buffer.copy(), "one\ntwo\nthree\nfour");
        assert!(buffer.view().copied());
        let _ = buffer.push("\n");
        assert!(!buffer.view().copied());
    }

    #[test]
    fn blank_lines_are_kept() {
        let mut buffer = LogBuffer::default();
        let _ = buffer.push("\n\n");
        assert_eq!(buffer.display(), "\n\n");
        assert_eq!(buffer.view().content().text(), buffer.display());
    }

    #[test]
    fn oversized_partial_line_resyncs_the_view() {
        let mut buffer = LogBuffer::default();
        let _ = buffer.push("head\n");
        let _ = buffer.push(&"x".repeat(PENDING_CAP + 10));
        assert_eq!(buffer.view().content().text(), buffer.display());
    }

    #[test]
    fn oversized_multibyte_partial_line_trims_on_a_char_boundary() {
        let mut buffer = LogBuffer::default();
        // Three-byte characters make the byte cut land mid-character.
        let _ = buffer.push(&"\u{3042}".repeat(PENDING_CAP / 3 + 7));
        assert!(buffer.display().len() <= PENDING_CAP);
        assert_eq!(buffer.view().content().text(), buffer.display());
    }
}
