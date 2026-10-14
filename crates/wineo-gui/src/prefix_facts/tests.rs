use super::*;
use crate::tests::{app, card};
use crate::{App, Message, portable_page, rescan, tools_page};
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
    let _ = app.update(Message::Remove("notes".into()));
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

fn recovery_report(app: &mut App, environments: &[&str]) {
    use crate::checkup::{Event, Scope};
    use wineo_core::validate::{Code, Fix, Problem, Report};

    app.section = crate::sidebar::Section::Checkup;
    let _ = app.update(Message::Checkup(Event::Open(Scope::Global)));
    let generation = app.checkup_generation(&Scope::Global);
    let problems = environments
        .iter()
        .map(|environment| Problem {
            environment: Some((*environment).into()),
            template: None,
            stray: None,
            area: Code::RestoreInterrupted.area(),
            level: Code::RestoreInterrupted.level(),
            code: Code::RestoreInterrupted,
            message: "interrupted".into(),
            fix: Some(Fix::RecoverSnapshot {
                environment: (*environment).into(),
            }),
        })
        .collect();
    let report = Report {
        problems,
        ..Report::default()
    };
    let _ = app.update(Message::Checkup(Event::Checked(
        Scope::Global,
        generation,
        Ok(report),
    )));
}

fn review_recovery(app: &mut App, environment: &str) {
    use crate::checkup::{Event, Scope};
    use wineo_core::validate::Fix;

    let _ = app.update(Message::Checkup(Event::ReviewRecovery(
        Scope::Global,
        Fix::RecoverSnapshot {
            environment: environment.into(),
        },
    )));
}

type Step = fn(&mut App);

