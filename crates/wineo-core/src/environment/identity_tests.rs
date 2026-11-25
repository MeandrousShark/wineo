use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

use super::*;
use crate::art::{self, Kind};
use crate::library::{self, Id};

/// Enough of a PNG for `art::detect`.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// An environment from before ids: a config and no library entry.
fn legacy() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(&home))).unwrap();
    let config = paths.environment_config("game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
    (root, paths)
}

fn entry(paths: &Paths) -> Option<library::Entry> {
    library::load(paths).environments.get("game").cloned()
}

fn library_bytes(paths: &Paths) -> Option<Vec<u8>> {
    fs::read(paths.library_file()).ok()
}

fn store(paths: &Paths, identity: EnvironmentIdentity) -> Result<art::Stored> {
    art::set_manual_bytes(paths, "game", None, Kind::Grid, PNG, "grid.png", identity)
}

/// Rewrites the config through a new file, as an editor saving it would.
fn rewrite(paths: &Paths) {
    let config = paths.environment_config("game");
    let text = fs::read_to_string(&config).unwrap();
    let staged = config.with_extension("saving");
    fs::write(&staged, format!("# edited\n{text}")).unwrap();
    fs::rename(staged, config).unwrap();
}

#[test]
fn acquire_mints_an_id_once_without_touching_the_config() {
    let (_root, paths) = legacy();
    let config = fs::read(paths.environment_config("game")).unwrap();
    let first = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    let id = first.id().expect("a writable library gets an id");
    assert_eq!(entry(&paths).unwrap().id, Some(id));
    let after_mint = library_bytes(&paths);
    assert_eq!(
        EnvironmentIdentity::acquire(&paths, "game").unwrap(),
        Some(first)
    );
    assert_eq!(library_bytes(&paths), after_mint, "the second read wrote");
    assert_eq!(fs::read(paths.environment_config("game")).unwrap(), config);
}

#[test]
fn acquire_adds_only_the_id_to_an_entry_it_finds() {
    let (_root, paths) = legacy();
    let added = "2026-10-17T19:20:05Z";
    fs::write(
        paths.library_file(),
        format!(
            "schema_version = 1\n\n[environments.game]\n# mine\nfavorite = true\nadded = {added}\n"
        ),
    )
    .unwrap();
    let id = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap()
        .id()
        .unwrap();
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        format!(
            "schema_version = 1\n\n[environments.game]\n# mine\nfavorite = true\nadded = {added}\nid = \"{id}\"\n"
        )
    );
}

#[test]
fn acquire_refuses_a_bad_name_before_writing_anything() {
    let (_root, paths) = legacy();
    // `../config` names the global config, which exists, so only the name
    // check stands between it and a library entry.
    fs::write(paths.global_config(), "").unwrap();
    assert!(matches!(
        EnvironmentIdentity::acquire(&paths, "../config"),
        Err(Error::InvalidName(_))
    ));
    assert_eq!(
        EnvironmentIdentity::acquire(&paths, "nothing").unwrap(),
        None
    );
    assert_eq!(library_bytes(&paths), None);
}

#[test]
fn two_acquires_racing_for_an_old_environment_end_with_one_id() {
    for _ in 0..20 {
        let (_root, paths) = legacy();
        let inner = Rc::new(RefCell::new(None));
        crate::art::on_test_hook("library.mint", {
            let (paths, inner) = (paths.clone(), inner.clone());
            move || {
                *inner.borrow_mut() = EnvironmentIdentity::acquire(&paths, "game").unwrap();
            }
        });
        let outer = EnvironmentIdentity::acquire(&paths, "game").unwrap();
        let inner = inner.borrow().expect("the hook ran");
        assert_eq!(outer, Some(inner));
        assert_eq!(entry(&paths).unwrap().id, inner.id());
    }
}

#[test]
fn a_mint_racing_rm_leaves_no_entry() {
    for _ in 0..20 {
        let (_root, paths) = legacy();
        crate::art::on_test_hook("library.mint", {
            let paths = paths.clone();
            move || {
                crate::trash::remove(&paths, "game", false).unwrap();
            }
        });
        assert_eq!(EnvironmentIdentity::acquire(&paths, "game").unwrap(), None);
        assert_eq!(entry(&paths), None);
    }
}

