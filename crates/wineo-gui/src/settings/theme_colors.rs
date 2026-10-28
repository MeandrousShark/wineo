//! The Appearance tab's Colors section: the nine roles of the theme showing,
//! read-only for a built-in, a field each for a custom theme.

use iced::theme::Mode;
use iced::widget::{Space, column, container, row, text, text_input};
use iced::{Center, Element, Fill, Font, border, font};
use wineo_core::themes::ROLES;

use super::appearance::{Edit, Form, NAME_TAKEN, send, swatch_palette};
use crate::Message;
use crate::chips::input_style;
use crate::focus_button;
use crate::panel::action_button;
use crate::theme::{Custom, Palette, Shown};
use crate::ui::{secondary_button, section as grouped, section_heading};

pub(crate) const COPY_ID: &str = "appearance-copy";
pub(crate) const RENAME_ID: &str = "appearance-rename";
pub(crate) const DELETE_ID: &str = "appearance-delete";
pub(crate) const NAME_ID: &str = "appearance-name";
pub(crate) const RENAME_APPLY_ID: &str = "appearance-rename-apply";
pub(crate) const RENAME_CANCEL_ID: &str = "appearance-rename-cancel";
pub(crate) const KEEP_ID: &str = "appearance-keep-theme";
pub(crate) const DELETE_CONFIRM_ID: &str = "appearance-delete-confirm";
pub(crate) const INVALID_COLOR: &str = "Invalid color: use #rrggbb.";

const LABELS: [&str; 9] = [
    "Background",
    "Surface",
    "Border",
    "Foreground",
    "Muted",
    "Primary",
    "Success",
    "Warning",
    "Danger",
];
const LABEL_WIDTH: f32 = 120.0;
const FIELD_WIDTH: f32 = 120.0;

/// The focus id of a role's hex field.
pub(crate) fn color_id(role: usize) -> String {
    format!("appearance-color-{}", ROLES[role])
}

fn semibold() -> Font {
    Font {
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    }
}

pub(super) fn section(
    form: &Form,
    mode: Mode,
    locked: bool,
    palette: Palette,
) -> Element<'static, Message> {
    let slot = form.showing_slot(mode);
    let shown = form.shown(slot);
    let custom = match shown {
        Shown::Custom(custom) if !custom.read_only => Some(custom),
        _ => None,
    };
    let enabled = !locked;
    let mut actions = row![control(
        "Copy to a custom theme",
        COPY_ID,
        enabled,
        Edit::Copy(slot),
        palette
    )]
    .spacing(8)
    .align_y(Center);
    if let Some(custom) = custom {
        actions = actions
            .push(control(
                "Rename",
                RENAME_ID,
                enabled,
                Edit::RenameOpen(custom.id.clone()),
                palette,
            ))
            .push(control(
                "Delete",
                DELETE_ID,
                enabled,
                Edit::AskDelete(custom.id.clone()),
                palette,
            ));
    }
    let head = row![
        text(shown.name().to_owned())
            .font(semibold())
            .color(palette.foreground)
            .width(Fill),
        actions,
    ]
    .spacing(12)
    .align_y(Center);
    let mut body = column![head].spacing(12);
    match shown {
        Shown::Preset(_) => {
            body = body.push(note("Built-in themes are read-only.", palette));
        }
        Shown::Custom(custom) if custom.read_only => {
            body = body.push(note(
                "A newer wineo made this theme, so it's read-only.",
                palette,
            ));
        }
        Shown::Custom(_) => {}
    }
    if let Some(custom) = custom {
        if let Some(rename) = form
            .renaming
            .as_ref()
            .filter(|rename| rename.id == custom.id)
        {
            body = body.push(rename_row(&rename.text, rename.taken, enabled, palette));
        }
        if form.deleting.as_deref() == Some(custom.id.as_str()) {
            body = body.push(delete_card(custom, enabled, palette));
        }
    }
    let improve = form.draft.improve_readability;
    let swatches = swatch_palette(shown.palette(), shown.kind(), improve);
    let raw = shown.palette();
    let mut roles = column![].spacing(8);
    for role in 0..ROLES.len() {
        roles = roles.push(match custom {
            Some(custom) => color_field(form, custom, role, swatches, enabled, palette),
            None => color_line(role, swatches, &crate::theme::hex(raw.role(role)), palette),
        });
    }
    column![
        section_heading("Colors", palette),
        grouped(body.push(roles), palette),
    ]
    .spacing(8)
    .into()
}

