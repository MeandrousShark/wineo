//! `wineo import epic`: logging in and out, listing the owned games, and
//! the question that downloads Legendary, against a fake Legendary.

mod common;

use std::fs;
use std::process::{Command, ExitStatus, Output};

use common::epic::{CODE, Epic, Fake};
use common::pty::{Pty, Seen};
use common::{stderr, stdout};
use serde_json::{Value, json};

const QUESTION: &str =
    "Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it? [Y/n]: ";
const HOST_ONLY: &str = "Epic games live in the host library. Run this without --portable.";
const LOGGED_OUT: &str = "Not logged in to Epic. Log in with wineo import epic --login.";
const NO_PYTHON: &str = "Epic needs Python 3.10 or newer to run Legendary.";

fn command(epic: &Epic, args: &[&str]) -> Command {
    let mut command = epic.command();
    command.args(args);
    command
}

/// With a stand-in for the download, so wineo's own copy can be fetched.
fn with_stand_in(epic: &Epic, args: &[&str]) -> Command {
    let stand_in = epic.stand_in(&Fake::default());
    let mut command = command(epic, args);
    command.env("WINEO_TEST_LEGENDARY_FILE", stand_in);
    command
}

/// Answers the login question with `pasted`, at a terminal.
fn paste(mut pty: Pty, pasted: &str) -> (ExitStatus, Seen) {
    pty.expect("Code: ");
    pty.send(pasted);
    pty.finish()
}

fn json_out(output: &Output) -> Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|error| panic!("not JSON ({error}): {}", stdout(output)))
}

