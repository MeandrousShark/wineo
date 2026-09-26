use std::fs;
use std::path::Path;
use std::process::Command;

fn write_environment(root: &Path, name: &str, contents: &str) {
    let dir = root.join("config/wineo/environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), contents).unwrap();
}

fn complete(root: &Path, shell: &str, words: &[&str]) -> Vec<String> {
    complete_with(root, None, shell, words)
}

fn complete_with(root: &Path, portable: Option<&Path>, shell: &str, words: &[&str]) -> Vec<String> {
    complete_in(root, portable, root, shell, words)
}

fn complete_in(
    root: &Path,
    portable: Option<&Path>,
    cwd: &Path,
    shell: &str,
    words: &[&str],
) -> Vec<String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wineo"));
    command
        .current_dir(cwd)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env_remove("WINEO_PORTABLE_DIR")
        .env("COMPLETE", shell)
        .env("_CLAP_COMPLETE_INDEX", (words.len() - 1).to_string())
        .env("_CLAP_IFS", "\n")
        .arg("--")
        .args(words);
    if let Some(dir) = portable {
        command.env("WINEO_PORTABLE_DIR", dir);
    }
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
        .env_remove("WINEO_PORTABLE_DIR")
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

fn write_portable(dir: &Path, name: &str, contents: &str) {
    let dir = dir.join("config/environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), contents).unwrap();
}

#[test]
fn completion_reads_the_selected_portable_folder() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let one = root.path().join("one");
    let two = root.path().join("two");
    write_portable(
        &one,
        "hermes",
        "prefix = 'prefixes/hermes'\nrunner = 'wine'\narch = 'win64'\n[targets.boat]\nexe = 'boat.exe'\n",
    );
    write_portable(
        &two,
        "helios",
        "prefix = 'prefixes/helios'\nrunner = 'wine'\narch = 'win64'\n",
    );
    let one_arg = one.to_str().unwrap();
    let two_arg = format!("--portable={}", two.display());

    let names = complete(
        root.path(),
        "fish",
        &["wineo", "--portable", one_arg, "start", "he"],
    );
    assert_eq!(names, ["hermes"]);
    let names = complete(root.path(), "fish", &["wineo", "start", &two_arg, "he"]);
    assert_eq!(names, ["helios"]);
    let names = complete_with(root.path(), Some(&two), "fish", &["wineo", "start", "he"]);
    assert_eq!(names, ["helios"]);
    let names = complete_with(
        root.path(),
        Some(&two),
        "fish",
        &["wineo", "--portable", one_arg, "start", "he"],
    );
    assert_eq!(names, ["hermes"]);
    let names = complete(
        root.path(),
        "fish",
        &[
            "wineo",
            "--portable",
            one_arg,
            "run",
            "hermes",
            "--target",
            "",
        ],
    );
    assert_eq!(names, ["boat"]);
    let names = complete(root.path(), "fish", &["wineo", "start", "h"]);
    assert!(names.contains(&"hades".to_owned()), "{names:?}");
    assert!(!names.contains(&"hermes".to_owned()), "{names:?}");
    assert!(!one.join("config/templates").exists());
    assert!(!root.path().join("missing").exists());
    complete(
        root.path(),
        "fish",
        &[
            "wineo",
            "--portable",
            root.path().join("missing").to_str().unwrap(),
            "start",
            "h",
        ],
    );
    assert!(!root.path().join("missing").exists());
}

#[test]
fn art_completes_environments_targets_and_kinds() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let names = complete(root.path(), "fish", &["wineo", "art", "list", "ha"]);
    assert_eq!(names, ["hades", "halo"]);
    let names = complete(root.path(), "fish", &["wineo", "art", "fetch", "hades.g"]);
    assert_eq!(names, ["hades.game"]);
    let kinds = complete(
        root.path(),
        "fish",
        &["wineo", "art", "fetch", "hades", "--only", ""],
    );
    assert_eq!(kinds, ["icon", "grid", "hero", "logo"]);
    for action in ["set", "rm"] {
        let names = complete(root.path(), "fish", &["wineo", "art", action, "ha"]);
        assert_eq!(names, ["hades", "halo"], "{action}");
        let names = complete(root.path(), "fish", &["wineo", "art", action, "hades."]);
        assert_eq!(names, ["hades.game", "hades.launcher"], "{action}");
        let names = complete(root.path(), "fish", &["wineo", "art", action, "hades.l"]);
        assert_eq!(names, ["hades.launcher"], "{action}");
        let kinds: Vec<String> =
            complete(root.path(), "fish", &["wineo", "art", action, "hades", ""])
                .into_iter()
                .filter(|word| !word.starts_with('-'))
                .collect();
        assert_eq!(kinds, ["icon", "grid", "hero", "logo"], "{action}");
    }
}

#[test]
fn path_arguments_complete_paths_of_their_kind() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let games = root.path().join("games");
    fs::create_dir_all(games.join("sub")).unwrap();
    fs::write(games.join("readme.txt"), "").unwrap();
    fs::write(games.join("run.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(games.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();

    let complete = |words: &[&str]| {
        let mut found = complete_in(root.path(), None, root.path(), "fish", words);
        found.sort();
        found
    };
    let dirs = ["games/sub/"];
    let files = ["games/readme.txt", "games/run.sh", "games/sub/"];

    assert_eq!(complete(&["wineo", "new", "--exe", "games/"]), files);
    assert_eq!(
        complete(&["wineo", "new", "--install-exe", "games/"]),
        files
    );
    assert_eq!(complete(&["wineo", "exec", "hades", "games/"]), files);
    for words in [
        &["wineo", "new", "--dir", "games/"][..],
        &["wineo", "clone", "hades", "copy", "--dir", "games/"],
        &["wineo", "import", "prefix", "games/"],
        &["wineo", "import", "prefix", "--dir", "games/"],
        &["wineo", "import", "heroic", "--config-dir", "games/"],
        &["wineo", "import", "heroic", "--dir", "games/"],
    ] {
        assert_eq!(complete(words), dirs, "{words:?}");
    }
}
