//! `wineo new --dry-run` reviews a Wine or native environment without writes.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

use common::*;

const RUNNER_MARKER: &str = "dry-run-runner-started";

fn guard_runners(f: &Fixture) {
    let script = r#"#!/bin/sh
printf '%s %s\n' "$0" "$*" >>"$WINEO_TEST_RUNNER_MARKER"
exit 97
"#;
    for runner in ["wine", "wineserver", "umu-run"] {
        fs::write(f.root.path().join("bin").join(runner), script).unwrap();
    }
}

fn command(f: &Fixture, args: &[&str]) -> Output {
    command_with_home(f, args, false)
}

fn command_with_home(f: &Fixture, args: &[&str], at_home: bool) -> Output {
    let mut command = if at_home {
        f.wineo_at_home()
    } else {
        f.wineo()
    };
    command
        .env(
            "WINEO_TEST_RUNNER_MARKER",
            f.root.path().join(RUNNER_MARKER),
        )
        .args(args);
    command.output().expect("wineo should start")
}

fn dry_run(f: &Fixture, args: &[&str]) -> Output {
    dry_run_with_home(f, args, false)
}

fn dry_run_with_home(f: &Fixture, args: &[&str], at_home: bool) -> Output {
    let before = snapshot(f.root.path());
    let result = command_with_home(f, args, at_home);
    let after = snapshot(f.root.path());
    assert_eq!(before, after, "dry run changed the fixture tree: {args:?}");
    assert!(
        !f.root.path().join(RUNNER_MARKER).exists(),
        "dry run started Wine or umu: {args:?}"
    );
    result
}

fn executable(f: &Fixture, relative: &str) -> String {
    let path = f.root.path().join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "game executable").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn text(output: &Output) -> String {
    stdout(output)
}

fn assert_review(output: &Output, expected: &str) {
    assert_success(output);
    assert_eq!(text(output), expected);
}

#[test]
fn dry_run_reviews_a_fresh_wine_prefix_without_writing() {
    let f = Fixture::new();
    guard_runners(&f);

    let result = dry_run(&f, &["new", "-n", "fresh", "--dry-run"]);
    assert_review(
        &result,
        &format!(
            "Would create environment fresh\n  prefix: {}/data/wineo/prefixes/fresh (new)\n  runner: wine\n  arch: win64\nDry run, nothing was created.\n",
            f.root.path().display()
        ),
    );
}

#[test]
fn dry_run_reviews_an_adopted_prefix_without_writing() {
    let f = Fixture::new();
    let prefix = f.root.path().join("data/wineo/prefixes/adopted");
    fs::create_dir_all(&prefix).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    guard_runners(&f);

    let result = dry_run(&f, &["new", "-n", "adopted", "--dry-run"]);
    assert_review(
        &result,
        &format!(
            "Would create environment adopted\n  prefix: {}/data/wineo/prefixes/adopted (existing prefix, adopted)\n  runner: wine\n  arch: win64\nDry run, nothing was created.\n",
            f.root.path().display()
        ),
    );
}

#[test]
fn dry_run_reviews_resolved_template_components() {
    let f = Fixture::new();
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.install_build("vkd3d", "2.14", &["x64", "x86"]);
    guard_runners(&f);

    let result = dry_run(
        &f,
        &["new", "-n", "gaming", "--template", "gaming", "--dry-run"],
    );
    assert_success(&result);
    let output = text(&result);
    assert!(
        output.starts_with("Would create environment gaming\n"),
        "{output}"
    );
    assert!(output.contains("  template: gaming, win64\n"), "{output}");
    assert!(
        output.contains("  components: dxvk dxvk-2.7, vkd3d-proton 2.14\n"),
        "{output}"
    );
    assert!(
        output.ends_with("Dry run, nothing was created.\n"),
        "{output}"
    );
}

#[test]
fn dry_run_notes_that_a_proton_template_omits_its_components() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.write_template(
        "proton",
        "runner = \"GE-Proton9-20\"\ndxvk = \"latest\"\nvkd3d = \"latest\"\n",
    );
    guard_runners(&f);

    let result = dry_run(
        &f,
        &[
            "new",
            "-n",
            "proton-game",
            "--template",
            "proton",
            "--dry-run",
        ],
    );
    assert_success(&result);
    let output = text(&result);
    assert!(output.contains("  template: proton, win64\n"), "{output}");
    assert!(!output.contains("  components:"), "{output}");
    assert!(
        output.ends_with("Dry run, nothing was created.\n"),
        "{output}"
    );
    assert_eq!(
        stderr(&result),
        "GE-Proton9-20 bundles its own DXVK and vkd3d-proton, so the template's are left out\n"
    );
}

