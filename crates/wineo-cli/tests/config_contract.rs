//! What unknown keys, newer versions, and a broken global config do across
//! commands. The parser's own matrix is in wineo-core's config tests.

mod common;

use std::fs;

use common::*;

const UNKNOWN_KEY_HINT: &str =
    "A newer version of wineo may have written it; update wineo or remove the key.";
const VERSION_HINT: &str = "A newer version of wineo wrote it; update wineo to use this file.";

const TARGET: &str = "[targets.hades]\nexe = \"hades\"\n";

fn environment_path(f: &Fixture, name: &str) -> std::path::PathBuf {
    f.config_dir().join(format!("environments/{name}.toml"))
}

#[test]
fn set_and_new_refuse_an_unknown_global_key() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    f.global_config("foo = 1\n");
    let config_before = fs::read_to_string(environment_path(&f, "hades")).unwrap();

    let set = f.run(&["set", "hades", "gamemode=true"]);
    assert!(!set.status.success());
    let message = stderr(&set);
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(
        message.trim_end().ends_with(UNKNOWN_KEY_HINT),
        "the hint is the last line: {message}"
    );
    assert_eq!(
        fs::read_to_string(environment_path(&f, "hades")).unwrap(),
        config_before
    );

    let new = f.run(&["new", "-n", "tool"]);
    assert!(!new.status.success());
    let message = stderr(&new);
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(message.contains(UNKNOWN_KEY_HINT), "{message}");
    assert!(!environment_path(&f, "tool").exists());
}

#[test]
fn set_refuses_an_unsupported_global_version_with_a_hint() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    f.global_config("schema_version = 2\n");
    let set = f.run(&["set", "hades", "gamemode=true"]);
    assert!(!set.status.success());
    let message = stderr(&set);
    assert!(
        message.contains("unsupported schema_version 2 in "),
        "{message}"
    );
    assert!(message.trim_end().ends_with(VERSION_HINT), "{message}");
}

#[test]
fn an_environment_with_an_unknown_key_shows_the_hint() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    let path = environment_path(&f, "hades");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}gamemod = true\n")).unwrap();

    let status = f.run(&["status", "hades"]);
    assert!(!status.status.success());
    let message = stderr(&status);
    assert!(message.contains("unknown field `gamemod`"), "{message}");
    assert!(
        message.trim_end().ends_with(UNKNOWN_KEY_HINT),
        "the hint is the last line: {message}"
    );
}

#[test]
fn a_template_with_an_unknown_key_shows_the_hint() {
    let f = Fixture::new();
    f.write_template("typo", "gamemod = true\n");
    let new = f.run(&["new", "-n", "tool", "--template", "typo"]);
    assert!(!new.status.success());
    let message = stderr(&new);
    assert!(message.contains("unknown field `gamemod`"), "{message}");
    assert!(message.trim_end().ends_with(UNKNOWN_KEY_HINT), "{message}");
    assert!(!environment_path(&f, "tool").exists());
}

#[test]
fn status_warns_and_goes_on_for_an_unknown_global_key() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    f.global_config("foo = 1\n");
    let status = f.run(&["status", "hades"]);
    assert_success(&status);
    let message = stderr(&status);
    assert!(
        message.starts_with("warning: using default settings: invalid config "),
        "{message}"
    );
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(message.trim_end().ends_with(UNKNOWN_KEY_HINT), "{message}");
    assert!(stdout(&status).starts_with("hades: inactive\n"));
}

#[test]
fn validate_reports_an_unknown_global_key_as_a_problem() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    f.global_config("foo = 1\n");
    let report = f.run(&["validate", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&report)).unwrap();
    let problem = json["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|problem| problem["code"] == "global_config_invalid")
        .unwrap_or_else(|| panic!("{json}"));
    let message = problem["message"].as_str().unwrap();
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(message.contains(UNKNOWN_KEY_HINT), "{message}");
    assert!(!report.status.success());
}

/// A global config that can't be read as one still lets a command run on
/// defaults, but says so on stderr and leaves stdout alone.
#[test]
fn a_global_config_that_falls_back_warns_on_stderr_only() {
    for broken in ["stop_timeout = [\n", "stop_timeout = \"soon\"\n"] {
        let f = Fixture::new();
        f.environment("hades", TARGET);
        let clean = f.run(&["set", "hades", "gamemode=true"]);
        assert_success(&clean);
        assert!(!stderr(&clean).contains("using default settings"));
        let before = stdout(&f.run(&["status", "hades", "--json"]));

        f.global_config(broken);
        let set = f.run(&["set", "hades", "gamemode=true"]);
        assert_success(&set);
        let message = stderr(&set);
        assert_eq!(
            message.matches("warning: using default settings: ").count(),
            1,
            "{broken}: {message}"
        );
        assert_eq!(stdout(&set), "");

        // `status` loads the global config strictly, so it warns on its own.
        let status = f.run(&["status", "hades", "--json"]);
        assert_success(&status);
        assert!(
            stderr(&status).starts_with("warning: "),
            "{}",
            stderr(&status)
        );
        assert_eq!(stdout(&status), before, "--json stdout is unchanged");
    }
}

#[test]
fn restore_warns_when_the_global_config_fell_back() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    let removed = f.run(&["rm", "hades"]);
    assert_success(&removed);
    let id = trash_id(&removed);

    f.global_config("stop_timeout = [\n");
    let restored = f.run(&["restore", &id]);
    assert_success(&restored);
    assert_eq!(
        stderr(&restored)
            .matches("warning: using default settings: ")
            .count(),
        1,
        "{}",
        stderr(&restored)
    );
    assert!(stdout(&restored).starts_with("Restored environment hades"));
}

#[test]
fn restore_refuses_an_unknown_global_key_after_putting_the_environment_back() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    let removed = f.run(&["rm", "hades"]);
    let id = trash_id(&removed);

    f.global_config("foo = 1\n");
    let restored = f.run(&["restore", &id]);
    let message = stderr(&restored);
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(message.contains(UNKNOWN_KEY_HINT), "{message}");
}

#[test]
fn rm_with_a_prefix_warns_when_the_global_config_fell_back() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    f.global_config("stop_timeout = [\n");
    let removed = f.run(&["rm", "hades", "--delete-prefix"]);
    assert_success(&removed);
    assert_eq!(
        stderr(&removed)
            .matches("warning: using default settings: ")
            .count(),
        1,
        "{}",
        stderr(&removed)
    );
}

#[test]
fn rm_with_a_prefix_refuses_an_unknown_global_key() {
    let f = Fixture::new();
    f.environment("hades", TARGET);
    f.global_config("foo = 1\n");
    let removed = f.run(&["rm", "hades", "--delete-prefix"]);
    assert!(!removed.status.success());
    assert!(
        stderr(&removed).contains(UNKNOWN_KEY_HINT),
        "{}",
        stderr(&removed)
    );
    assert!(environment_path(&f, "hades").exists());
}
