use iced::widget::{
    Column, Space, button, column, container, opaque, pick_list, row, scrollable, stack, text,
    text_input, toggler,
};
use iced::{Center, Color, Element, Fill, Font, border, font};

use super::form::{self, Choice, Field, GamescopeState, Source, Switch};
use super::{Editor, Event, Page, View};
use crate::Message;
use crate::panel::{action_button, error_box};
use crate::theme::Palette;

const LABEL_WIDTH: f32 = 180.0;
const CONTROL_WIDTH: f32 = 260.0;
const PAGE_WIDTH: f32 = 760.0;

fn send(event: Event) -> Message {
    Message::Settings(event)
}

fn semibold() -> Font {
    Font {
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    }
}

impl Page {
    pub(crate) fn view(&self, palette: Palette, spinner: usize) -> Element<'_, Message> {
        let back = button(text("< Back").size(14))
            .on_press_maybe((!self.saving).then(|| send(Event::Back)))
            .padding([6, 12])
            .style(move |_, status| secondary(palette, status));
        let header = container(
            row![
                back,
                text(format!("{} settings", self.name))
                    .size(18)
                    .font(semibold())
                    .color(palette.foreground),
            ]
            .spacing(16)
            .align_y(Center),
        )
        .padding([8, 16])
        .width(Fill);
        let body: Element<'_, Message> = match &self.view {
            View::Loading => text("Loading...").color(palette.muted).into(),
            View::Error(error) => {
                let mut content = column![error_box(error, palette)].spacing(12);
                if self.has_file() {
                    content = content.push(
                        button(text("Open config file").size(14))
                            .on_press(send(Event::OpenConfig))
                            .padding([6, 12])
                            .style(move |_, status| secondary(palette, status)),
                    );
                }
                if let Some(error) = &self.error {
                    content = content.push(text(error).size(13).color(palette.danger));
                }
                content.into()
            }
            View::Form(editor) => editor_view(editor, palette),
        };
        let scroll = scrollable(
            container(container(body).max_width(PAGE_WIDTH).width(Fill))
                .padding(24)
                .center_x(Fill),
        )
        .width(Fill)
        .height(Fill);
        let mut page = column![header, divider(palette), scroll];
        if matches!(self.view, View::Form(_)) {
            page = page
                .push(divider(palette))
                .push(self.save_bar(palette, spinner));
        }
        let base = container(page)
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style::default().background(palette.background));
        if self.leave_prompt {
            stack![base, leave_prompt(palette)].into()
        } else {
            base.into()
        }
    }
    fn save_bar(&self, palette: Palette, spinner: usize) -> Element<'_, Message> {
        let mut bar = column![].spacing(6);
        for note in &self.notes {
            bar = bar.push(text(note).size(13).color(palette.muted));
        }
        if let Some(error) = &self.error {
            bar = bar.push(text(error).size(13).color(palette.danger));
        }
        let changes = self.changes();
        let status = if self.changed_on_disk {
            "The config file changed on disk".to_owned()
        } else {
            match changes {
                0 => "No changes".to_owned(),
                1 => "1 change".to_owned(),
                count => format!("{count} changes"),
            }
        };
        let mut actions = row![text(status).size(14).color(if self.changed_on_disk {
            palette.warning
        } else {
            palette.muted
        })]
        .spacing(8)
        .align_y(Center);
        actions = actions.push(Space::new().width(Fill));
        if self.saving {
            let spin = ["◐", "◓", "◑", "◒"][spinner % 4];
            actions = actions.push(text(spin).size(16).color(palette.muted));
            actions = actions.push(text("Saving...").size(14).color(palette.muted));
        }
        let revert_enabled = !self.saving && (self.is_dirty() || self.changed_on_disk);
        actions = actions.push(
            button(text("Revert").size(14))
                .on_press_maybe(revert_enabled.then(|| send(Event::Revert)))
                .height(32)
                .padding([6, 16])
                .style(move |_, status| secondary(palette, status)),
        );
        let label = if self.changed_on_disk {
            "Reload"
        } else {
            "Save"
        };
        actions = actions.push(action_button(
            label,
            palette.primary,
            self.can_save(),
            send(Event::Save),
        ));
        container(bar.push(actions))
            .padding([12, 16])
            .width(Fill)
            .style(move |_| container::Style::default().background(palette.surface))
            .into()
    }
}

