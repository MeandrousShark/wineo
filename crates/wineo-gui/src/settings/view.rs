use iced::widget::{
    Column, Space, button, column, container, opaque, pick_list, row, scrollable, stack, text,
    text_input, toggler,
};
use iced::{Center, Color, Element, Fill, Font, border, font};

use wineo_core::art::Kind;
use wineo_core::components::Component;
use wineo_core::config::{self, Arch};
use wineo_core::key_store::KeySource;

use super::form::{self, Build, Choice, Field, GamescopeFields, List, Source, Switch};
use super::global;
use super::load::Scan;
use super::{Editor, Event, Page, TargetEdit, View};
use crate::chips::{Chips, input_style};
use crate::panel::{action_button, error_box};
use crate::theme::Palette;
use crate::ui::{
    PickListStyle, ScrollStyle, secondary_button, section as grouped_section, section_heading,
};
use crate::{Message, WHEEL_STEP_SCALE, wheel};
use wineo_core::paths::Paths;

const LABEL_WIDTH: f32 = 180.0;
const CONTROL_WIDTH: f32 = 260.0;
const PAGE_WIDTH: f32 = 760.0;
const SCROLL_ID: &str = "settings-scroll";
const STEAM_KEY_ID: &str = "settings-steam-key";

fn send(event: Event) -> Message {
    Message::Settings(event)
}

// Shown in the config's own `~/...` form, which is also what the field accepts.
fn root_placeholder(field: global::Field, paths: &Paths) -> String {
    let root = field.default_root(paths);
    paths
        .contract(&root)
        .unwrap_or_else(|_| root.display().to_string())
}

fn semibold() -> Font {
    Font {
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    }
}

impl Page {
    pub(crate) fn view(&self, paths: &Paths, palette: Palette) -> Element<'_, Message> {
        // Global settings is a sidebar section, so the sidebar is its way out.
        let back = (!self.is_global()).then(|| {
            secondary_button("Back", palette)
                .on_press_maybe((!self.saving).then(|| send(Event::Back)))
        });
        let header = container(
            row![
                back,
                text(format!("{} settings", self.name))
                    .size(22)
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
                        secondary_button("Open config file", palette)
                            .on_press(send(Event::OpenConfig)),
                    );
                }
                if let Some(error) = &self.error {
                    content = content.push(text(error).size(13).color(palette.danger));
                }
                content.into()
            }
            View::Form(editor) => {
                editor_view(&self.name, editor, self.locked(), self.resolving(), palette)
            }
            View::Global(editor) => global_view(editor, self, paths, self.locked(), palette),
        };
        let scroll = scrollable(
            container(container(body).max_width(PAGE_WIDTH).width(Fill))
                .padding(24)
                .center_x(Fill),
        )
        .wineo_style(palette)
        .id(iced::widget::Id::new(SCROLL_ID))
        .width(Fill)
        .height(Fill);
        let mut page = column![
            header,
            divider(palette),
            wheel::scale(scroll, WHEEL_STEP_SCALE)
        ];
        if matches!(self.view, View::Form(_) | View::Global(_)) {
            page = page.push(divider(palette)).push(self.save_bar(palette));
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
    fn save_bar(&self, palette: Palette) -> Element<'_, Message> {
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
            actions = actions.push(crate::spinner::spinner(palette, 16.0));
            actions = actions.push(text("Saving...").size(14).color(palette.muted));
        }
        let revert_enabled = !self.saving && (self.is_dirty() || self.changed_on_disk);
        actions = actions.push(
            secondary_button("Revert", palette)
                .on_press_maybe(revert_enabled.then(|| send(Event::Revert))),
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
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(0).width(1).color(palette.border))
            })
            .into()
    }
}

fn editor_view<'a>(
    name: &'a str,
    editor: &'a Editor,
    locked: bool,
    resolving: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let info = &editor.info;
    let draft = &editor.form.draft;
    if let Some(proxied) = &draft.proxied {
        return proxied_editor_view(editor, proxied, locked, palette);
    }
    let mut sections = column![].spacing(24).width(Fill);
    if !info.native {
        let runner = super::runner_picker::picker(
            &info.runners,
            draft.runner.as_deref().unwrap_or_default(),
            locked,
            palette,
            Event::RunnerPicked,
        );
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
        let fields =
            gamescope_fields(
                &draft.fields,
                &editor.args_input,
                locked,
                palette,
                |edit| match edit {
                    FieldEdit::Text(field, value) => Event::FieldText(field, value),
                    FieldEdit::Choice(field, choice) => Event::FieldChoice(field, choice),
                    FieldEdit::GrabCursor(on) => Event::GrabCursor(on),
                    FieldEdit::ArgsInput(value) => Event::ArgsInput(value),
                    FieldEdit::ArgsAdd => Event::ArgsAdd,
                    FieldEdit::ArgsRemove(index) => Event::ArgsRemove(index),
                },
            );
        launch = launch.push(indented(fields));
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
        env_rows(
            &draft.env,
            &editor.new_env,
            draft.env_problems(),
            draft.new_env_problem(&editor.new_env.key, &editor.new_env.value),
            locked,
            palette,
        ),
        palette,
    ));
    sections = sections.push(section(
        "Targets",
        targets(name, editor, locked, resolving, palette),
        palette,
    ));
    sections = sections.push(section(
        "Advanced",
        advanced(editor, locked, palette),
        palette,
    ));
    sections.into()
}