#[test]
fn dry_run_reviews_a_found_exe_and_desktop_entry() {
    let f = Fixture::new();
    let exe = executable(&f, "games/Game.exe");
    guard_runners(&f);

    let result = dry_run(
        &f,
        &["new", "-n", "targeted", "--exe", &exe, "-d", "--dry-run"],
    );
    assert_success(&result);
    let output = text(&result);
    assert!(
        output.contains(&format!("  target: targeted -> {exe}\n")),
        "{output}"
    );
    assert!(output.contains("  desktop entry: targeted\n"), "{output}");
    assert!(
        output.ends_with("Dry run, nothing was created.\n"),
        "{output}"
    );
}

#[test]
fn dry_run_fails_for_a_missing_exe_without_writing() {
    let f = Fixture::new();
    let exe = f
        .root
        .path()
        .join("games/missing.exe")
        .display()
        .to_string();
    guard_runners(&f);

    let result = dry_run(
        &f,
        &["new", "-n", "missing-exe", "--exe", &exe, "--dry-run"],
    );
    assert!(!result.status.success());
    assert_eq!(stderr(&result), format!("error: {exe} was not found.\n"));
    assert_eq!(stdout(&result), "");
}

#[test]
fn dry_run_marks_windows_exes_unchecked() {
    let f = Fixture::new();
    guard_runners(&f);

    let result = dry_run(
        &f,
        &[
            "new",
            "-n",
            "windows-exe",
            "--exe",
            r"Z:\Games\Game.exe",
            "--dry-run",
        ],
    );
    assert_success(&result);
    assert!(
        text(&result).contains(
            r"  target: windows-exe -> Z:\Games\Game.exe (checked once the prefix exists)"
        ),
        "{}",
        text(&result)
    );
}

#[test]
fn dry_run_checks_an_installer_and_shows_it_in_the_review() {
    let f = Fixture::new();
    let installer = f.root.path().join("games/setup.exe");
    fs::create_dir_all(installer.parent().unwrap()).unwrap();
    fs::write(&installer, "installer").unwrap();
    let installer = installer.display().to_string();
    guard_runners(&f);

    let result = dry_run(
        &f,
        &[
            "new",
            "-n",
            "installer-game",
            "--install-exe",
            &installer,
            "--dry-run",
        ],
    );
    assert_success(&result);
    assert!(
        text(&result).contains(&format!("  installer: {installer}\n")),
        "{}",
        text(&result)
    );

    let missing = f
        .root
        .path()
        .join("games/missing-setup.exe")
        .display()
        .to_string();
    let real_result = command(
        &f,
        &["new", "-n", "missing-installer", "--install-exe", &missing],
    );
    let missing_result = dry_run(
        &f,
        &[
            "new",
            "-n",
            "missing-installer",
            "--install-exe",
            &missing,
            "--dry-run",
        ],
    );
    assert!(!missing_result.status.success());
    assert_eq!(missing_result.status.code(), real_result.status.code());
    assert_eq!(stdout(&missing_result), stdout(&real_result));
    assert_eq!(stderr(&missing_result), stderr(&real_result));
}

#[test]
fn dry_run_reviews_native_environments_with_and_without_an_exe() {
    let f = Fixture::new();
    let exe = executable(&f, "games/native-game");
    guard_runners(&f);

    let bare = dry_run(&f, &["new", "-n", "native-bare", "--native", "--dry-run"]);
    assert_review(
        &bare,
        "Would create native environment native-bare\nDry run, nothing was created.\n",
    );

    let with_exe = dry_run(
        &f,
        &[
            "new",
            "-n",
            "native-game",
            "--native",
            "--exe",
            &exe,
            "-d",
            "--dry-run",
        ],
    );
    assert_review(
        &with_exe,
        &format!(
            "Would create native environment native-game\n  target: native-game -> {exe}\n  desktop entry: native-game\nDry run, nothing was created.\n"
        ),
    );
}

#[test]
fn dry_run_failures_match_the_real_run_for_taken_names_and_unknown_runners() {
    let f = Fixture::new();
    f.environment("taken", "");
    guard_runners(&f);

    let real_taken = command(&f, &["new", "-n", "taken"]);
    let dry_taken = dry_run(&f, &["new", "-n", "taken", "--dry-run"]);
    assert_eq!(dry_taken.status.code(), real_taken.status.code());
    assert_eq!(stdout(&dry_taken), stdout(&real_taken));
    assert_eq!(stderr(&dry_taken), stderr(&real_taken));

    let real_runner = command(&f, &["new", "-n", "unknown-runner", "--runner", "nope"]);
    let dry_runner = dry_run(
        &f,
        &[
            "new",
            "-n",
            "unknown-runner",
            "--runner",
            "nope",
            "--dry-run",
        ],
    );
    assert_eq!(dry_runner.status.code(), real_runner.status.code());
    assert_eq!(stdout(&dry_runner), stdout(&real_runner));
    assert_eq!(stderr(&dry_runner), stderr(&real_runner));
}

