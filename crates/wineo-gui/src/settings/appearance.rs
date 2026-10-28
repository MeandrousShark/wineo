//! The global page's Appearance tab: which theme shows, whether it follows
//! the system, Improve readability, and custom themes. Its edits preview at
//! once.

use std::collections::BTreeMap;

use iced::theme::Mode;
use iced::widget::{Space, column, container, row, text};
use iced::{Center, Element, Fill, border};
use wineo_core::themes::{ROLES, normalize_color};

use super::Event;
use crate::Message;
use crate::library_menu::{CHECK, check_box, icon};
use crate::theme::{self, Appearance, Custom, PRESETS, Palette, PresetKind, Shown};
use crate::ui::{ListRow, list_group, list_row, list_row_text, section_heading};

pub(crate) const FOLLOW_ID: &str = "appearance-follow-system";
pub(crate) const READABILITY_ID: &str = "appearance-readability";

/// One change to the appearance draft, or to how the Colors section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    FollowSystem(bool),
    /// The id of the theme for the slot.
    Theme(Slot, String),
    ImproveReadability(bool),
    /// Copies the theme the slot shows to a new custom theme, which the slot
    /// then shows.
    Copy(Slot),
    /// A custom theme's id, a role's index in `ROLES`, and the typed hex.
    Color(String, usize, String),
    /// Opens the name field for a custom theme's id.
    RenameOpen(String),
    RenameText(String),
    RenameApply,
    RenameCancel,
    /// Asks before deleting a custom theme's id.
    AskDelete(String),
    KeepTheme,
    Delete(String),
}

/// Which theme setting a list picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    Dark,
    Light,
    /// The one theme used when not following the system.
    Theme,
}

impl Slot {
    pub(crate) fn list(self) -> &'static str {
        match self {
            Self::Dark => "appearance-dark",
            Self::Light => "appearance-light",
            Self::Theme => "appearance-theme",
        }
    }
    fn heading(self) -> &'static str {
        match self {
            Self::Dark => "Dark theme",
            Self::Light => "Light theme",
            Self::Theme => "Theme",
        }
    }
    pub(crate) fn kind(self) -> Option<PresetKind> {
        match self {
            Self::Dark => Some(PresetKind::Dark),
            Self::Light => Some(PresetKind::Light),
            Self::Theme => None,
        }
    }
    pub(crate) fn id(self, appearance: &Appearance) -> &str {
        match self {
            Self::Dark => &appearance.dark,
            Self::Light => &appearance.light,
            Self::Theme => &appearance.theme,
        }
    }
    fn id_mut(self, appearance: &mut Appearance) -> &mut String {
        match self {
            Self::Dark => &mut appearance.dark,
            Self::Light => &mut appearance.light,
            Self::Theme => &mut appearance.theme,
        }
    }
    /// The focus id of the row for theme `id` in this slot's list.
    pub(crate) fn row_id(self, id: &str) -> String {
        format!("{}-{id}", self.list())
    }
}

/// The saved appearance and custom themes, and the drafts the page previews.
/// Theme edits are the difference between the two lists, so a save that
/// lands partway leaves exactly what didn't land as edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Form {
    pub(crate) loaded: Appearance,
    pub(crate) draft: Appearance,
    pub(crate) loaded_themes: Vec<Custom>,
    pub(crate) themes: Vec<Custom>,
    /// Hex text that isn't a color, by theme id and role, so the field keeps
    /// what was typed while the theme keeps its last good color.
    inputs: BTreeMap<(String, usize), String>,
    pub(crate) renaming: Option<Rename>,
    pub(crate) deleting: Option<String>,
    /// Numbers the temporary ids of copies not saved yet.
    copies: u32,
}

/// The open name field of a custom theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rename {
    pub(crate) id: String,
    pub(crate) text: String,
    /// The last Rename was refused for a name another theme has.
    pub(crate) taken: bool,
}

