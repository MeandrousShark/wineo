//! Old spellings of flags and subcommands keep working and warn on stderr;
//! the new spellings and their aliases work quietly.

mod common;

use common::*;

const DELETE_PREFIX: &str = "warning: --delete-prefix is deprecated; use --include-prefix\n";
const SNAPSHOT_DELETE: &str = "warning: snapshot delete is deprecated; use snapshot remove\n";

fn created(f: &Fixture, name: &str) {
    assert_success(&f.run(&["new", "-n", name]));
}

#[test]
fn rm_delete_prefix_still_works_and_warns_once() {
    let f = Fixture::new();
    created(&f, "old");
    created(&f, "new");
    let prefix = |name: &str| f.root.path().join("data/wineo/prefixes").join(name);
    assert!(prefix("old").exists());

    let old = f.run(&["rm", "old", "--delete-prefix"]);
    assert_success(&old);
    assert_eq!(stderr(&old), DELETE_PREFIX);
    assert!(!stdout(&old).contains("deprecated"), "{}", stdout(&old));
    assert!(!prefix("old").exists());

    let new = f.run(&["rm", "new", "--include-prefix"]);
    assert_success(&new);
    assert!(!stderr(&new).contains("deprecated"), "{}", stderr(&new));
    assert!(!prefix("new").exists());
    // Same text apart from the name and the trash id.
    assert!(
        stdout(&old).contains("and its prefix to trash"),
        "{}",
        stdout(&old)
    );
    assert!(
        stdout(&new).contains("and its prefix to trash"),
        "{}",
        stdout(&new)
    );
}

#[test]
fn rm_delete_prefix_keeps_its_refusals_and_exit_code() {
    let f = Fixture::new();
    let old = f.run(&["rm", "missing", "--delete-prefix"]);
    let new = f.run(&["rm", "missing", "--include-prefix"]);
    assert_eq!(old.status.code(), new.status.code());
    assert!(!old.status.success());
    assert!(stderr(&old).starts_with(DELETE_PREFIX), "{}", stderr(&old));
    assert_eq!(stderr(&old).replacen(DELETE_PREFIX, "", 1), stderr(&new));
}

#[test]
fn rm_help_shows_only_the_new_flag_and_the_remove_alias() {
    let f = Fixture::new();
    let help = stdout(&f.run(&["rm", "--help"]));
    assert!(help.contains("--include-prefix"), "{help}");
    assert!(!help.contains("delete-prefix"), "{help}");
    let top = stdout(&f.run(&["--help"]));
    let line = top
        .lines()
        .find(|l| l.contains("Remove an environment"))
        .unwrap();
    assert!(line.contains("[alias: remove]"), "{line}");
}

#[test]
fn remove_is_an_alias_of_rm_without_a_warning() {
    let f = Fixture::new();
    created(&f, "game");
    let removed = f.run(&["remove", "game", "--include-prefix"]);
    assert_success(&removed);
    assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
    assert!(!f.config_dir().join("environments/game.toml").exists());
}

#[test]
fn build_and_runner_removal_accept_rm_and_remove() {
    let f = Fixture::new();
    for component in ["dxvk", "vkd3d"] {
        let dirs: &[&str] = if component == "dxvk" {
            &["x64", "x32"]
        } else {
            &["x64", "x86"]
        };
        // The newest build stays, since a template asks for latest.
        f.install_build(component, "9.9", dirs);
        for (verb, name) in [("rm", "1.0"), ("remove", "2.0")] {
            f.install_build(component, name, dirs);
            let removed = f.run(&[component, verb, name, "--yes"]);
            assert_success(&removed);
            assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
            assert!(
                !f.root
                    .path()
                    .join("data/wineo")
                    .join(component)
                    .join(name)
                    .exists(),
                "{component} {verb}"
            );
        }
    }
    for (verb, name) in [("rm", "wine-a"), ("remove", "wine-b")] {
        f.install_runner(name);
        let removed = f.run(&["runners", verb, name, "--yes"]);
        assert_success(&removed);
        assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
        assert!(!f.root.path().join("data/wineo/runners").join(name).exists());
    }
}

#[test]
fn art_removal_accepts_rm_and_remove() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let png = f.root.path().join("grid.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
    for verb in ["rm", "remove"] {
        assert_success(&f.run(&["art", "set", "hades", "grid", png.to_str().unwrap()]));
        let removed = f.run(&["art", verb, "hades", "grid"]);
        assert_success(&removed);
        assert_eq!(stdout(&removed), "Removed grid from hades\n");
        assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
    }
}

#[test]
fn snapshot_delete_warns_once_and_remove_and_rm_do_not() {
    let f = Fixture::new();
    f.environment("game", "[targets.game]\nexe = \"game.exe\"\n");
    // No snapshot exists, so every spelling fails the same way; what differs
    // is only whether the deprecation line comes first.
    let old = f.run(&["snapshot", "delete", "game", "none"]);
    assert!(!old.status.success());
    assert!(
        stderr(&old).starts_with(SNAPSHOT_DELETE),
        "{}",
        stderr(&old)
    );
    for verb in ["remove", "rm"] {
        let new = f.run(&["snapshot", verb, "game", "none"]);
        assert_eq!(new.status.code(), old.status.code());
        assert_eq!(stderr(&new), stderr(&old).replacen(SNAPSHOT_DELETE, "", 1));
    }

    let help = stdout(&f.run(&["snapshot", "--help"]));
    let line = help
        .lines()
        .find(|l| l.contains("Remove a named snapshot"))
        .unwrap();
    assert!(
        line.contains("remove") && line.contains("[alias: rm]"),
        "{line}"
    );
    assert!(!help.contains("delete"), "{help}");
}
