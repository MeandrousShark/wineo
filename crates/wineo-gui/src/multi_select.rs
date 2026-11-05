//! Marking several cards at once: Ctrl+click, Shift+click, Space, and
//! Ctrl+A, cleared by Esc or a plain click. Marks only ever hold shown
//! games, last for the session, and are never saved.

use std::collections::{BTreeSet, HashSet};

use iced::widget::container;
use iced::{Element, Fill, Task, border};

use crate::library_menu::{CHECK, icon};
use crate::theme::Palette;
use crate::{App, Message, ui};

/// How strongly a marked card's art is washed in the primary color.
pub(crate) const WASH: f32 = 0.25;
const CHECK_CHIP: f32 = 32.0;
const CHECK_ICON: f32 = 20.0;

/// Where a Shift+click range starts and what the last one added.
#[derive(Debug, Default)]
pub(crate) struct Range {
    /// The last card marked by Ctrl+click or Space.
    anchor: Option<String>,
    /// The marks the last Shift+click added, which the next one replaces,
    /// as file managers do. Ctrl+click, Space, and Ctrl+A keep them.
    added: BTreeSet<String>,
}

impl App {
    /// A press on a card, or `Message::Select` from anywhere: Ctrl toggles
    /// its mark and Shift marks a range, both leaving the selection and
    /// the panel alone. A plain press clears the marks and selects.
    pub(crate) fn card_pressed(&mut self, name: String) -> Task<Message> {
        let held = self.modifiers;
        if held.shift() {
            self.mark_range(&name, held.control());
            Task::none()
        } else if held.control() {
            self.toggle_mark(&name);
            Task::none()
        } else {
            self.clear_marks();
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
            self.toggle_mark(&name);
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
        if !self.shown_names().contains(name) {
            return;
        }
        self.mark_range.added.clear();
        if self.marked.remove(name) {
            if self.mark_range.anchor.as_deref() == Some(name) {
                self.mark_range.anchor = None;
            }
        } else {
            self.marked.insert(name.to_owned());
            self.mark_range.anchor = Some(name.to_owned());
        }
    }

    /// Marks from the anchor to `name` in shown order. The anchor is the
    /// last card marked by Ctrl+click or Space, else the selected card,
    /// else `name` itself. With Ctrl held too, the last range stays.
    fn mark_range(&mut self, name: &str, keep_last: bool) {
        let shown: Vec<String> = self
            .filtered()
            .iter()
            .map(|card| card.name.clone())
            .collect();
        let Some(to) = shown.iter().position(|shown| shown == name) else {
            return;
        };
        let position = |anchor: &Option<String>| {
            anchor
                .as_ref()
                .and_then(|anchor| shown.iter().position(|shown| shown == anchor))
        };
        let from = match position(&self.mark_range.anchor).or_else(|| position(&self.selected)) {
            Some(from) => from,
            None => {
                self.mark_range.anchor = Some(name.to_owned());
                to
            }
        };
        let added = std::mem::take(&mut self.mark_range.added);
        if !keep_last {
            for name in &added {
                self.marked.remove(name);
            }
        }
        for name in &shown[from.min(to)..=from.max(to)] {
            if self.marked.insert(name.clone()) {
                self.mark_range.added.insert(name.clone());
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
    /// a game drops its mark and nothing else does.
    pub(crate) fn prune_marks(&mut self) {
        if self.marked.is_empty() && self.mark_range.anchor.is_none() {
            return;
        }
        let shown: HashSet<String> = self
            .filtered()
            .iter()
            .map(|card| card.name.clone())
            .collect();
        self.marked.retain(|name| shown.contains(name));
        self.mark_range.added.retain(|name| shown.contains(name));
        if self
            .mark_range
            .anchor
            .as_ref()
            .is_some_and(|anchor| !shown.contains(anchor))
        {
            self.mark_range.anchor = None;
        }
    }

    fn shown_names(&self) -> BTreeSet<String> {
        self.filtered()
            .iter()
            .map(|card| card.name.clone())
            .collect()
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

/// The check centered on a marked card, clear of the four corners.
pub(crate) fn check(palette: Palette, alpha: f32) -> Element<'static, Message> {
    let fill = palette.primary;
    // The renderer's tint ignores its alpha, so the icon fades by opacity.
    let chip = container(icon(CHECK, ui::accent_ink(fill), CHECK_ICON).opacity(alpha))
        .width(CHECK_CHIP)
        .height(CHECK_CHIP)
        .center(CHECK_CHIP)
        .style(move |_| {
            container::Style::default()
                .background(fill.scale_alpha(alpha))
                .border(border::rounded(CHECK_CHIP / 2.0))
        });
    container(chip).center(Fill).into()
}

#[cfg(test)]
mod tests;