/// What Save writes for the Appearance tab, in the order it writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToSave {
    /// New themes, each still under its temporary id.
    pub(crate) creates: Vec<Custom>,
    /// Themes whose colors changed.
    pub(crate) updates: Vec<Custom>,
    /// File ids and new names.
    pub(crate) renames: Vec<(String, String)>,
    /// `[appearance]`, when it changed. It may name a new theme by its
    /// temporary id, which the save maps to the real one first.
    pub(crate) appearance: Option<Appearance>,
    /// File ids.
    pub(crate) deletes: Vec<String>,
}

/// A copy's id until Save creates its file. Core refuses a `/` in an id, so
/// one that was never mapped can't name a file.
const NEW_PREFIX: &str = "custom:new/";

pub(crate) fn is_new(id: &str) -> bool {
    id.starts_with(NEW_PREFIX)
}

pub(crate) const NAME_TAKEN: &str = "Name taken.";

impl Form {
    pub(crate) fn new(appearance: Appearance, themes: Vec<Custom>) -> Self {
        Self {
            loaded: appearance.clone(),
            draft: appearance,
            loaded_themes: themes.clone(),
            themes,
            inputs: BTreeMap::new(),
            renaming: None,
            deleting: None,
            copies: 0,
        }
    }
    /// The draft appearance and themes, which a load compares to tell edits
    /// made while it ran.
    pub(crate) fn drafts(&self) -> (Appearance, Vec<Custom>) {
        (self.draft.clone(), self.themes.clone())
    }
    /// `mode` is the system's, which decides the theme showing while
    /// following it.
    pub(crate) fn apply(&mut self, edit: Edit, mode: Mode) {
        match edit {
            Edit::FollowSystem(on) => {
                // Pending with the maintainer: the single list starts on the
                // theme showing, so turning following off changes nothing on
                // screen.
                if self.draft.follow_system && !on {
                    let showing = self.showing_slot(mode);
                    self.draft.theme = self.shown(showing).id().to_owned();
                }
                self.draft.follow_system = on;
            }
            Edit::Theme(slot, id) => *slot.id_mut(&mut self.draft) = id,
            Edit::ImproveReadability(on) => self.draft.improve_readability = on,
            Edit::Copy(slot) => self.copy(slot),
            Edit::Color(id, role, text) => self.set_color(&id, role, text),
            Edit::RenameOpen(id) => {
                if let Some(theme) = self.editable(&id) {
                    self.renaming = Some(Rename {
                        text: theme.name.clone(),
                        id,
                        taken: false,
                    });
                }
            }
            Edit::RenameText(text) => {
                if let Some(rename) = &mut self.renaming {
                    rename.text = text;
                    rename.taken = false;
                }
            }
            Edit::RenameApply => self.rename(),
            Edit::RenameCancel => self.renaming = None,
            Edit::AskDelete(id) => {
                if self.editable(&id).is_some() {
                    self.deleting = Some(id);
                }
            }
            Edit::KeepTheme => self.deleting = None,
            Edit::Delete(id) => {
                if self.deleting.as_deref() == Some(id.as_str()) {
                    self.delete(&id);
                }
            }
        }
    }
    /// The slot whose theme the app shows under `mode`.
    pub(crate) fn showing_slot(&self, mode: Mode) -> Slot {
        if !self.draft.follow_system {
            Slot::Theme
        } else if mode == Mode::Light {
            Slot::Light
        } else {
            Slot::Dark
        }
    }
    pub(crate) fn shown(&self, slot: Slot) -> Shown<'_> {
        theme::shown(slot.id(&self.draft), slot.kind(), &self.themes)
    }
    /// The draft's custom theme `id`, unless it's read-only.
    fn editable(&self, id: &str) -> Option<&Custom> {
        self.themes
            .iter()
            .find(|theme| theme.id == id && !theme.read_only)
    }
    fn editable_mut(&mut self, id: &str) -> Option<&mut Custom> {
        self.themes
            .iter_mut()
            .find(|theme| theme.id == id && !theme.read_only)
    }
    /// Whether a custom theme other than `id` is called `name`, ignoring case.
    fn name_taken(&self, name: &str, id: Option<&str>) -> bool {
        let name = name.to_lowercase();
        self.themes
            .iter()
            .any(|theme| Some(theme.id.as_str()) != id && theme.name.to_lowercase() == name)
    }
    fn copy(&mut self, slot: Slot) {
        let source = self.shown(slot);
        let based_on = match source {
            Shown::Preset(preset) => Some(preset.id.to_owned()),
            Shown::Custom(custom) => custom.based_on.clone(),
        };
        let base = format!("{} copy", source.name());
        let mut name = base.clone();
        let mut attempt = 1;
        while self.name_taken(&name, None)
            || PRESETS
                .iter()
                .any(|preset| preset.name.to_lowercase() == name.to_lowercase())
        {
            attempt += 1;
            name = format!("{base} {attempt}");
        }
        let theme = Custom {
            id: String::new(),
            name,
            kind: source.kind(),
            based_on,
            colors: theme::colors_of(source.palette()),
            read_only: false,
        };
        self.copies += 1;
        let id = format!("{NEW_PREFIX}{}", self.copies);
        *slot.id_mut(&mut self.draft) = id.clone();
        self.themes.push(Custom { id, ..theme });
        self.renaming = None;
        self.deleting = None;
    }
    fn set_color(&mut self, id: &str, role: usize, text: String) {
        if role >= ROLES.len() {
            return;
        }
        let key = (id.to_owned(), role);
        let Some(theme) = self.editable_mut(id) else {
            return;
        };
        match normalize_color(&text) {
            Some(color) => {
                *theme::color_mut(&mut theme.colors, role) = Some(color);
                self.inputs.remove(&key);
            }
            None => {
                self.inputs.insert(key, text);
            }
        }
    }
    fn rename(&mut self) {
        let Some(rename) = &self.renaming else {
            return;
        };
        let (id, name) = (rename.id.clone(), rename.text.trim().to_owned());
        if name.is_empty() {
            return;
        }
        if self.name_taken(&name, Some(&id)) {
            if let Some(rename) = &mut self.renaming {
                rename.taken = true;
            }
            return;
        }
        if let Some(theme) = self.editable_mut(&id) {
            theme.name = name;
        }
        self.renaming = None;
    }
    fn delete(&mut self, id: &str) {
        let Some(index) = self.themes.iter().position(|theme| theme.id == id) else {
            return;
        };
        let theme = self.themes.remove(index);
        for slot in [Slot::Dark, Slot::Light, Slot::Theme] {
            let kind = slot.kind().or(Some(theme.kind));
            let current = slot.id_mut(&mut self.draft);
            if *current == id {
                *current = theme::default_preset(kind).id.to_owned();
            }
        }
        self.inputs.retain(|(theme, _), _| theme != id);
        if self.renaming.as_ref().is_some_and(|rename| rename.id == id) {
            self.renaming = None;
        }
        self.deleting = None;
    }
    /// The text a role's field shows: what was typed when it isn't a color,
    /// otherwise the color the theme has, filled from its base.
    pub(crate) fn color_text(&self, theme: &Custom, role: usize) -> String {
        self.inputs
            .get(&(theme.id.clone(), role))
            .cloned()
            .unwrap_or_else(|| theme::hex(theme.palette().role(role)))
    }
    pub(crate) fn color_invalid(&self, id: &str, role: usize) -> bool {
        self.inputs.contains_key(&(id.to_owned(), role))
    }
    /// One per setting that differs from the saved one, the way
    /// `library::set_appearance` writes them, plus one per new or deleted
    /// theme, rename, and changed color.
    pub(crate) fn changes(&self) -> usize {
        let (before, after) = (&self.loaded, &self.draft);
        let settings = [
            before.follow_system != after.follow_system,
            before.dark != after.dark,
            before.light != after.light,
            before.theme != after.theme,
            before.improve_readability != after.improve_readability,
        ]
        .into_iter()
        .filter(|changed| *changed)
        .count();
        let mut themes = 0;
        for theme in &self.themes {
            match self.loaded_theme(&theme.id) {
                None => themes += 1,
                Some(saved) => {
                    themes += usize::from(saved.name != theme.name);
                    themes +=
                        usize::from(saved.kind != theme.kind || saved.based_on != theme.based_on);
                    themes += saved
                        .colors
                        .roles()
                        .iter()
                        .zip(theme.colors.roles())
                        .filter(|(saved, draft)| saved.1 != draft.1)
                        .count();
                }
            }
        }
        themes += self
            .loaded_themes
            .iter()
            .filter(|saved| !self.themes.iter().any(|theme| theme.id == saved.id))
            .count();
        settings + themes
    }
    fn loaded_theme(&self, id: &str) -> Option<&Custom> {
        self.loaded_themes.iter().find(|theme| theme.id == id)
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.changes() > 0
    }
    pub(crate) fn revert(&mut self) {
        self.draft = self.loaded.clone();
        self.themes = self.loaded_themes.clone();
        self.clear_inputs();
    }
    fn clear_inputs(&mut self) {
        self.inputs.clear();
        self.renaming = None;
        self.deleting = None;
    }
    /// Edits stay against the new base; without any, the draft follows the
    /// files, so a hand edit shows.
    pub(crate) fn rebase(&mut self, appearance: Appearance, themes: Vec<Custom>) {
        if self.is_dirty() {
            // A theme whose file went away has nothing left to edit, and
            // one that appeared was never the draft's to delete.
            let before = std::mem::take(&mut self.loaded_themes);
            self.themes.retain(|theme| {
                is_new(&theme.id)
                    || themes.iter().any(|saved| saved.id == theme.id)
                    || !before.iter().any(|saved| saved.id == theme.id)
            });
            for saved in &themes {
                let known = before.iter().any(|theme| theme.id == saved.id)
                    || self.themes.iter().any(|theme| theme.id == saved.id);
                if !known {
                    self.themes.push(saved.clone());
                }
            }
        } else {
            self.draft = appearance.clone();
            self.themes = themes.clone();
            self.clear_inputs();
        }
        self.loaded = appearance;
        self.loaded_themes = themes;
    }
    /// Gives each copy a save created its real id, wherever the draft uses
    /// the temporary one.
    pub(crate) fn map_created(&mut self, created: &[(String, String)]) {
        for (temporary, real) in created {
            for theme in &mut self.themes {
                if theme.id == *temporary {
                    theme.id.clone_from(real);
                }
            }
            for slot in [Slot::Dark, Slot::Light, Slot::Theme] {
                let id = slot.id_mut(&mut self.draft);
                if id == temporary {
                    id.clone_from(real);
                }
            }
            self.inputs = std::mem::take(&mut self.inputs)
                .into_iter()
                .map(|((id, role), text)| {
                    let id = if id == *temporary { real.clone() } else { id };
                    ((id, role), text)
                })
                .collect();
            if let Some(rename) = &mut self.renaming
                && rename.id == *temporary
            {
                rename.id.clone_from(real);
            }
            if self.deleting.as_ref() == Some(temporary) {
                self.deleting = Some(real.clone());
            }
        }
    }
    /// What Save writes, when there is anything to write.
    pub(crate) fn edits(&self) -> Option<ToSave> {
        if !self.is_dirty() {
            return None;
        }
        let mut save = ToSave {
            creates: Vec::new(),
            updates: Vec::new(),
            renames: Vec::new(),
            appearance: (self.draft != self.loaded).then(|| self.draft.clone()),
            deletes: Vec::new(),
        };
        for theme in &self.themes {
            match self.loaded_theme(&theme.id) {
                None if is_new(&theme.id) => save.creates.push(theme.clone()),
                // Its file went away under the draft, which a reload drops.
                None => {}
                Some(saved) => {
                    if saved.colors != theme.colors
                        || saved.kind != theme.kind
                        || saved.based_on != theme.based_on
                    {
                        save.updates.push(theme.clone());
                    }
                    if saved.name != theme.name {
                        save.renames
                            .push((theme.file_id().to_owned(), theme.name.clone()));
                    }
                }
            }
        }
        for saved in &self.loaded_themes {
            if !self.themes.iter().any(|theme| theme.id == saved.id) {
                save.deletes.push(saved.file_id().to_owned());
            }
        }
        Some(save)
    }
}

