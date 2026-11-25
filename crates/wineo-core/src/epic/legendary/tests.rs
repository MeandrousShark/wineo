use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::epic::account;
use crate::epic::fake::{Fake, Setup, fixture};

fn found(setup: &Setup, setting: Option<&str>, env: Env) -> Result<Found, Error> {
    find_with(&setup.paths, setting, env, &Token::new())
}

fn usable(result: Result<Found, Error>) -> Legendary {
    match result.unwrap() {
        Found::Usable(legendary) => *legendary,
        Found::Missing(missing) => panic!("nothing usable: {missing:?}"),
    }
}

#[test]
fn the_setting_comes_first_then_the_own_copy_then_path() {
    let setup = Setup::new();
    setup.python("3.12.1");
    let on_path = setup.on_path(&Fake::default());
    let own = setup.own_copy(&Fake::default());
    let set = setup.root.path().join("opt/legendary");
    setup.legendary_at(&set, &Fake::version("0.22.0"));

    let legendary = usable(found(&setup, Some(set.to_str().unwrap()), setup.env()));
    assert_eq!(legendary.source(), Source::Setting);
    assert_eq!(legendary.path(), set);
    assert_eq!(legendary.version(), Version::new(0, 22, 0));

    let legendary = usable(found(&setup, None, setup.env()));
    assert_eq!(legendary.source(), Source::Own);
    assert_eq!(legendary.path(), own);
    assert_eq!(legendary.version(), FLOOR);

    fs::remove_file(&own).unwrap();
    let legendary = usable(found(&setup, None, setup.env()));
    assert_eq!(legendary.source(), Source::Path);
    assert_eq!(legendary.path(), fs::canonicalize(on_path).unwrap());

    fs::remove_file(setup.bin.join("legendary")).unwrap();
    let Found::Missing(missing) = found(&setup, None, setup.env()).unwrap() else {
        panic!("found a Legendary with none left");
    };
    assert_eq!(missing.older_on_path, None);
    assert!(setup.sweep().is_empty());
}

#[test]
fn an_unusable_setting_is_an_error_not_a_fall_through() {
    let setup = Setup::new();
    setup.python("3.12.1");
    setup.on_path(&Fake::default());
    setup.own_copy(&Fake::default());
    let home = setup.root.path();

    let missing = found(&setup, Some("~/nowhere/legendary"), setup.env()).unwrap_err();
    assert_eq!(
        missing.to_string(),
        "[epic] legendary is set to ~/nowhere/legendary, which can't be used: there is nothing there."
    );

    let old = home.join("old/legendary");
    setup.legendary_at(&old, &Fake::version("0.20.34"));
    let too_old = found(&setup, Some("~/old/legendary"), setup.env()).unwrap_err();
    assert!(
        matches!(too_old, Error::SettingTooOld { found, .. } if found == Version::new(0, 20, 34))
    );
    assert_eq!(
        too_old.to_string(),
        "[epic] legendary is set to ~/old/legendary, which is Legendary 0.20.34. wineo needs 0.21.1 or newer."
    );

    let plain = home.join("plain");
    fs::write(&plain, "not a program").unwrap();
    let error = found(&setup, Some(plain.to_str().unwrap()), setup.env()).unwrap_err();
    assert!(
        error.to_string().ends_with("it isn't executable."),
        "{error}"
    );

    let silent = home.join("silent/legendary");
    setup.legendary_at(
        &silent,
        &Fake {
            noise: Some("--version"),
            ..Fake::default()
        },
    );
    let error = found(&setup, Some(silent.to_str().unwrap()), setup.env()).unwrap_err();
    assert!(matches!(error, Error::Setting { .. }), "{error}");
    assert!(setup.sweep().is_empty());
}

