//! The `[input]` table: the controller switch and which button does what.
//! A bad value counts as unset for that key only, like the rest of
//! `library.toml`.

use std::collections::BTreeMap;

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use super::{edit, store_value, table_mut};
use crate::error::Result;
use crate::paths::Paths;

/// What a controller button can do. The declaration order is the order
/// conflicts resolve in, and a stored format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PadAction {
    Confirm,
    Back,
    Favorite,
    Mark,
    PageLeft,
    PageRight,
    First,
    Last,
    Layout,
    Fullscreen,
    Random,
}

impl PadAction {
    pub const ALL: [Self; 11] = [
        Self::Confirm,
        Self::Back,
        Self::Favorite,
        Self::Mark,
        Self::PageLeft,
        Self::PageRight,
        Self::First,
        Self::Last,
        Self::Layout,
        Self::Fullscreen,
        Self::Random,
    ];

    /// The key `[input.buttons]` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::Back => "back",
            Self::Favorite => "favorite",
            Self::Mark => "mark",
            Self::PageLeft => "page_left",
            Self::PageRight => "page_right",
            Self::First => "first",
            Self::Last => "last",
            Self::Layout => "layout",
            Self::Fullscreen => "fullscreen",
            Self::Random => "random",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == value)
    }

    /// The button the action starts on.
    pub fn default_button(self) -> PadButton {
        match self {
            Self::Confirm => PadButton::A,
            Self::Back => PadButton::B,
            Self::Favorite => PadButton::Y,
            Self::Mark => PadButton::X,
            Self::PageLeft => PadButton::Lb,
            Self::PageRight => PadButton::Rb,
            Self::First => PadButton::Lt,
            Self::Last => PadButton::Rt,
            Self::Layout => PadButton::View,
            Self::Fullscreen => PadButton::Start,
            Self::Random => PadButton::Ls,
        }
    }
}

/// A controller button by its printed label in Xbox terms, whatever pad it
/// is on: `A` is the button printed A, which sits east on a Nintendo pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PadButton {
    A,
    B,
    X,
    Y,
    Lb,
    Rb,
    Lt,
    Rt,
    View,
    Start,
    Ls,
    Rs,
}

impl PadButton {
    pub const ALL: [Self; 12] = [
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::Lb,
        Self::Rb,
        Self::Lt,
        Self::Rt,
        Self::View,
        Self::Start,
        Self::Ls,
        Self::Rs,
    ];

    /// The value `[input.buttons]` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::X => "x",
            Self::Y => "y",
            Self::Lb => "lb",
            Self::Rb => "rb",
            Self::Lt => "lt",
            Self::Rt => "rt",
            Self::View => "view",
            Self::Start => "start",
            Self::Ls => "ls",
            Self::Rs => "rs",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|button| button.as_str() == value)
    }
}

/// How the desktop app reads a game controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// Whether pad input drives the library.
    pub controller: bool,
    /// The remaps the file holds, as written: an unknown action or button
    /// isn't here, and a remap onto the action's own default is.
    /// `bindings` is what they come to.
    pub buttons: BTreeMap<PadAction, PadButton>,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            controller: true,
            buttons: BTreeMap::new(),
        }
    }
}

/// An action that couldn't have the button it asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    pub action: PadAction,
    pub wanted: PadButton,
    /// The earlier action that already has `wanted`.
    pub taken_by: PadAction,
    /// What the action got instead: its default, or nothing when that's
    /// taken too.
    pub button: Option<PadButton>,
}

/// The button each action ends up on, once conflicts are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    buttons: [Option<PadButton>; PadAction::ALL.len()],
}

impl Bindings {
    pub fn button(&self, action: PadAction) -> Option<PadButton> {
        self.buttons[action as usize]
    }

    pub fn action(&self, button: PadButton) -> Option<PadAction> {
        PadAction::ALL
            .into_iter()
            .find(|action| self.button(*action) == Some(button))
    }
}

impl Input {
    pub fn bindings(&self) -> Bindings {
        self.resolve().0
    }

    /// The actions that lost their button, in action order.
    pub fn conflicts(&self) -> Vec<Conflict> {
        self.resolve().1
    }

