//! Private branch passwords through the CLI: asked for at a terminal or read
//! from standard input, saved once GOG lists the branch, used by later
//! checks and updates, removed when refused or on logout.

use std::io::Write;
use std::process::Stdio;

use super::branches::{beta, installed_build, record, rows, words};
use super::*;

/// "SECRET" is what the fake GOG's output check looks for, so a password
/// that shows up on stdout or stderr fails the test that ran it.
const PASSWORD: &str = "SECRET-branch-pass";
const PROMPT: &str = "Branch beta of Papers, Please is private. Password: ";

/// Papers, Please installed at 1.2.76, with a private beta branch at 1.3.0.
fn private_world() -> (Fixture, GogApi) {
    let (f, api) = installed();
    beta(&api, "400", "1.3.0-beta");
    api.content.set_branch_password("beta", PASSWORD);
    (f, api)
}

fn keys_file(f: &Fixture) -> PathBuf {
    f.paths().host_data().join("gog-branches.key")
}

/// The password saved for the beta branch, if any.
fn saved(f: &Fixture) -> Option<String> {
    let text = fs::read_to_string(keys_file(f)).ok()?;
    let all: Value = serde_json::from_str(&text).unwrap();
    // By account, then game, then branch: one account logs in here.
    let accounts = all.as_object().unwrap();
    assert!(accounts.len() <= 1, "{} accounts", accounts.len());
    accounts
        .values()
        .find_map(|games| games[GAME.to_string()]["beta"].as_str())
        .map(str::to_owned)
}

/// Runs wineo with `input` on its standard input, which is not a terminal.
fn with_stdin(f: &Fixture, api: &GogApi, args: &[&str], input: &str) -> Output {
    let mut command = f.gog_command(api);
    command
        .args(args)
        .env("DISPLAY", ":0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    gog_api::checked(child.wait_with_output().unwrap())
}

fn in_terminal(f: &Fixture, api: &GogApi, args: &[&str]) -> Pty {
    let mut command = f.gog_command(api);
    command.env("DISPLAY", ":0").args(args);
    Pty::spawn(command)
}

/// Follows the beta branch with the password given on standard input.
fn follow_beta(f: &Fixture, api: &GogApi) {
    let out = with_stdin(
        f,
        api,
        &[
            "update",
            NAME,
            "--branch",
            "beta",
            "--password-stdin",
            "--yes",
        ],
        &format!("{PASSWORD}\n"),
    );
    assert_success(&out);
    assert_eq!(saved(f).as_deref(), Some(PASSWORD));
}

#[test]
fn the_password_is_asked_for_at_a_terminal_without_echo_and_saved_once_accepted() {
    let (f, api) = private_world();
    let mut pty = in_terminal(&f, &api, &["update", NAME, "--branch", "beta", "--yes"]);
    pty.expect(PROMPT);
    pty.send(PASSWORD);
    let (status, output) = pty.finish();
    assert!(status.success(), "{output:?}");
    assert!(!output.contains(PASSWORD), "the password was echoed");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(installed_build(&f), "400");
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
    let mode = fs::metadata(keys_file(&f)).unwrap().permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o600
    );
}

#[test]
fn password_stdin_reads_one_line_and_saves_it() {
    let (f, api) = private_world();
    let out = with_stdin(
        &f,
        &api,
        &[
            "update",
            NAME,
            "--branch",
            "beta",
            "--password-stdin",
            "--yes",
        ],
        &format!("{PASSWORD}\r\n"),
    );
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
    for text in [stdout(&out), stderr(&out)] {
        assert!(!text.contains(PASSWORD), "{text}");
    }
}

