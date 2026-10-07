use std::io;
use std::process::{Command, Output, Stdio};

mod common;

fn closed_stdout(command: &mut Command) -> Output {
    let (reader, writer) = io::pipe().unwrap();
    drop(reader);

    command
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn assert_quiet_closed_pipe(output: &Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(141), "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn templates_closed_stdout_does_not_panic() {
    let fixture = common::Fixture::new();
    let output = closed_stdout(fixture.wineo().arg("templates"));
    assert_quiet_closed_pipe(&output);
}

#[test]
fn generated_completions_closed_stdout_does_not_panic() {
    let fixture = common::Fixture::new();
    let output = closed_stdout(fixture.wineo().args(["completions", "elvish"]));
    assert_quiet_closed_pipe(&output);
}

#[test]
fn dynamic_completions_closed_stdout_does_not_print_error() {
    let fixture = common::Fixture::new();
    let control = fixture.wineo().env("COMPLETE", "fish").output().unwrap();
    assert!(control.status.success());
    assert!(!control.stdout.is_empty());

    let output = closed_stdout(fixture.wineo().env("COMPLETE", "fish"));
    assert_quiet_closed_pipe(&output);
}

#[test]
fn config_print_closed_stdout_exits_141() {
    let fixture = common::Fixture::new();
    fixture.environment("app", "");
    let config = fixture.config_dir().join("environments/app.toml");
    let content = std::fs::read_to_string(&config).unwrap();
    std::fs::write(config, content.trim_end()).unwrap();
    let output = closed_stdout(fixture.wineo().args(["config", "app"]));
    assert_quiet_closed_pipe(&output);
}

#[test]
fn list_json_closed_stdout_exits_141() {
    let fixture = common::Fixture::new();
    let output = closed_stdout(fixture.wineo().args(["list", "--json"]));
    assert_quiet_closed_pipe(&output);
}

#[test]
fn templates_warning_closed_stderr_exits_141() {
    let fixture = common::Fixture::new();
    std::fs::create_dir_all(fixture.config_dir().join("templates")).unwrap();
    std::fs::write(
        fixture.config_dir().join("templates/broken.toml"),
        "[broken",
    )
    .unwrap();
    let (reader, writer) = io::pipe().unwrap();
    drop(reader);
    let output = fixture
        .wineo()
        .arg("templates")
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(141));
}

#[test]
fn error_message_closed_stderr_exits_141() {
    let fixture = common::Fixture::new();
    let (reader, writer) = io::pipe().unwrap();
    drop(reader);
    let output = fixture
        .wineo()
        .args(["config", "missing"])
        .stderr(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(141));
    assert!(output.stdout.is_empty());
}

#[test]
fn help_closed_stdout_exits_141() {
    let fixture = common::Fixture::new();
    let output = closed_stdout(fixture.wineo().arg("--help"));
    assert_quiet_closed_pipe(&output);
}

#[test]
fn usage_error_closed_stderr_exits_141() {
    let fixture = common::Fixture::new();
    let (reader, writer) = io::pipe().unwrap();
    drop(reader);
    let output = fixture
        .wineo()
        .arg("--invalid")
        .stderr(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(141));
    assert!(output.stdout.is_empty());
}

#[test]
fn logs_closed_stdout_exits_141() {
    let fixture = common::Fixture::new();
    fixture.environment("app", "");
    let log = fixture.root.path().join("state/wineo/app/output.log");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(log, "Wine output\n").unwrap();
    let output = closed_stdout(fixture.wineo().args(["logs", "app"]));
    assert_quiet_closed_pipe(&output);
}