/// A theme the lists offer: the built-in presets, then the custom themes.
#[derive(Debug, Clone)]
pub(crate) struct Choice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: PresetKind,
    pub(crate) description: String,
    pub(crate) palette: Palette,
}

pub(crate) fn choices(customs: &[Custom]) -> Vec<Choice> {
    let presets = PRESETS.iter().map(|preset| Choice {
        id: preset.id.to_owned(),
        name: preset.name.to_owned(),
        kind: preset.kind,
        description: preset.description.to_owned(),
        palette: preset.palette,
    });
    // In `themes::list`'s order, which a rename or a copy changes before
    // the files are read again.
    let mut customs: Vec<&Custom> = customs.iter().collect();
    customs.sort_by_cached_key(|custom| (custom.name.to_lowercase(), custom.id.clone()));
    let customs = customs.into_iter().map(|custom| Choice {
        id: custom.id.clone(),
        name: custom.name.clone(),
        kind: custom.kind,
        description: format!("Custom, based on {}.", custom.base().name),
        palette: custom.palette(),
    });
    presets.chain(customs).collect()
}

/// Pending with the maintainer: swatches show the colors the app would
/// paint, so with Improve readability on they show the adjusted palette.
pub(crate) fn swatch_palette(palette: Palette, kind: PresetKind, improve: bool) -> Palette {
    if improve {
        theme::improve(palette, kind)
    } else {
        palette
    }
}

