use super::*;
use crate::{removal, sidebar};
use iced::event::Status;
use iced::keyboard::{Key, Modifiers, key::Named};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use wineo_core::paths::Paths;
use wineo_core::trash;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    let mut app = App::boot(paths).0;
    app.refreshing = false;
    app.trash_refreshing = false;
    (root, app)
}

fn native(paths: &Paths, name: &str) {
    let config = paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
}

fn wine(paths: &Paths, name: &str) -> std::path::PathBuf {
    let prefix = paths.default_prefix_root().join(name);
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let config = paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        format!(
            "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
            prefix.display()
        ),
    )
    .unwrap();
    prefix
}

fn remove(app: &App, name: &str) -> trash::Entry {
    trash::remove(&app.paths, name, false).unwrap()
}

fn open_trash(app: &mut App) {
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    load_current(app);
}

fn load_current(app: &mut App) {
    let result = trash_work::read(&app.paths);
    let _ = app.update(Message::Trash(Event::Loaded(app.trash_generation, result)));
}

fn send(app: &mut App, event: Event) {
    let _ = app.update(Message::Trash(event));
}

fn set_removed_at(paths: &Paths, id: &str, at: &str) {
    let file = paths.trash_dir().join(id).join("manifest.toml");
    let manifest = fs::read_to_string(&file).unwrap();
    let rewritten = manifest
        .lines()
        .map(|line| {
            if line.starts_with("removed_at = ") {
                format!("removed_at = {at:?}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(file, rewritten).unwrap();
}

fn trash_metadata_identity(paths: &Paths) -> String {
    fs::create_dir_all(paths.trash_dir()).unwrap();
    let path = paths.trash_dir();
    let metadata = fs::symlink_metadata(&path).unwrap();
    let (mount, birth) = match rustix::fs::statx(
        rustix::fs::CWD,
        &path,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        rustix::fs::StatxFlags::MNT_ID | rustix::fs::StatxFlags::BTIME,
    ) {
        Ok(stat) => {
            let mount = (stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0)
                .then_some(stat.stx_mnt_id);
            let birth = (stat.stx_mask & rustix::fs::StatxFlags::BTIME.bits() != 0)
                .then_some((stat.stx_btime.tv_sec, stat.stx_btime.tv_nsec));
            (mount, birth)
        }
        Err(rustix::io::Errno::NOSYS) => (None, None),
        Err(error) => panic!("could not inspect trash storage identity: {error}"),
    };
    let mut fields = format!("device = {}\ninode = {}", metadata.dev(), metadata.ino());
    if let Some(mount) = mount {
        fields.push_str(&format!("\nmount = {mount}"));
    }
    if let Some((seconds, nanos)) = birth {
        fields.push_str(&format!("\nbirth = [{seconds}, {nanos}]"));
    }
    fields
}

fn make_pending(
    paths: &Paths,
    entry: &trash::Entry,
    phase: &str,
    intent: &str,
    metadata_identity: &str,
) {
    let file = paths.trash_dir().join(&entry.id).join("manifest.toml");
    let manifest = fs::read_to_string(&file)
        .unwrap()
        .replace("phase = \"trashed\"", &format!("phase = \"{phase}\""));
    fs::write(file, &manifest).unwrap();
    let nested = manifest
        .lines()
        .map(|line| {
            if let Some(header) = line.strip_prefix("[[") {
                format!("[[manifest.{header}\n")
            } else if let Some(header) = line.strip_prefix('[') {
                format!("[manifest.{header}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect::<String>();
    let root = paths.environments_dir().parent().unwrap().to_owned();
    let marker = format!(
        "version = 1\nintent = '{intent}'\nfinished = false\nroot = '{}'\n\n[metadata_identity]\n{metadata_identity}\n\n[manifest]\n{nested}",
        root.display(),
    );
    let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let filename = format!(
        "trash-{}-{:016x}.toml",
        entry.id,
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    );
    let dir = paths.prefix_locks_dir().join(".trash-pending");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(filename), marker).unwrap();
}

fn review(app: &mut App, selection: Selection) -> trash::PruneReview {
    send(app, Event::Review(selection.clone()));
    assert!(app.trash_page.reviewing);
    let result = trash_work::review_selection(&app.paths, &selection);
    let generation = app.trash_page.review_generation;
    send(app, Event::Reviewed(generation, result.clone()));
    result.unwrap()
}

fn start_restore(app: &mut App, entry: &trash::Entry) -> u64 {
    send(app, Event::Restore(entry.id.clone()));
    assert!(matches!(
        app.change,
        Some(removal::Change::Trash(Change::Restore { .. }))
    ));
    app.change_generation
}

fn finish_restore(app: &mut App, generation: u64, id: &str) -> Result<(), String> {
    let result = trash::restore(&app.paths, id)
        .map_err(|error| app.paths.contract_text(&format!("{error:#}")));
    send(app, Event::Restored(generation, result.clone()));
    result
}

#[test]
fn list_is_newest_first_and_uses_the_cli_relative_age_label() {
    let (_root, mut app) = fixture();
    native(&app.paths, "older");
    let older = remove(&app, "older");
    native(&app.paths, "newer");
    let newer = remove(&app, "newer");
    set_removed_at(&app.paths, &older.id, "2000-01-01T00:00:00Z");
    set_removed_at(&app.paths, &newer.id, "2020-01-01T00:00:00Z");

    open_trash(&mut app);
    let snapshot = app.trash_page.snapshot().unwrap();
    assert_eq!(snapshot.entries[0].id, newer.id);
    assert_eq!(snapshot.entries[1].id, older.id);
    let now = jiff::Timestamp::now();
    let age = wineo_core::time::relative(now, "2020-01-01T00:00:00Z".parse().unwrap());
    let label = entry_label(&snapshot.entries[0], now);
    assert_eq!(label, format!("newer   removed {age}"));
    assert!(entry_label(&snapshot.entries[1], now).contains("older   removed"));
}

#[test]
fn restore_success_and_collisions_are_reported_on_the_row() {
    let (_root, mut app) = fixture();
    native(&app.paths, "restored");
    let restored = remove(&app, "restored");
    native(&app.paths, "collision");
    let collision = remove(&app, "collision");
    open_trash(&mut app);

    let generation = start_restore(&mut app, &restored);
    assert_eq!(finish_restore(&mut app, generation, &restored.id), Ok(()));
    assert!(app.change.is_none());
    assert!(
        app.trash_page
            .notice
            .as_deref()
            .unwrap()
            .contains("restored")
    );
    load_current(&mut app);
    assert!(
        !app.trash_page
            .snapshot()
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.id == restored.id)
    );

    native(&app.paths, "collision");
    let generation = start_restore(&mut app, &collision);
    let error = finish_restore(&mut app, generation, &collision.id).unwrap_err();
    assert!(error.contains("already exists") || error.contains("collision"));
    assert_eq!(app.trash_page.row_errors.get(&collision.id), Some(&error));
}

#[test]
fn restore_refuses_a_replaced_prefix_and_plain_remove_notes_what_stays() {
    let (_root, mut app) = fixture();
    let prefix = wine(&app.paths, "wine-game");
    let entry = remove(&app, "wine-game");
    assert_eq!(entry.prefix_left.as_deref(), Some(prefix.as_path()));
    let note = prefix_note(&app.paths, &entry).unwrap();
    assert!(note.contains("wine-game's prefix at"));
    assert!(note.ends_with("stays where it is."));
    assert!(note.contains(&app.paths.contract_text(&prefix.display().to_string())));
    assert!(hover_label(&app.paths, &entry).contains(&entry.id));
    assert!(hover_label(&app.paths, &entry).contains("Prefix left in place:"));

    fs::remove_dir_all(&prefix).unwrap();
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "replacement\n").unwrap();
    open_trash(&mut app);
    let generation = start_restore(&mut app, &entry);
    let error = finish_restore(&mut app, generation, &entry.id).unwrap_err();
    assert!(error.contains("changed") || error.contains("identity"));
    assert_eq!(app.trash_page.row_errors.get(&entry.id), Some(&error));
}

#[test]
fn restore_partial_success_stays_visible_after_the_entry_is_removed() {
    let (root, mut app) = fixture();
    let exe = root.path().join("game.sh");
    fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let config = app.paths.environment_config("desktop-game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!(
            "kind = 'native'\n[targets.game]\nexe = '{}'\ndesktop = true\n",
            exe.display()
        ),
    )
    .unwrap();
    let entry = remove(&app, "desktop-game");
    let shortcut = app
        .paths
        .applications_dir()
        .join("wineo-desktop-game-game.desktop");
    fs::create_dir_all(shortcut.parent().unwrap()).unwrap();
    fs::write(&shortcut, "unrelated shortcut").unwrap();
    open_trash(&mut app);

    let generation = start_restore(&mut app, &entry);
    let error = finish_restore(&mut app, generation, &entry.id).unwrap_err();
    assert!(error.contains("was not overwritten"));
    assert_eq!(app.trash_page.row_errors.get(&entry.id), Some(&error));
    assert!(app.paths.environment_config("desktop-game").exists());
    assert!(trash::list(&app.paths).unwrap().is_empty());

    load_current(&mut app);
    assert!(app.trash_page.snapshot().unwrap().entries.is_empty());
    assert_eq!(app.trash_page.row_errors.get(&entry.id), Some(&error));
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    load_current(&mut app);
    assert_eq!(app.trash_page.row_errors.get(&entry.id), Some(&error));
}

#[test]
fn included_prefix_is_named_in_the_row_label() {
    let (_root, mut app) = fixture();
    let _prefix = wine(&app.paths, "included");
    let entry = trash::remove(&app.paths, "included", true).unwrap();
    assert!(entry.include_prefix);
    open_trash(&mut app);
    assert!(
        entry_label(
            &app.trash_page.snapshot().unwrap().entries[0],
            jiff::Timestamp::now()
        )
        .contains("prefix included")
    );
}

#[test]
fn each_selector_builds_its_review_and_reviewed_selection_is_frozen() {
    let (_root, mut app) = fixture();
    native(&app.paths, "old");
    let old = remove(&app, "old");
    native(&app.paths, "fresh");
    let fresh = remove(&app, "fresh");
    set_removed_at(&app.paths, &old.id, "2000-01-01T00:00:00Z");
    open_trash(&mut app);

    let selected = review(&mut app, Selection::Id(old.id.clone()));
    assert_eq!(selected.entries.len(), 1);
    assert_eq!(selected.entries[0].id, old.id);
    send(&mut app, Event::Dismiss);
    let old_only = review(&mut app, Selection::OlderThan(30));
    assert_eq!(old_only.entries.len(), 1);
    assert_eq!(old_only.entries[0].id, old.id);
    assert!(old_only.cutoff.is_some());
    send(&mut app, Event::Dismiss);
    let all = review(&mut app, Selection::All);
    assert_eq!(all.entries.len(), 2);

    let frozen = app.trash_page.review.clone().unwrap();
    set_removed_at(&app.paths, &old.id, "2001-01-01T00:00:00Z");
    native(&app.paths, "arrived-after-review");
    let late = remove(&app, "arrived-after-review");
    send(&mut app, Event::Confirm);
    let ids = match &app.change {
        Some(removal::Change::Trash(Change::Prune { ids })) => ids,
        _ => panic!("confirm did not start pruning"),
    };
    let frozen_ids = frozen
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(*ids, frozen_ids);
    assert!(!ids.contains(&late.id));
    let outcomes = trash_work::prune_selection(&app.paths, &frozen);
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().any(|outcome| {
        outcome.id == old.id
            && outcome
                .result
                .as_ref()
                .unwrap_err()
                .contains("changed since confirmation")
    }));
    assert!(
        outcomes
            .iter()
            .any(|outcome| outcome.id == fresh.id && outcome.result.is_ok())
    );
    assert!(
        trash::list(&app.paths)
            .unwrap()
            .iter()
            .any(|entry| entry.id == late.id)
    );
}

#[test]
fn review_errors_are_shown_with_entries_and_remain_after_pruning() {
    let (_root, mut app) = fixture();
    native(&app.paths, "kept");
    let kept = remove(&app, "kept");
    native(&app.paths, "damaged");
    let damaged = remove(&app, "damaged");
    open_trash(&mut app);
    let manifest = app
        .paths
        .trash_dir()
        .join(&damaged.id)
        .join("manifest.toml");
    fs::write(manifest, "not valid TOML = [").unwrap();

    let reviewed = review(&mut app, Selection::All);
    assert_eq!(reviewed.entries.len(), 1);
    assert_eq!(reviewed.entries[0].id, kept.id);
    assert_eq!(reviewed.errors.len(), 1);
    assert_eq!(reviewed.errors[0].0, damaged.id);
    assert!(review_errors(&reviewed).unwrap().contains(&damaged.id));

    let frozen = app.trash_page.review.clone().unwrap();
    send(&mut app, Event::Confirm);
    assert!(
        app.trash_page
            .error
            .as_deref()
            .unwrap()
            .contains(&damaged.id)
    );
    let outcomes = trash_work::prune_selection(&app.paths, &frozen);
    let generation = app.change_generation;
    send(&mut app, Event::Pruned(generation, Ok(outcomes)));
    assert_eq!(app.trash_page.results.len(), 1);
    assert_eq!(app.trash_page.results[0].id, kept.id);
    assert!(app.trash_page.results[0].result.is_ok());
    assert!(
        app.trash_page
            .error
            .as_deref()
            .unwrap()
            .contains(&damaged.id)
    );
}

#[test]
fn empty_review_and_invalid_days_show_short_feedback() {
    let (_root, mut app) = fixture();
    open_trash(&mut app);
    let empty = review(&mut app, Selection::All);
    assert!(empty.entries.is_empty());
    assert_eq!(
        app.trash_page.notice.as_deref(),
        Some("No trash entries selected.")
    );
    assert!(app.trash_page.snapshot().unwrap().entries.is_empty());

    for days in ["", "0", "01", "1.5", "-1", "4294967296"] {
        send(&mut app, Event::Days(days.into()));
        send(&mut app, Event::ReviewOlder);
        assert!(
            app.trash_page
                .error
                .as_deref()
                .unwrap()
                .contains("positive whole days")
        );
        assert!(!app.trash_page.reviewing);
    }
}

#[test]
fn partial_prune_results_survive_refresh_and_section_changes_until_next_action() {
    let (_root, mut app) = fixture();
    native(&app.paths, "changed");
    let changed = remove(&app, "changed");
    native(&app.paths, "deleted");
    let deleted = remove(&app, "deleted");
    open_trash(&mut app);
    let reviewed = review(&mut app, Selection::All);
    let frozen = reviewed.clone();
    set_removed_at(&app.paths, &changed.id, "2001-01-01T00:00:00Z");
    send(&mut app, Event::Confirm);
    let generation = app.change_generation;
    let outcomes = trash_work::prune_selection(&app.paths, &frozen);
    send(&mut app, Event::Pruned(generation, Ok(outcomes)));
    assert_eq!(app.trash_page.results.len(), 2);
    assert!(
        app.trash_page
            .results
            .iter()
            .any(|result| result.id == changed.id && result.result.is_err())
    );
    assert!(
        app.trash_page
            .results
            .iter()
            .any(|result| result.id == deleted.id && result.result.is_ok())
    );
    load_current(&mut app);

    let _ = app.update(Message::Section(sidebar::Section::Library));
    assert!(
        app.trash_page
            .results
            .iter()
            .any(|result| result.id == changed.id)
    );
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    load_current(&mut app);
    assert_eq!(app.trash_page.results.len(), 2);
    send(&mut app, Event::Days("30".into()));
    assert!(app.trash_page.results.is_empty());
}

#[test]
fn invalid_trash_disables_actions_and_check_again_reloads() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let entry = remove(&app, "game");
    let pending = app.paths.prefix_locks_dir().join(".trash-pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(pending.join("trash-broken.toml"), "broken marker").unwrap();
    open_trash(&mut app);
    assert!(app.trash_page.snapshot.as_ref().unwrap().is_err());
    assert!(app.trash_badge.warning);

    let change_generation = app.change_generation;
    send(&mut app, Event::Restore(entry.id.clone()));
    send(&mut app, Event::Review(Selection::All));
    send(&mut app, Event::Recover(entry.id.clone()));
    send(&mut app, Event::ReviewOlder);
    assert!(app.change.is_none());
    assert!(!app.trash_page.reviewing);
    assert_eq!(app.change_generation, change_generation);

    fs::remove_file(pending.join("trash-broken.toml")).unwrap();
    send(&mut app, Event::CheckAgain);
    assert!(app.trash_page.loading);
    assert!(app.trash_page.snapshot.as_ref().unwrap().is_err());
    load_current(&mut app);
    assert_eq!(app.trash_page.snapshot().unwrap().entries.len(), 1);
}

#[test]
fn unreadable_trash_list_blocks_page_actions_until_check_again() {
    let (_root, mut app) = fixture();
    let dir = app.paths.trash_dir();
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    fs::write(&dir, "trash storage is a file").unwrap();
    assert!(trash::list(&app.paths).is_err());
    open_trash(&mut app);
    assert!(app.trash_page.snapshot.as_ref().unwrap().is_err());
    assert!(app.trash_badge.warning);
    assert_eq!(app.trash_badge.count, None);

    let generation = app.trash_generation;
    send(&mut app, Event::Days("90".into()));
    send(&mut app, Event::Review(Selection::All));
    assert_eq!(app.trash_page.days, "30");
    assert!(!app.trash_page.reviewing);
    assert!(app.trash_page.review.is_none());

    fs::remove_file(&dir).unwrap();
    fs::create_dir(&dir).unwrap();
    send(&mut app, Event::CheckAgain);
    assert!(app.trash_page.loading);
    assert_ne!(app.trash_generation, generation);
    load_current(&mut app);
    assert!(app.trash_page.snapshot().unwrap().entries.is_empty());
    assert_eq!(app.trash_badge.count, Some(0));
}

#[test]
fn pending_removing_and_restoring_entries_disable_rows_and_recover() {
    let (_root, mut app) = fixture();
    native(&app.paths, "removing");
    let removing = remove(&app, "removing");
    native(&app.paths, "restoring");
    let restoring = remove(&app, "restoring");
    let file = app
        .paths
        .trash_dir()
        .join(&removing.id)
        .join("manifest.toml");
    let text = fs::read_to_string(&file)
        .unwrap()
        .replace("phase = \"trashed\"", "phase = \"removing\"");
    fs::write(file, text).unwrap();
    let file = app
        .paths
        .trash_dir()
        .join(&restoring.id)
        .join("manifest.toml");
    let text = fs::read_to_string(&file)
        .unwrap()
        .replace("phase = \"trashed\"", "phase = \"restoring\"");
    fs::write(file, text).unwrap();
    open_trash(&mut app);
    let snapshot = app.trash_page.snapshot().unwrap();
    assert_eq!(snapshot.pending.len(), 2);
    assert!(!app.trash_page.row_enabled(&removing.id));
    assert!(!app.trash_page.row_enabled(&restoring.id));

    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::Refresh);
    let badge = sidebar::TrashBadge::read(&app.paths);
    let _ = app.update(Message::TrashReloaded(app.trash_generation, badge));
    assert!(app.trash_badge.warning);
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    load_current(&mut app);

    send(&mut app, Event::Restore(removing.id.clone()));
    send(&mut app, Event::Review(Selection::Id(removing.id.clone())));
    assert!(app.change.is_none() && !app.trash_page.reviewing);
    send(&mut app, Event::Recover(removing.id.clone()));
    assert!(matches!(
        app.change,
        Some(removal::Change::Trash(Change::Recover { ref id, .. })) if id == &removing.id
    ));
    let generation = app.change_generation;
    let result = trash_work::recover_entry(&app.paths, removing.id.clone());
    send(&mut app, Event::Recovered(generation, result));
    assert!(app.change.is_none());
    load_current(&mut app);
    assert_eq!(app.trash_page.snapshot().unwrap().pending.len(), 1);
    assert_eq!(
        app.trash_page.snapshot().unwrap().pending[0].id,
        restoring.id
    );

    send(&mut app, Event::Recover(restoring.id.clone()));
    let generation = app.change_generation;
    let result = trash_work::recover_entry(&app.paths, restoring.id.clone());
    assert_eq!(result, Ok(()));
    send(&mut app, Event::Recovered(generation, result));
    load_current(&mut app);
    assert!(app.trash_page.snapshot().unwrap().pending.is_empty());
    assert!(!app.trash_badge.warning);
}

#[test]
fn real_pending_markers_recover_both_phases_and_keep_collision_errors_visible() {
    let (_root, mut app) = fixture();
    let metadata_identity = trash_metadata_identity(&app.paths);
    native(&app.paths, "removing-marker");
    let removing = remove(&app, "removing-marker");
    native(&app.paths, "restoring-marker");
    let restoring = remove(&app, "restoring-marker");
    make_pending(
        &app.paths,
        &removing,
        "removing",
        "remove",
        &metadata_identity,
    );
    make_pending(
        &app.paths,
        &restoring,
        "restoring",
        "restore",
        &metadata_identity,
    );
    let pending = trash::pending_entries(&app.paths).unwrap();
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().any(|entry| entry.id == removing.id));
    assert!(pending.iter().any(|entry| entry.id == restoring.id));

    open_trash(&mut app);
    let snapshot = app.trash_page.snapshot().unwrap();
    assert_eq!(snapshot.pending.len(), 2);
    assert!(!app.trash_page.row_enabled(&removing.id));
    assert!(!app.trash_page.row_enabled(&restoring.id));

    native(&app.paths, "removing-marker");
    send(&mut app, Event::Recover(removing.id.clone()));
    let generation = app.change_generation;
    let error = trash_work::recover_entry(&app.paths, removing.id.clone()).unwrap_err();
    assert!(error.contains("collision"), "{error}");
    send(&mut app, Event::Recovered(generation, Err(error.clone())));
    assert_eq!(
        app.trash_page.recovery_errors.get(&removing.id),
        Some(&error)
    );
    load_current(&mut app);
    assert_eq!(trash::pending_entries(&app.paths).unwrap().len(), 2);

    fs::remove_file(app.paths.environment_config("removing-marker")).unwrap();
    send(&mut app, Event::Recover(removing.id.clone()));
    let generation = app.change_generation;
    assert_eq!(
        trash_work::recover_entry(&app.paths, removing.id.clone()),
        Ok(())
    );
    send(&mut app, Event::Recovered(generation, Ok(())));
    load_current(&mut app);
    assert_eq!(trash::pending_entries(&app.paths).unwrap().len(), 1);

    send(&mut app, Event::Recover(restoring.id.clone()));
    let generation = app.change_generation;
    assert_eq!(
        trash_work::recover_entry(&app.paths, restoring.id.clone()),
        Ok(())
    );
    send(&mut app, Event::Recovered(generation, Ok(())));
    load_current(&mut app);
    assert!(trash::pending_entries(&app.paths).unwrap().is_empty());
    assert!(app.trash_page.snapshot().unwrap().pending.is_empty());
}

#[test]
fn only_one_management_change_runs_and_switching_keeps_its_result() {
    let (_root, mut app) = fixture();
    native(&app.paths, "first");
    let first = remove(&app, "first");
    native(&app.paths, "second");
    let second = remove(&app, "second");
    native(&app.paths, "idle");
    app.cards = crate::library::load_library(&app.paths).unwrap();
    open_trash(&mut app);

    let generation = start_restore(&mut app, &first);
    let _ = app.update(Message::Section(sidebar::Section::Library));
    assert_eq!(
        app.remove_reason("idle").as_deref(),
        Some("Busy: restoring first")
    );
    let _ = app.update(Message::Remove("idle".into()));
    assert!(app.removal.is_none());
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    send(&mut app, Event::Restore(second.id.clone()));
    send(&mut app, Event::Review(Selection::All));
    assert!(app.removal.is_none());
    assert!(!app.trash_page.reviewing);
    assert_eq!(
        app.change.as_ref().unwrap().reason(),
        "Busy: restoring first"
    );
    let _ = app.update(Message::Section(sidebar::Section::Library));
    assert!(matches!(
        app.change,
        Some(removal::Change::Trash(Change::Restore { .. }))
    ));
    let result = trash::restore(&app.paths, &first.id)
        .map_err(|error| app.paths.contract_text(&format!("{error:#}")));
    send(&mut app, Event::Restored(generation, result));
    assert!(app.change.is_none());
    assert!(app.trash_page.notice.as_deref().unwrap().contains("first"));
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    load_current(&mut app);
    assert!(app.trash_page.notice.as_deref().unwrap().contains("first"));
}

#[test]
fn stale_list_reads_are_dropped() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let entry = remove(&app, "game");
    open_trash(&mut app);
    let stale = app.trash_generation;
    let old_snapshot = app
        .trash_page
        .snapshot
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let _ = app.reload_trash_page();
    assert_ne!(app.trash_generation, stale);
    send(&mut app, Event::Loaded(stale, Err("late read".into())));
    assert!(app.trash_page.loading);
    assert_eq!(
        app.trash_page
            .snapshot
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .entries[0]
            .id,
        old_snapshot.entries[0].id
    );
    load_current(&mut app);
    assert_eq!(app.trash_page.snapshot().unwrap().entries[0].id, entry.id);
}

