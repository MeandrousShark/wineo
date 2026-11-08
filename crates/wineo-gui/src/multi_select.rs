//! Marking several cards at once: Ctrl+click, Shift+click, Space, and
//! Ctrl+A, cleared by Esc or a plain click. Marks only ever hold shown
//! games, last for the session, and are never saved.

use std::collections::BTreeSet;

use iced::widget::container;
use iced::{Element, Fill, Task, border};

use crate::app_selection::cursor;
use crate::library_menu::{CHECK, icon};
use crate::slot::SlotId;
use crate::theme::Palette;
use crate::{App, Message, ui};

/// How strongly a marked card's art is washed in the primary color.
pub(crate) const WASH: f32 = 0.25;
/// The poster's check is a corner mark like the status chip, so it takes
/// their size and icon size.
const CHECK_CHIP: f32 = crate::library_actions::MARK_SIZE;
const CHECK_ICON: f32 = 14.0;

/// Where a Shift+click range starts and what the last one added.
#[derive(Debug, Default)]
pub(crate) struct Range {
    /// The last card marked by Ctrl+click or Space.
    anchor: Option<String>,
    /// The anchor's copy that was clicked, or the cursor's for Space, so a
    /// range starts there rather than at another copy of the same game.
    anchor_slot: Option<SlotId>,
    /// The marks the last Shift+click added, which the next one replaces,
    /// as file managers do. Ctrl+click, Space, and Ctrl+A keep them.
    added: BTreeSet<String>,
}

impl App {
    /// A press on `name`'s card in `slot`, or `Message::Select` from
    /// anywhere with no slot: Ctrl toggles its mark and Shift marks a
    /// range, both leaving the selection and the panel alone. A plain press
    /// clears the marks, puts the cursor on the slot, and selects.
    pub(crate) fn card_pressed(&mut self, name: String, slot: Option<SlotId>) -> Task<Message> {
        let held = self.modifiers;
        if held.shift() {
            self.mark_range(&name, slot.as_ref(), held.control());
            Task::none()
        } else if held.control() {
            self.toggle_mark_at(&name, slot);
            Task::none()
        } else {
            self.clear_marks();
            self.header_cursor = None;
            if let Some(slot) = slot {
                // Another copy of the open panel's game only moves the
                // cursor, rather than closing the panel as its own card
                // does.
                let elsewhere = self.panel_open
                    && self.selected.as_deref() == Some(&name)
                    && self.selected_slot().is_some_and(|cursor| cursor != slot);
                self.active_slot = Some(slot);
                if elsewhere {
                    return Task::none();
                }
            }
            self.select_card(name)
        }
    }

    /// Space's answer to whether a control has keyboard focus. A focused
    /// control keeps Space for itself.
    pub(crate) fn space_answered(&mut self, focused: bool) {
        // Anything could have opened while the answer was on its way.
        if focused || !self.marking_keys_free() || !self.selected_shown() {
            return;
        }
        if let Some(name) = self.selected.clone() {
            let slot = self.selected_slot();
            self.toggle_mark_at(&name, slot);
        }
    }

    /// Ctrl+A and Space mark only on the Library's grid, with nothing
    /// else taking the keys: the same scope as F.
    pub(crate) fn marking_keys_free(&self) -> bool {
        self.section == crate::sidebar::Section::Library
            && self.settings.is_none()
            && !self.feature_page_open()
            && !self.input_covered()
            && !self.library_menu_shown()
            && self.action_menu.is_none()
            && !self.version_visible()
            && !(self.checkup_visible() && !self.panel_log_visible())
    }

    pub(crate) fn toggle_mark(&mut self, name: &str) {
        self.toggle_mark_at(name, None);
    }

    /// Toggles `name`'s mark from its copy in `slot`, which a range then
    /// starts from.
    fn toggle_mark_at(&mut self, name: &str, slot: Option<SlotId>) {
        if !self.shown_names().contains(name) {
            return;
        }
        self.mark_range.added.clear();
        if self.marked.remove(name) {
            if self.mark_range.anchor.as_deref() == Some(name) {
                self.mark_range.anchor = None;
                self.mark_range.anchor_slot = None;
            }
        } else {
            self.marked.insert(name.to_owned());
            self.mark_range.anchor = Some(name.to_owned());
            self.mark_range.anchor_slot = slot;
        }
    }