#[test]
fn a_mint_racing_validates_stray_removal_leaves_no_entry() {
    for _ in 0..20 {
        let (_root, paths) = legacy();
        library::set_favorite(&paths, "game", true).unwrap();
        crate::art::on_test_hook("library.mint", {
            let paths = paths.clone();
            move || {
                fs::remove_file(paths.environment_config("game")).unwrap();
                let removed = library::remove_strays(&paths, || Ok(BTreeSet::new())).unwrap();
                assert_eq!(removed, ["game"]);
            }
        });
        assert_eq!(EnvironmentIdentity::acquire(&paths, "game").unwrap(), None);
        assert_eq!(entry(&paths), None);
    }
}

/// Puts `library.toml` in some state.
type MakeLibrary = Box<dyn Fn(&Paths)>;

/// Library states `acquire` can't write an id into, each left as it was.
fn unwritable_libraries() -> Vec<(&'static str, MakeLibrary)> {
    vec![
        (
            "unreadable",
            Box::new(|paths: &Paths| fs::write(paths.library_file(), "[broken\n").unwrap()),
        ),
        (
            "newer",
            Box::new(|paths: &Paths| {
                fs::write(paths.library_file(), "schema_version = 99\n").unwrap()
            }),
        ),
        (
            "read-only",
            Box::new(|paths: &Paths| {
                fs::write(paths.library_file(), "schema_version = 1\n").unwrap();
                set_mode(paths.library_file().parent().unwrap(), 0o555);
            }),
        ),
    ]
}