    /// Goes through the actions in order. One whose button is taken falls
    /// back to its default if that's free, and is left unbound otherwise.
    /// That covers a hand edit that gives two actions one button, and a
    /// newer wineo whose new action starts on a button the user remapped.
    fn resolve(&self) -> (Bindings, Vec<Conflict>) {
        let mut buttons = [None; PadAction::ALL.len()];
        let mut conflicts = Vec::new();
        let holder = |buttons: &[Option<PadButton>], button| {
            PadAction::ALL
                .into_iter()
                .find(|action| buttons[*action as usize] == Some(button))
        };
        for action in PadAction::ALL {
            let wanted = self
                .buttons
                .get(&action)
                .copied()
                .unwrap_or_else(|| action.default_button());
            let Some(taken_by) = holder(&buttons, wanted) else {
                buttons[action as usize] = Some(wanted);
                continue;
            };
            let fallback = action.default_button();
            let button = (holder(&buttons, fallback).is_none()).then_some(fallback);
            buttons[action as usize] = button;
            conflicts.push(Conflict {
                action,
                wanted,
                taken_by,
                button,
            });
        }
        (Bindings { buttons }, conflicts)
    }
}

pub(super) fn read_input(document: &DocumentMut) -> Input {
    let default = Input::default();
    let Some(table) = document.get("input").and_then(Item::as_table_like) else {
        return default;
    };
    Input {
        controller: table
            .get("controller")
            .and_then(Item::as_bool)
            .unwrap_or(default.controller),
        buttons: read_buttons(table.get("buttons")),
    }
}

/// Each key on its own: an unknown action or button costs only itself.
fn read_buttons(item: Option<&Item>) -> BTreeMap<PadAction, PadButton> {
    let Some(table) = item.and_then(Item::as_table_like) else {
        return BTreeMap::new();
    };
    table
        .iter()
        .filter_map(|(name, value)| {
            Some((
                PadAction::parse(name)?,
                value.as_str().and_then(PadButton::parse)?,
            ))
        })
        .collect()
}

/// Writes only what differs from what the file reads as, so an unchanged
/// `Input` never rewrites it. A remap onto the action's own button is the
/// same as none, which keeps `[input.buttons]` to the changed actions, and
/// an action put back removes its key.
pub fn set_input(paths: &Paths, input: &Input) -> Result<bool> {
    edit(paths, |document| {
        let mut changed = false;
        if read_input(document).controller != input.controller {
            store_value(
                input_table_mut(document),
                "controller",
                Value::from(input.controller),
            );
            changed = true;
        }
        changed |= set_buttons(document, &input.buttons);
        changed
    })
}

fn set_buttons(document: &mut DocumentMut, wanted: &BTreeMap<PadAction, PadButton>) -> bool {
    let have = read_buttons(
        document
            .get("input")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("buttons")),
    );
    let changed_only = |action: PadAction, button: Option<&PadButton>| {
        button
            .copied()
            .filter(|button| *button != action.default_button())
    };
    let mut changed = false;
    for action in PadAction::ALL {
        let wanted = changed_only(action, wanted.get(&action));
        if wanted == changed_only(action, have.get(&action)) {
            continue;
        }
        match wanted {
            Some(button) => store_value(
                buttons_table_mut(document),
                action.as_str(),
                Value::from(button.as_str()),
            ),
            None => {
                buttons_table_mut(document).remove(action.as_str());
            }
        }
        changed = true;
    }
    if changed {
        // Nothing left to say, so the header goes too.
        let input = input_table_mut(document);
        if input
            .get("buttons")
            .and_then(Item::as_table_like)
            .is_some_and(|buttons| buttons.is_empty())
        {
            input.remove("buttons");
        }
    }
    changed
}

fn input_table_mut(document: &mut DocumentMut) -> &mut dyn TableLike {
    table_mut(document.as_table_mut(), "input")
}

fn buttons_table_mut(document: &mut DocumentMut) -> &mut dyn TableLike {
    let input = input_table_mut(document);
    if input
        .get("buttons")
        .is_none_or(|item| !item.is_table_like())
    {
        input.insert("buttons", Item::Table(Table::new()));
    }
    input
        .get_mut("buttons")
        .and_then(Item::as_table_like_mut)
        .expect("buttons was just made a table")
}