#[test]
fn confirm_escape_tab_and_close_wait_during_prune() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let entry = remove(&app, "game");
    open_trash(&mut app);
    review(&mut app, Selection::All);
    assert!(app.trash_confirm_open());
    assert_eq!(app.trash_confirm_controls(), vec![CANCEL_ID, CONFIRM_ID]);
    let _ = app.update(Message::Key(
        Key::Named(Named::Tab),
        Modifiers::empty(),
        Status::Ignored,
    ));
    let _ = app.update(Message::Key(
        Key::Named(Named::Escape),
        Modifiers::empty(),
        Status::Ignored,
    ));
    assert!(app.trash_page.review.is_none());

    review(&mut app, Selection::All);
    let frozen = app.trash_page.review.clone().unwrap();
    send(&mut app, Event::Confirm);
    let generation = app.change_generation;
    assert!(matches!(
        app.change,
        Some(removal::Change::Trash(Change::Prune { .. }))
    ));
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt && !app.closing);
    let _ = app.update(Message::KeepWorking);
    assert!(!app.close_prompt && app.change.is_some());
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    let _ = app.update(Message::CancelAndClose);
    assert!(app.closing && app.change.is_some());
    let outcomes = trash_work::prune_selection(&app.paths, &frozen);
    send(&mut app, Event::Pruned(generation, Ok(outcomes)));
    assert!(app.change.is_none());
    assert!(app.closing);
    assert_eq!(app.trash_page.results.len(), 1);
    assert_eq!(app.trash_page.results[0].id, entry.id);
}