fn proxied_editor_view<'a>(
    editor: &'a Editor,
    proxied: &'a form::ProxiedValues,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let game = column![
        setting_row(
            "Title",
            text_field(
                "Steam game title",
                proxied.title.as_deref().unwrap_or_default(),
                locked,
                |title| send(Event::ProxiedTitle(title)),
                palette,
            ),
            None,
            palette,
        ),
        switch_row(
            "Desktop entry",
            proxied.desktop,
            None,
            |on| send(Event::ProxiedDesktop(on)),
            None,
            locked,
            palette,
        ),
    ]
    .spacing(12);
    column![
        section("Game", game.into(), palette),
        section(
            "Tags",
            list_chips(editor, List::Tags, locked, palette),
            palette
        ),
    ]
    .spacing(24)
    .width(Fill)
    .into()
}

fn global_view<'a>(
    editor: &'a global::Editor,
    page: &'a Page,
    paths: &Paths,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let (steamgriddb, steam_key, gog) = (&page.steamgriddb, &page.steam_key, &page.gog);
    let draft = &editor.form.draft;
    let mut launch = column![].spacing(12);
    for key in Switch::ALL {
        launch = launch.push(switch_row(
            key.label(),
            draft.switch(key).unwrap_or(false),
            None,
            move |on| send(Event::GlobalSwitch(key, on)),
            None,
            locked,
            palette,
        ));
    }
    let runner = super::runner_picker::picker(
        &editor.runners,
        &draft.runner,
        locked,
        palette,
        Event::GlobalRunner,
    );
    let arch: Element<'a, Message> = if locked {
        locked_choice(draft.arch.as_str(), palette)
    } else {
        pick_list([Arch::Win64, Arch::Win32], Some(draft.arch), |arch| {
            send(Event::GlobalArch(arch))
        })
        .on_open(send(Event::MenuOpened))
        .on_close(send(Event::MenuClosed))
        .width(CONTROL_WIDTH)
        .wineo_style(palette)
        .into()
    };
    let mut new_env = column![
        setting_row("Runner", runner, None, palette),
        setting_row("Arch", arch, None, palette),
    ]
    .spacing(12);
    for field in global::Field::ROOTS {
        let placeholder = root_placeholder(field, paths);
        new_env = new_env.push(setting_row(
            field.label(),
            text_field(
                &placeholder,
                draft.field(field),
                locked,
                move |value| send(Event::GlobalField(field, value)),
                palette,
            ),
            None,
            palette,
        ));
        let value = draft.field(field);
        if value != editor.form.loaded.field(field)
            && !value.is_empty()
            && !std::path::Path::new(value).is_absolute()
            && !value.starts_with("~/")
        {
            new_env = new_env.push(
                text("The path must be absolute or start with ~/.")
                    .size(12)
                    .color(palette.danger),
            );
        }
    }
    let portable = paths.portable_root().is_some();
    new_env = new_env.push(switch_row(
        "Builds from Steam, Heroic, and Lutris",
        draft.external_runners == Some(true),
        None,
        |on| send(Event::GlobalExternalRunners(on)),
        None,
        locked || portable,
        palette,
    ));
    if portable {
        new_env = new_env.push(
            text("Not used in a portable folder.")
                .size(13)
                .color(palette.muted),
        );
    }
    let mut runs = column![].spacing(12);
    for field in global::Field::NUMBERS {
        let placeholder = match field {
            global::Field::StopTimeout => config::DEFAULT_STOP_TIMEOUT.to_string(),
            global::Field::LogLimitMb => config::DEFAULT_LOG_LIMIT_MB.to_string(),
            _ => unreachable!("number fields"),
        };
        runs = runs.push(setting_row(
            field.label(),
            text_field(
                &placeholder,
                draft.field(field),
                locked,
                move |value| send(Event::GlobalField(field, value)),
                palette,
            ),
            None,
            palette,
        ));
    }
    let mut art = column![].spacing(12);
    for kind in Kind::ALL {
        let label = match kind {
            Kind::Icon => "Icon",
            Kind::Grid => "Grid",
            Kind::Hero => "Hero",
            Kind::Logo => "Logo",
        };
        art = art.push(switch_row(
            label,
            draft.art_fetch.kinds().contains(&kind),
            None,
            move |on| send(Event::GlobalArtKind(kind, on)),
            None,
            locked,
            palette,
        ));
    }
    column![
        section("Launch defaults", launch.into(), palette),
        section("New environments", new_env.into(), palette),
        section("Runs", runs.into(), palette),
        section("Art to fetch", art.into(), palette),
        section(
            "SteamGridDB",
            key_row(steamgriddb_row(steamgriddb, paths), palette),
            palette,
        ),
        container(section(
            "Steam Web API",
            key_row(steam_key_row(steam_key, paths), palette),
            palette,
        ))
        .id(iced::widget::Id::new(STEAM_KEY_ID)),
        section(
            "GOG",
            column![
                gog_install_rows(editor, paths, locked, palette),
                gog_row(gog, locked, palette)
            ]
            .spacing(12)
            .into(),
            palette
        ),
        section(
            "Environment variables",
            env_rows(
                &draft.env,
                &editor.new_env,
                draft.env_problems(),
                draft.new_env_problem(&editor.new_env.key, &editor.new_env.value),
                locked,
                palette
            ),
            palette
        ),
    ]
    .spacing(24)
    .width(Fill)
    .into()
}