pub(super) fn send(edit: Edit) -> Message {
    Message::Settings(Event::Appearance(edit))
}

pub(super) fn tab(
    form: &Form,
    locked: bool,
    palette: Palette,
    mode: Mode,
) -> Element<'static, Message> {
    let draft = &form.draft;
    let choices = choices(&form.themes);
    let mut content = column![check_row(
        FOLLOW_ID,
        "Follow the system",
        None,
        draft.follow_system,
        Edit::FollowSystem(!draft.follow_system),
        locked,
        palette,
    )]
    .spacing(24)
    .width(Fill);
    let slots: &[Slot] = if draft.follow_system {
        &[Slot::Dark, Slot::Light]
    } else {
        &[Slot::Theme]
    };
    for slot in slots {
        content = content.push(
            column![
                section_heading(slot.heading(), palette),
                theme_list(*slot, &choices, form, locked, palette),
            ]
            .spacing(8),
        );
    }
    content
        .push(super::theme_colors::section(form, mode, locked, palette))
        .push(check_row(
            READABILITY_ID,
            "Improve readability",
            Some("Raises contrast in themes that need it."),
            draft.improve_readability,
            Edit::ImproveReadability(!draft.improve_readability),
            locked,
            palette,
        ))
        .into()
}

/// A check row in a grouped surface of its own, so it takes focus and Enter
/// or Space like the choice rows.
fn check_row(
    id: &'static str,
    label: &'static str,
    line: Option<&'static str>,
    on: bool,
    edit: Edit,
    locked: bool,
    palette: Palette,
) -> Element<'static, Message> {
    let mut content = column![
        row![check_box(on, palette), list_row_text(label)]
            .spacing(8)
            .align_y(Center)
    ]
    .spacing(2);
    if let Some(line) = line {
        // Lined up with the label, past the box and its gap.
        content = content.push(container(text(line).size(12).color(palette.muted)).padding(
            iced::Padding {
                left: 24.0,
                ..iced::Padding::ZERO
            },
        ));
    }
    list_group(
        id,
        [ListRow::new(id, list_row(content, palette), send(edit)).enabled(!locked)],
        palette,
    )
}

