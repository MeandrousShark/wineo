use iced::widget::{
    Column, Space, button, column, container, opaque, pick_list, row, scrollable, stack, text,
    text_input, toggler,
};
use iced::{Center, Color, Element, Fill, Font, border, font};

use wineo_core::components::Component;

use super::form::{self, Build, Choice, Field, List, Source, Switch};
use super::{Editor, Event, Page, View};
use crate::Message;
use crate::chips::{Chips, input_style, secondary};
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
            View::Form(editor) => editor_view(&self.name, editor, self.locked(), palette),
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
        if let Some(warning) = self.global_warning() {
            bar = bar.push(text(warning).size(13).color(palette.warning));
        }
        for note in &self.notes {
            bar = bar.push(text(note).size(13).color(palette.muted));
        }
        if let Some(error) = &self.error {
            bar = bar.push(text(error).size(13).color(palette.danger));
        }
        if let Some(problem) = self.problem().filter(|_| self.is_dirty()) {
            bar = bar.push(text(problem).size(13).color(palette.danger));
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
        actions = actions.push(if self.changed_on_disk {
            action_button(
                "Reload",
                palette.primary,
                self.can_reload(),
                send(Event::Reload),
            )
        } else {
            action_button("Save", palette.primary, self.can_save(), send(Event::Save))
        });
        container(bar.push(actions))
            .padding([12, 16])
            .width(Fill)
            .style(move |_| container::Style::default().background(palette.surface))
            .into()
    }
}

fn editor_view<'a>(
    name: &'a str,
    editor: &'a Editor,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let info = &editor.info;
    let draft = &editor.form.draft;
    let mut sections = column![].spacing(24).width(Fill);
    if !info.native {
        let selected = info
            .runners
            .iter()
            .find(|choice| Some(&choice.name) == draft.runner.as_ref());
        let runner: Element<'_, Message> = if locked {
            locked_choice(draft.runner.as_deref().unwrap_or_default(), palette)
        } else {
            pick_list(info.runners.as_slice(), selected, |choice| {
                send(Event::RunnerPicked(choice))
            })
            .on_open(send(Event::MenuOpened))
            .on_close(send(Event::MenuClosed))
            .width(CONTROL_WIDTH)
            .into()
        };
        let proton = info.current.as_ref().is_some_and(|current| current.proton);
        let arch = if proton { "win64" } else { info.arch.as_str() };
        let mut rows = column![
            setting_row("Runner", runner, None, palette),
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
            Some(source),
            move |on| send(Event::Switch(*switch, on)),
            (source == Source::Here).then(|| send(Event::Reset(*switch))),
            locked,
            palette,
        ));
    }
    // There's no global gamescope to fall back to, so this is a plain switch.
    launch = launch.push(switch_row(
        "Gamescope",
        draft.gamescope,
        None,
        |on| send(Event::Gamescope(on)),
        None,
        locked,
        palette,
    ));
    if draft.gamescope {
        launch = launch.push(
            container(gamescope_fields(editor, locked, palette)).padding(iced::Padding {
                left: 24.0,
                ..iced::Padding::ZERO
            }),
        );
    }
    sections = sections.push(section("Launch", launch.into(), palette));
    if !info.native {
        sections = sections.push(section(
            "Components",
            components(name, editor, locked, palette),
            palette,
        ));
        sections = sections.push(section(
            "Winetricks",
            list_chips(editor, List::Winetricks, locked, palette),
            palette,
        ));
    }
    sections = sections.push(section(
        "Tags",
        list_chips(editor, List::Tags, locked, palette),
        palette,
    ));
    sections = sections.push(section(
        "Environment variables",
        env_rows(editor, locked, palette),
        palette,
    ));
    sections = sections.push(section(
        "Advanced",
        advanced(editor, locked, palette),
        palette,
    ));
    sections.into()
}