/// One saved API key's row: its status, notes, and controls. The SteamGridDB
/// and Steam keys share it, so both behave the same. Nothing here ever holds
/// a saved key; the typed one only reaches a secure input.
pub(super) struct KeyRow<'a> {
    status: String,
    override_note: Option<String>,
    warning: Option<String>,
    status_error: Option<&'a str>,
    result: Option<&'a str>,
    error: Option<&'a str>,
    hint: Option<String>,
    /// Whether a key is saved, so only the button that applies shows. None
    /// while unknown, or when a variable's key could hide a saved one,
    /// shows both.
    saved: Option<bool>,
    busy: Option<super::steamgriddb::Busy>,
    input_open: bool,
    input: &'a super::steamgriddb::Secret,
    placeholder: &'static str,
    open: Event,
    cancel: Event,
    set: Event,
    remove: Event,
    text: fn(super::steamgriddb::Secret) -> Event,
}

fn saved(source: Option<KeySource>) -> Option<bool> {
    match source {
        None => Some(false),
        Some(KeySource::Keyring | KeySource::File) => Some(true),
        Some(KeySource::Variable) => None,
    }
}

fn status_or_checking(label: Option<String>, failed: bool) -> String {
    label.unwrap_or_else(|| {
        if failed {
            "Key status unavailable".to_owned()
        } else {
            "Checking...".to_owned()
        }
    })
}

pub(super) fn steamgriddb_row<'a>(
    settings: &'a super::steamgriddb::Settings,
    paths: &Paths,
) -> KeyRow<'a> {
    KeyRow {
        status: status_or_checking(
            settings.status.as_ref().map(|status| status.label(paths)),
            settings.status_error.is_some(),
        ),
        override_note: settings
            .status
            .as_ref()
            .is_some_and(|status| status.override_note)
            .then(|| "WINEO_STEAMGRIDDB_KEY is set and overrides the saved key".to_owned()),
        warning: settings.warning.clone().or_else(|| {
            settings
                .status
                .as_ref()
                .and_then(|status| status.warning(paths))
        }),
        status_error: settings.status_error.as_deref(),
        result: settings.result.as_deref(),
        error: settings.error.as_deref(),
        hint: None,
        saved: settings
            .status
            .as_ref()
            .and_then(|status| saved(status.source)),
        busy: settings.busy,
        input_open: settings.input_open,
        input: &settings.input,
        placeholder: "SteamGridDB API key",
        open: Event::SteamGridDbKeySetOpen,
        cancel: Event::SteamGridDbKeySetCancel,
        set: Event::SteamGridDbKeySet,
        remove: Event::SteamGridDbKeyRemove,
        text: Event::SteamGridDbKeyText,
    }
}

pub(super) fn steam_key_row<'a>(
    settings: &'a super::steam_key::Settings,
    paths: &Paths,
) -> KeyRow<'a> {
    KeyRow {
        status: status_or_checking(
            settings.status.as_ref().map(|status| status.label(paths)),
            settings.status_error.is_some(),
        ),
        override_note: settings
            .status
            .as_ref()
            .is_some_and(|status| status.override_note)
            .then(|| {
                format!(
                    "{} is set and overrides the saved key",
                    wineo_core::steam::key::KEY_VAR
                )
            }),
        warning: settings.warning.clone().or_else(|| {
            settings
                .status
                .as_ref()
                .and_then(|status| status.warning(paths))
        }),
        status_error: settings.status_error.as_deref(),
        result: settings.result.as_deref(),
        error: settings.error.as_deref(),
        hint: Some(format!("Get a key at {}.", super::steam_key::KEY_PAGE)),
        saved: settings
            .status
            .as_ref()
            .and_then(|status| saved(status.source)),
        busy: settings.busy,
        input_open: settings.input_open,
        input: &settings.input,
        placeholder: "Steam Web API key",
        open: Event::SteamKeySetOpen,
        cancel: Event::SteamKeySetCancel,
        set: Event::SteamKeySet,
        remove: Event::SteamKeyRemove,
        text: Event::SteamKeyText,
    }
}

impl KeyRow<'_> {
    fn busy_label(&self) -> Option<&'static str> {
        self.busy.map(|busy| match busy {
            super::steamgriddb::Busy::Saving => "Checking and saving...",
            super::steamgriddb::Busy::Removing => "Removing...",
        })
    }

    /// Every piece of text the row shows, in order.
    #[cfg(test)]
    pub(super) fn texts(&self) -> Vec<String> {
        let mut texts = vec!["Key".to_owned(), self.status.clone()];
        texts.extend(self.override_note.clone());
        texts.extend(self.warning.clone());
        texts.extend(
            [self.status_error, self.result, self.error]
                .into_iter()
                .flatten()
                .map(str::to_owned),
        );
        texts.extend(self.busy_label().map(str::to_owned));
        texts.extend(self.hint.clone());
        if self.input_open {
            texts.extend([self.placeholder, "Save", "Cancel"].map(str::to_owned));
        } else {
            if self.saved != Some(true) {
                texts.push("Set key...".to_owned());
            }
            if self.saved != Some(false) {
                texts.push("Remove key".to_owned());
            }
        }
        texts
    }
}

