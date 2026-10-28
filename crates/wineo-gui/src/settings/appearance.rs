//! The global page's Appearance tab: which theme shows, whether it follows
//! the system, and Improve readability. Its edits preview at once.

use iced::widget::{Space, column, container, row, text};
use iced::{Center, Element, Fill, border};

use super::Event;
use crate::Message;
use crate::library_menu::{CHECK, check_box, icon};
use crate::theme::{self, Appearance, PRESETS, Palette, PresetKind};
use crate::ui::{ListRow, list_group, list_row, list_row_text, section_heading};

pub(crate) const FOLLOW_ID: &str = "appearance-follow-system";
pub(crate) const READABILITY_ID: &str = "appearance-readability";

/// One change to the appearance draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    FollowSystem(bool),
    /// The id of the theme for the slot.
    Theme(Slot, String),
    ImproveReadability(bool),
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
    fn kind(self) -> Option<PresetKind> {
        match self {
            Self::Dark => Some(PresetKind::Dark),
            Self::Light => Some(PresetKind::Light),
            Self::Theme => None,
        }
    }
    fn id(self, appearance: &Appearance) -> &str {
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

/// The saved appearance and the draft the page previews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Form {
    pub(crate) loaded: Appearance,
    pub(crate) draft: Appearance,
}

impl Form {
    pub(crate) fn new(appearance: Appearance) -> Self {
        Self {
            loaded: appearance.clone(),
            draft: appearance,
        }
    }
    pub(crate) fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::FollowSystem(on) => self.draft.follow_system = on,
            Edit::Theme(slot, id) => *slot.id_mut(&mut self.draft) = id,
            Edit::ImproveReadability(on) => self.draft.improve_readability = on,
        }
    }
    /// One per setting that differs from the saved one, the way
    /// `library::set_appearance` writes them.
    pub(crate) fn changes(&self) -> usize {
        let (before, after) = (&self.loaded, &self.draft);
        [
            before.follow_system != after.follow_system,
            before.dark != after.dark,
            before.light != after.light,
            before.theme != after.theme,
            before.improve_readability != after.improve_readability,
        ]
        .into_iter()
        .filter(|changed| *changed)
        .count()
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.draft != self.loaded
    }
    pub(crate) fn revert(&mut self) {
        self.draft = self.loaded.clone();
    }
    /// Edits stay against the new base; without any, the draft follows the
    /// file, so a hand edit shows.
    pub(crate) fn rebase(&mut self, appearance: Appearance) {
        if !self.is_dirty() {
            self.draft = appearance.clone();
        }
        self.loaded = appearance;
    }
    /// What Save writes, when there is anything to write.
    pub(crate) fn edits(&self) -> Option<Appearance> {
        self.is_dirty().then(|| self.draft.clone())
    }
}

/// A theme the lists offer. Built-in presets only for now; custom themes
/// join them later.
#[derive(Debug, Clone)]
pub(crate) struct Choice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: PresetKind,
    pub(crate) description: String,
    pub(crate) palette: Palette,
}

pub(crate) fn choices() -> Vec<Choice> {
    PRESETS
        .iter()
        .map(|preset| Choice {
            id: preset.id.to_owned(),
            name: preset.name.to_owned(),
            kind: preset.kind,
            description: preset.description.to_owned(),
            palette: preset.palette,
        })
        .collect()
}

fn send(edit: Edit) -> Message {
    Message::Settings(Event::Appearance(edit))
}

pub(super) fn tab(form: &Form, locked: bool, palette: Palette) -> Element<'static, Message> {
    let draft = &form.draft;
    let choices = choices();
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
                theme_list(*slot, &choices, draft, locked, palette),
            ]
            .spacing(8),
        );
    }
    content
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
    draft: &Appearance,
    locked: bool,
    palette: Palette,
) -> Element<'static, Message> {
    // The row that shows as chosen is the theme the app falls back to for an
    // unknown id, so the list never disagrees with what's painted.
    let chosen = theme::shown(slot.id(draft), slot.kind()).id;
    let rows: Vec<_> = choices
        .iter()
        .filter(|choice| slot.kind().is_none_or(|kind| choice.kind == kind))
        .map(|choice| {
            theme_row(
                slot,
                choice,
                choice.id == chosen,
                slot == Slot::Theme,
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
    locked: bool,
    palette: Palette,
) -> ListRow<'static, Message> {
    let mut head = row![list_row_text(choice.name.clone()).color(if chosen {
        palette.primary
    } else {
        palette.foreground
    })]
    .spacing(8)
    .align_y(Center);
    if show_kind {
        let kind = match choice.kind {
            PresetKind::Dark => "Dark",
            PresetKind::Light => "Light",
        };
        head = head.push(text(kind).size(12).color(palette.muted));
    }
    let words = column![
        head,
        text(choice.description.clone())
            .size(12)
            .color(palette.muted)
    ]
    .spacing(2)
    .width(Fill);
    // A check as well as the color, so the choice doesn't rest on color alone.
    let mark: Element<'static, Message> = if chosen {
        icon(CHECK, palette.primary, 16.0).into()
    } else {
        Space::new().width(16).height(16).into()
    };
    let content = row![swatch(choice.palette), words, mark]
        .spacing(12)
        .align_y(Center);
    ListRow::new(
        slot.row_id(&choice.id),
        list_row(content, palette),
        send(Edit::Theme(slot, choice.id.clone())),
    )
    .enabled(!locked)
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