fn components<'a>(
    name: &'a str,
    editor: &'a Editor,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let mut rows = column![].spacing(12);
    let mut notes = Vec::new();
    for component in Component::ALL {
        let selected = Build(editor.form.draft.build(component).map(str::to_owned));
        let control: Element<'_, Message> = if locked || !editor.build_enabled(name, component) {
            locked_choice(&selected.to_string(), palette)
        } else {
            let installed = match component {
                Component::Dxvk => &editor.info.dxvk_builds,
                Component::Vkd3d => &editor.info.vkd3d_builds,
            };
            let choices = form::build_choices(installed, selected.0.as_deref());
            pick_list(choices, Some(selected), move |build| {
                send(Event::BuildPicked(component, build))
            })
            .on_open(send(Event::MenuOpened))
            .on_close(send(Event::MenuClosed))
            .width(CONTROL_WIDTH)
            .into()
        };
        let note = (component == Component::Vkd3d && !editor.form.vkd3d_enabled())
            .then(|| text("needs DXVK").size(13).color(palette.muted).into());
        rows = rows.push(setting_row(component.label(), control, note, palette));
        notes.extend(editor.build_refusal(name, component));
    }
    for note in notes {
        rows = rows.push(text(note).size(13).color(palette.muted));
    }
    if let Some(refusal) = &editor.build_refusal {
        rows = rows.push(text(refusal).size(13).color(palette.danger));
    }
    rows.into()
}

fn list_chips(editor: &Editor, list: List, locked: bool, palette: Palette) -> Element<'_, Message> {
    let items = editor.form.draft.list(list);
    let input = editor.list_input(list);
    Chips {
        items,
        input,
        placeholder: match list {
            List::Winetricks => "a verb, like corefonts",
            List::Tags => "a tag",
        },
        error: list.problem(items, input),
        on_input: Box::new(move |value| send(Event::ListInput(list, value))),
        on_add: send(Event::ListAdd(list)),
        on_remove: Box::new(move |index| send(Event::ListRemove(list, index))),
        enabled: !locked,
    }
    .view(palette)
}

const ENV_KEY_WIDTH: f32 = 220.0;

fn env_rows(editor: &Editor, locked: bool, palette: Palette) -> Element<'_, Message> {
    let draft = &editor.form.draft;
    let mut rows = column![].spacing(8);
    let field = |placeholder: &str, value: &str, width: iced::Length| {
        text_input(placeholder, value)
            .width(width)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status))
    };
    for (index, (row, problem)) in draft.env.iter().zip(draft.env_problems()).enumerate() {
        let line = row![
            field("NAME", &row.key, ENV_KEY_WIDTH.into())
                .on_input_maybe((!locked).then_some(move |key| send(Event::EnvKey(index, key)))),
            field("value", &row.value, Fill).on_input_maybe(
                (!locked).then_some(move |value| send(Event::EnvValue(index, value)))
            ),
            button(text("x").size(13))
                .on_press_maybe((!locked).then(|| send(Event::EnvRemove(index))))
                .padding([6, 10])
                .style(move |_, status| secondary(palette, status)),
        ]
        .spacing(8)
        .align_y(Center);
        rows = rows.push(line);
        if let Some(problem) = problem {
            rows = rows.push(text(problem).size(12).color(palette.danger));
        }
    }
    let new = &editor.new_env;
    let problem = (!new.key.is_empty() || !new.value.is_empty())
        .then(|| draft.new_env_problem(&new.key, &new.value))
        .flatten();
    let can_add = !locked && problem.is_none() && !new.key.is_empty();
    rows = rows.push(
        row![
            field("NAME", &new.key, ENV_KEY_WIDTH.into())
                .on_input_maybe((!locked).then_some(|key| send(Event::NewEnvKey(key))))
                .on_submit_maybe(can_add.then(|| send(Event::EnvAdd))),
            field("value", &new.value, Fill)
                .on_input_maybe((!locked).then_some(|value| send(Event::NewEnvValue(value))))
                .on_submit_maybe(can_add.then(|| send(Event::EnvAdd))),
            button(text("+").size(14))
                .on_press_maybe(can_add.then(|| send(Event::EnvAdd)))
                .padding([6, 10])
                .style(move |_, status| secondary(palette, status)),
        ]
        .spacing(8)
        .align_y(Center),
    );
    // A value typed before its name isn't an error yet.
    if let Some(problem) = problem.filter(|_| !new.key.is_empty()) {
        rows = rows.push(text(problem).size(12).color(palette.danger));
    }
    rows.into()
}