fn editor_view(editor: &Editor, palette: Palette) -> Element<'_, Message> {
    let info = &editor.info;
    let draft = &editor.form.draft;
    let mut sections = column![].spacing(24).width(Fill);
    if !info.native {
        let selected = info
            .runners
            .iter()
            .find(|choice| Some(&choice.name) == draft.runner.as_ref());
        let runner = pick_list(info.runners.as_slice(), selected, |choice| {
            send(Event::RunnerPicked(choice))
        })
        .width(CONTROL_WIDTH);
        let proton = info.current.as_ref().is_some_and(|current| current.proton);
        let arch = if proton { "win64" } else { info.arch.as_str() };
        let mut rows = column![
            setting_row("Runner", runner.into(), None, palette),
            setting_row(
                "Arch",
                text(arch).color(palette.foreground).into(),
                Some(
                    text("fixed when the prefix is created")
                        .size(13)
                        .color(palette.muted)
                        .into()
                ),
                palette,
            ),
        ]
        .spacing(12);
        if let Some(refusal) = &editor.refusal {
            rows = rows.push(text(refusal).size(13).color(palette.danger));
        }
        sections = sections.push(section("Runner", rows.into(), palette));
    }
    let mut launch = column![].spacing(12);
    let switches: &[Switch] = if info.native {
        &[Switch::Gamemode, Switch::Mangohud]
    } else {
        &[Switch::Gamemode, Switch::Mangohud, Switch::Wayland]
    };
    for switch in switches {
        let (on, source) = form::switch_state(draft.switch(*switch), info.inherited.get(*switch));
        launch = launch.push(switch_row(
            switch.label(),
            on,
            source,
            move |on| send(Event::Switch(*switch, on)),
            send(Event::Reset(*switch)),
            palette,
        ));
    }
    let (on, source) = form::gamescope_state(draft.gamescope);
    launch = launch.push(switch_row(
        "Gamescope",
        on,
        source,
        |on| send(Event::Gamescope(on)),
        send(Event::ResetGamescope),
        palette,
    ));
    if draft.gamescope == GamescopeState::On {
        launch = launch.push(container(gamescope_fields(&draft.fields, palette)).padding(
            iced::Padding {
                left: 24.0,
                ..iced::Padding::ZERO
            },
        ));
    }
    sections = sections.push(section("Launch", launch.into(), palette));
    sections.into()
}

fn gamescope_fields(fields: &form::GamescopeFields, palette: Palette) -> Element<'_, Message> {
    let mut rows = Column::new().spacing(10);
    for field in Field::ALL {
        let control: Element<'_, Message> = match field {
            Field::Filter | Field::Scaler | Field::Window => {
                let options: Vec<Choice> = std::iter::once(Choice(None))
                    .chain(field.choices().into_iter().map(|value| Choice(Some(value))))
                    .collect();
                pick_list(options, Some(fields.choice(field)), move |choice| {
                    send(Event::FieldChoice(field, choice))
                })
                .width(CONTROL_WIDTH)
                .into()
            }
            Field::GrabCursor => toggler(fields.grab_cursor)
                .on_toggle(|on| send(Event::GrabCursor(on)))
                .into(),
            _ => {
                let placeholder = if field == Field::Args {
                    "flags separated by spaces"
                } else {
                    "unset"
                };
                text_input(placeholder, fields.text(field))
                    .on_input(move |value| send(Event::FieldText(field, value)))
                    .width(if field == Field::Args {
                        CONTROL_WIDTH * 1.5
                    } else {
                        CONTROL_WIDTH
                    })
                    .padding([6, 10])
                    .style(move |_, status| input_style(palette, status))
                    .into()
            }
        };
        rows = rows.push(
            row![
                text(field.label())
                    .color(palette.muted)
                    .width(LABEL_WIDTH - 24.0),
                control
            ]
            .spacing(12)
            .align_y(Center),
        );
    }
    rows.into()
}