#[test]
fn without_a_terminal_or_password_stdin_the_password_is_asked_for_by_message() {
    let (f, api) = private_world();
    let out = update(&f, &api, &["--branch", "beta", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("error: Branch beta is private. Give its password with --password-stdin."),
        "{}",
        stderr(&out)
    );
    assert_eq!(saved(&f), None);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
}

#[test]
fn password_stdin_needs_a_branch_and_takes_no_all_or_check() {
    let (f, api) = installed();
    for args in [
        &["update", NAME, "--password-stdin"][..],
        &["update", "--all", "--password-stdin"],
        &["update", "--check", "--password-stdin"],
        &["update", NAME, "--builds", "--password-stdin"],
        &["update", NAME, "--build", "100", "--password-stdin"],
        &["update", NAME, "--latest", "--password-stdin"],
        &["import", "gog", "--game", "1", "--password-stdin"],
        &["import", "gog", "--password-stdin"],
    ] {
        let out = gog(&f, &api, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}

#[test]
fn a_saved_password_serves_checks_updates_all_and_the_listing() {
    let (f, api) = private_world();
    follow_beta(&f, &api);
    newer(&api);
    beta(&api, "500", "1.3.1-beta");
    api.content.set_branch_password("beta", PASSWORD);

    // A check sees the new beta build.
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    assert_success(&out);
    let status = &json_of(&out)["environments"][0];
    assert_eq!(status["branch"], "beta");
    assert_eq!(status["available"], "500");
    assert_eq!(status["update"], "available");

    // The listing marks the private branch's builds, and says so in JSON.
    let out = update(&f, &api, &["--builds"]);
    assert_success(&out);
    let table = rows(&out);
    assert_eq!(
        table[3],
        words(&[
            "beta",
            "500",
            "1.3.1-beta",
            "2026-01-02",
            "latest,",
            "followed,",
            "private"
        ]),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        table[4],
        words(&[
            "beta",
            "400",
            "1.3.0-beta",
            "2026-01-02",
            "installed,",
            "followed,",
            "private"
        ]),
        "{}",
        stdout(&out)
    );
    // The line stays while GOG has private branches, listed or not.
    assert!(
        stdout(&out).ends_with("GOG also has private branches. Name one with --branch NAME.\n"),
        "{}",
        stdout(&out)
    );
    let out = update(&f, &api, &["--builds", "--json"]);
    let listing = json_of(&out);
    let private: Vec<(String, bool)> = listing["builds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|build| {
            (
                build["build"].as_str().unwrap().to_owned(),
                build["private"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        private,
        [
            ("200".to_owned(), false),
            ("100".to_owned(), false),
            ("500".to_owned(), true),
            ("400".to_owned(), true),
        ]
    );
    assert_eq!(listing["private_branches"], true);

    // A plain update and --all use it too, with nothing asked.
    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.1-beta");
    beta(&api, "600", "1.3.2-beta");
    api.content.set_branch_password("beta", PASSWORD);
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.2-beta");
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
}

#[test]
fn entering_a_password_again_replaces_the_saved_one() {
    let (f, api) = private_world();
    follow_beta(&f, &api);
    api.content.set_branch_password("beta", "SECRET-changed");
    let out = with_stdin(
        &f,
        &api,
        &[
            "update",
            NAME,
            "--branch",
            "beta",
            "--password-stdin",
            "--yes",
        ],
        "SECRET-changed\n",
    );
    assert_success(&out);
    assert_eq!(saved(&f).as_deref(), Some("SECRET-changed"));
}

#[test]
fn a_refused_password_is_removed_and_said() {
    let (f, api) = private_world();
    follow_beta(&f, &api);
    // The owner changes it: the saved one is refused on the next use.
    api.content.set_branch_password("beta", "SECRET-changed");
    let refused = "GOG lists no branch beta for Papers, Please, or refused its password.";

    // A dry run leaves the password, and --all fails the game either way.
    let out = gog(&f, &api, &["update", "--all", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(saved(&f).is_some(), "a dry run keeps the password");
    // A check says so and removes it.
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    assert_success(&out);
    assert_eq!(json_of(&out)["environments"][0]["reason"], refused);
    assert_eq!(saved(&f), None);
    // Entered again, it is saved, and refused the same way by a table check.
    api.content.set_branch_password("beta", PASSWORD);
    follow_beta(&f, &api);
    api.content.set_branch_password("beta", "SECRET-changed");
    let out = gog(&f, &api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("can't compare"), "{text}");
    assert!(text.contains(refused), "{text}");
    assert_eq!(saved(&f), None);

    // An update that finds it refused says so too.
    api.content.set_branch_password("beta", PASSWORD);
    follow_beta(&f, &api);
    api.content.set_branch_password("beta", "SECRET-changed");
    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "error: GOG lists no branch beta for Papers, Please, or refused its password."
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(saved(&f), None);

    // A wrong one given on standard input is not saved.
    let out = with_stdin(
        &f,
        &api,
        &[
            "update",
            NAME,
            "--branch",
            "beta",
            "--password-stdin",
            "--yes",
        ],
        "SECRET-wrong\n",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("GOG lists no branch beta for Papers, Please, or refused its password."),
        "{}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("SECRET-wrong"));
    assert_eq!(saved(&f), None);
}

#[test]
fn a_typo_on_a_game_with_private_branches_asks_then_says_it_may_be_refused() {
    let (f, api) = private_world();
    let mut pty = in_terminal(&f, &api, &["update", NAME, "--branch", "bata", "--yes"]);
    pty.expect("Branch bata of Papers, Please is private. Password: ");
    pty.send(PASSWORD);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1));
    assert!(
        output.contains("GOG lists no branch bata for Papers, Please, or refused its password."),
        "{output:?}"
    );
    assert!(!output.contains(PASSWORD));
    assert_eq!(saved(&f), None);
}

#[test]
fn a_typo_on_a_game_without_private_branches_does_not_ask() {
    let (f, api) = installed();
    beta(&api, "400", "1.3.0-beta");
    let pty = in_terminal(&f, &api, &["update", NAME, "--branch", "bata", "--yes"]);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1));
    assert!(!output.contains("Password"), "{output:?}");
    assert!(
        output.contains(&format!(
            "GOG lists no branch bata for Papers, Please. See: wineo update {NAME} --builds"
        )),
        "{output:?}"
    );
}

#[test]
fn a_game_whose_password_is_missing_cannot_be_compared_and_asks_when_updated() {
    let (f, api) = private_world();
    follow_beta(&f, &api);
    newer(&api);
    beta(&api, "500", "1.3.1-beta");
    api.content.set_branch_password("beta", PASSWORD);
    // The keyring was reset.
    fs::remove_file(keys_file(&f)).unwrap();

    let note = format!("Enter its password again with: wineo update {NAME} --branch beta");
    let out = gog(&f, &api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("can't compare"), "{text}");
    assert!(text.contains(&note), "{text}");
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    let status = &json_of(&out)["environments"][0];
    assert_eq!(status["update"], "cant_compare");
    assert_eq!(status["reason"], note.as_str());
    assert_eq!(status["applicable"], false);

    // --all fails for that game without prompting.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("Failed {NAME}: {note}")),
        "{}",
        stderr(&out)
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");

    // Without a terminal, a plain update names a command that works.
    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1));
    let command = format!("wineo update {NAME} --branch beta --password-stdin");
    assert!(
        stderr(&out).contains(&format!(
            "error: Branch beta is private. Give its password with: {command}"
        )),
        "{}",
        stderr(&out)
    );
    let args: Vec<&str> = command.split(' ').skip(1).collect();
    let mut args = args;
    args.push("--yes");
    let out = with_stdin(&f, &api, &args, &format!("{PASSWORD}\n"));
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.1-beta");
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
    fs::remove_file(keys_file(&f)).unwrap();
    api.content.set_branch_password("beta", PASSWORD);

    // At a terminal, it asks, updates, and saves what was accepted.
    let mut pty = in_terminal(&f, &api, &["update", NAME, "--yes"]);
    pty.expect(PROMPT);
    pty.send(PASSWORD);
    let (status, output) = pty.finish();
    assert!(status.success(), "{output:?}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.1-beta");
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
}

#[test]
fn logging_out_removes_every_saved_password() {
    let (f, api) = private_world();
    follow_beta(&f, &api);
    assert!(keys_file(&f).exists());
    assert_success(&gog(&f, &api, &["import", "gog", "--logout"]));
    assert!(!keys_file(&f).exists());
}

#[test]
fn an_unfinished_import_of_a_private_branch_asks_for_the_password_too() {
    let (f, api) = world();
    beta(&api, "400", "1.3.0-beta");
    api.content.set_branch_password("beta", PASSWORD);
    let out = install(&f, &api, &["--branch", "beta"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Branch beta is private. Give its password with --password-stdin."),
        "{}",
        stderr(&out)
    );
    let out = with_stdin(
        &f,
        &api,
        &[
            "import",
            "gog",
            "--game",
            "1207658924",
            "--runner",
            "galaxy",
            "--yes",
            "--branch",
            "beta",
            "--password-stdin",
        ],
        &format!("{PASSWORD}\n"),
    );
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
}

#[test]
fn a_password_keeps_the_spaces_around_it_through_stdin_the_terminal_and_the_file() {
    let (f, api) = private_world();
    let spaced = "SECRET  edge spaces ";
    api.content.set_branch_password("beta", spaced);
    // Only the line ending goes, `\r\n` or not.
    for ending in ["\n", "\r\n"] {
        let out = with_stdin(
            &f,
            &api,
            &[
                "update",
                NAME,
                "--branch",
                "beta",
                "--password-stdin",
                "--yes",
            ],
            &format!("{spaced}{ending}"),
        );
        assert_success(&out);
        assert_eq!(saved(&f).as_deref(), Some(spaced));
        fs::remove_file(keys_file(&f)).unwrap();
    }
    // The server holds the same password, so the saved one works as is.
    follow_spaced(&f, &api, spaced);
    assert_eq!(saved(&f).as_deref(), Some(spaced));
    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);

    // And at a terminal.
    fs::remove_file(keys_file(&f)).unwrap();
    let mut pty = in_terminal(&f, &api, &["update", NAME, "--yes"]);
    pty.expect(PROMPT);
    pty.send(spaced);
    let (status, output) = pty.finish();
    assert!(status.success(), "{output:?}");
    assert_eq!(saved(&f).as_deref(), Some(spaced));

    // Nothing but a line ending is no password.
    fs::remove_file(keys_file(&f)).unwrap();
    let out = with_stdin(
        &f,
        &api,
        &[
            "update",
            NAME,
            "--branch",
            "beta",
            "--password-stdin",
            "--yes",
        ],
        "\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("No password was given on standard input."),
        "{}",
        stderr(&out)
    );
}

fn follow_spaced(f: &Fixture, api: &GogApi, password: &str) {
    let out = with_stdin(
        f,
        api,
        &[
            "update",
            NAME,
            "--branch",
            "beta",
            "--password-stdin",
            "--yes",
        ],
        &format!("{password}\n"),
    );
    assert_success(&out);
}

/// Drops what the sidecar saved of the operation's target, as a crash or an
/// old copy can leave it.
fn lose_pending(f: &Fixture) {
    let path =
        wineo_core::gog::folder::sidecar_file(&f.paths(), &fs::canonicalize(folder(f)).unwrap());
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(saved.as_object_mut().unwrap().remove("pending").is_some());
    fs::write(&path, saved.to_string()).unwrap();
}

/// The words of a command an error suggests, after `wineo`.
fn suggested(error: &str, prefix: &str) -> Vec<String> {
    let line = error
        .lines()
        .find(|line| line.contains(prefix))
        .unwrap_or_else(|| panic!("no suggestion in {error}"));
    let command = line.split_once(prefix).unwrap().1.trim();
    command.split(' ').map(str::to_owned).collect()
}

#[test]
fn the_suggested_command_finishes_an_interrupted_update_whose_password_and_target_are_gone() {
    let (f, api) = private_world();
    follow_beta(&f, &api);
    newer(&api);
    beta(&api, "500", "1.3.1-beta");
    api.content.set_branch_password("beta", PASSWORD);
    let mut pty = paused(&f, &api, &["update", NAME, "--yes"]);
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(state(&f).journal.is_some());
    lose_pending(&f);
    fs::remove_file(keys_file(&f)).unwrap();

    // Without a terminal, it names a command.
    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let args = suggested(&stderr(&out), "with: wineo ");
    assert_eq!(
        args,
        ["update", NAME, "--branch", "beta", "--password-stdin"]
    );

    // Another branch, or any other change, still waits for the update.
    for other in [
        &["--branch", "gamma", "--password-stdin"][..],
        &["--default-branch"],
        &["--build", "100"],
        &["--latest"],
    ] {
        let mut args = vec!["update", NAME];
        args.extend(other);
        args.push("--yes");
        let out = with_stdin(&f, &api, &args, &format!("{PASSWORD}\n"));
        assert_eq!(out.status.code(), Some(1), "{other:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(
                "has an unfinished GOG operation, so it can't change its branch or build yet."
            ),
            "{other:?}: {}",
            stderr(&out)
        );
    }
    assert_eq!(saved(&f), None);

    // The suggested command, as given, finishes it and keeps the password.
    let mut args = args.iter().map(String::as_str).collect::<Vec<_>>();
    args.push("--yes");
    let out = with_stdin(&f, &api, &args, &format!("{PASSWORD}\n"));
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.1-beta");
    assert!(state(&f).journal.is_none());
    assert_eq!(saved(&f).as_deref(), Some(PASSWORD));
    assert_eq!(record(&f).follow.branch.as_deref(), Some("beta"));
}