fn set_mode(dir: &Path, mode: u32) {
    fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_library_that_cant_take_an_id_falls_back_to_the_fingerprint() {
    for (case, make) in unwritable_libraries() {
        let (_root, paths) = legacy();
        make(&paths);
        let before = library_bytes(&paths);
        let identity = EnvironmentIdentity::acquire(&paths, "game")
            .unwrap()
            .unwrap();
        assert_eq!(identity.id(), None, "{case}");
        assert_eq!(library_bytes(&paths), before, "{case}");
        // As in 1.9.0: art lands while the config is untouched, and an edit
        // reads as a change.
        let config_dir = paths.library_file().parent().unwrap().to_owned();
        set_mode(&config_dir, 0o755);
        store(&paths, identity).unwrap_or_else(|error| panic!("{case}: {error}"));
        rewrite(&paths);
        assert_eq!(
            identity.is_current(&paths, "game"),
            Currency::Changed,
            "{case}"
        );
        assert!(matches!(
            store(&paths, identity),
            Err(Error::ArtEnvironmentChanged(_))
        ));
    }
}

#[test]
fn a_fingerprint_stays_current_when_another_process_mints() {
    let (_root, paths) = legacy();
    let held = EnvironmentIdentity::peek(&paths, "game").unwrap();
    assert_eq!(held.id(), None);
    let minted = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    assert!(minted.id().is_some());
    assert_eq!(held.is_current(&paths, "game"), Currency::Current);
    assert_eq!(
        held.matches(&EnvironmentIdentity::observe(&paths, "game")),
        Currency::Current
    );
    assert_eq!(
        held.is_current_with(&paths, "game", &mut library::Cache::default()),
        Currency::Current
    );
    store(&paths, held).unwrap();
}

#[test]
fn an_edit_keeps_an_id_current_and_the_art_lands() {
    let (_root, paths) = legacy();
    let identity = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    type Edit<'a> = Box<dyn Fn() + 'a>;
    let edits: [(&str, Edit); 4] = [
        (
            "settings::update",
            Box::new(|| {
                crate::settings::update(
                    &paths,
                    crate::settings::ConfigFile::Environment("game"),
                    &[("title", "Game")],
                    &[],
                )
                .unwrap();
            }),
        ),
        (
            "tags",
            Box::new(|| {
                crate::settings::change_tags(
                    &paths,
                    "game",
                    crate::settings::TagChange::Add("rpg"),
                )
                .unwrap();
            }),
        ),
        (
            "targets",
            Box::new(|| {
                let current = Environment::load(&paths, "game").unwrap();
                let mut batch = crate::targets::Batch::new(current.config.targets.clone());
                batch
                    .add(
                        "game",
                        crate::targets::Spec {
                            exe: "/bin/true".to_owned(),
                            args: Vec::new(),
                            cwd: None,
                            name: None,
                            desktop: false,
                        },
                    )
                    .unwrap();
                assert!(matches!(
                    crate::targets::write(&paths, "game", &batch).unwrap(),
                    crate::targets::Written::Saved
                ));
            }),
        ),
        ("an editor", Box::new(|| rewrite(&paths))),
    ];
    for (kind, (how, edit)) in Kind::ALL.into_iter().zip(edits) {
        let before = EnvironmentIdentity::fingerprint(&paths, "game");
        edit();
        assert_ne!(
            EnvironmentIdentity::fingerprint(&paths, "game"),
            before,
            "{how} saved in place"
        );
        assert_eq!(
            identity.is_current(&paths, "game"),
            Currency::Current,
            "{how}"
        );
        art::set_manual_bytes(&paths, "game", None, kind, PNG, "art.png", identity)
            .unwrap_or_else(|error| panic!("{how}: {error}"));
    }
}

#[test]
fn a_replacement_is_changed_for_an_id_and_a_fingerprint() {
    let (_root, paths) = legacy();
    let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
    let id = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    crate::test_support::replace_environment(&paths, "game").unwrap();
    for identity in [fingerprint, id] {
        assert_eq!(identity.is_current(&paths, "game"), Currency::Changed);
        assert_eq!(
            identity.matches(&EnvironmentIdentity::observe(&paths, "game")),
            Currency::Changed
        );
        assert_eq!(
            identity.is_current_with(&paths, "game", &mut library::Cache::default()),
            Currency::Changed
        );
        assert!(matches!(
            store(&paths, identity),
            Err(Error::ArtEnvironmentChanged(_))
        ));
    }
}

#[test]
fn an_id_holder_whose_library_breaks_is_told_so_not_that_it_changed() {
    let (_root, paths) = legacy();
    let identity = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    fs::write(paths.library_file(), "[broken\n").unwrap();
    assert_eq!(
        identity.is_current(&paths, "game"),
        Currency::LibraryUnreadable
    );
    assert_eq!(
        identity.matches(&EnvironmentIdentity::observe(&paths, "game")),
        Currency::LibraryUnreadable
    );
    assert_eq!(
        identity.is_current_with(&paths, "game", &mut library::Cache::default()),
        Currency::LibraryUnreadable
    );
    let error = store(&paths, identity).unwrap_err();
    assert!(
        matches!(&error, Error::ArtLibraryUnreadable { env, .. } if env == "game"),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "library.toml can't be read. Artwork for `game` wasn't stored."
    );
    let error = art::remove(&paths, "game", None, Kind::Grid, identity).unwrap_err();
    assert_eq!(
        error.to_string(),
        "library.toml can't be read. Artwork for `game` wasn't removed."
    );
    assert!(!paths.art_dir("game").join("grid.png").exists());
}

#[test]
fn looking_never_writes_the_library() {
    let (_root, paths) = legacy();
    // A legacy entry, which a write would add an id to.
    fs::write(
        paths.library_file(),
        "schema_version = 1\n[environments.game]\nfavorite = true\n",
    )
    .unwrap();
    let before = fs::metadata(paths.library_file()).unwrap();
    let bytes = library_bytes(&paths);
    let held = EnvironmentIdentity::peek(&paths, "game").unwrap();
    let observed = EnvironmentIdentity::observe(&paths, "game");
    assert_eq!(observed.preferred(), Some(held));
    assert_eq!(held.is_current(&paths, "game"), Currency::Current);
    assert_eq!(held.matches(&observed), Currency::Current);
    assert_eq!(
        held.is_current_with(&paths, "game", &mut library::Cache::default()),
        Currency::Current
    );
    let after = fs::metadata(paths.library_file()).unwrap();
    assert_eq!(library_bytes(&paths), bytes);
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(entry(&paths).unwrap().id, None);
}

#[test]
fn a_missing_config_matches_nothing_and_peeks_as_nothing() {
    let (_root, paths) = legacy();
    let id = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    let fingerprint = EnvironmentIdentity::fingerprint(&paths, "game").unwrap();
    fs::remove_file(paths.environment_config("game")).unwrap();
    let observed = EnvironmentIdentity::observe(&paths, "game");
    assert_eq!(observed.preferred(), None);
    assert_eq!(EnvironmentIdentity::peek(&paths, "game"), None);
    for identity in [id, fingerprint] {
        assert_eq!(identity.matches(&observed), Currency::Changed);
        assert_eq!(identity.is_current(&paths, "game"), Currency::Changed);
        assert_eq!(
            identity.is_current_with(&paths, "game", &mut library::Cache::default()),
            Currency::Changed
        );
    }
}

#[test]
fn upgrade_trades_a_current_fingerprint_for_the_id() {
    let (_root, paths) = legacy();
    let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
    let upgraded = fingerprint.upgrade(&paths, "game").unwrap();
    let id = upgraded.id().expect("an id was minted");
    assert_eq!(entry(&paths).unwrap().id, Some(id));
    // An id is kept, and one already there is adopted, not replaced.
    assert_eq!(upgraded.upgrade(&paths, "game").unwrap(), upgraded);
    assert_eq!(fingerprint.upgrade(&paths, "game").unwrap(), upgraded);
}

#[test]
fn upgrade_refuses_an_environment_replaced_while_it_waits_for_the_lock() {
    for _ in 0..20 {
        let (_root, paths) = legacy();
        let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
        let replaced = Rc::new(RefCell::new(None));
        crate::art::on_test_hook("library.mint", {
            let (paths, replaced) = (paths.clone(), replaced.clone());
            move || {
                crate::test_support::replace_environment(&paths, "game").unwrap();
                *replaced.borrow_mut() = library_bytes(&paths);
            }
        });
        let error = fingerprint.upgrade(&paths, "game").unwrap_err();
        assert!(matches!(error, Error::ArtEnvironmentChanged(_)), "{error}");
        let replaced = replaced.borrow().clone();
        assert!(replaced.is_some(), "the hook ran");
        assert_eq!(library_bytes(&paths), replaced, "upgrade wrote");
    }
}

#[test]
fn upgrade_keeps_the_fingerprint_when_the_library_cant_take_an_id() {
    for (case, make) in unwritable_libraries() {
        let (_root, paths) = legacy();
        let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
        make(&paths);
        let before = library_bytes(&paths);
        assert_eq!(
            fingerprint.upgrade(&paths, "game").unwrap(),
            fingerprint,
            "{case}"
        );
        assert_eq!(library_bytes(&paths), before, "{case}");
        set_mode(paths.library_file().parent().unwrap(), 0o755);
    }
}

#[test]
fn upgrade_refuses_a_changed_config_even_when_the_library_cant_take_an_id() {
    for (case, make) in unwritable_libraries() {
        let (_root, paths) = legacy();
        let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
        make(&paths);
        rewrite(&paths);
        let before = library_bytes(&paths);
        let result = fingerprint.upgrade(&paths, "game");
        set_mode(paths.library_file().parent().unwrap(), 0o755);
        assert!(
            matches!(result, Err(Error::ArtEnvironmentChanged(_))),
            "{case}: {result:?}"
        );
        assert_eq!(library_bytes(&paths), before, "{case}");
    }
}

/// The checks that run while the library lock is held see a change made
/// only once the lock is taken, which a check made before it would miss.
#[test]
fn the_checks_before_a_mint_run_while_the_library_lock_is_held() {
    for _ in 0..20 {
        let (_root, paths) = legacy();
        let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
        crate::art::on_test_hook("library.mint.locked", {
            let paths = paths.clone();
            move || rewrite(&paths)
        });
        assert!(matches!(
            fingerprint.upgrade(&paths, "game"),
            Err(Error::ArtEnvironmentChanged(_))
        ));
        assert_eq!(library_bytes(&paths), None);

        crate::art::on_test_hook("library.mint.locked", {
            let paths = paths.clone();
            move || fs::remove_file(paths.environment_config("game")).unwrap()
        });
        assert_eq!(EnvironmentIdentity::acquire(&paths, "game").unwrap(), None);
        assert_eq!(library_bytes(&paths), None);
    }
}

#[test]
fn upgrade_refuses_a_fingerprint_that_no_longer_matches() {
    let (_root, paths) = legacy();
    let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
    rewrite(&paths);
    assert!(matches!(
        fingerprint.upgrade(&paths, "game"),
        Err(Error::ArtEnvironmentChanged(_))
    ));
    assert_eq!(library_bytes(&paths), None);
    fs::remove_file(paths.environment_config("game")).unwrap();
    assert!(matches!(
        fingerprint.upgrade(&paths, "game"),
        Err(Error::ArtEnvironmentChanged(_))
    ));
}

#[test]
fn a_native_creation_returns_the_id_its_entry_has() {
    let (_root, paths) = legacy();
    let global = GlobalConfig::default();
    let (_, identity) =
        Environment::create_native_titled(&paths, &global, "other", None, None, false, |_| {})
            .unwrap();
    let id = identity.unwrap().id().unwrap();
    assert_eq!(library::load(&paths).environments["other"].id, Some(id));
}

#[test]
fn a_failed_stamp_over_a_stale_entry_returns_the_fingerprint_not_the_stale_id() {
    let (_root, paths) = legacy();
    let stale = Id::parse("0123456789abcdef0123456789abcdef").unwrap();
    library::put(
        &paths,
        "other",
        Some(&library::Entry {
            id: Some(stale),
            ..library::Entry::default()
        }),
    )
    .unwrap();
    let config_dir = paths.library_file().parent().unwrap().to_owned();
    // Readable, so the stale id is there to be picked up, but not writable,
    // so the stamp fails. The new config goes in its own directory.
    set_mode(&config_dir, 0o555);
    let mut warnings = Vec::new();
    let created = Environment::create_native_titled(
        &paths,
        &GlobalConfig::default(),
        "other",
        None,
        None,
        false,
        |warning| warnings.push(warning),
    );
    set_mode(&config_dir, 0o755);
    let (_, identity) = created.unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let identity = identity.unwrap();
    assert_eq!(identity.id(), None);
    assert_eq!(
        identity,
        EnvironmentIdentity::fingerprint(&paths, "other").unwrap()
    );
    assert_eq!(library::load(&paths).environments["other"].id, Some(stale));
}

#[test]
fn trash_and_restore_keep_the_id() {
    let (_root, paths) = legacy();
    let identity = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    assert!(identity.id().is_some());
    let entry = crate::trash::remove(&paths, "game", false).unwrap();
    assert_eq!(identity.is_current(&paths, "game"), Currency::Changed);
    assert!(crate::trash::restore(&paths, &entry.id).unwrap().is_empty());
    assert_eq!(identity.is_current(&paths, "game"), Currency::Current);
    store(&paths, identity).unwrap();
}

#[test]
fn the_test_replacement_refuses_to_keep_an_old_id() {
    let (_root, paths) = legacy();
    EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    crate::test_support::replace_environment(&paths, "game").unwrap();
    // Readable but not writable, so the old id would stay.
    set_mode(paths.library_file().parent().unwrap(), 0o555);
    let result = crate::test_support::replace_environment(&paths, "game");
    set_mode(paths.library_file().parent().unwrap(), 0o755);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("without a new id"), "{error}");
}