fn section<'a>(
    title: &'a str,
    content: Element<'a, Message>,
    palette: Palette,
) -> Element<'a, Message> {
    column![
        text(title)
            .size(16)
            .font(semibold())
            .color(palette.foreground),
        container(content).padding(16).width(Fill).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(8).width(1).color(palette.border))
        }),
    ]
    .spacing(8)
    .into()
}

fn setting_row<'a>(
    label: &'a str,
    control: Element<'a, Message>,
    note: Option<Element<'a, Message>>,
    palette: Palette,
) -> Element<'a, Message> {
    let mut line = row![
        text(label).color(palette.foreground).width(LABEL_WIDTH),
        control
    ]
    .spacing(12)
    .align_y(Center);
    if let Some(note) = note {
        line = line.push(note);
    }
    line.into()
}

fn switch_row<'a>(
    label: &'a str,
    on: bool,
    source: Source,
    toggle: impl Fn(bool) -> Message + 'a,
    reset: Message,
    palette: Palette,
) -> Element<'a, Message> {
    let mut line = row![
        text(label).color(palette.foreground).width(LABEL_WIDTH),
        container(toggler(on).on_toggle(toggle)).width(64),
        text(source.label()).size(13).color(palette.muted).width(80),
    ]
    .spacing(12)
    .align_y(Center);
    // Only an overridden row can go back to inheriting.
    if source == Source::Here {
        line = line.push(
            button(text("Reset").size(13))
                .on_press(reset)
                .padding([4, 10])
                .style(move |_, status| secondary(palette, status)),
        );
    }
    line.into()
}

fn leave_prompt(palette: Palette) -> Element<'static, Message> {
    let card = container(
        column![
            text("Discard unsaved changes?")
                .size(16)
                .font(semibold())
                .color(palette.foreground),
            row![
                Space::new().width(Fill),
                button(text("Keep editing").size(14))
                    .on_press(send(Event::KeepEditing))
                    .height(32)
                    .padding([6, 16])
                    .style(move |_, status| secondary(palette, status)),
                action_button("Discard", palette.danger, true, send(Event::Discard)),
            ]
            .spacing(8)
            .align_y(Center),
        ]
        .spacing(16),
    )
    .width(380)
    .padding(20)
    .style(move |_| {
        container::Style::default()
            .background(palette.background)
            .border(border::rounded(8).width(1).color(palette.border))
    });
    opaque(
        container(card)
            .center(Fill)
            .style(|_| container::Style::default().background(Color::BLACK.scale_alpha(0.5))),
    )
}

fn divider(palette: Palette) -> Element<'static, Message> {
    container(Space::new())
        .width(Fill)
        .height(1)
        .style(move |_| container::Style::default().background(palette.border))
        .into()
}

fn secondary(palette: Palette, status: button::Status) -> button::Style {
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered) {
                palette.border
            } else {
                palette.surface
            }
            .into(),
        ),
        text_color: palette
            .foreground
            .scale_alpha(if disabled { 0.4 } else { 1.0 }),
        border: border::rounded(6).width(1).color(palette.border),
        ..button::Style::default()
    }
}

fn input_style(palette: Palette, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: palette.background.into(),
        border: border::rounded(6).width(1).color(
            if matches!(status, text_input::Status::Focused { .. }) {
                palette.primary
            } else {
                palette.border
            },
        ),
        icon: palette.muted,
        placeholder: palette.muted,
        value: palette.foreground,
        selection: palette.primary.scale_alpha(0.4),
    }
}
