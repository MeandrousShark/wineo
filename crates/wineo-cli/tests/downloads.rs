mod common;

use common::{Fixture, assert_success, stdout};

#[test]
fn new_keeps_an_alias_and_active_runs_keep_their_concrete_version() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    assert_success(&f.run(&["new", "-n", "alias", "--runner", "GE-Proton-Latest"]));
    assert!(stdout(&f.run(&["config", "alias"])).contains("runner = \"GE-Proton-Latest\""));
    assert_success(&f.run(&["set", "alias", "targets.game.exe=game.exe"]));
    assert_success(&f.run(&["start", "alias"]));
    let state = f.runtime("alias").read_state().unwrap().unwrap();
    assert_eq!(state.runner.unwrap().name(), "GE-Proton9-20");
    f.install_proton("GE-Proton11-7");
    assert!(stdout(&f.run(&["status", "alias"])).contains("GE-Proton9-20"));
    assert_success(&f.run(&["stop", "alias"]));
    assert_success(&f.run(&["start", "alias"]));
    assert_eq!(
        f.runtime("alias")
            .read_state()
            .unwrap()
            .unwrap()
            .runner
            .unwrap()
            .name(),
        "GE-Proton11-7"
    );
    assert_success(&f.run(&["stop", "alias"]));
}

#[test]
fn downloads_are_explicit_and_missing_aliases_never_fall_back() {
    let f = Fixture::new();
    assert!(
        !f.run(&["new", "-n", "missing", "--runner", "GE-Proton-Latest"])
            .status
            .success()
    );
    assert!(!f.config_dir().join("environments/missing.toml").exists());
    assert!(!f.run(&["download"]).status.success());
    assert_success(&f.run(&["download", "--help"]));
    assert_success(&f.run(&["runners"]));
}
