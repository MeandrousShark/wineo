//! GOG game folders through the CLI: ownership across rm, restore, set,
//! clone, and portable copy, and what validate reports and fixes.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Fixture, assert_success, stderr, stdout};

const ID: u64 = 1094900565;

fn write_config(f: &Fixture, name: &str, text: &str) {
    let dir = f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), text).unwrap();
}

/// A native RimWorld whose folder state names it as owner.
fn rimworld(f: &Fixture, mid_update: bool) -> PathBuf {
    let folder = f.root.path().join("Games/RimWorld");
    fs::create_dir_all(&folder).unwrap();
    write_config(
        f,
        "rimworld",
        &format!(
            "kind = \"native\"\ngog_id = \"{ID}\"\n\n[targets.game]\nexe = \"{}\"\n",
            folder.join("RimWorldLinux").display()
        ),
    );
    f.gog_folder(&folder, Some("rimworld"), ID, mid_update);
    folder
}

fn owner(f: &Fixture, folder: &Path) -> Option<String> {
    f.gog_state(folder).unwrap().owner
}

fn trash_id(output: &std::process::Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("Restore with: wineo restore "))
        .unwrap()
        .to_owned()
}

#[test]
fn rm_of_an_owner_mid_update_needs_force() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    let refused = f.run(&["rm", "rimworld"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains(
            "rimworld is mid-update. Finish it with wineo update rimworld, or remove it anyway with --force."
        ),
        "{}",
        stderr(&refused)
    );
    assert!(!f.created_config("rimworld").is_empty());
    assert_eq!(owner(&f, &folder).as_deref(), Some("rimworld"));

    let removed = f.run(&["rm", "rimworld", "--force"]);
    assert_success(&removed);
    assert!(f.created_config("rimworld").is_empty());
    let state = f.gog_state(&folder).unwrap();
    assert_eq!(state.owner, None);
    assert!(state.journal.is_some());

    assert_success(&f.run(&["restore", &trash_id(&removed)]));
    assert_eq!(owner(&f, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn restore_is_refused_while_another_environment_owns_the_folder() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    let removed = f.run(&["rm", "rimworld"]);
    assert_success(&removed);
    assert_eq!(owner(&f, &folder), None);
    f.gog_folder(&folder, Some("copy"), ID, false);
    let refused = f.run(&["restore", &trash_id(&removed)]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains(&format!("{} belongs to copy.", folder.display())),
        "{}",
        stderr(&refused)
    );
    assert!(f.created_config("rimworld").is_empty());
}

#[test]
fn set_refuses_gog_id_and_unset_releases_the_folder() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    let refused = f.run(&["set", "rimworld", "gog_id=1"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("gog_id is set by wineo import gog."),
        "{}",
        stderr(&refused)
    );
    assert!(f.created_config("rimworld").contains(&ID.to_string()));

    assert_success(&f.run(&["set", "rimworld", "--unset", "gog_id"]));
    assert!(!f.created_config("rimworld").contains("gog_id"));
    assert_eq!(owner(&f, &folder), None);
}

#[test]
fn unset_mid_update_is_refused() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    let refused = f.run(&["set", "rimworld", "--unset", "gog_id"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("rimworld is mid-update. Finish it with wineo update rimworld."),
        "{}",
        stderr(&refused)
    );
    assert!(f.created_config("rimworld").contains("gog_id"));
    assert_eq!(owner(&f, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn clones_and_portable_copies_drop_gog_id() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    let cloned = f.run(&["clone", "rimworld", "copy"]);
    assert_success(&cloned);
    assert!(
        stdout(&cloned).contains("  change: gog_id removed\n"),
        "{}",
        stdout(&cloned)
    );
    assert!(!f.created_config("copy").contains("gog_id"));
    assert!(f.created_config("rimworld").contains("gog_id"));

    let dir = f.root.path().join("usb");
    let copied = f.run(&[
        "portable",
        "copy",
        "rimworld",
        "--to",
        dir.to_str().unwrap(),
    ]);
    assert_success(&copied);
    assert!(
        stdout(&copied).contains("  change: gog_id removed\n"),
        "{}",
        stdout(&copied)
    );
    let config = fs::read_to_string(dir.join("config/environments/rimworld.toml")).unwrap();
    assert!(!config.contains("gog_id"), "{config}");
    assert_eq!(owner(&f, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn a_wine_clone_drops_gog_id() {
    let f = Fixture::new();
    let prefix = f.root.path().join("source-prefix");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    write_config(
        &f,
        "witcher",
        &format!(
            "prefix = {:?}\nrunner = 'wine'\narch = 'win64'\ngog_id = '1207658924'\n",
            prefix.to_str().unwrap()
        ),
    );
    let cloned = f.run(&["clone", "witcher", "copy"]);
    assert_success(&cloned);
    assert!(
        stdout(&cloned).contains("gog_id removed"),
        "{}",
        stdout(&cloned)
    );
    assert!(!f.created_config("copy").contains("gog_id"));
}

#[test]
fn proxied_environments_refuse_gog_id() {
    let f = Fixture::new();
    write_config(
        &f,
        "bg3",
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\ngog_id = \"1\"\n",
    );
    let output = f.run(&["validate", "bg3"]);
    assert!(
        stdout(&output).contains("bg3 is a Steam game, so it can't set gog_id.")
            || stderr(&output).contains("bg3 is a Steam game, so it can't set gog_id."),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
}

#[test]
fn validate_reports_a_lost_state_and_fixes_a_missing_owner() {
    let f = Fixture::new();
    write_config(
        &f,
        "lost",
        &format!("kind = \"native\"\ngog_id = \"{ID}\"\n"),
    );
    let report = f.run(&["validate"]);
    assert!(
        stdout(&report).contains(&format!(
            "lost's GOG state is missing. Migrate it again with wineo import gog --game {ID}, or stop tracking it with wineo set lost --unset gog_id."
        )),
        "{}",
        stdout(&report)
    );

    for mid_update in [false, true] {
        let folder = f.root.path().join(format!("Games/gone-{mid_update}"));
        fs::create_dir_all(&folder).unwrap();
        f.gog_folder(&folder, Some("gone"), 1, mid_update);
        let fixed = f.run(&["validate", "--fix"]);
        assert!(
            stdout(&fixed).contains("names gone, which doesn't exist."),
            "{}",
            stdout(&fixed)
        );
        let state = f.gog_state(&folder).unwrap();
        assert_eq!(state.owner, None);
        assert_eq!(state.journal.is_some(), mid_update);
    }
}