fn key_row<'a>(row_data: KeyRow<'a>, palette: Palette) -> Element<'a, Message> {
    let busy_label = row_data.busy_label();
    let KeyRow {
        status,
        override_note,
        warning,
        status_error,
        result,
        error,
        hint,
        saved,
        busy,
        input_open,
        input,
        placeholder,
        open,
        cancel,
        set,
        remove,
        text: on_text,
    } = row_data;
    let mut rows = column![setting_row(
        "Key",
        text(status).color(palette.foreground).into(),
        None,
        palette,
    )]
    .spacing(12);
    if let Some(note) = override_note {
        rows = rows.push(text(note).size(13).color(palette.muted));
    }
    if let Some(warning) = warning {
        rows = rows.push(text(warning).size(13).color(palette.warning));
    }
    if let Some(error) = status_error {
        rows = rows.push(text(error).size(13).color(palette.danger));
    }
    if let Some(result) = result {
        rows = rows.push(text(result).size(13).color(palette.muted));
    }
    if let Some(error) = error {
        rows = rows.push(text(error).size(13).color(palette.danger));
    }
    if let Some(label) = busy_label {
        rows = rows.push(
            row![
                crate::spinner::spinner(palette, 16.0),
                text(label).size(13).color(palette.muted),
            ]
            .spacing(8)
            .align_y(Center),
        );
    }
    if let Some(hint) = hint {
        rows = rows.push(text(hint).size(13).color(palette.muted));
    }
    if input_open {
        let field = text_input(placeholder, input.as_str())
            .secure(true)
            .on_input_maybe(
                busy.is_none()
                    .then_some(move |value| send(on_text(super::steamgriddb::Secret::new(value)))),
            )
            .width(Fill)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status));
        rows = rows.push(
            row![
                field,
                action_button(
                    "Save",
                    palette.primary,
                    busy.is_none() && !input.as_str().trim().is_empty(),
                    send(set),
                ),
                secondary_button("Cancel", palette)
                    .on_press_maybe(busy.is_none().then(|| send(cancel))),
            ]
            .spacing(8)
            .align_y(Center),
        );
    } else {
        let mut buttons = row![].spacing(8).align_y(Center);
        if saved != Some(true) {
            buttons = buttons.push(action_button(
                "Set key...",
                palette.primary,
                busy.is_none(),
                send(open),
            ));
        }
        if saved != Some(false) {
            buttons = buttons.push(
                secondary_button("Remove key", palette)
                    .on_press_maybe(busy.is_none().then(|| send(remove))),
            );
        }
        rows = rows.push(buttons);
    }
    rows.into()
}

fn gog_send(event: super::gog::Event) -> Message {
    send(Event::Gog(event))
}

/// Every piece of text the GOG row shows, in order. Never the pasted
/// address, which only the secure input holds.
#[cfg(test)]
pub(super) fn gog_texts(settings: &super::gog::Settings) -> Vec<String> {
    let mut texts = vec![
        "Account".to_owned(),
        status_or_checking(
            settings.status.as_ref().map(super::gog::Status::label),
            settings.status_error.is_some(),
        ),
    ];
    texts.extend(settings.warning.clone());
    texts.extend(
        [&settings.status_error, &settings.result, &settings.error]
            .into_iter()
            .flatten()
            .cloned(),
    );
    texts.extend(settings.busy.map(|busy| busy.label().to_owned()));
    if settings.input_open {
        texts.push("Log in to GOG in your browser. If it doesn't open, go to:".to_owned());
        texts.extend(settings.login_url.clone());
        texts.push("Then copy the address of the page you land on.".to_owned());
        texts.extend(["Copy link", "Address", "Log in", "Cancel"].map(str::to_owned));
    } else {
        if settings.shows_login() {
            texts.push("Log in".to_owned());
        }
        if settings.shows_logout() {
            texts.push("Log out".to_owned());
        }
    }
    texts
}

/// Where GOG installs go and in which language. Both are form fields, saved
/// with the rest, unlike the login below them.
fn gog_install_rows<'a>(
    editor: &'a global::Editor,
    paths: &Paths,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let draft = &editor.form.draft;
    // GOG games live in the host library, so a portable folder has none.
    let portable = paths.portable_root().is_some();
    let off = locked || portable;
    let games = global::Field::GamesRoot;
    let placeholder = root_placeholder(games, paths);
    let mut rows = column![
        setting_row(
            games.label(),
            text_field(
                &placeholder,
                draft.field(games),
                off,
                move |value| send(Event::GlobalField(games, value)),
                palette,
            ),
            Some(
                secondary_button("Browse...", palette)
                    .on_press_maybe((!off).then(|| send(Event::BrowseGamesRoot)))
                    .into()
            ),
            palette,
        ),
        setting_row(
            global::Field::GogLanguage.label(),
            text_field(
                wineo_core::gog::language::DEFAULT,
                draft.field(global::Field::GogLanguage),
                off,
                |value| send(Event::GlobalField(global::Field::GogLanguage, value)),
                palette,
            ),
            None,
            palette,
        ),
    ]
    .spacing(12);
    if portable {
        rows = rows.push(
            text("Not used in a portable folder.")
                .size(13)
                .color(palette.muted),
        );
    } else if let Some(problem) = editor.form.gog_problem() {
        rows = rows.push(text(problem).size(12).color(palette.danger));
    } else {
        rows = rows.push(
            text("A locale tag like fr-FR. Games without it install in English.")
                .size(12)
                .color(palette.muted),
        );
    }
    rows.into()
}

