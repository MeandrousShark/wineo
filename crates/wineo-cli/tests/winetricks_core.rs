mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use common::{Fixture, write_script};
use wineo_core::cancel::Token;
use wineo_core::installer::Output;
use wineo_core::runners::Runners;
use wineo_core::winetricks;

#[test]
fn unattended_install_calls_each_runner_once_in_order() {
    let f = Fixture::new();
    f.install_runner("fake-wine");
    f.install_proton("fake-proton");
    f.environment_with_runner("wine-app", "fake-wine", "");
    f.proton_environment("proton-app", "fake-proton", "");

    let script = f.root.path().join("bin/winetricks");
    write_script(&script, "#!/bin/sh\necho \"winetricks: $*\"\n");

    for (runner, app, expected) in [
        ("fake-wine", "wine-app", "winetricks: -q corefonts win10"),
        (
            "fake-proton",
            "proton-app",
            "umu-run: winetricks -q corefonts win10",
        ),
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "unattended_child", "--nocapture"])
            .env("WINEO_WINETRICKS_TEST_ROOT", f.root.path())
            .env("WINEO_WINETRICKS_TEST_RUNNER", runner)
            .env("WINEO_WINETRICKS_TEST_APP", app)
            .env("PATH", &f.path_var)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8_lossy(&output.stdout);
        assert_eq!(printed.matches(expected).count(), 1, "{printed}");
    }
}

#[test]
fn unattended_child() {
    let Some(root) = std::env::var_os("WINEO_WINETRICKS_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let runner_name = std::env::var("WINEO_WINETRICKS_TEST_RUNNER").unwrap();
    let app = std::env::var("WINEO_WINETRICKS_TEST_APP").unwrap();
    let runner = Runners::at(root.join("data/wineo/runners"))
        .resolve(&runner_name)
        .unwrap();
    let prefix = root.join("prefixes").join(&app);
    winetricks::install_unattended(
        &app,
        &runner,
        &prefix,
        &BTreeMap::new(),
        &["corefonts".to_owned(), "win10".to_owned()],
        5,
        &Token::new(),
        Output::Inherit,
        |_| {},
    )
    .unwrap();
}
