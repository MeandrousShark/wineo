mod common;

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use rustix::fs::OFlags;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

use common::{Fixture, assert_success, stderr, stdout};

fn native(f: &Fixture, name: &str) {
    let dir = f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.toml")),
        "kind = 'native'\n# raw config\n",
    )
    .unwrap();
}

fn remove(f: &Fixture, name: &str) -> String {
    native(f, name);
    let removed = f.run(&["rm", name]);
    assert_success(&removed);
    stdout(&removed)
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .to_owned()
}

#[test]
fn repeated_names_have_independent_ids_and_restore_collisions_do_not_overwrite() {
    let f = Fixture::new();
    let first = remove(&f, "game");
    let second = remove(&f, "game");
    assert_ne!(first, second);
    native(&f, "game");
    fs::write(
        f.config_dir().join("environments/game.toml"),
        "kind = 'native'\n# new\n",
    )
    .unwrap();
    assert_eq!(first.split_once('-').unwrap().1.len(), 8);
    let collision = f.run(&["restore", &first]);
    assert!(!collision.status.success());
    assert!(stderr(&collision).contains("already exists"));
    assert_eq!(f.created_config("game"), "kind = 'native'\n# new\n");
    assert!(stdout(&f.run(&["trash", "list"])).contains(&first));
    assert!(stdout(&f.run(&["trash", "list"])).contains(&second));
}

#[test]
fn prune_needs_explicit_selection_and_confirmation() {
    let f = Fixture::new();
    let id = remove(&f, "game");
    let help = f.run(&["prune"]);
    assert_success(&help);
    assert!(stdout(&help).contains("--older-than"));
    let refused = f.run(&["prune", "--all"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("--yes"));
    assert!(stdout(&f.run(&["trash", "list"])).contains(&id));
    assert_success(&f.run(&["prune", &id, "--yes"]));
    assert!(stdout(&f.run(&["trash", "list"])).trim().is_empty());
}

#[test]
fn prune_older_than_only_selects_entries_past_the_frozen_cutoff() {
    let f = Fixture::new();
    let recent = remove(&f, "recent");
    let old = remove(&f, "old");
    let manifest = f
        .root
        .path()
        .join(format!("data/wineo/trash/{old}/manifest.toml"));
    let text = fs::read_to_string(&manifest).unwrap();
    let line = text
        .lines()
        .find(|line| line.starts_with("removed_at = "))
        .unwrap();
    fs::write(
        &manifest,
        text.replace(line, "removed_at = '2020-01-01T00:00:00Z'"),
    )
    .unwrap();
    for age in ["0d", "-1d", "1.5d", "1w"] {
        let result = f.run(&["prune", "--older-than", age, "--yes"]);
        assert!(!result.status.success(), "{age}");
    }
    assert_success(&f.run(&["prune", "--older-than", "1d", "--yes"]));
    let listed = stdout(&f.run(&["trash", "list"]));
    assert!(listed.contains(&recent));
    assert!(!listed.contains(&old));
}

#[test]
fn broken_config_restores_byte_for_byte_with_a_warning() {
    let f = Fixture::new();
    let path = f.config_dir().join("environments/broken.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let raw = b"not a valid = toml\n# preserve this\n";
    fs::write(&path, raw).unwrap();
    let removed = f.run(&["rm", "broken"]);
    assert_success(&removed);
    let text = stdout(&removed);
    let id = text
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap();
    let restored = f.run(&["restore", id]);
    assert_success(&restored);
    assert!(stderr(&restored).contains("raw config is still invalid"));
    assert_eq!(fs::read(path).unwrap(), raw);
}

#[test]
fn bare_trash_lists_entries_with_relative_removal_time() {
    let f = Fixture::new();
    let id = remove(&f, "game");
    let manifest = f
        .root
        .path()
        .join(format!("data/wineo/trash/{id}/manifest.toml"));
    let text = fs::read_to_string(&manifest).unwrap();
    let line = text
        .lines()
        .find(|line| line.starts_with("removed_at = "))
        .unwrap();
    fs::write(
        &manifest,
        text.replace(line, "removed_at = '2020-01-01T00:00:00Z'"),
    )
    .unwrap();

    let bare = f.run(&["trash"]);
    assert_success(&bare);
    let listing = stdout(&bare);
    let entry = listing.lines().find(|line| line.contains(&id)).unwrap();
    assert!(entry.contains("removed "), "{entry}");
    assert!(entry.contains("days ago"), "{entry}");
    assert!(!entry.contains("2020-01-01T00:00:00Z"), "{entry}");
    assert!(entry.ends_with("days ago"), "native has no prefix: {entry}");

    let explicit = f.run(&["trash", "list"]);
    assert_success(&explicit);
    assert!(stdout(&explicit).contains(&id));
}

#[test]
fn rm_prints_restore_and_prune_commands_for_a_native_environment() {
    let f = Fixture::new();
    native(&f, "game");
    let removed = f.run(&["rm", "game"]);
    assert_success(&removed);
    let output = stdout(&removed);
    let id = output
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap();
    assert_eq!(
        output,
        format!(
            "Removed environment game to trash {id}\n\
             Restore with: wineo restore {id}\n\
             To permanently delete, run: wineo prune {id}\n"
        )
    );
}

#[test]
fn rm_delete_prefix_help_matches_the_accepted_wording() {
    let f = Fixture::new();
    let help = f.run(&["rm", "--help"]);
    assert_success(&help);
    assert!(stdout(&help).contains("Also move the Wine prefix and snapshots to trash"));
}

#[test]
fn canceling_interactive_prune_keeps_the_entry() {
    let f = Fixture::new();
    let id = remove(&f, "game");
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let slave_name = ptsname(&master, Vec::new()).unwrap();
    let slave = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(OFlags::NOCTTY.bits() as i32)
        .open(slave_name.to_str().unwrap())
        .unwrap();
    let mut command = f.wineo();
    command
        .args(["prune", &id])
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave);
    let mut child = command.spawn().unwrap();
    let mut master = fs::File::from(master);
    master.write_all(b"n\r").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(stdout(&f.run(&["trash", "list"])).contains(&id));
}