fn gog_row<'a>(
    settings: &'a super::gog::Settings,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    use super::gog::Event as GogEvent;
    let status = status_or_checking(
        settings.status.as_ref().map(super::gog::Status::label),
        settings.status_error.is_some(),
    );
    let mut rows = column![setting_row(
        "Account",
        text(status).color(palette.foreground).into(),
        None,
        palette,
    )]
    .spacing(12);
    if let Some(warning) = &settings.warning {
        rows = rows.push(text(warning).size(13).color(palette.warning));
    }
    if let Some(error) = &settings.status_error {
        rows = rows.push(text(error).size(13).color(palette.danger));
    }
    if let Some(result) = &settings.result {
        rows = rows.push(text(result).size(13).color(palette.muted));
    }
    if let Some(error) = &settings.error {
        rows = rows.push(text(error).size(13).color(palette.danger));
    }
    if let Some(busy) = settings.busy {
        rows = rows.push(
            row![
                crate::spinner::spinner(palette, 16.0),
                text(busy.label()).size(13).color(palette.muted),
            ]
            .spacing(8)
            .align_y(Center),
        );
    }
    let idle = settings.busy.is_none() && !locked;
    if settings.input_open {
        rows = rows.push(
            text("Log in to GOG in your browser. If it doesn't open, go to:")
                .size(13)
                .color(palette.muted),
        );
        if let Some(url) = &settings.login_url {
            rows = rows.push(
                row![
                    // One long word, so it has to break mid-word to fit.
                    text(url)
                        .size(13)
                        .color(palette.foreground)
                        .width(Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    secondary_button("Copy link", palette).on_press(gog_send(GogEvent::CopyLink)),
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        rows = rows.push(
            text("Then copy the address of the page you land on.")
                .size(13)
                .color(palette.muted),
        );
        let field = text_input("Address", settings.input.as_str())
            .secure(true)
            .on_input_maybe(idle.then_some(|value| {
                gog_send(GogEvent::LoginText(super::steamgriddb::Secret::new(value)))
            }))
            .on_submit_maybe(
                (idle && !settings.input.as_str().trim().is_empty())
                    .then(|| gog_send(GogEvent::Login)),
            )
            .width(Fill)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status));
        rows = rows.push(
            row![
                field,
                action_button(
                    "Log in",
                    palette.primary,
                    idle && !settings.input.as_str().trim().is_empty(),
                    gog_send(GogEvent::Login),
                ),
                secondary_button("Cancel", palette).on_press_maybe(
                    settings
                        .busy
                        .is_none()
                        .then(|| gog_send(GogEvent::LoginCancel))
                ),
            ]
            .spacing(8)
            .align_y(Center),
        );
    } else {
        let mut buttons = row![].spacing(8).align_y(Center);
        if settings.shows_login() {
            buttons = buttons.push(action_button(
                "Log in",
                palette.primary,
                idle,
                gog_send(GogEvent::LoginOpen),
            ));
        }
        if settings.shows_logout() {
            buttons = buttons.push(secondary_button("Log out", palette).on_press_maybe(
                (idle && settings.logged_in()).then(|| gog_send(GogEvent::Logout)),
            ));
        }
        rows = rows.push(buttons);
    }
    rows.into()
}

/// Finds the Steam key row, for the import page's link to it.
pub(super) fn locate_steam_key() -> iced::Task<crate::grid_motion::CardBounds> {
    crate::grid_motion::locate(
        iced::widget::Id::new(STEAM_KEY_ID),
        iced::widget::Id::new(SCROLL_ID),
    )
}

/// Scrolls the page so a located row sits near the top.
pub(super) fn scroll_to_row(bounds: crate::grid_motion::CardBounds) -> iced::Task<Message> {
    let top = bounds.card.y - bounds.viewport.y;
    iced::widget::operation::scroll_to(
        iced::widget::Id::new(SCROLL_ID),
        iced::widget::scrollable::AbsoluteOffset {
            x: 0.0,
            y: (top - 24.0).max(0.0),
        },
    )
}

fn indented(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content)
        .padding(iced::Padding {
            left: 24.0,
            ..iced::Padding::ZERO
        })
        .into()
}

fn target_event(target: &str, edit: TargetEdit) -> Message {
    send(Event::Target(target.to_owned(), edit))
}

fn targets<'a>(
    name: &'a str,
    editor: &'a Editor,
    locked: bool,
    resolving: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let mut rows = column![].spacing(12);
    if editor.form.draft.targets.is_empty() {
        rows = rows.push(text("No targets yet.").size(13).color(palette.muted));
    }
    for target in editor.form.draft.targets.keys() {
        rows = rows.push(target_card(name, editor, target, locked, palette));
    }
    rows = rows.push(add_target(editor, locked, resolving, palette));
    rows.into()
}

fn text_field<'a>(
    placeholder: &str,
    value: &str,
    locked: bool,
    on_input: impl Fn(String) -> Message + 'a,
    palette: Palette,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input_maybe((!locked).then_some(on_input))
        .width(Fill)
        .padding([6, 10])
        .style(move |_, status| input_style(palette, status))
        .into()
}

fn target_card<'a>(
    name: &'a str,
    editor: &'a Editor,
    target: &'a str,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let form = &editor.form;
    let values = &form.draft.targets[target];
    let open = editor.open_cards.contains(target);
    let is_default = form.draft.default_target.as_deref() == Some(target);
    let mut title = format!("{} {target}", if open { "v" } else { ">" });
    if is_default {
        title.push_str(" (default)");
    }
    let header = button(
        row![
            text(title).font(semibold()).color(palette.foreground),
            text(values.exe.as_str()).size(13).color(palette.muted),
        ]
        .spacing(12)
        .align_y(Center),
    )
    .on_press(send(Event::Card(target.to_owned(), !open)))
    .padding([4, 0])
    .style(|_, _| button::Style::default());
    let mut card = column![header].spacing(12);
    if open {
        card = card.push(indented(target_body(
            name, editor, target, is_default, locked, palette,
        )));
    }
    container(card)
        .padding(12)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.background)
                .border(border::rounded(6).width(1).color(palette.border))
        })
        .into()
}