fn control(
    label: &'static str,
    id: &'static str,
    enabled: bool,
    edit: Edit,
    palette: Palette,
) -> Element<'static, Message> {
    let message = send(edit);
    focus_button::control(
        secondary_button(label, palette)
            .on_press_maybe(enabled.then(|| message.clone()))
            .into(),
        enabled,
        id,
        message,
    )
}

fn note(line: &'static str, palette: Palette) -> Element<'static, Message> {
    text(line).size(13).color(palette.muted).into()
}

fn role_label(role: usize, palette: Palette) -> Element<'static, Message> {
    text(LABELS[role])
        .color(palette.foreground)
        .width(LABEL_WIDTH)
        .into()
}

fn chip(color: iced::Color, palette: Palette) -> Element<'static, Message> {
    container(Space::new())
        .width(24)
        .height(24)
        .style(move |_| {
            container::Style::default()
                .background(color)
                .border(border::rounded(4).width(1).color(palette.border))
        })
        .into()
}

/// A built-in or read-only role: its swatch and its hex.
fn color_line(
    role: usize,
    swatches: Palette,
    hex: &str,
    palette: Palette,
) -> Element<'static, Message> {
    row![
        role_label(role, palette),
        chip(swatches.role(role), palette),
        text(hex.to_owned()).size(13).color(palette.muted),
    ]
    .spacing(12)
    .align_y(Center)
    .into()
}

/// A custom theme's role: its swatch, which keeps the last good color, and
/// a hex field that applies only a color.
fn color_field(
    form: &Form,
    custom: &Custom,
    role: usize,
    swatches: Palette,
    enabled: bool,
    palette: Palette,
) -> Element<'static, Message> {
    let id = custom.id.clone();
    let input = text_input("#rrggbb", &form.color_text(custom, role))
        .id(iced::widget::Id::from(color_id(role)))
        .on_input_maybe(enabled.then_some(move |value| send(Edit::Color(id.clone(), role, value))))
        .width(FIELD_WIDTH)
        .padding([6, 10])
        .style(move |_, status| input_style(palette, status));
    let mut line = row![
        role_label(role, palette),
        chip(swatches.role(role), palette),
        input
    ]
    .spacing(12)
    .align_y(Center);
    if form.color_invalid(&custom.id, role) {
        line = line.push(text(INVALID_COLOR).size(13).color(palette.danger));
    }
    line.into()
}

fn rename_row(
    name: &str,
    taken: bool,
    enabled: bool,
    palette: Palette,
) -> Element<'static, Message> {
    let filled = !name.trim().is_empty();
    let mut input = text_input("Name", name)
        .id(iced::widget::Id::new(NAME_ID))
        .on_input_maybe(enabled.then_some(|value| send(Edit::RenameText(value))))
        .width(Fill)
        .padding([6, 10])
        .style(move |_, status| input_style(palette, status));
    if enabled && filled {
        input = input.on_submit(send(Edit::RenameApply));
    }
    let apply = send(Edit::RenameApply);
    let line = row![
        input,
        focus_button::control(
            action_button("Rename", palette.primary, enabled && filled, apply.clone()),
            enabled && filled,
            RENAME_APPLY_ID,
            apply,
        ),
        control(
            "Cancel",
            RENAME_CANCEL_ID,
            enabled,
            Edit::RenameCancel,
            palette
        ),
    ]
    .spacing(8)
    .align_y(Center);
    let mut rows = column![line].spacing(4);
    if taken {
        rows = rows.push(text(NAME_TAKEN).size(13).color(palette.danger));
    }
    rows.into()
}

fn delete_card(custom: &Custom, enabled: bool, palette: Palette) -> Element<'static, Message> {
    let delete = send(Edit::Delete(custom.id.clone()));
    let actions = row![
        Space::new().width(Fill),
        control("Keep it", KEEP_ID, enabled, Edit::KeepTheme, palette),
        focus_button::control(
            action_button("Delete", palette.danger, enabled, delete.clone()),
            enabled,
            DELETE_CONFIRM_ID,
            delete,
        ),
    ]
    .spacing(8)
    .align_y(Center);
    container(
        column![
            text(format!("Delete {}?", custom.name)).color(palette.foreground),
            actions
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Fill)
    .style(move |_| {
        container::Style::default()
            .background(palette.surface)
            .border(border::rounded(6).width(1).color(palette.danger))
    })
    .into()
}