#[test]
fn a_real_rm_and_create_refuses_the_old_identity_and_takes_the_new_one() {
    let (_root, paths) = legacy();
    let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
    let old = EnvironmentIdentity::acquire(&paths, "game")
        .unwrap()
        .unwrap();
    assert!(old.id().is_some());
    crate::trash::remove(&paths, "game", false).unwrap();
    let (_, new) = Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "game",
        None,
        false,
        |_| {},
    )
    .unwrap();
    let new = new.unwrap();
    assert!(new.id().is_some());
    assert_ne!(new, old);
    for held in [old, fingerprint] {
        assert_eq!(held.is_current(&paths, "game"), Currency::Changed);
        assert!(matches!(
            store(&paths, held),
            Err(Error::ArtEnvironmentChanged(_))
        ));
    }
    assert!(!paths.art_dir("game").join("grid.png").exists());
    store(&paths, new).unwrap();
}

#[test]
fn after_its_own_write_a_fingerprint_is_taken_again_and_an_id_kept() {
    let (_root, paths) = legacy();
    let fingerprint = EnvironmentIdentity::peek(&paths, "game").unwrap();
    rewrite(&paths);
    assert!(fingerprint.is_current(&paths, "game").is_changed());
    let retaken = fingerprint.after_own_write(&paths, "game").unwrap();
    assert_eq!(retaken.id(), None);
    assert!(retaken.is_current(&paths, "game").is_current());
    assert_eq!(library_bytes(&paths), None, "it wrote the library");

    let id = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
    rewrite(&paths);
    assert_eq!(id.after_own_write(&paths, "game"), Some(id));

    fs::remove_file(paths.environment_config("game")).unwrap();
    assert_eq!(retaken.after_own_write(&paths, "game"), None);
}
