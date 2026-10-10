use std::collections::VecDeque;
use std::time::{Duration, Instant};

use iced::widget::{
    Space, button, column, container, image, pick_list, row, scrollable, stack, text, text_editor,
};
use iced::{Center, Color, ContentFit, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::error::Error;
use wineo_core::hooks::Phase;
use wineo_core::launch::LaunchEvent;
use wineo_core::operation::Operation as CoreOperation;
use wineo_core::runtime::State;
use wineo_core::winetricks::WinetricksEvent;

use crate::artwork;
use crate::focus_button;
use crate::library::Card;
use crate::logview::{self, LogView};
use crate::theme::Palette;
use crate::ui::{self, PickListStyle};
use crate::worker::LogSource;
use crate::{App, Message, Operation, OperationKind, worker};

const PANEL_WIDTH: f32 = 420.0;
const PANEL_ACTION_WIDTH: f32 = 82.0;
pub(crate) const LOG_ID: &str = "panel-log";
const LOG_LINES: usize = 2000;
const PANEL_PLAY_ID: &str = "panel-play";
const PANEL_STOP_ID: &str = "panel-stop";
const PANEL_LOG_ID: &str = "panel-log";
const PANEL_SETTINGS_ID: &str = "panel-settings";
const PANEL_ART_ID: &str = "panel-art";
const PANEL_ACTIONS_ID: &str = "panel-actions";
const PANEL_CANCEL_ID: &str = "panel-cancel";
const PANEL_FORCE_STOP_ID: &str = "panel-force-stop";
const HOOK_CANCEL_ID: &str = "hook-stop-cancel";
const HOOK_CONFIRM_ID: &str = "hook-stop-confirm";
const HOOK_CONFIRM_TEXT: &str = "Kill the after hook?";
const PANEL_CLEANUP_ID: &str = "panel-cleanup";
/// Stop turns into Force stop only after this long, so a double-click on
/// Stop can't kill a game that was about to close.
pub(crate) const FORCE_STOP_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelError {
    pub(crate) text: String,
    pub(crate) cleanup: bool,
    pub(crate) output: Option<String>,
}

impl PanelError {
    pub(crate) fn from_error(error: &anyhow::Error) -> Self {
        match error.downcast_ref::<Error>() {
            Some(Error::LostRun(_)) => Self {
                text: format!("{error:#}"),
                cleanup: true,
                output: None,
            },
            Some(Error::Busy {
                holder,
                holder_label,
                ..
            }) => Self {
                text: holder.map_or_else(
                    || format!("Environment is busy: {holder_label}"),
                    |holder| match holder {
                        CoreOperation::Hook(_) => {
                            format!("Environment is busy {}", holder.describe())
                        }
                        _ => format!("Environment is busy with {}", holder.as_str()),
                    },
                ),
                cleanup: false,
                output: None,
            },
            _ => Self {
                text: format!("{error:#}"),
                cleanup: false,
                output: None,
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
        LaunchEvent::AfterHook(_) => Some("Running its after hook"),
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
        && !card.active()
        && card.install_allows_play()
        && !operation_busy
        && target.is_some()
}

pub(crate) fn stop_enabled(card: &Card, operation_busy: bool) -> bool {
    (!card.busy && card.active() || matches!(card.busy_holder, Some(CoreOperation::Hook(_))))
        && !operation_busy
}

fn busy_notice(card: &Card, operation_busy: bool) -> Option<(String, bool)> {
    if let Some(holder) = &card.busy_holder {
        Some((
            match holder {
                CoreOperation::Hook(Phase::Before) => "Running its before hook".to_owned(),
                CoreOperation::Hook(Phase::After) => "Running its after hook".to_owned(),
                _ => format!("Busy with {}", holder.as_str()),
            },
            false,
        ))
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
        self.panel_motion.close(std::time::Instant::now());
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
            after_hook: false,
        });
        self.panel_error = None;
        self.panel_note = None;
        worker::start(self.paths.clone(), name, target, token, install_done)
    }
    fn after_hook_active(&self, card: &Card) -> bool {
        card.busy_holder == Some(CoreOperation::Hook(Phase::After))
            || self.operation.as_ref().is_some_and(|operation| {
                operation.name == card.name
                    && operation.kind == OperationKind::Start
                    && operation.after_hook
            })
    }
    /// Our own Stop waits on the after hook with no deadline, so it mustn't
    /// lock out the Stop that offers to kill that hook.
    fn own_stop_waiting_on_after(&self, card: &Card) -> bool {
        self.after_hook_active(card)
            && self.operation.as_ref().is_some_and(|operation| {
                operation.name == card.name && operation.kind == OperationKind::Stop
            })
    }
    /// Our own polite Stop has had `FORCE_STOP_DELAY` and the program still
    /// hasn't closed. An after hook keeps its own Kill confirmation instead.
    pub(crate) fn force_stop_ready(&self, card: &Card, now: Instant) -> bool {
        !self.after_hook_active(card)
            && self.operation.as_ref().is_some_and(|operation| {
                operation.name == card.name
                    && operation.kind == OperationKind::Stop
                    && !operation.cancelled
            })
            && self
                .force_stop_at
                .as_ref()
                .is_some_and(|(name, at)| *name == card.name && now >= *at)
    }
    pub(crate) fn stop_selected(&mut self) -> Task<Message> {
        let Some(card) = self
            .selected
            .as_ref()
            .and_then(|name| self.cards.iter().find(|card| &card.name == name))
        else {
            return Task::none();
        };
        if self.force_stop_ready(card, Instant::now()) {
            let name = card.name.clone();
            if let Some(operation) = &mut self.operation {
                operation.cancelled = true;
                operation.status = "Force stopping...".to_owned();
            }
            self.force_stop_at = None;
            return worker::stop(self.paths.clone(), name, true);
        }
        let cleanup = card.busy_holder.is_none()
            && (card.unresolved || self.panel_error.as_ref().is_some_and(|error| error.cleanup));
        let own_start = self.operation.as_ref().is_some_and(|operation| {
            operation.name == card.name && operation.kind == OperationKind::Start
        });
        let own = own_start || self.own_stop_waiting_on_after(card);
        if !cleanup
            && !self.after_hook_active(card)
            && !stop_enabled(card, self.operation.is_some() && !own)
        {
            return Task::none();
        }
        if (self.operation.is_some() && !own) || self.tool_prefix_busy(&card.name) {
            return Task::none();
        }
        let name = card.name.clone();
        if self.after_hook_active(card) {
            self.hook_stop_identity = wineo_core::operation::Locks::new(&self.paths)
                .hook_group(&name)
                .ok()
                .flatten()
                .filter(|(phase, _)| *phase == Phase::After)
                .map(|(_, running)| running);
            self.hook_stop_confirm = Some(name);
            return iced::widget::operation::focus(iced::widget::Id::new(HOOK_CANCEL_ID));
        }
        if own_start && matches!(card.busy_holder, Some(CoreOperation::Hook(_))) {
            return self.update(Message::CancelStart);
        }
        if own_start && card.state != State::Starting {
            return Task::none();
        }
        // Steam's start handoff has returned, so Stop can replace Start while waiting for the game.
        if !own_start || card.state == State::Starting {
            self.operation = Some(Operation {
                name: name.clone(),
                kind: OperationKind::Stop,
                token: None,
                cancelled: false,
                status: "Stopping...".to_owned(),
                install_done: None,
                installing: false,
                after_hook: false,
            });
            self.force_stop_at = Some((name.clone(), Instant::now() + FORCE_STOP_DELAY));
            self.panel_error = None;
            self.panel_note = None;
        }
        worker::stop(self.paths.clone(), name, false)
    }
    pub(crate) fn confirm_hook_stop(&mut self) -> Task<Message> {
        let Some(name) = self.hook_stop_confirm.take() else {
            return Task::none();
        };
        let expected = self.hook_stop_identity.take().or_else(|| {
            wineo_core::operation::Locks::new(&self.paths)
                .hook_group(&name)
                .ok()
                .flatten()
                .filter(|(phase, _)| *phase == Phase::After)
                .map(|(_, running)| running)
        });
        if self.selected.as_deref() != Some(&name) {
            return Task::none();
        }
        let Some(expected) = expected else {
            self.panel_note = Some(worker::AFTER_HOOK_ENDED_NOTE.to_owned());
            return Task::none();
        };
        let own_start = self.operation.as_ref().is_some_and(|operation| {
            operation.name == name && operation.kind == OperationKind::Start
        });
        if !own_start {
            self.operation = Some(Operation {
                name: name.clone(),
                kind: OperationKind::Stop,
                token: None,
                cancelled: false,
                status: "Stopping...".to_owned(),
                install_done: None,
                installing: false,
                after_hook: false,
            });
            self.panel_error = None;
            self.panel_note = None;
        }
        worker::stop_after(self.paths.clone(), name, Some(expected))
    }

    pub(crate) fn hook_stop_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Task<Message> {
        use iced::keyboard::{Key, key::Named};
        match key {
            Key::Named(Named::Escape) => {
                self.hook_stop_confirm = None;
                self.hook_stop_identity = None;
                Task::none()
            }
            Key::Named(Named::Tab) => iced::advanced::widget::operate(focus_button::cycle(
                &[HOOK_CANCEL_ID, HOOK_CONFIRM_ID],
                modifiers.shift(),
            ))
            .discard(),
            _ => Task::none(),
        }
    }

    pub(crate) fn hook_stop_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let content = column![
            ui::section_heading(HOOK_CONFIRM_TEXT, palette),
            text(self.hook_stop_confirm.as_deref().unwrap_or_default())
                .size(14)
                .color(palette.muted),
            row![
                Space::new().width(Fill),
                focus_button::control(
                    ui::secondary_button("Cancel", palette)
                        .on_press(Message::CancelHookStop)
                        .into(),
                    true,
                    HOOK_CANCEL_ID,
                    Message::CancelHookStop,
                ),
                focus_button::control(
                    ui::accent_button("Kill", palette.danger, true, Message::ConfirmHookStop),
                    true,
                    HOOK_CONFIRM_ID,
                    Message::ConfirmHookStop,
                ),
            ]
            .spacing(12),
        ]
        .spacing(16);
        container(content)
            .padding(24)
            .width((self.window_size.width - 40.0).clamp(280.0, 600.0))
            .style(move |_| ui::section_style(palette))
            .into()
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
        let hero_height = 220;
        let target = self.chosen_target(card);
        let art = target.and_then(|target| card.panel_art.get(target));
        let hero = art.and_then(|art| art.hero.as_ref());
        let logo = art.and_then(|art| art.logo.as_ref());
        let hero_image = hero.and_then(|art| self.images.get(&(art.clone(), artwork::Kind::Hero)));
        let logo_image = logo.and_then(|art| self.images.get(&(art.clone(), artwork::Kind::Logo)));
        let hero: Element<'_, Message> = match hero_image {
            Some(handle) => image(handle.clone())
                .width(Fill)
                .height(hero_height)
                .content_fit(ContentFit::Cover)
                .into(),
            None => container(Space::new())
                .width(Fill)
                .height(hero_height)
                .style(move |_| container::Style::default().background(palette.surface))
                .into(),
        };
        let banner = container(hero).width(Fill).height(hero_height).clip(true);
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
                    .height(hero_height)
                    .align_bottom(Fill),
            );
        }
        banner_layers = banner_layers.push(
            container(close)
                .width(Fill)
                .height(hero_height)
                .align_right(Fill)
                .align_top(Fill)
                .padding(12),
        );
        let mut body = column![banner_layers].spacing(0).width(Fill);
        let mut heading = column![ui::page_title(&card.display_title, palette)].spacing(4);
        if card.display_title != card.name {
            heading = heading.push(text(&card.name).size(13).color(palette.muted));
        }
        body = body.push(container(heading).width(Fill).padding([12, 16]));
        let operation_busy = self.operation.is_some() || self.tool_blocks_launch(&card.name);
        let mut actions = row![].spacing(8).align_y(Center);
        let mut target_picker: Option<Element<'_, Message>> = None;
        if card.error.is_none() {
            let enabled = play_enabled(card, operation_busy, target);
            let play = panel_action_button(
                card.launch_button_label(),
                palette.primary,
                enabled,
                Message::Play,
                PANEL_PLAY_ID,
            );
            actions = actions.push(play);
            if card.targets.len() > 1 {
                target_picker = Some(
                    row![
                        text("Launch target:").size(14),
                        pick_list(
                            card.targets.clone(),
                            self.target.clone(),
                            Message::TargetPicked,
                        )
                        .wineo_style(palette)
                        .width(Fill),
                    ]
                    .spacing(12)
                    .align_y(Center)
                    .into(),
                );
            }
        }
        let own_starting = card.state == State::Starting
            && self.operation.as_ref().is_some_and(|operation| {
                operation.name == card.name && operation.kind == OperationKind::Start
            });
        let own_hook = (matches!(card.busy_holder, Some(CoreOperation::Hook(_)))
            || self.after_hook_active(card))
            && self.operation.as_ref().is_some_and(|operation| {
                operation.name == card.name && operation.kind == OperationKind::Start
            });
        let force_ready = self.force_stop_ready(card, Instant::now());
        let enabled = force_ready
            || (self.after_hook_active(card) && !operation_busy)
            || self.own_stop_waiting_on_after(card)
            || own_hook
            || own_starting
            || stop_enabled(card, operation_busy);
        actions = actions.push(panel_action_button(
            if force_ready { "Force stop" } else { "Stop" },
            palette.danger,
            enabled,
            Message::Stop,
            PANEL_STOP_ID,
        ));
        let has_log = self.log_source(card).is_some();
        let mut inner = column![].spacing(8).width(Fill);
        if self.panel_log_visible() {
            inner = inner.push(self.log_content());
        } else if self.checkup_visible() {
            inner = inner
                .push(self.checkup_view(crate::checkup::Scope::Environment(card.name.clone())));
        } else if let Some(error) = &card.error {
            inner = inner.push(error_box(error, palette));
        } else {
            inner = inner.push(detail_row("State", card.state_word(), palette));
            if let Some(steam) = &card.steam {
                inner = inner.push(detail_row("Launcher", "Steam", palette));
                if let Some(proton) = &steam.proton {
                    inner = inner.push(detail_row("Proton", proton, palette));
                }
                if let Some(size) = steam.size {
                    inner = inner.push(detail_row(
                        "Install size",
                        &crate::size::format_bytes(size),
                        palette,
                    ));
                }
                if card.state == State::Starting {
                    inner = inner.push(text("Waiting for Steam...").size(14).color(palette.muted));
                }
                if let Some(warning) = card.install_warning() {
                    inner = inner.push(text(warning).size(14).color(palette.warning));
                }
            }
            inner = inner.push(detail_row("Playtime", &card.playtime, palette));
            inner = inner.push(detail_row("Sessions", &card.sessions, palette));
            inner = inner.push(detail_row("Last played", &card.last_played, palette));
            if !card.tags.is_empty() {
                inner = inner.push(detail_row("Tags", &card.tags.join(", "), palette));
            }
        }
        if !self.panel_log_visible() && !self.checkup_visible() {
            if let Some(target_picker) = target_picker {
                inner = inner.push(target_picker);
            }
            inner = inner.push(ui::separator(palette));
            inner = inner.push(actions);
            let mut extra = row![].spacing(8);
            if has_log {
                extra = extra.push(panel_log_button(
                    "Log",
                    palette,
                    PANEL_LOG_ID,
                    Message::OpenLog,
                ));
            }
            extra = extra.push(panel_secondary_button(
                "Settings",
                palette,
                PANEL_SETTINGS_ID,
                Message::OpenSettings,
            ));
            extra = extra.push(panel_secondary_button(
                "Art",
                palette,
                PANEL_ART_ID,
                Message::OpenArt,
            ));
            extra = extra.push(panel_secondary_button(
                "Actions",
                palette,
                PANEL_ACTIONS_ID,
                Message::Context(card.name.clone()),
            ));
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
            if operation.kind == OperationKind::Start
                && card.state != State::Starting
                && card.busy_holder != Some(CoreOperation::Hook(Phase::After))
                && !operation.after_hook
            {
                let label = if operation.cancelled {
                    "Force stop"
                } else {
                    "Cancel"
                };
                let id = if operation.cancelled {
                    PANEL_FORCE_STOP_ID
                } else {
                    PANEL_CANCEL_ID
                };
                inner = inner.push(panel_secondary_button(
                    label,
                    palette,
                    id,
                    Message::CancelStart,
                ));
            }
        }
        if let Some(note) = &self.panel_note {
            inner = inner.push(text(note).size(12).color(palette.muted));
        }
        if let Some((notice, cleanup)) = busy_notice(card, operation_busy) {
            inner = inner.push(text(notice).size(12).color(palette.muted));
            if cleanup {
                inner = inner.push(panel_secondary_button(
                    "Clean up",
                    palette,
                    PANEL_CLEANUP_ID,
                    Message::CleanUp,
                ));
            }
        }
        if let Some(error) = &self.panel_error {
            let mut box_content =
                column![text(&error.text).size(14).color(palette.danger)].spacing(8);
            if let Some(output) = &error.output {
                box_content = box_content.push(
                    text(output)
                        .size(12)
                        .font(iced::Font::MONOSPACE)
                        .color(palette.foreground),
                );
            }
            if error.cleanup && !card.unresolved && card.busy_holder.is_none() && !operation_busy {
                box_content = box_content.push(panel_secondary_button(
                    "Clean up",
                    palette,
                    PANEL_CLEANUP_ID,
                    Message::CleanUp,
                ));
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
        if self.panel_log_visible() || self.checkup_visible() {
            body = body.push(ui::section(inner, palette).height(Fill));
        } else {
            body = body.push(scrollable(ui::section(inner, palette)).height(Fill));
        }
        container(body)
            .width(PANEL_WIDTH)
            .height(Fill)
            .style(move |_| {
                container::Style::default()
                    .background(palette.background)
                    .border(border::rounded(0).width(1).color(palette.border))
                    .shadow(iced::Shadow {
                        color: Color::BLACK.scale_alpha(0.16),
                        offset: iced::Vector::new(-6.0, 0.0),
                        blur_radius: 16.0,
                    })
            })
            .into()
    }
}