    /// Marks from the anchor to `name`'s card in `slot`, or the copy the
    /// cursor is on or its first without one, in shown order. The anchor is
    /// the copy last marked by Ctrl+click or Space, else the cursor's card,
    /// else `name` itself. With Ctrl held too, the last range stays.
    fn mark_range(&mut self, name: &str, slot: Option<&SlotId>, keep_last: bool) {
        let places = self.grid_places();
        let active = self.active_slot.as_ref();
        let to = match slot {
            Some(slot) => places.index(slot),
            None => cursor(&places, Some(name), active),
        };
        let Some(to) = to else {
            return;
        };
        // An anchor whose copy is no longer shown, or that came with none,
        // falls back to the copy the cursor is on when it's the cursor's
        // game, else its first.
        let anchor = self.mark_range.anchor.as_deref();
        let from = self
            .mark_range
            .anchor_slot
            .as_ref()
            .filter(|slot| Some(slot.name()) == anchor)
            .and_then(|slot| places.index(slot))
            .or_else(|| cursor(&places, anchor, active))
            .or_else(|| self.selected_index(&places));
        let from = match from {
            Some(from) => from,
            None => {
                self.mark_range.anchor = Some(name.to_owned());
                self.mark_range.anchor_slot = Some(places.ids[to].clone());
                to
            }
        };
        let added = std::mem::take(&mut self.mark_range.added);
        if !keep_last {
            for name in &added {
                self.marked.remove(name);
            }
        }
        for slot in &places.ids[from.min(to)..=from.max(to)] {
            if self.marked.insert(slot.name().to_owned()) {
                self.mark_range.added.insert(slot.name().to_owned());
            }
        }
    }

    pub(crate) fn mark_all(&mut self) {
        self.marked = self.shown_names();
        self.mark_range.added.clear();
    }

    pub(crate) fn clear_marks(&mut self) {
        self.marked.clear();
        self.mark_range = Range::default();
    }

    /// Drops the marks on games no longer shown. Every change to the cards
    /// or the order ends here, so a filter, search, or removal that hides
    /// a game drops its mark, as does collapsing every section it's in
    /// (plan 0062, question 22), and nothing else does.
    pub(crate) fn prune_marks(&mut self) {
        if self.marked.is_empty() && self.mark_range.anchor.is_none() {
            return;
        }
        let shown = self.visible_names();
        self.marked.retain(|name| shown.contains(name));
        self.mark_range.added.retain(|name| shown.contains(name));
        if self
            .mark_range
            .anchor
            .as_ref()
            .is_some_and(|anchor| !shown.contains(anchor))
        {
            self.mark_range.anchor = None;
            self.mark_range.anchor_slot = None;
        }
    }

    /// The games Ctrl+A and a mark can reach: those with a copy in view.
    fn shown_names(&self) -> BTreeSet<String> {
        self.visible_names()
    }
}

/// The primary wash over a marked card's art, under its corner layers.
pub(crate) fn wash(palette: Palette, alpha: f32) -> Element<'static, Message> {
    let color = palette.primary.scale_alpha(WASH * alpha);
    container(iced::widget::Space::new().width(Fill).height(Fill))
        .width(Fill)
        .height(Fill)
        .style(move |_| {
            container::Style::default()
                .background(color)
                .border(border::rounded(4))
        })
        .into()
}

/// The check chip for a marked card's top-left corner, where the grid
/// lays it out before the status chip and the GOG pill.
pub(crate) fn check(palette: Palette, alpha: f32) -> Element<'static, Message> {
    let fill = palette.primary;
    // The renderer's tint ignores its alpha, so the icon fades by opacity.
    container(icon(CHECK, ui::accent_ink(fill), CHECK_ICON).opacity(alpha))
        .width(CHECK_CHIP)
        .height(CHECK_CHIP)
        .center(CHECK_CHIP)
        .style(move |_| {
            container::Style::default()
                .background(fill.scale_alpha(alpha))
                .border(border::rounded(CHECK_CHIP / 2.0))
        })
        .into()
}

#[cfg(test)]
mod tests;