/// Every way a screen showing a reason can open. `prepare` reaches the
/// state just before it, then a prefix symlink is retargeted behind the
/// app's back and `open` is the one user-visible step.
fn openings() -> Vec<(&'static str, Step, Step)> {
    use crate::checkup::{Event as Checkup, Scope};
    use crate::sidebar::Section;
    use crate::snapshots_page::{Event as Snapshots, Review};
    use wineo_core::validate::Report;

    fn none(_: &mut App) {}
    vec![
        ("context menu", none, |app| {
            let _ = app.update(Message::Context("b".into()));
        }),
        ("remove dialog", none, |app| {
            let _ = app.update(Message::Remove("b".into()));
        }),
        ("select a card", none, |app| {
            let _ = app.update(Message::Select("b".into()));
        }),
        (
            "panel after Trash",
            |app| app.section = Section::Trash,
            |app| {
                let _ = app.update(Message::Section(Section::Library));
            },
        ),
        (
            "panel after Art",
            |app| app.art_page = Some("b".into()),
            |app| {
                let _ = app.update(Message::Section(Section::Trash));
                let _ = app.update(Message::Section(Section::Library));
            },
        ),
        (
            "tool page Back",
            |app| {
                let _ = app.update(Message::OpenTool("a".into(), tools_page::Kind::Winetricks));
            },
            |app| {
                let _ = app.update(Message::Tool(
                    "a".into(),
                    tools_page::Kind::Winetricks,
                    tools_page::Event::Back,
                ));
            },
        ),
        ("open a tool page", none, |app| {
            let _ = app.update(Message::OpenTool("b".into(), tools_page::Kind::Exec));
        }),
        ("portable page", none, |app| {
            let _ = app.update(Message::CopyPortable("b".into()));
        }),
        ("snapshots page", none, |app| {
            let _ = app.update(Message::SnapshotsEnvironment("b".into()));
        }),
        (
            "snapshot confirmation",
            |app| {
                let _ = app.update(Message::SnapshotsEnvironment("b".into()));
            },
            |app| {
                let _ = app.update(Message::Snapshots(Snapshots::DeleteReview("x".into())));
            },
        ),
        (
            "snapshot review arriving",
            |app| {
                let _ = app.update(Message::SnapshotsEnvironment("b".into()));
                let _ = app.update(Message::Snapshots(Snapshots::RestoreReview("x".into())));
            },
            |app| {
                let generation = app.snapshots_page.as_ref().unwrap().generation();
                let _ = app.update(Message::Snapshots(Snapshots::Reviewed(
                    generation,
                    Ok(Review::Recover),
                )));
            },
        ),
        (
            "portable review arriving",
            |app| {
                let _ = app.update(Message::CopyPortable("b".into()));
                for event in [
                    portable_page::Event::SourceLoaded(Ok(true)),
                    portable_page::Event::Folder("/tmp/portable-out".into()),
                    portable_page::Event::Review,
                ] {
                    let _ = app.update(Message::Portable("b".into(), event));
                }
            },
            |app| {
                let _ = app.update(Message::Portable(
                    "b".into(),
                    portable_page::Event::ReviewLoaded(1, Err("failed".into())),
                ));
            },
        ),
        ("global Checkup", none, |app| {
            let _ = app.update(Message::Section(Section::Checkup));
        }),
        ("environment Checkup", none, |app| {
            let _ = app.update(Message::Checkup(Checkup::Open(Scope::Environment(
                "b".into(),
            ))));
        }),
        (
            "rollback confirmation",
            |app| recovery_report(app, &["b"]),
            |app| {
                review_recovery(app, "b");
                assert!(app.checkup_recovery_open(), "the confirmation did not open");
            },
        ),
        (
            "rollback confirmation target",
            |app| {
                recovery_report(app, &["a", "b"]);
                review_recovery(app, "a");
            },
            |app| {
                review_recovery(app, "b");
                assert_eq!(app.checkup_screen().3, Some("b"));
            },
        ),
        (
            "panel log closing",
            |app| {
                app.selected = Some("b".into());
                app.panel_open = true;
                app.log_view = true;
            },
            |app| {
                let _ = app.update(Message::CloseLog);
            },
        ),
        (
            "Checkup log closing",
            |app| {
                app.section = Section::Checkup;
                app.log_view = true;
                app.checkup_log_scope = Some(Scope::Global);
            },
            |app| {
                let _ = app.update(Message::CloseLog);
            },
        ),
        (
            "Checkup report arriving",
            |app| {
                app.section = Section::Checkup;
                let _ = app.update(Message::Checkup(Checkup::Open(Scope::Global)));
            },
            |app| {
                let generation = app.checkup_generation(&Scope::Global);
                let _ = app.update(Message::Checkup(Checkup::Checked(
                    Scope::Global,
                    generation,
                    Ok(Report::default()),
                )));
            },
        ),
        (
            "snapshots loading",
            |app| {
                let _ = app.update(Message::SnapshotsEnvironment("b".into()));
            },
            |app| {
                let generation = app.snapshots_page.as_ref().unwrap().generation();
                let _ = app.update(Message::Snapshots(Snapshots::Loaded(
                    generation,
                    Err("unreadable".into()),
                )));
            },
        ),
        (
            "tool run finishing",
            |app| {
                let _ = app.update(Message::OpenTool("a".into(), tools_page::Kind::Winetricks));
                let _ = app.update(Message::Tool(
                    "a".into(),
                    tools_page::Kind::Winetricks,
                    tools_page::Event::OpenWinetricks,
                ));
                assert!(app.tool_pages.values().any(tools_page::Page::busy));
            },
            |app| {
                let key = ("a".to_owned(), tools_page::Kind::Winetricks);
                let generation = app.tool_pages[&key].generation();
                let _ = app.update(Message::Tool(
                    "a".into(),
                    tools_page::Kind::Winetricks,
                    tools_page::Event::Finished(generation, Ok(Some(0))),
                ));
            },
        ),
    ]
}

#[test]
fn every_opening_shows_facts_read_after_the_external_change() {
    for (name, prepare, open) in openings() {
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
        prepare(&mut app);
        // A running in the prefix B shares blocks B, until B moves away.
        app.cards[0].state = State::Running;
        app.refresh_facts();
        assert!(app.remove_reason_with(&app.facts, "b").is_some(), "{name}");

        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(app.remove_reason_with(&app.facts, "b").is_some(), "{name}");
        let before = app.screen_key();
        open(&mut app);
        assert_ne!(app.screen_key(), before, "{name} did not change the screen");
        assert!(app.remove_reason_with(&app.facts, "b").is_none(), "{name}");
    }
}

#[test]
fn a_default_app_has_no_stored_facts() {
    let app = app();
    assert!(!app.facts.restore_pending("anything"));
    assert!(app.facts.resolved(Path::new("/nowhere")).is_none());
}
