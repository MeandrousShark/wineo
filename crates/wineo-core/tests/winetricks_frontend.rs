use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use wineo_core::cancel::Token;
use wineo_core::paths::Paths;
use wineo_core::winetricks::{self, EnsureEvent, EnsureOutput, WinetricksEvent};

#[allow(dead_code)]
#[path = "common/runners.rs"]
mod runners;

#[path = "common/test_script.rs"]
mod test_script;
use test_script::write_script;

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_script(path, body);
}

#[test]
fn install_missing_builds_prefix_without_opening_picker() {
    if let Some(root) = std::env::var_os("WINEO_WINETRICKS_FRONTEND_ROOT") {
        let root = Path::new(&root);
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap();
        let prefix = paths.default_prefix_root().join("notes");
        let config = paths.environment_config("notes");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            format!(
                "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\nwinetricks = ['corefonts']\n",
                prefix.display()
            ),
        )
        .unwrap();
        let mut saw_build = false;
        let mut saw_install = false;
        let mut events = |event| match event {
            EnsureEvent::Launch(wineo_core::launch::LaunchEvent::BuildingPrefix { .. }) => {
                saw_build = true
            }
            EnsureEvent::Winetricks(WinetricksEvent::Installing(_)) => saw_install = true,
            _ => {}
        };
        winetricks::ensure_in(
            &paths,
            "notes",
            &BTreeMap::new(),
            &Token::new(),
            EnsureOutput::Log(&root.join("install.log")),
            &mut events,
        )
        .unwrap();
        assert!(saw_build);
        assert!(saw_install);
        assert!(prefix.join("drive_c").is_dir());
        assert_eq!(
            fs::read_to_string(prefix.join("winetricks.log")).unwrap(),
            "corefonts\n"
        );
        assert_eq!(
            fs::read_to_string(prefix.join("invocations")).unwrap(),
            "<-q>\n<corefonts>\n"
        );
        winetricks::ensure_in(
            &paths,
            "notes",
            &BTreeMap::new(),
            &Token::new(),
            EnsureOutput::Log(&root.join("install.log")),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(prefix.join("invocations")).unwrap(),
            "<-q>\n<corefonts>\n"
        );
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let runner = paths.default_runner_root().join("fake/bin");
    script(&runner.join("wine"), runners::FAKE_WINE);
    script(&runner.join("wineserver"), runners::FAKE_WINESERVER);
    let bin = root.path().join("bin");
    script(
        &bin.join("winetricks"),
        "#!/bin/sh\nfor arg in \"$@\"; do\n  printf '<%s>\\n' \"$arg\" >>\"$WINEPREFIX/invocations\"\n  case \"$arg\" in -*) ;; *) printf '%s\\n' \"$arg\" >>\"$WINEPREFIX/winetricks.log\" ;; esac\ndone\n",
    );
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "install_missing_builds_prefix_without_opening_picker",
            "--nocapture",
        ])
        .env("WINEO_WINETRICKS_FRONTEND_ROOT", root.path())
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
