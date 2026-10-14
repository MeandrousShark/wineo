//! The filesystem facts behind the "can't change this prefix" reasons.
//!
//! Reasons also depend on in-memory state that changes on any message, so they
//! are always computed live. Only these two facts touch the disk: whether an
//! interrupted restore left a marker, and where a prefix really points once
//! symlinks are resolved. `view` reads them from `Stored`, which is refreshed
//! when the library loads and when a menu, dialog, or page opens, so a redraw
//! never reaches the filesystem. Handlers that act read them `Live`.
//!
//! Refreshing is one rule, not a call in every handler that opens something:
//! `App::update` compares `screen_key` before and after each message and
//! refreshes once when it changed. The library merge refreshes too.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use wineo_core::paths::{Paths, resolve};

use crate::library::Card;

impl crate::App {
    /// A hash of what the user is looking at that can show a reason from the
    /// stored facts: the page, the selected card and whether its panel is up,
    /// the open context menu, remove dialog, and tool page, the snapshot and
    /// portable steps (so a review or confirmation arriving counts), and the
    /// Checkup scope, phase and rollback confirmation target, and whether a
    /// log has replaced the controls. Phases are discrete (loading, working,
    /// finished), never progress values. It hashes ids and
    /// discriminants only, so it is cheap to take on every message.
    pub(crate) fn screen_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        std::mem::discriminant(&self.section).hash(&mut hasher);
        self.selected.hash(&mut hasher);
        self.panel_open.hash(&mut hasher);
        self.action_menu.hash(&mut hasher);
        self.removal
            .as_ref()
            .map(|dialog| &dialog.name)
            .hash(&mut hasher);
        self.tool_page.hash(&mut hasher);
        self.portable_page.hash(&mut hasher);
        self.snapshots_page
            .as_ref()
            .map(crate::snapshots_page::Page::screen)
            .hash(&mut hasher);
        self.portable_page
            .as_ref()
            .and_then(|name| self.portable_pages.get(name))
            .map(crate::portable_page::Page::screen)
            .hash(&mut hasher);
        self.art_page.hash(&mut hasher);
        self.settings.is_some().hash(&mut hasher);
        self.flow.is_some().hash(&mut hasher);
        self.checkup_screen().hash(&mut hasher);
        self.log_view.hash(&mut hasher);
        self.checkup_log_scope.hash(&mut hasher);
        self.tool_page
            .as_ref()
            .and_then(|key| self.tool_pages.get(key))
            .map(crate::tools_page::Page::busy)
            .hash(&mut hasher);
        hasher.finish()
    }
}

pub(crate) trait Facts {
    /// `prefix` with symlinks resolved, None when the source can't say.
    fn resolved(&self, prefix: &Path) -> Option<Cow<'_, Path>>;
    fn restore_pending(&self, name: &str) -> bool;
}

pub(crate) struct Live<'a>(pub(crate) &'a Paths);

impl Facts for Live<'_> {
    fn resolved(&self, prefix: &Path) -> Option<Cow<'_, Path>> {
        Some(Cow::Owned(resolve(prefix)))
    }

    fn restore_pending(&self, name: &str) -> bool {
        wineo_core::snapshot::check_recovery_marker(self.0, name).is_err()
    }
}

#[derive(Default)]
pub(crate) struct Stored {
    restore_pending: HashSet<String>,
    // Keyed by the card's own prefix path, so a card whose prefix changed
    // since the last refresh matches nothing instead of the old target.
    resolved: HashMap<PathBuf, PathBuf>,
}

impl Stored {
    pub(crate) fn read(paths: &Paths, cards: &[Card]) -> Self {
        let live = Live(paths);
        Self {
            restore_pending: cards
                .iter()
                .filter(|card| live.restore_pending(&card.name))
                .map(|card| card.name.clone())
                .collect(),
            resolved: cards
                .iter()
                .filter_map(|card| card.prefix.as_deref())
                .map(|prefix| (prefix.to_owned(), resolve(prefix)))
                .collect(),
        }
    }
}

impl Facts for Stored {
    fn resolved(&self, prefix: &Path) -> Option<Cow<'_, Path>> {
        self.resolved
            .get(prefix)
            .map(|path| Cow::Borrowed(path.as_path()))
    }

    fn restore_pending(&self, name: &str) -> bool {
        self.restore_pending.contains(name)
    }
}

#[cfg(test)]
mod tests;