fn panel_action_button(
    label: &'static str,
    color: Color,
    enabled: bool,
    message: Message,
    id: &'static str,
) -> Element<'static, Message> {
    let content = ui::accent_control(label, color, enabled, message.clone())
        .width(PANEL_ACTION_WIDTH)
        .into();
    focus_button::control(content, enabled, id, message)
}

fn panel_secondary_button(
    label: &'static str,
    palette: Palette,
    id: &'static str,
    message: Message,
) -> Element<'static, Message> {
    let content = ui::secondary_button(label, palette)
        .on_press(message.clone())
        .into();
    focus_button::control(content, true, id, message)
}

fn panel_log_button(
    label: &'static str,
    palette: Palette,
    id: &'static str,
    message: Message,
) -> Element<'static, Message> {
    let content = ui::secondary_button(label, palette)
        .width(if label == "Log" {
            iced::Length::Fixed(PANEL_ACTION_WIDTH)
        } else {
            iced::Length::Shrink
        })
        .on_press(message.clone())
        .into();
    focus_button::control(content, true, id, message)
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
    ui::accent_button(label, color, enabled, message)
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
            kind: None,
            display_title: "game".into(),
            steam: None,
            state,
            lost: false,
            prefix: None,
            busy: false,
            busy_holder: None,
            unresolved: false,
            error: None,
            playtime: "never played".into(),
            sessions: "0".to_owned(),
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
    fn steam_starting_and_client_runs_can_stop_and_cannot_play() {
        for state in [State::Starting, State::Running, State::SteamRunning] {
            let mut card = card(state);
            card.kind = Some(crate::library::CardKind::Proxied);
            assert!(!play_enabled(&card, false, Some("steam")));
            assert!(stop_enabled(&card, false));
            assert!(!stop_enabled(&card, true));
        }
    }

    #[test]
    fn steam_play_requires_an_installed_or_updating_game() {
        for (state, enabled) in [
            ("installed", true),
            ("updating", true),
            ("not_installed", false),
            ("unavailable", false),
            ("unknown", false),
        ] {
            let mut card = card(State::Inactive);
            card.kind = Some(crate::library::CardKind::Proxied);
            card.steam = Some(crate::library::SteamDetails {
                install: wineo_core::report::InstallReport {
                    state,
                    problem: None,
                    progress: None,
                },
                proton: None,
                size: None,
            });
            assert_eq!(
                play_enabled(&card, false, Some("steam")),
                enabled,
                "{state}"
            );
        }
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
        busy.busy_holder = Some(Operation::Create);
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
    fn hooks_are_readable_busy_states_with_stop_available() {
        for (phase, label) in [
            (Phase::Before, "Running its before hook"),
            (Phase::After, "Running its after hook"),
        ] {
            let mut busy = card(State::Inactive);
            busy.busy = true;
            busy.busy_holder = Some(Operation::Hook(phase));
            assert_eq!(busy.state_word(), "busy");
            assert_eq!(busy_notice(&busy, false), Some((label.into(), false)));
            assert!(!play_enabled(&busy, false, Some("game")));
            assert!(stop_enabled(&busy, false));
            assert!(!stop_enabled(&busy, true));
            let error = anyhow::Error::new(Error::Busy {
                subject: "game".into(),
                holder: Some(Operation::Hook(phase)),
                holder_label: Operation::Hook(phase).describe(),
            })
            .context("launch preparation failed");
            let mapped = PanelError::from_error(&error);
            assert_eq!(
                mapped.text,
                format!("Environment is busy {}", Operation::Hook(phase).describe())
            );
            assert!(!mapped.cleanup);
            assert!(mapped.output.is_none());
        }
        assert_eq!(
            event_status(&LaunchEvent::AfterHook(Token::new())),
            Some("Running its after hook".into())
        );
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