fn theme_list(
    slot: Slot,
    choices: &[Choice],
    form: &Form,
    locked: bool,
    palette: Palette,
) -> Element<'static, Message> {
    // The row that shows as chosen is the theme the app falls back to for an
    // unknown id, so the list never disagrees with what's painted.
    let chosen = form.shown(slot).id().to_owned();
    let improve = form.draft.improve_readability;
    let rows: Vec<_> = choices
        .iter()
        .filter(|choice| slot.kind().is_none_or(|kind| choice.kind == kind))
        .map(|choice| {
            theme_row(
                slot,
                choice,
                choice.id == chosen,
                slot == Slot::Theme,
                improve,
                locked,
                palette,
            )
        })
        .collect();
    list_group(slot.list(), rows, palette)
}

fn theme_row(
    slot: Slot,
    choice: &Choice,
    chosen: bool,
    show_kind: bool,
    improve: bool,
    locked: bool,
    palette: Palette,
) -> ListRow<'static, Message> {
    let colors = RowColors::new(chosen, !locked, palette);
    let mut head = row![list_row_text(choice.name.clone()).color(colors.name)]
        .spacing(8)
        .align_y(Center);
    if show_kind {
        let kind = match choice.kind {
            PresetKind::Dark => "Dark",
            PresetKind::Light => "Light",
        };
        head = head.push(text(kind).size(12).color(colors.detail));
    }
    let words = column![
        head,
        text(choice.description.clone())
            .size(12)
            .color(colors.detail)
    ]
    .spacing(2)
    .width(Fill);
    // A check as well as the color, so the choice doesn't rest on color alone.
    let mark: Element<'static, Message> = if chosen {
        icon(CHECK, colors.name, 16.0).into()
    } else {
        Space::new().width(16).height(16).into()
    };
    let content = row![
        swatch(swatch_palette(choice.palette, choice.kind, improve)),
        words,
        mark
    ]
    .spacing(12)
    .align_y(Center);
    ListRow::new(
        slot.row_id(&choice.id),
        list_row(content, palette),
        send(Edit::Theme(slot, choice.id.clone())),
    )
    .enabled(!locked)
}

