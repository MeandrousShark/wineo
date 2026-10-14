//! The filesystem facts behind the "can't change this prefix" reasons.
//!
//! Reasons also depend on in-memory state that changes on any message, so they
//! are always computed live. Only these two facts touch the disk: whether an
//! interrupted restore left a marker, and where a prefix really points once
//! symlinks are resolved. `view` reads them from `Stored`, which is refreshed
//! when the library loads and when a menu, dialog, or page opens, so a redraw
//! never reaches the filesystem. Handlers that act read them `Live`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use wineo_core::paths::{Paths, resolve};

use crate::library::Card;

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
mod tests {
    use super::*;
    use crate::tests::{app, card};
    use crate::{App, Message, rescan, tools_page};
    use wineo_core::runtime::State;

    fn booted(root: &Path) -> App {
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap();
        let mut app = App::boot(paths).0;
        app.refreshing = false;
        app
    }

    fn mark(app: &App, name: &str) {
        let marker = app.paths.restore_marker(name);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, "marker").unwrap();
    }

    fn unmark(app: &App, name: &str) {
        std::fs::remove_file(app.paths.restore_marker(name)).unwrap();
    }

    const PENDING: Option<&str> = Some("Roll back restore first.");

    #[test]
    fn view_reasons_follow_the_stored_facts_not_the_disk() {
        let root = tempfile::tempdir().unwrap();
        let mut app = booted(root.path());
        app.cards = vec![card("notes", State::Inactive, 0)];
        app.refresh_facts();
        mark(&app, "notes");

        assert_eq!(app.remove_reason("notes").as_deref(), PENDING);
        assert_eq!(app.remove_reason_with(&app.facts, "notes"), None);
        assert!(!app.wine_actions_with(&app.facts, "notes"));

        let _ = app.update(Message::Context("notes".into()));
        assert_eq!(
            app.remove_reason_with(&app.facts, "notes").as_deref(),
            PENDING
        );
        assert!(app.wine_actions_with(&app.facts, "notes"));

        unmark(&app, "notes");
        assert_eq!(
            app.remove_reason_with(&app.facts, "notes").as_deref(),
            PENDING
        );
        assert_eq!(app.remove_reason("notes"), None);

        // Opening the dialog reads the disk again.
        let _ = app.update(Message::Context("notes".into()));
        let _ = app.open_remove("notes".into());
        assert_eq!(app.remove_reason_with(&app.facts, "notes"), None);
    }

    #[test]
    fn the_stored_reason_text_still_follows_live_state() {
        let root = tempfile::tempdir().unwrap();
        let mut app = booted(root.path());
        app.cards = vec![card("notes", State::Inactive, 0)];
        app.refresh_facts();
        assert_eq!(app.remove_reason_with(&app.facts, "notes"), None);

        // Only the disk facts are stored, so a game starting shows at once.
        app.cards[0].state = State::Running;
        assert_eq!(
            app.remove_reason_with(&app.facts, "notes").as_deref(),
            Some("Stop it first")
        );
    }

    #[test]
    fn a_rescan_that_reports_a_change_refreshes_the_facts() {
        let root = tempfile::tempdir().unwrap();
        let mut app = booted(root.path());
        let notes = card("notes", State::Inactive, 0);
        app.cards = vec![notes.clone()];
        app.refresh_facts();
        mark(&app, "notes");
        assert_eq!(app.remove_reason_with(&app.facts, "notes"), None);

        let mut changed = notes;
        changed.playtime = "1h".into();
        let scan = rescan::Scan {
            revisions: app.shown.revisions(),
            library: Some(Ok(vec![changed])),
            ..rescan::Scan::default()
        };
        let _ = app.update(Message::Rescanned(Box::new(scan)));
        assert_eq!(
            app.remove_reason_with(&app.facts, "notes").as_deref(),
            PENDING
        );
    }

    #[test]
    fn opening_a_tool_page_refreshes_the_facts() {
        let root = tempfile::tempdir().unwrap();
        let mut app = booted(root.path());
        app.cards = vec![card("notes", State::Inactive, 0)];
        app.refresh_facts();
        mark(&app, "notes");
        assert_eq!(app.remove_reason_with(&app.facts, "notes"), None);

        let _ = app.update(Message::OpenTool("notes".into(), tools_page::Kind::Exec));
        assert_eq!(
            app.tool_start_reason_with(&app.facts, "notes", tools_page::Kind::Exec)
                .as_deref(),
            PENDING
        );
    }

    #[test]
    fn shared_prefixes_are_matched_through_stored_symlink_targets() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let other = root.path().join("other");
        let link = root.path().join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut app = booted(root.path());
        app.cards = [("a", real.clone()), ("b", link.clone())]
            .into_iter()
            .map(|(name, prefix)| {
                let mut card = card(name, State::Inactive, 0);
                card.prefix = Some(prefix);
                card
            })
            .collect();
        app.refresh_facts();
        assert!(app.same_prefix_with(&app.facts, "a", "b"));

        // Retargeting the link is not seen until the facts are refreshed.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(app.same_prefix_with(&app.facts, "a", "b"));
        assert!(!app.same_prefix_with(&Live(&app.paths), "a", "b"));
        app.refresh_facts();
        assert!(!app.same_prefix_with(&app.facts, "a", "b"));

        // A prefix changed since the refresh matches nothing.
        app.cards[1].prefix = Some(root.path().join("new"));
        assert!(!app.same_prefix_with(&app.facts, "a", "b"));
    }

    #[test]
    fn selecting_a_card_refreshes_the_facts() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let other = root.path().join("other");
        let link = root.path().join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut app = booted(root.path());
        app.cards = [("a", real), ("b", link.clone())]
            .into_iter()
            .map(|(name, prefix)| {
                let mut card = card(name, State::Inactive, 0);
                card.prefix = Some(prefix);
                card
            })
            .collect();
        app.refresh_facts();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(app.same_prefix_with(&app.facts, "a", "b"));

        let _ = app.update(Message::Select("b".into()));
        assert!(!app.same_prefix_with(&app.facts, "a", "b"));
    }

    #[test]
    fn opening_checkup_or_its_recovery_confirmation_refreshes_the_facts() {
        use crate::checkup::{Event, Scope};
        use wineo_core::validate::Fix;

        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let other = root.path().join("other");
        let link = root.path().join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mut app = booted(root.path());
        app.cards = [("a", real), ("b", link.clone())]
            .into_iter()
            .map(|(name, prefix)| {
                let mut card = card(name, State::Inactive, 0);
                card.prefix = Some(prefix);
                card
            })
            .collect();
        let recover = Fix::RecoverSnapshot {
            environment: "b".into(),
        };
        let _ = app.update(Message::OpenTool("a".into(), tools_page::Kind::Winetricks));
        let _ = app.update(Message::Tool(
            "a".into(),
            tools_page::Kind::Winetricks,
            tools_page::Event::OpenWinetricks,
        ));
        app.section = crate::sidebar::Section::Checkup;
        let retarget = |to: &Path| {
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(to, &link).unwrap();
        };
        let busy = |app: &App| app.tool_prefix_busy_with(&app.facts, "b");

        retarget(&root.path().join("real"));
        app.refresh_facts();
        assert!(busy(&app));

        // Reopening the global page sees the retarget with no card change.
        retarget(&other);
        assert!(busy(&app));
        let _ = app.update(Message::Checkup(Event::Open(Scope::Global)));
        assert!(!busy(&app));

        retarget(&root.path().join("real"));
        let _ = app.update(Message::Checkup(Event::Open(Scope::Environment(
            "b".into(),
        ))));
        assert!(busy(&app));

        retarget(&other);
        let _ = app.update(Message::Checkup(Event::ReviewRecovery(
            Scope::Global,
            recover,
        )));
        assert!(!busy(&app));
    }

    #[test]
    fn a_default_app_has_no_stored_facts() {
        let app = app();
        assert!(!app.facts.restore_pending("anything"));
        assert!(app.facts.resolved(Path::new("/nowhere")).is_none());
    }
}