fn target_body<'a>(
    name: &'a str,
    editor: &'a Editor,
    target: &'a str,
    is_default: bool,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let form = &editor.form;
    let info = &editor.info;
    let values = &form.draft.targets[target];
    let owned = || target.to_owned();
    let mut rows = column![
        setting_row(
            "Exe",
            text_field(
                "path to the program",
                &values.exe,
                locked,
                move |exe| target_event(target, TargetEdit::Exe(exe)),
                palette
            ),
            Some(
                secondary_button("Browse...", palette)
                    .on_press_maybe(
                        (!locked).then(|| send(Event::BrowsePath(Some(target.to_owned()), false)))
                    )
                    .into()
            ),
            palette,
        ),
        setting_row(
            "Args",
            Chips {
                items: &values.args,
                input: editor.target_args_input(target),
                placeholder: "one argument",
                error: None,
                on_input: Box::new(move |value| target_event(target, TargetEdit::ArgsInput(value))),
                on_add: target_event(target, TargetEdit::ArgsAdd),
                on_remove: Box::new(move |index| {
                    target_event(target, TargetEdit::ArgsRemove(index))
                }),
                enabled: !locked,
            }
            .view(palette),
            None,
            palette,
        ),
        setting_row(
            "Working dir",
            text_field(
                "the exe's own",
                &values.cwd,
                locked,
                move |cwd| target_event(target, TargetEdit::Cwd(cwd)),
                palette
            ),
            Some(
                secondary_button("Browse...", palette)
                    .on_press_maybe(
                        (!locked).then(|| send(Event::BrowsePath(Some(target.to_owned()), true)))
                    )
                    .into()
            ),
            palette,
        ),
    ]
    .spacing(12);
    if form.draft.targets.len() > 1 {
        rows = rows.push(switch_row(
            "Default",
            is_default,
            None,
            move |on| target_event(target, TargetEdit::Default(on)),
            None,
            locked,
            palette,
        ));
    }
    // A hand-written entry in a portable folder can still be turned off.
    let refusal = editor.desktop_refusal().filter(|_| !values.desktop);
    rows = rows.push(switch_row(
        "Desktop entry",
        values.desktop,
        None,
        move |on| target_event(target, TargetEdit::Desktop(on)),
        None,
        locked || refusal.is_some(),
        palette,
    ));
    if let Some(refusal) = refusal {
        rows = rows.push(text(refusal).size(13).color(palette.muted));
    }
    rows = rows.push(setting_row(
        "Entry name",
        text_field(
            target,
            &values.name,
            locked,
            move |value| target_event(target, TargetEdit::Name(value)),
            palette,
        ),
        None,
        palette,
    ));
    let switches: &[Switch] = if info.native {
        &[Switch::Gamemode, Switch::Mangohud]
    } else {
        &Switch::ALL
    };
    for switch in switches {
        let (on, source) = form.target_switch(target, *switch, &info.inherited);
        rows = rows.push(switch_row(
            switch.label(),
            on,
            Some(source),
            move |on| target_event(target, TargetEdit::Switch(*switch, on)),
            (source == Source::Here).then(|| target_event(target, TargetEdit::Reset(*switch))),
            locked,
            palette,
        ));
    }
    let (on, source) = form.target_gamescope(target);
    rows = rows.push(switch_row(
        "Gamescope",
        on,
        Some(source),
        move |on| target_event(target, TargetEdit::Gamescope(on)),
        (source == Source::Here).then(|| target_event(target, TargetEdit::GamescopeReset)),
        locked,
        palette,
    ));
    if let Some(fields) = form.target_fields(target) {
        if source == Source::Here {
            rows = rows.push(indented(gamescope_fields(
                fields,
                editor.target_gamescope_args_input(target),
                locked,
                palette,
                move |edit| {
                    Event::Target(
                        owned(),
                        match edit {
                            FieldEdit::Text(field, value) => TargetEdit::FieldText(field, value),
                            FieldEdit::Choice(field, choice) => {
                                TargetEdit::FieldChoice(field, choice)
                            }
                            FieldEdit::GrabCursor(on) => TargetEdit::GrabCursor(on),
                            FieldEdit::ArgsInput(value) => TargetEdit::GamescopeArgsInput(value),
                            FieldEdit::ArgsAdd => TargetEdit::GamescopeArgsAdd,
                            FieldEdit::ArgsRemove(index) => TargetEdit::GamescopeArgsRemove(index),
                        },
                    )
                },
            )));
        } else {
            rows = rows.push(indented(inherited_fields(fields, target, locked, palette)));
        }
    }
    rows = rows.push(remove_target(name, editor, target, locked, palette));
    rows.into()
}

/// The environment's gamescope fields a target runs with, shown read-only.
/// Its own table would replace them all, so editing starts from a copy.
fn inherited_fields<'a>(
    fields: &GamescopeFields,
    target: &'a str,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let mut rows = column![].spacing(6);
    let mut any = false;
    for field in Field::ALL {
        let value = match field {
            Field::Filter | Field::Scaler | Field::Window => {
                fields.choice(field).0.map(str::to_owned)
            }
            Field::GrabCursor => fields.grab_cursor.then(|| "on".to_owned()),
            Field::Args => (!fields.args.is_empty()).then(|| fields.args.join(" ")),
            _ => Some(fields.text(field).to_owned()).filter(|value| !value.is_empty()),
        };
        if let Some(value) = value {
            any = true;
            rows = rows.push(
                row![
                    text(field.label())
                        .color(palette.muted)
                        .width(LABEL_WIDTH - 24.0),
                    text(value).color(palette.muted),
                ]
                .spacing(12),
            );
        }
    }
    if !any {
        rows = rows.push(
            text("The environment's gamescope has no options set.")
                .size(13)
                .color(palette.muted),
        );
    }
    rows.push(
        secondary_button("Edit here", palette)
            .on_press_maybe((!locked).then(|| target_event(target, TargetEdit::GamescopeHere))),
    )
    .into()
}