/// A theme row's text colors. The labels are colored here, so they'd skip
/// `list_row_style`'s disabled text; a locked row takes it the same way, and
/// its supporting text fades below that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RowColors {
    /// The name and the chosen row's check.
    pub(crate) name: iced::Color,
    /// The description and the Dark or Light label.
    pub(crate) detail: iced::Color,
}

impl RowColors {
    pub(crate) fn new(chosen: bool, enabled: bool, palette: Palette) -> Self {
        if !enabled {
            return Self {
                name: palette.muted,
                detail: palette.muted.scale_alpha(0.6),
            };
        }
        Self {
            name: if chosen {
                palette.primary
            } else {
                palette.foreground
            },
            detail: palette.muted,
        }
    }
}

/// A small sample of a theme: its ground, a panel in its surface color with
/// an accent chip, and text in its foreground.
fn swatch(colors: Palette) -> Element<'static, Message> {
    let chip = container(Space::new())
        .width(10)
        .height(10)
        .style(move |_| {
            container::Style::default()
                .background(colors.primary)
                .border(border::rounded(3))
        });
    let panel = container(chip).padding(4).style(move |_| {
        container::Style::default()
            .background(colors.surface)
            .border(border::rounded(4).width(1).color(colors.border))
    });
    container(
        row![panel, text("Aa").size(12).color(colors.foreground)]
            .spacing(6)
            .align_y(Center),
    )
    .padding([4, 6])
    .style(move |_| {
        container::Style::default()
            .background(colors.background)
            .border(border::rounded(6).width(1).color(colors.border))
    })
    .into()
}