fn advanced(editor: &Editor, locked: bool, palette: Palette) -> Element<'_, Message> {
    let draft = &editor.form.draft;
    let info = &editor.info;
    let muted = |value: &str| -> Element<'_, Message> {
        text(value.to_owned()).color(palette.muted).into()
    };
    let field = |value: &str, on_input: fn(String) -> Event| -> Element<'_, Message> {
        text_input("unset", value)
            .on_input_maybe((!locked).then_some(move |value| send(on_input(value))))
            .width(Fill)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status))
            .into()
    };
    let kind = if info.native { "native" } else { "wine" };
    let mut rows = column![setting_row("Kind", muted(kind), None, palette)].spacing(12);
    if !info.native {
        rows = rows.push(setting_row(
            "Prefix",
            field(&draft.prefix, Event::Prefix),
            None,
            palette,
        ));
    }
    if editor.protonfixes_shown() {
        rows = rows.push(setting_row(
            "Game id",
            field(&draft.gameid, Event::GameId),
            None,
            palette,
        ));
        rows = rows.push(setting_row(
            "Store",
            field(&draft.store, Event::Store),
            None,
            palette,
        ));
    }
    if let Some(heroic_id) = &info.heroic_id {
        rows = rows.push(setting_row("Heroic id", muted(heroic_id), None, palette));
    }
    rows.into()
}

fn gamescope_fields(editor: &Editor, locked: bool, palette: Palette) -> Element<'_, Message> {
    let fields = &editor.form.draft.fields;
    let mut rows = Column::new().spacing(10);
    for field in Field::ALL {
        let control: Element<'_, Message> = match field {
            Field::Filter | Field::Scaler | Field::Window if locked => {
                locked_choice(&fields.choice(field).to_string(), palette)
            }
            Field::Filter | Field::Scaler | Field::Window => {
                let options: Vec<Choice> = std::iter::once(Choice(None))
                    .chain(field.choices().into_iter().map(|value| Choice(Some(value))))
                    .collect();
                pick_list(options, Some(fields.choice(field)), move |choice| {
                    send(Event::FieldChoice(field, choice))
                })
                .on_open(send(Event::MenuOpened))
                .on_close(send(Event::MenuClosed))
                .width(CONTROL_WIDTH)
                .into()
            }
            Field::GrabCursor => toggler(fields.grab_cursor)
                .on_toggle_maybe((!locked).then_some(|on| send(Event::GrabCursor(on))))
                .into(),
            Field::Args => Chips {
                items: &fields.args,
                input: &editor.args_input,
                placeholder: "one flag or value",
                error: None,
                on_input: Box::new(|value| send(Event::ArgsInput(value))),
                on_add: send(Event::ArgsAdd),
                on_remove: Box::new(|index| send(Event::ArgsRemove(index))),
                enabled: !locked,
            }
            .view(palette),
            _ => text_input("unset", fields.text(field))
                .on_input_maybe(
                    (!locked).then_some(move |value| send(Event::FieldText(field, value))),
                )
                .width(CONTROL_WIDTH)
                .padding([6, 10])
                .style(move |_, status| input_style(palette, status))
                .into(),
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
    source: Option<Source>,
    toggle: impl Fn(bool) -> Message + 'a,
    reset: Option<Message>,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let mut line = row![
        text(label).color(palette.foreground).width(LABEL_WIDTH),
        container(toggler(on).on_toggle_maybe((!locked).then_some(toggle))).width(64),
        text(source.map_or("", Source::label))
            .size(13)
            .color(palette.muted)
            .width(80),
    ]
    .spacing(12)
    .align_y(Center);
    // Only an overridden row can go back to inheriting.
    if let Some(reset) = reset {
        line = line.push(
            button(text("Reset").size(13))
                .on_press_maybe((!locked).then_some(reset))
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

/// A pick list's value while the page is locked, since iced's pick list has
/// no disabled state.
fn locked_choice(value: &str, palette: Palette) -> Element<'static, Message> {
    container(text(value.to_owned()).color(palette.muted))
        .width(CONTROL_WIDTH)
        .padding([5, 10])
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(4).width(1).color(palette.border))
        })
        .into()
}