fn remove_target<'a>(
    name: &'a str,
    editor: &'a Editor,
    target: &'a str,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let button_row = |content: Element<'a, Message>| -> Element<'a, Message> {
        row![Space::new().width(Fill), content]
            .spacing(8)
            .align_y(Center)
            .into()
    };
    if editor.removing.as_deref() != Some(target) {
        return button_row(
            secondary_button("Remove target", palette)
                .on_press_maybe((!locked).then(|| target_event(target, TargetEdit::AskRemove)))
                .into(),
        );
    }
    // Only what's saved has art or an entry on disk.
    let saved = editor.form.loaded.targets.get(target);
    let mut lines =
        column![text(format!("Remove {target} from {name}?")).color(palette.foreground)].spacing(4);
    if let Some(saved) = saved.filter(|saved| saved.desktop) {
        let entry = if saved.name.is_empty() {
            target
        } else {
            saved.name.as_str()
        };
        lines = lines.push(
            text(format!("Its desktop entry \"{entry}\" goes too."))
                .size(13)
                .color(palette.muted),
        );
    }
    if let Some(art) = editor
        .info
        .target_art
        .get(target)
        .filter(|_| saved.is_some())
    {
        lines = lines.push(
            text(format!("Its art in {art} goes too."))
                .size(13)
                .color(palette.muted),
        );
    }
    lines = lines.push(
        text("The program's files and the prefix are left alone.")
            .size(13)
            .color(palette.muted),
    );
    let actions = row![
        Space::new().width(Fill),
        secondary_button("Keep it", palette).on_press(send(Event::KeepTarget)),
        action_button(
            "Remove",
            palette.danger,
            !locked,
            target_event(target, TargetEdit::Remove),
        ),
    ]
    .spacing(8)
    .align_y(Center);
    container(lines.push(actions).spacing(8))
        .padding(12)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(6).width(1).color(palette.danger))
        })
        .into()
}

fn add_target(
    editor: &Editor,
    locked: bool,
    resolving: bool,
    palette: Palette,
) -> Element<'_, Message> {
    if !editor.add_open {
        return secondary_button("+ Add target", palette)
            .on_press(send(Event::AddOpen(true)))
            .into();
    }
    let muted =
        |line: String| -> Element<'_, Message> { text(line).size(13).color(palette.muted).into() };
    let mut rows = column![
        text("Add target")
            .font(semibold())
            .color(palette.foreground)
    ]
    .spacing(10);
    match &editor.scan {
        _ if editor.info.native => {
            rows = rows.push(muted(
                "A native environment has no prefix, so enter a path or a name on PATH.".to_owned(),
            ));
        }
        None | Some(Scan::Scanning) => {
            rows = rows.push(muted("Looking for installed programs...".to_owned()));
        }
        Some(Scan::NoPrefix) => {
            rows = rows.push(muted(
                "The prefix doesn't exist yet, so enter a path.".to_owned(),
            ));
        }
        Some(Scan::Failed(error)) => {
            rows = rows.push(text(error.as_str()).size(13).color(palette.danger));
        }
        Some(Scan::Programs {
            programs,
            held_back,
            unreadable,
        }) => {
            for notice in unreadable {
                rows = rows.push(muted(notice.clone()));
            }
            let mut shown: Vec<_> = programs.iter().collect();
            if editor.show_held_back {
                shown.extend(held_back);
            }
            if shown.is_empty() {
                rows = rows.push(muted(
                    "Nothing installed was found, so enter a path.".to_owned(),
                ));
            }
            for program in shown {
                rows = rows.push(
                    row![
                        text(program.name.as_str())
                            .color(palette.foreground)
                            .width(LABEL_WIDTH),
                        text(program.exe.as_str())
                            .size(13)
                            .color(palette.muted)
                            .width(Fill),
                        secondary_button("Pick", palette).on_press_maybe(
                            (!locked).then(|| send(Event::ProgramPicked(program.clone())))
                        ),
                    ]
                    .spacing(12)
                    .align_y(Center),
                );
            }
            if !held_back.is_empty() {
                let label = if editor.show_held_back {
                    "Hide the ones held back".to_owned()
                } else {
                    format!(
                        "Show {} held back as noise, like uninstallers",
                        held_back.len()
                    )
                };
                rows = rows.push(
                    secondary_button(label, palette)
                        .on_press(send(Event::ShowHeldBack(!editor.show_held_back))),
                );
            }
        }
    }
    let new = &editor.new_target;
    let can_add = !locked && !resolving && editor.can_add_target();
    let path_hint = if editor.info.native {
        "a path, or a name on PATH"
    } else {
        "a Unix path or a Windows one"
    };
    rows = rows.push(setting_row(
        "Name",
        text_input("suggested from the exe", &new.name)
            .on_input_maybe((!locked).then_some(|name| send(Event::NewTargetName(name))))
            .on_submit_maybe(can_add.then(|| send(Event::TargetAdd)))
            .width(Fill)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status))
            .into(),
        None,
        palette,
    ));
    if let Some(problem) = editor.form.new_target_problem(&new.name) {
        rows = rows.push(text(problem).size(12).color(palette.danger));
    }
    rows = rows.push(setting_row(
        "Exe",
        text_input(path_hint, &new.path)
            .on_input_maybe((!locked).then_some(|path| send(Event::NewTargetPath(path))))
            .on_submit_maybe(can_add.then(|| send(Event::TargetAdd)))
            .width(Fill)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status))
            .into(),
        Some(
            secondary_button("Browse...", palette)
                .on_press_maybe((!locked).then(|| send(Event::BrowsePath(None, false))))
                .into(),
        ),
        palette,
    ));
    if let Some(error) = &editor.add_error {
        rows = rows.push(text(error.as_str()).size(12).color(palette.danger));
    }
    if let Some(name) = editor.duplicate_target() {
        rows = rows.push(
            text(format!("Already used by target `{name}`."))
                .size(12)
                .color(palette.muted),
        );
    }
    let checking = if resolving { "Checking..." } else { "" };
    rows = rows.push(
        row![
            text(checking).size(13).color(palette.muted),
            Space::new().width(Fill),
            secondary_button("Cancel", palette).on_press(send(Event::AddOpen(false))),
            action_button("Add", palette.primary, can_add, send(Event::TargetAdd)),
        ]
        .spacing(8)
        .align_y(Center),
    );
    container(rows)
        .padding(12)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.background)
                .border(border::rounded(6).width(1).color(palette.border))
        })
        .into()
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
        let is_set = selected.0.is_some();
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
            .wineo_style(palette)
            .into()
        };
        let refused = editor.build_refusal(name, component).is_some();
        let note: Option<Element<'_, Message>> = if refused && is_set {
            // Core refuses every save while this is set, so removing it is
            // the one thing left to do with it.
            Some(
                secondary_button("Remove", palette)
                    .on_press_maybe((!locked).then(|| send(Event::BuildRemoved(component))))
                    .into(),
            )
        } else {
            (component == Component::Vkd3d && !editor.form.vkd3d_enabled())
                .then(|| text("needs DXVK").size(13).color(palette.muted).into())
        };
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
            List::Winetricks => "a package, like corefonts or dotnet48",
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