#[test]
fn busy_prune_refuses_restore_and_remove_until_it_finishes() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let entry = remove(&app, "game");
    native(&app.paths, "idle");
    app.cards = crate::library::load_library(&app.paths).unwrap();
    open_trash(&mut app);
    review(&mut app, Selection::All);
    let frozen = app.trash_page.review.clone().unwrap();
    send(&mut app, Event::Confirm);
    let generation = app.change_generation;
    let _ = app.update(Message::Section(sidebar::Section::Library));
    assert_eq!(
        app.remove_reason("idle").as_deref(),
        Some("Busy: deleting trash entries")
    );
    let _ = app.update(Message::Remove("idle".into()));
    assert!(app.removal.is_none());
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    send(&mut app, Event::Restore(entry.id.clone()));
    assert!(matches!(
        app.change,
        Some(removal::Change::Trash(Change::Prune { .. }))
    ));
    assert!(app.removal.is_none());
    let outcomes = trash_work::prune_selection(&app.paths, &frozen);
    send(&mut app, Event::Pruned(generation, Ok(outcomes)));
    assert!(app.change.is_none());
}

#[test]
fn active_remove_refuses_trash_restore_and_prune() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let entry = remove(&app, "game");
    open_trash(&mut app);
    let _ = app.update(Message::Section(sidebar::Section::Library));
    native(&app.paths, "active");
    app.cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Remove("active".into()));
    assert!(app.removal.is_some());
    let _ = app.update(Message::ConfirmRemove);
    assert!(matches!(app.change, Some(removal::Change::Remove { .. })));
    let change_generation = app.change_generation;
    send(&mut app, Event::Restore(entry.id.clone()));
    send(&mut app, Event::Review(Selection::All));
    assert_eq!(app.change_generation, change_generation);
    assert!(matches!(app.change, Some(removal::Change::Remove { .. })));
    assert!(!app.trash_page.reviewing);
    let _ = app.update(Message::CancelRemove);
    let _ = app.update(Message::RemoveFinished(
        app.change_generation,
        Err("test cancellation".into()),
    ));
}