#[test]
fn dry_run_shows_an_external_runner_owner() {
    let f = Fixture::new();
    f.global_config("external_runners = true\n");
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");
    guard_runners(&f);

    let result = dry_run_with_home(
        &f,
        &[
            "new",
            "-n",
            "steam-runner",
            "--runner",
            "GE-Proton11-3",
            "--dry-run",
        ],
        true,
    );
    assert_success(&result);
    assert!(
        text(&result).contains("  runner: GE-Proton11-3 (Steam)\n"),
        "{}",
        text(&result)
    );
}

#[test]
fn dry_run_reviews_an_alias_and_its_resolved_runner() {
    let f = Fixture::new();
    f.install_proton("GE-Proton11-4");
    guard_runners(&f);

    let result = dry_run(
        &f,
        &[
            "new",
            "-n",
            "alias-game",
            "--runner",
            "GE-Proton-Latest",
            "--dry-run",
        ],
    );
    assert_success(&result);
    assert!(
        text(&result).contains("  runner: GE-Proton-Latest -> GE-Proton11-4\n"),
        "{}",
        text(&result)
    );
}

#[test]
fn dry_run_requires_a_name_and_cannot_be_interactive() {
    let f = Fixture::new();
    guard_runners(&f);

    let missing_name = dry_run(&f, &["new", "--dry-run"]);
    assert!(!missing_name.status.success());
    assert_eq!(
        stderr(&missing_name),
        "error: wineo new needs a terminal without -n. Use: wineo new -n <name> [-t <template>] [--native] [--exe <path> [-d]] [--install-exe <path>]\n"
    );

    let interactive = dry_run(
        &f,
        &["new", "-n", "interactive", "--dry-run", "--interactive"],
    );
    assert!(!interactive.status.success());
    assert!(
        stderr(&interactive).contains("cannot be used with"),
        "{}",
        stderr(&interactive)
    );
}

#[test]
fn dry_run_art_without_a_key_prints_the_existing_skip_note() {
    let f = Fixture::new();
    guard_runners(&f);

    let result = dry_run(&f, &["new", "-n", "no-art-key", "--art", "--dry-run"]);
    assert_success(&result);
    assert!(
        text(&result).ends_with("Dry run, nothing was created.\n"),
        "{}",
        text(&result)
    );
    assert_eq!(
        stderr(&result),
        "note: run `wineo art key` to fetch art from SteamGridDB\n"
    );
}

#[test]
fn dry_run_skips_art_key_lookup_when_no_kinds_are_enabled() {
    let f = Fixture::new();
    f.global_config("[art]\nfetch = []\n");
    guard_runners(&f);

    let result = dry_run(&f, &["new", "-n", "art-disabled", "--art", "--dry-run"]);
    assert_success(&result);
    assert_eq!(stderr(&result), "");
    assert!(
        !text(&result).contains("Art from SteamGridDB"),
        "{}",
        text(&result)
    );
    assert!(text(&result).ends_with("Dry run, nothing was created.\n"));
}

#[test]
fn dry_run_art_search_failure_warns_without_changing_the_exit_code() {
    let f = Fixture::new();
    guard_runners(&f);

    let before = snapshot(f.root.path());
    let mut command = f.wineo();
    let result = command
        .args(["new", "-n", "art-search-failed", "--art", "--dry-run"])
        .env(
            "WINEO_TEST_RUNNER_MARKER",
            f.root.path().join(RUNNER_MARKER),
        )
        .env("WINEO_STEAMGRIDDB_KEY", "test-key")
        .env("HTTPS_PROXY", "http://127.0.0.1:0")
        .env("ALL_PROXY", "http://127.0.0.1:0")
        .env("NO_PROXY", "")
        .output()
        .expect("wineo should start");
    let after = snapshot(f.root.path());
    assert_eq!(before, after, "failed art search changed the fixture tree");
    assert!(!f.root.path().join(RUNNER_MARKER).exists());
    assert_success(&result);
    assert!(
        stderr(&result).contains("warning: art fetch failed:"),
        "{}",
        stderr(&result)
    );
    assert!(
        text(&result).ends_with("Dry run, nothing was created.\n"),
        "{}",
        text(&result)
    );
    assert!(
        !f.root
            .path()
            .join("data/wineo/art/art-search-failed")
            .exists()
    );
}