#[test]
fn an_older_legendary_on_path_is_passed_over_and_named() {
    let setup = Setup::new();
    let path = setup.on_path(&Fake::version("0.20.34"));
    let Found::Missing(missing) = found(&setup, None, setup.env()).unwrap() else {
        panic!("used a Legendary below the floor");
    };
    assert_eq!(
        missing.older_on_path,
        Some((fs::canonicalize(path).unwrap(), Version::new(0, 20, 34)))
    );

    setup.on_path(&Fake::version("0.21.1"));
    assert_eq!(
        usable(found(&setup, None, setup.env())).source(),
        Source::Path
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn an_unknown_version_line_makes_a_path_copy_unusable() {
    let setup = Setup::new();
    setup.on_path(&Fake {
        noise: Some("--version"),
        ..Fake::default()
    });
    let Found::Missing(missing) = found(&setup, None, setup.env()).unwrap() else {
        panic!("used a Legendary whose version couldn't be read");
    };
    assert_eq!(missing.older_on_path, None);
    assert!(setup.sweep().is_empty());
}

#[test]
fn python_must_be_there_and_new_enough() {
    let setup = Setup::new();
    let no_python = python_with(&setup.env(), &Token::new()).unwrap_err();
    assert!(matches!(no_python, Error::Python));
    assert_eq!(
        no_python.to_string(),
        "Epic needs Python 3.10 or newer to run Legendary."
    );

    setup.python("3.9.18");
    assert!(matches!(
        python_with(&setup.env(), &Token::new()),
        Err(Error::Python)
    ));

    setup.python("3.10.0");
    assert_eq!(
        python_with(&setup.env(), &Token::new()).unwrap(),
        fs::canonicalize(setup.bin.join("python3")).unwrap()
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn the_own_copy_needs_python_and_leaves_path_to_try_without_it() {
    let setup = Setup::new();
    setup.own_copy(&Fake::default());
    assert!(matches!(
        found(&setup, None, setup.env()),
        Err(Error::Python)
    ));

    setup.python("3.8.10");
    assert!(matches!(
        found(&setup, None, setup.env()),
        Err(Error::Python)
    ));

    setup.on_path(&Fake::default());
    assert_eq!(
        usable(found(&setup, None, setup.env())).source(),
        Source::Path
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn the_own_copy_runs_as_python3_file() {
    let setup = Setup::new();
    setup.python("3.12.1");
    let own = setup.own_copy(&Fake::default());
    let legendary = setup.find(setup.env());
    assert_eq!(
        account::status(&legendary, &Token::new()).unwrap(),
        account::Status::LoggedOut
    );
    let calls = setup.python_calls();
    let last = calls.last().unwrap();
    assert_eq!(*last, format!("{} status --offline --json", own.display()));
    assert!(
        calls.iter().all(|call| !call.starts_with("-I")),
        "{calls:?}"
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn legendary_runs_with_its_own_environment_and_wineos_is_untouched() {
    let setup = Setup::new();
    let extra = [
        ("LGDRY_NO_WINE", "1"),
        ("LEGENDARY_WRAPPER_EXE", "/x"),
        ("PYTHONPATH", "/x"),
        ("PYTHONSAFEPATH", "1"),
        ("XDG_CACHE_HOME", "/cache"),
        ("LANG", "de_DE.UTF-8"),
        ("LC_ALL", "de_DE.UTF-8"),
        ("HTTPS_PROXY", "http://proxy:3128"),
        ("REQUESTS_CA_BUNDLE", "/ca.pem"),
    ];
    let env = setup.env_with(&extra);
    let before: Vec<_> = std::env::vars_os().collect();
    let shown_before = format!("{env:?}");
    setup.on_path(&Fake::default());
    let legendary = setup.find(env.clone());
    account::status(&legendary, &Token::new()).unwrap();

    let seen = setup.seen("env", "status");
    let lines: Vec<&str> = seen.lines().collect();
    for gone in [
        "LGDRY_NO_WINE=",
        "LEGENDARY_WRAPPER_EXE=",
        "PYTHONPATH=",
        "PYTHONSAFEPATH=",
    ] {
        assert!(
            !lines.iter().any(|line| line.starts_with(gone)),
            "{gone} in {seen}"
        );
    }
    let epic = setup.paths.host_data().join("epic");
    for line in [
        format!("LEGENDARY_CONFIG_PATH={}", epic.join("legendary").display()),
        format!("XDG_CACHE_HOME={}", epic.join("cache").display()),
        "LANG=de_DE.UTF-8".to_owned(),
        "LC_ALL=de_DE.UTF-8".to_owned(),
        "HTTPS_PROXY=http://proxy:3128".to_owned(),
        "REQUESTS_CA_BUNDLE=/ca.pem".to_owned(),
    ] {
        assert!(lines.contains(&line.as_str()), "{line} not in {seen}");
    }
    // Only the child's: neither wineo's own environment nor the one it
    // was handed changed.
    assert_eq!(std::env::vars_os().collect::<Vec<_>>(), before);
    assert_eq!(format!("{env:?}"), shown_before);
    assert_eq!(
        env.get("XDG_CACHE_HOME"),
        Some(std::ffi::OsStr::new("/cache"))
    );
    assert_eq!(env.get("PYTHONPATH"), Some(std::ffi::OsStr::new("/x")));
    assert!(setup.sweep().is_empty());
}

#[test]
fn legendary_gets_no_stdin_and_a_process_group_of_its_own() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake::default());
    account::status(&legendary, &Token::new()).unwrap();
    assert_eq!(setup.seen("stdin", "status").trim(), "/dev/null");
    let stat = setup.seen("stat", "status");
    let pid = stat.split_whitespace().next().unwrap();
    let fields: Vec<&str> = stat
        .rsplit(')')
        .next()
        .unwrap()
        .split_whitespace()
        .collect();
    // State, parent, then the process group.
    assert_eq!(fields[2], pid, "{stat}");
    assert_ne!(
        fields[2],
        rustix::process::getpgrp().as_raw_nonzero().to_string()
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn the_state_folder_is_private_and_gets_its_config_once() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake::default());
    account::status(&legendary, &Token::new()).unwrap();
    let state = crate::epic::state_dir(&setup.paths);
    for dir in [&state, &crate::epic::cache_dir(&setup.paths)] {
        let mode = fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", dir.display());
    }
    let config = state.join("config.ini");
    assert_eq!(fs::read_to_string(&config).unwrap(), CONFIG_INI);
    fs::write(&config, "[Legendary]\nlocale = de-DE\n").unwrap();
    account::status(&legendary, &Token::new()).unwrap();
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "[Legendary]\nlocale = de-DE\n"
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_flood_on_stderr_doesnt_stall_the_answer() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake {
        flood: Some("status"),
        ..Fake::default()
    });
    assert_eq!(
        account::status(&legendary, &Token::new()).unwrap(),
        account::Status::LoggedOut
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_command_that_hangs_is_stopped_with_its_group() {
    let setup = Setup::new();
    let mut legendary = setup.legendary(&Fake {
        hang: Some("status"),
        ..Fake::default()
    });
    legendary.set_timeout(Duration::from_millis(500));
    let started = Instant::now();
    let error = account::status(&legendary, &Token::new()).unwrap_err();
    assert!(
        matches!(&error, Error::TimedOut { command, .. } if command == "status --offline --json"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_cancel_interrupts_the_group() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake {
        hang: Some("status"),
        ..Fake::default()
    });
    let token = Token::new();
    let canceler = {
        let token = token.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            token.cancel();
        })
    };
    let started = Instant::now();
    let error = account::status(&legendary, &token).unwrap_err();
    canceler.join().unwrap();
    assert!(error.is_canceled(), "{error}");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(setup.sweep().is_empty());
}

#[test]
fn an_exit_code_that_isnt_0_is_a_failure_with_its_last_line() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake::default());
    let error = legendary
        .run(&["list", "--json", "-T"], None, &Token::new())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "`legendary list --json -T` failed: exit code 1: ValueError: No saved credentials"
    );
    let error = legendary
        .run(&["nonsense"], None, &Token::new())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("`legendary nonsense` failed: exit code 2"),
        "{error}"
    );
    assert!(fixture("logged-out-list.stderr").exists());
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_symlink_where_the_own_copy_goes_isnt_the_own_copy() {
    let setup = Setup::new();
    setup.python("3.12.1");
    let real = setup.root.path().join("elsewhere/legendary");
    setup.legendary_at(&real, &Fake::default());
    let own = own_copy::file(&setup.paths, own_copy::PINNED.version);
    fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &own).unwrap();
    let Found::Missing(missing) = found(&setup, None, setup.env()).unwrap() else {
        panic!("took a symlink for wineo's own copy");
    };
    assert_eq!(missing.older_on_path, None);
    assert!(
        setup.python_calls().is_empty(),
        "{:?}",
        setup.python_calls()
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_secret_cut_by_the_stderr_limit_leaves_no_fragment() {
    let setup = Setup::new();
    let secret = "SecretCode0123456789";
    let fragment = &secret[10..];
    // The kept tail starts exactly at the fragment.
    let filler = "y".repeat(run::STDERR_LIMIT - fragment.len() - 2);
    let text = format!("{}\nbefore {secret}\n{filler}\n", "x".repeat(4096));
    assert!(text.ends_with(&format!("{fragment}\n{filler}\n")));
    assert_eq!(text.len() - text.find(fragment).unwrap(), run::STDERR_LIMIT);
    let stderr = setup.root.path().join("stderr");
    fs::write(&stderr, &text).unwrap();
    crate::test_script::write_script(
        setup.bin.join("legendary"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'legendary version \"0.21.1\", codename \"x\"'; exit 0; fi\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >&2; done <'{}'\nexit 0\n",
            stderr.display()
        ),
    );
    let legendary = setup.find(setup.env());
    let output = legendary
        .run(&["auth", "--code"], Some(secret), &Token::new())
        .unwrap();
    assert!(
        !output.stderr.contains(fragment),
        "a piece of the code survived"
    );
    assert_eq!(output.stderr, format!("{filler}\n"));
    assert!(setup.sweep().is_empty());
}