/// Nothing wineo printed or left in a file holds the code, and nothing it
/// started is still running.
#[track_caller]
fn assert_code_kept_and_nothing_left(epic: &Epic, seen: &Seen) {
    assert!(
        !seen.raw().contains("SECRET"),
        "the code showed up on the terminal"
    );
    assert_eq!(
        epic.files_holding("SECRET"),
        Vec::<std::path::PathBuf>::new()
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[track_caller]
fn assert_fails_with(output: &Output, message: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", stderr(output));
    assert!(
        stderr(output).contains(message),
        "expected {message:?} in:\n{}",
        stderr(output)
    );
}

#[test]
fn login_opens_the_page_reads_the_code_unseen_and_names_the_account() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let mut pty = Pty::spawn(command(&epic, &["import", "epic", "--login"]));
    pty.expect("Log in to Epic in your browser. If it doesn't open, go to:");
    pty.expect("https://legendary.gl/epiclogin");
    pty.expect("Then paste what the page shows, or only its authorizationCode.");
    let (status, seen) = paste(pty, CODE);
    assert!(status.success(), "{seen}");
    assert!(seen.contains("Logged in to Epic as REDACTED."), "{seen}");
    assert!(epic.logged_in());
    assert_eq!(epic.browser_calls(), ["https://legendary.gl/epiclogin"]);
    assert_eq!(
        epic.calls(),
        [
            "--version",
            "status --offline --json",
            "auth --code <code>",
            "status --offline --json",
        ]
    );
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn login_takes_the_whole_page_epic_shows() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let page = format!(
        r#"{{"warning":"Do not share this code","redirectUrl":"https://localhost/launcher/authorized?code={CODE}","authorizationCode":"{CODE}","sid":null}}"#
    );
    let pty = Pty::spawn(command(&epic, &["import", "epic", "--login"]));
    let (status, seen) = paste(pty, &page);
    assert!(status.success(), "{seen}");
    assert!(epic.logged_in());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn a_refused_login_exits_1_without_repeating_the_code() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let pty = Pty::spawn(command(&epic, &["import", "epic", "--login"]));
    let (status, seen) = paste(pty, "SECRETwrong0123");
    assert_eq!(status.code(), Some(1), "{seen}");
    assert!(seen.contains("Epic didn't accept that login"), "{seen}");
    assert!(!epic.logged_in());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn something_that_isnt_a_code_is_refused_before_legendary_sees_it() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let pty = Pty::spawn(command(&epic, &["import", "epic", "--login"]));
    let (status, seen) = paste(pty, "SECRET not-a-code");
    assert_eq!(status.code(), Some(1), "{seen}");
    assert!(
        seen.contains(
            "That isn't an Epic login code. Paste the code, or the whole page Epic showed."
        ),
        "{seen}"
    );
    assert!(!epic.calls().iter().any(|call| call.starts_with("auth")));
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn logging_in_again_deletes_the_saved_login_first() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let pty = Pty::spawn(command(&epic, &["import", "epic", "--login"]));
    let (status, seen) = paste(pty, CODE);
    assert!(status.success(), "{seen}");
    let calls = epic.calls();
    let delete = calls.iter().position(|call| call == "auth --delete");
    let code = calls.iter().position(|call| call == "auth --code <code>");
    assert!(
        delete.is_some() && delete < code,
        "auth --delete didn't come first: {calls:?}"
    );
    assert!(epic.logged_in());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn logging_in_needs_a_terminal_and_runs_nothing_without_one() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let output = epic.run(&["import", "epic", "--login"]);
    assert_fails_with(&output, "Logging in to Epic needs a terminal.");
    assert_eq!(epic.calls(), Vec::<String>::new());
    assert_eq!(epic.browser_calls(), Vec::<String>::new());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_login_dry_run_needs_no_terminal_and_changes_nothing() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let output = epic.run(&["import", "epic", "--login", "--dry-run", "--yes"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Dry run, nothing was changed.\n");
    assert_eq!(epic.calls(), ["--version"]);
    assert!(!epic.logged_in());
    assert_eq!(epic.browser_calls(), Vec::<String>::new());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn answering_no_to_the_download_is_a_cancel() {
    let epic = Epic::new();
    epic.python("3.14.0");
    let mut pty = Pty::spawn(with_stand_in(&epic, &["import", "epic", "--login"]));
    pty.expect(QUESTION);
    pty.send("n");
    let (status, seen) = pty.finish();
    assert_eq!(status.code(), Some(130), "{seen}");
    assert!(seen.contains("Import canceled."), "{seen}");
    assert!(!seen.contains("Downloading"), "{seen}");
    assert!(!epic.own_copy().exists());
    assert_eq!(epic.browser_calls(), Vec::<String>::new());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn answering_yes_downloads_wineos_copy_and_logs_in_with_it() {
    let epic = Epic::new();
    epic.python("3.14.0");
    let mut pty = Pty::spawn(with_stand_in(&epic, &["import", "epic", "--login"]));
    pty.expect(QUESTION);
    pty.send("");
    pty.expect("Downloading Legendary 0.21.1...");
    pty.expect("Downloaded Legendary 0.21.1.");
    let (status, seen) = paste(pty, CODE);
    assert!(status.success(), "{seen}");
    assert!(seen.contains("Logged in to Epic as REDACTED."), "{seen}");
    assert_eq!(
        fs::read(epic.own_copy()).unwrap(),
        fs::read(epic.file("stand-in/legendary_linux_x64")).unwrap()
    );
    let own = epic.own_copy().display().to_string();
    assert!(!epic.programs().is_empty());
    assert!(epic.programs().iter().all(|program| *program == own));
    assert!(epic.logged_in());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn yes_agrees_to_the_download_at_a_terminal() {
    let epic = Epic::new();
    epic.python("3.14.0");
    let mut pty = Pty::spawn(with_stand_in(
        &epic,
        &["import", "epic", "--login", "--yes"],
    ));
    pty.expect("Downloaded Legendary 0.21.1.");
    let (status, seen) = paste(pty, CODE);
    assert!(status.success(), "{seen}");
    assert!(!seen.contains("Download it?"), "{seen}");
    assert!(epic.own_copy().is_file());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn yes_downloads_without_a_terminal() {
    let epic = Epic::new();
    epic.python("3.14.0");
    epic.log_in();
    let output = with_stand_in(&epic, &["import", "epic", "--list", "--refresh", "--yes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("Downloading Legendary 0.21.1..."));
    assert!(!stderr(&output).contains("Download it?"));
    assert!(stdout(&output).contains("Offline Game"));
    assert!(epic.own_copy().is_file());
    assert_eq!(epic.list_calls(), 1);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn the_download_needs_a_terminal_or_yes() {
    let epic = Epic::new();
    epic.python("3.14.0");
    epic.log_in();
    let output = with_stand_in(&epic, &["import", "epic", "--list", "--refresh"])
        .output()
        .unwrap();
    assert_fails_with(
        &output,
        "Downloading Legendary needs a terminal, or --yes to agree to it.",
    );
    assert!(!epic.own_copy().exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_dry_run_says_what_it_would_ask_and_downloads_nothing() {
    let epic = Epic::new();
    epic.python("3.14.0");
    for args in [
        ["import", "epic", "--login", "--dry-run"].as_slice(),
        &["import", "epic", "--list", "--refresh", "--dry-run"],
        &[
            "import",
            "epic",
            "--list",
            "--refresh",
            "--dry-run",
            "--yes",
        ],
    ] {
        let output = with_stand_in(&epic, args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
        assert_eq!(
            stderr(&output),
            "Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?\n\
             Dry run, so wineo didn't ask or download anything.\n",
            "{args:?}"
        );
        assert_eq!(stdout(&output), "", "{args:?}");
        assert!(!epic.own_copy().exists(), "{args:?}");
    }
    assert_eq!(epic.calls(), Vec::<String>::new());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn without_a_new_enough_python_nothing_is_offered() {
    let epic = Epic::new();
    for python in [None, Some("3.9.18")] {
        if let Some(version) = python {
            epic.python(version);
        }
        for args in [
            ["import", "epic", "--list", "--refresh", "--yes"].as_slice(),
            &["import", "epic", "--login", "--dry-run"],
        ] {
            let output = with_stand_in(&epic, args).output().unwrap();
            assert_fails_with(&output, NO_PYTHON);
            assert!(!stderr(&output).contains("Download"), "{python:?} {args:?}");
            assert!(!epic.own_copy().exists());
        }
    }
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn an_older_legendary_on_path_is_passed_over_and_named_in_the_offer() {
    let epic = Epic::new();
    epic.python("3.14.0");
    let older = epic.on_path(&Fake::version("0.20.34"));
    let output = with_stand_in(&epic, &["import", "epic", "--login", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "The legendary on PATH ({}) is 0.20.34, older than 0.21.1.\n\
             Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?\n\
             Dry run, so wineo didn't ask or download anything.\n",
            older.display()
        )
    );
    assert_eq!(epic.calls(), ["--version"]);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn the_setting_comes_before_wineos_copy_which_comes_before_path() {
    let epic = Epic::new();
    epic.python("3.14.0");
    epic.log_in();
    let setting = epic.file("mine/legendary");
    epic.legendary_at(&setting, &Fake::default());
    let own = epic.own_copy_from(&Fake::default());
    let on_path = epic.on_path(&Fake::default());
    let set = epic.run(&[
        "set",
        "--global",
        &format!("epic.legendary={}", setting.display()),
    ]);
    assert!(set.status.success(), "{}", stderr(&set));

    let ran = |expected: &std::path::Path| {
        let before = epic.programs().len();
        let output = epic.run(&["import", "epic", "--list", "--refresh"]);
        assert!(output.status.success(), "{}", stderr(&output));
        let programs = epic.programs();
        assert!(programs.len() > before);
        assert!(
            programs[before..]
                .iter()
                .all(|program| *program == expected.display().to_string()),
            "expected only {} in {:?}",
            expected.display(),
            &programs[before..]
        );
    };
    ran(&setting);
    let unset = epic.run(&["set", "--global", "--unset", "epic.legendary"]);
    assert!(unset.status.success(), "{}", stderr(&unset));
    ran(&own);
    fs::remove_file(&own).unwrap();
    ran(&on_path);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn an_unusable_setting_is_an_error_for_every_command_and_never_falls_through() {
    let epic = Epic::new();
    epic.log_in();
    epic.on_path(&Fake::default());
    let older = epic.file("old/legendary");
    epic.legendary_at(&older, &Fake::version("0.20.34"));
    // Under the test's HOME, so shown with `~`.
    let cases = [
        (
            older.display().to_string(),
            "[epic] legendary is set to ~/old/legendary, which is Legendary 0.20.34. wineo needs 0.21.1 or newer.",
        ),
        (
            epic.file("nothing").display().to_string(),
            "[epic] legendary is set to ~/nothing, which can't be used: there is nothing there.",
        ),
    ];
    for (value, message) in cases {
        fs::create_dir_all(epic.config().parent().unwrap()).unwrap();
        fs::write(
            epic.config(),
            format!("[epic]\nlegendary = {}\n", toml_string(&value)),
        )
        .unwrap();
        for args in [
            ["import", "epic", "--list"].as_slice(),
            &["import", "epic", "--list", "--refresh"],
            &["import", "epic", "--logout"],
            &["import", "epic", "--login", "--dry-run"],
            &["import", "epic"],
        ] {
            let output = epic.run(args);
            assert_fails_with(&output, message);
            assert_eq!(stdout(&output), "", "{args:?}");
        }
    }
    // The good one on PATH was never tried, and the login is still there.
    assert!(
        !epic
            .programs()
            .iter()
            .any(|program| program.ends_with("epic-bin/legendary"))
    );
    assert!(epic.logged_in());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

fn toml_string(text: &str) -> String {
    format!("'{text}'")
}

#[test]
fn logout_deletes_the_login_and_the_saved_list() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let listed = epic.run(&["import", "epic", "--list"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(epic.cache().is_file());

    let output = epic.run(&["import", "epic", "--logout"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Removed the Epic login.\n");
    assert!(epic.calls().contains(&"auth --delete".to_owned()));
    assert!(!epic.logged_in());
    assert!(!epic.cache().exists());

    let again = epic.run(&["import", "epic", "--logout"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(stdout(&again), "No Epic login was saved.\n");
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn logout_without_legendary_removes_the_login_itself_and_never_offers_it() {
    let epic = Epic::new();
    epic.python("3.14.0");
    epic.log_in();
    fs::create_dir_all(epic.cache().parent().unwrap()).unwrap();
    fs::write(epic.cache(), "{}").unwrap();
    let output = with_stand_in(&epic, &["import", "epic", "--logout"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Removed the Epic login.\n");
    assert_eq!(stderr(&output), "");
    assert!(!epic.logged_in());
    assert!(!epic.cache().exists());
    assert!(!epic.own_copy().exists());
    assert_eq!(epic.calls(), Vec::<String>::new());

    // The rest of Legendary's folder stays, as its own logout leaves it.
    assert!(epic.state().is_dir());
    let again = epic.run(&["import", "epic", "--logout"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(stdout(&again), "No Epic login was saved.\n");
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn logout_is_refused_in_a_portable_root() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let portable = epic.file("portable");
    let output = command(&epic, &["--portable"])
        .arg(&portable)
        .args(["import", "epic", "--logout"])
        .output()
        .unwrap();
    assert_fails_with(&output, HOST_ONLY);
    assert!(epic.logged_in());
    assert_eq!(epic.calls(), Vec::<String>::new());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn logout_takes_no_yes_or_dry_run_and_list_flags_need_list() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    for args in [
        ["import", "epic", "--logout", "--yes"].as_slice(),
        &["import", "epic", "--logout", "--dry-run"],
        &["import", "epic", "--refresh"],
        &["import", "epic", "--json"],
        &["import", "epic", "--login", "--list"],
        &["import", "epic", "--logout", "--list"],
    ] {
        let output = epic.run(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    assert!(epic.logged_in());
    assert_eq!(epic.calls(), Vec::<String>::new());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn list_prints_a_table_of_the_owned_games() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let output = epic.run(&["import", "epic", "--list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        "ID             TITLE             STATE\n\
         Dlc Game       Dlc Game          not installed\n\
         AppOffline     Offline Game      not installed\n\
         AppPrereq      Prereq Game®      not installed\n\
         AppThirdParty  Third Party Game  unavailable: needs an account at another launcher\n\
         AppToken       Token Game 2020   not installed\n"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

/// The probe's list, with one game managed by another launcher and one
/// with no Windows build.
fn mixed_list(epic: &Epic) -> Fake {
    let list = epic.edited_list(|list| {
        for game in list.as_array_mut().unwrap() {
            match game["app_name"].as_str().unwrap() {
                "AppPrereq" => {
                    game["metadata"]["customAttributes"]["ThirdPartyManagedApp"] =
                        json!({"type": "STRING", "value": "Origin"});
                }
                "AppToken" => {
                    let windows = game["asset_infos"]["Windows"].take();
                    game["asset_infos"] = json!({"Mac": windows});
                }
                _ => {}
            }
        }
    });
    Fake {
        list,
        ..Fake::default()
    }
}

#[test]
fn list_json_has_every_game_with_its_state_and_reason() {
    let epic = Epic::new();
    epic.on_path(&mixed_list(&epic));
    epic.log_in();
    let output = epic.run(&["import", "epic", "--list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let game = |id: &str, title: &str, state: &str, reason: Value| {
        json!({
            "id": id,
            "title": title,
            "state": state,
            "environment": null,
            "version": null,
            "update": null,
            "reason": reason,
            "dlc": [],
        })
    };
    assert_eq!(
        json_out(&output),
        json!({
            "games": [
                game("Dlc Game", "Dlc Game", "not_installed", Value::Null),
                game("AppOffline", "Offline Game", "not_installed", Value::Null),
                game("AppPrereq", "Prereq Game®", "unavailable", json!("third_party")),
                game("AppThirdParty", "Third Party Game", "unavailable", json!("needs_linking")),
                game("AppToken", "Token Game 2020", "unavailable", json!("no_windows_build")),
            ],
            "errors": [],
            "warnings": [],
        })
    );

    let table = epic.run(&["import", "epic", "--list"]);
    let text = stdout(&table);
    for (title, label) in [
        ("Prereq Game®", "unavailable: another launcher manages it"),
        (
            "Third Party Game",
            "unavailable: needs an account at another launcher",
        ),
        ("Token Game 2020", "unavailable: no Windows build"),
    ] {
        assert!(
            text.lines()
                .any(|line| line.contains(title) && line.ends_with(label)),
            "{title} isn't {label:?} in:\n{text}"
        );
    }
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn an_empty_library_says_so() {
    let epic = Epic::new();
    let list = epic.edited_list(|list| *list = json!([]));
    epic.on_path(&Fake {
        list,
        ..Fake::default()
    });
    epic.log_in();
    let output = epic.run(&["import", "epic", "--list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "You don't own any Epic games.\n");
    let output = epic.run(&["import", "epic", "--list", "--json"]);
    assert_eq!(
        json_out(&output),
        json!({"games": [], "errors": [], "warnings": []})
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn the_list_is_saved_and_used_again_until_a_refresh() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    for _ in 0..2 {
        let output = epic.run(&["import", "epic", "--list"]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
    assert_eq!(epic.list_calls(), 1);
    let output = epic.run(&["import", "epic", "--list", "--refresh"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(epic.list_calls(), 2);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn listing_while_logged_out_says_to_log_in() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let output = epic.run(&["import", "epic", "--list"]);
    assert_fails_with(&output, LOGGED_OUT);
    assert_eq!(stdout(&output), "");
    assert_eq!(epic.list_calls(), 0);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn listing_without_legendary_or_a_saved_list_never_offers_the_download() {
    let epic = Epic::new();
    epic.python("3.14.0");
    epic.log_in();
    let output = with_stand_in(&epic, &["import", "epic", "--list"])
        .output()
        .unwrap();
    assert_fails_with(&output, LOGGED_OUT);
    assert!(!stderr(&output).contains("Download"));
    assert!(!epic.own_copy().exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

/// Lists once with a Legendary on `PATH`, so the list is saved, then takes
/// that Legendary away.
fn saved_list(epic: &Epic) -> Value {
    let on_path = epic.on_path(&Fake::default());
    epic.log_in();
    let output = epic.run(&["import", "epic", "--list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    fs::remove_file(on_path).unwrap();
    json_out(&output)["games"].clone()
}

#[test]
fn listing_without_legendary_shows_the_saved_list_with_a_warning() {
    let epic = Epic::new();
    let games = saved_list(&epic);
    epic.python("3.14.0");
    let warning = "Legendary isn't installed, so this is the saved list.";
    let output = with_stand_in(&epic, &["import", "epic", "--list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        json_out(&output),
        json!({"games": games, "errors": [], "warnings": [warning]})
    );
    assert_eq!(stderr(&output), format!("warning: {warning}\n"));

    let table = epic.run(&["import", "epic", "--list"]);
    assert!(table.status.success(), "{}", stderr(&table));
    assert!(stdout(&table).contains("Offline Game"));
    assert_eq!(stderr(&table), format!("warning: {warning}\n"));
    assert!(!epic.own_copy().exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn listing_with_wineos_copy_but_no_python_says_python_is_needed() {
    let epic = Epic::new();
    epic.own_copy_from(&Fake::default());
    epic.log_in();
    let output = epic.run(&["import", "epic", "--list"]);
    assert_fails_with(&output, NO_PYTHON);

    let epic = Epic::new();
    let games = saved_list(&epic);
    epic.own_copy_from(&Fake::default());
    let output = epic.run(&["import", "epic", "--list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let warning = format!("{NO_PYTHON} This is the saved list.");
    assert_eq!(
        json_out(&output),
        json!({"games": games, "errors": [], "warnings": [warning]})
    );
    assert_eq!(
        epic.programs().len(),
        3,
        "only the first listing ran Legendary"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_dry_run_refresh_uses_the_saved_list_and_fetches_nothing() {
    let epic = Epic::new();
    let games = saved_list(&epic);
    epic.on_path(&Fake::default());
    let output = epic.run(&[
        "import",
        "epic",
        "--list",
        "--refresh",
        "--dry-run",
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let warning = "Dry run, so the list wasn't refreshed.";
    assert_eq!(
        json_out(&output),
        json!({"games": games, "errors": [], "warnings": [warning]})
    );
    assert_eq!(epic.list_calls(), 1);

    let empty = Epic::new();
    empty.on_path(&Fake::default());
    empty.log_in();
    let output = empty.run(&["import", "epic", "--list", "--refresh", "--dry-run"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "Dry run, so the owned Epic games weren't fetched.\n"
    );
    assert_eq!(empty.list_calls(), 0);
    assert!(!empty.cache().exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
    assert_eq!(empty.sweep(), Vec::<i32>::new());
}

#[test]
fn a_failed_refresh_falls_back_to_the_saved_list_with_a_warning() {
    let epic = Epic::new();
    let games = saved_list(&epic);
    epic.on_path(&Fake {
        list_fails: true,
        ..Fake::default()
    });
    let output = epic.run(&["import", "epic", "--list", "--refresh", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let json = json_out(&output);
    assert_eq!(json["games"], games);
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .starts_with("Couldn't refresh owned Epic games: "),
        "{warnings:?}"
    );

    fs::remove_file(epic.cache()).unwrap();
    let output = epic.run(&["import", "epic", "--list", "--refresh", "--json"]);
    assert_fails_with(&output, "Couldn't get owned Epic games: ");
    assert_eq!(stdout(&output), "");
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn listing_works_in_a_portable_root_with_the_hosts_login() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let output = command(&epic, &["--portable"])
        .arg(epic.file("portable"))
        .args(["import", "epic", "--list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(json_out(&output)["games"].as_array().unwrap().len(), 5);
    assert!(epic.cache().is_file());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn bare_import_epic_lists_when_logged_in() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let output = epic.run(&["import", "epic"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("ID "), "{}", stdout(&output));
    assert!(stdout(&output).contains("Token Game 2020"));
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn bare_import_epic_logs_in_when_logged_out() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let pty = Pty::spawn(command(&epic, &["import", "epic"]));
    let (status, seen) = paste(pty, CODE);
    assert!(status.success(), "{seen}");
    assert!(seen.contains("Logged in to Epic as REDACTED."), "{seen}");
    assert!(epic.logged_in());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn logging_in_works_in_a_portable_root_and_saves_the_login_on_the_host() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    let portable = epic.file("portable");
    let mut command = command(&epic, &["--portable"]);
    command.arg(&portable).args(["import", "epic", "--login"]);
    let (status, seen) = paste(Pty::spawn(command), CODE);
    assert!(status.success(), "{seen}");
    assert!(epic.logged_in());
    assert!(!portable.join("data").exists());
    assert_code_kept_and_nothing_left(&epic, &seen);
}

#[test]
fn the_import_menu_offers_epic_right_after_gog() {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    let mut pty = Pty::spawn(command(&epic, &["import"]));
    pty.expect("Import from where?");
    pty.expect("4) A GOG game");
    pty.expect("5) An Epic game");
    pty.expect("6) Emulated games");
    pty.expect("Source [1]: ");
    pty.send("5");
    pty.expect("Token Game 2020");
    let (status, seen) = pty.finish();
    assert!(status.success(), "{seen}");
    assert_eq!(epic.list_calls(), 1);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}