fn env_rows<'a>(
    env: &'a [form::EnvRow],
    new: &'a form::EnvRow,
    problems: Vec<Option<String>>,
    new_problem: Option<String>,
    locked: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let mut rows = column![].spacing(8);
    let field = |placeholder: &str, value: &str, width: iced::Length| {
        text_input(placeholder, value)
            .width(width)
            .padding([6, 10])
            .style(move |_, status| input_style(palette, status))
    };
    for (index, (row, problem)) in env.iter().zip(problems).enumerate() {
        let line = row![
            field("NAME", &row.key, ENV_KEY_WIDTH.into())
                .on_input_maybe((!locked).then_some(move |key| send(Event::EnvKey(index, key)))),
            field("value", &row.value, Fill).on_input_maybe(
                (!locked).then_some(move |value| send(Event::EnvValue(index, value)))
            ),
            secondary_button("x", palette)
                .on_press_maybe((!locked).then(|| send(Event::EnvRemove(index)))),
        ]
        .spacing(8)
        .align_y(Center);
        rows = rows.push(line);
        if let Some(problem) = problem {
            rows = rows.push(text(problem).size(12).color(palette.danger));
        }
    }
    let problem = (!new.key.is_empty() || !new.value.is_empty())
        .then_some(new_problem)
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
            secondary_button("+", palette).on_press_maybe(can_add.then(|| send(Event::EnvAdd))),
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
        if let Some(problem) = editor.form.prefix_problem() {
            rows = rows.push(text(problem).size(12).color(palette.danger));
        }
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

/// A gamescope field edit, mapped to the page event for the environment or
/// a target.
enum FieldEdit {
    Text(Field, String),
    Choice(Field, Choice),
    GrabCursor(bool),
    ArgsInput(String),
    ArgsAdd,
    ArgsRemove(usize),
}

fn gamescope_fields<'a>(
    fields: &'a GamescopeFields,
    args_input: &'a str,
    locked: bool,
    palette: Palette,
    on: impl Fn(FieldEdit) -> Event + Clone + 'a,
) -> Element<'a, Message> {
    let mut rows = Column::new().spacing(10);
    for field in Field::ALL {
        let on = on.clone();
        let control: Element<'_, Message> = match field {
            Field::Filter | Field::Scaler | Field::Window if locked => {
                locked_choice(&fields.choice(field).to_string(), palette)
            }
            Field::Filter | Field::Scaler | Field::Window => {
                let options: Vec<Choice> = std::iter::once(Choice(None))
                    .chain(field.choices().into_iter().map(|value| Choice(Some(value))))
                    .collect();
                pick_list(options, Some(fields.choice(field)), move |choice| {
                    send(on(FieldEdit::Choice(field, choice)))
                })
                .on_open(send(Event::MenuOpened))
                .on_close(send(Event::MenuClosed))
                .width(CONTROL_WIDTH)
                .wineo_style(palette)
                .into()
            }
            Field::GrabCursor => toggler(fields.grab_cursor)
                .on_toggle_maybe(
                    (!locked).then_some(move |on_: bool| send(on(FieldEdit::GrabCursor(on_)))),
                )
                .into(),
            Field::Args => {
                let add = send(on(FieldEdit::ArgsAdd));
                let input = on.clone();
                Chips {
                    items: &fields.args,
                    input: args_input,
                    placeholder: "one flag or value",
                    error: None,
                    on_input: Box::new(move |value| send(input(FieldEdit::ArgsInput(value)))),
                    on_add: add,
                    on_remove: Box::new(move |index| send(on(FieldEdit::ArgsRemove(index)))),
                    enabled: !locked,
                }
                .view(palette)
            }
            _ => text_input("unset", fields.text(field))
                .on_input_maybe(
                    (!locked).then_some(move |value| send(on(FieldEdit::Text(field, value)))),
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
        section_heading(title, palette),
        grouped_section(content, palette),
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
        line = line
            .push(secondary_button("Reset", palette).on_press_maybe((!locked).then_some(reset)));
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
                secondary_button("Keep editing", palette).on_press(send(Event::KeepEditing)),
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
