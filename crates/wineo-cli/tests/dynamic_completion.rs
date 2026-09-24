use std::fs;
use std::path::Path;
use std::process::Command;

fn write_environment(root: &Path, name: &str, contents: &str) {
    let dir = root.join("config/wineo/environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), contents).unwrap();
}

fn complete(root: &Path, shell: &str, words: &[&str]) -> Vec<String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wineo"));
    command
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("COMPLETE", shell)
        .env("_CLAP_COMPLETE_INDEX", (words.len() - 1).to_string())
        .env("_CLAP_IFS", "\n")
        .arg("--")
        .args(words);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{shell} {:?}: {}",
        words,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split(['\t', ':']).next().unwrap().to_owned())
        .collect()
}

fn fixture(root: &Path) {
    write_environment(
        root,
        "hades",
        "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n[targets.game]\nexe = 'game.exe'\n[targets.launcher]\nexe = 'launcher.exe'\n",
    );
    write_environment(
        root,
        "halo",
        "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n[targets.coop]\nexe = 'coop.exe'\n",
    );
    write_environment(root, "broken", "not valid toml = [");
}

#[test]
fn fish_completes_environment_positions_and_filters_prefix() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    for command in [
        "setup",
        "clone",
        "config",
        "start",
        "stop",
        "restart",
        "programs",
        "rm",
        "status",
        "logs",
        "run",
        "exec",
        "set",
        "winetricks",
    ] {
        let names = complete(root.path(), "fish", &["wineo", command, "ha"]);
        assert!(names.contains(&"hades".to_owned()), "{command}: {names:?}");
        assert!(names.contains(&"halo".to_owned()), "{command}: {names:?}");
        assert!(
            !names.contains(&"broken".to_owned()),
            "{command}: {names:?}"
        );
    }
    let names = complete(root.path(), "fish", &["wineo", "stop", "br"]);
    assert!(names.contains(&"broken".to_owned()), "{names:?}");
}

#[test]
fn fish_omits_environment_names_in_other_positions() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    for words in [
        vec!["wineo", "new", "ha"],
        vec!["wineo", "clone", "hades", "ha"],
        vec!["wineo", "set", "--global", "ha"],
        vec!["wineo", "set", "hades", "ha"],
        vec!["wineo", "run", "hades", "--", "ha"],
        vec!["wineo", "exec", "hades", "winecfg", "ha"],
        vec!["wineo", "winetricks", "hades", "ha"],
        vec!["wineo", "winetricks", "--", "--env", "ha"],
    ] {
        let names = complete(root.path(), "fish", &words);
        assert!(!names.contains(&"halo".to_owned()), "{words:?}: {names:?}");
    }
}

#[test]
fn targets_use_only_the_selected_environment() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    for command in ["start", "run", "restart"] {
        let names = complete(
            root.path(),
            "fish",
            &["wineo", command, "hades", "--target", ""],
        );
        assert!(names.contains(&"game".to_owned()), "{command}: {names:?}");
        assert!(
            names.contains(&"launcher".to_owned()),
            "{command}: {names:?}"
        );
        assert!(!names.contains(&"coop".to_owned()), "{command}: {names:?}");

        let names = complete(root.path(), "fish", &["wineo", command, "halo", "-t", "co"]);
        assert!(names.contains(&"coop".to_owned()), "{command}: {names:?}");
        assert!(!names.contains(&"game".to_owned()), "{command}: {names:?}");

        let names = complete(
            root.path(),
            "fish",
            &[
                "wineo",
                "-v",
                command,
                "--env",
                "KEY=VALUE",
                "hades",
                "--target",
                "la",
            ],
        );
        assert!(
            names.contains(&"launcher".to_owned()),
            "{command}: {names:?}"
        );
        assert!(!names.contains(&"game".to_owned()), "{command}: {names:?}");

        for words in [
            vec!["wineo", command, "--target", ""],
            vec!["wineo", command, "broken", "--target", ""],
            vec![
                "wineo",
                command,
                "--env",
                "KEY=VALUE",
                "hades",
                "--",
                "--target",
                "",
            ],
        ] {
            let names = complete(root.path(), "fish", &words);
            assert!(!names.contains(&"game".to_owned()), "{words:?}: {names:?}");
        }
    }
}

#[test]
fn unreadable_config_location_returns_no_candidates() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("config"), "not a directory").unwrap();
    let names = complete(root.path(), "fish", &["wineo", "start", ""]);
    assert!(!names.contains(&"hades".to_owned()), "{names:?}");
    assert!(!names.contains(&"broken".to_owned()), "{names:?}");
}

#[test]
fn bash_and_zsh_callbacks_use_the_same_live_config() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    for shell in ["bash", "zsh"] {
        let names = complete(root.path(), shell, &["wineo", "start", "ha"]);
        assert!(names.contains(&"hades".to_owned()), "{shell}: {names:?}");
        let names = complete(
            root.path(),
            shell,
            &["wineo", "start", "hades", "--target", "ga"],
        );
        assert!(names.contains(&"game".to_owned()), "{shell}: {names:?}");
    }
}

#[test]
fn zsh_wrapper_completes_on_first_autoload() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wineo"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .args(["completions", "zsh"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let script = String::from_utf8(output.stdout).unwrap();
    assert!(script.starts_with("#compdef wineo\n"));
    assert!(script.contains("source <(COMPLETE=zsh wineo)"));
    assert!(script.contains("if [[ -n ${CURRENT-} ]]; then"));
    assert!(script.contains("_clap_dynamic_completer_wineo \"$@\""));
}

#[test]
fn fish_shell_sources_live_environment_and_target_completion() {
    if Command::new("fish").arg("--version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_wineo")).parent().unwrap();
    let path = std::env::join_paths(std::iter::once(binary_dir.to_owned()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let output = Command::new("fish")
        .args([
            "--no-config",
            "-c",
            "wineo completions fish | source; complete -C 'wineo start ha'; complete -C 'wineo start hades --target ga'",
        ])
        .env("PATH", path)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = String::from_utf8(output.stdout).unwrap();
    assert!(lines.lines().any(|line| line == "hades"), "{lines}");
    assert!(lines.lines().any(|line| line == "game"), "{lines}");
}
