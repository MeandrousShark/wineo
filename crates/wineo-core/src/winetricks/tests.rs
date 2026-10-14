use std::fs;

use crate::test_script::write_script;

use super::*;

#[test]
fn installed_reads_whole_trimmed_lines() {
    let root = tempfile::tempdir().unwrap();
    assert!(installed(root.path()).unwrap().is_empty());
    fs::write(root.path().join("winetricks.log"), "").unwrap();
    assert!(installed(root.path()).unwrap().is_empty());
    fs::write(
        root.path().join("winetricks.log"),
        "vcrun2022_arm  \nwin10\nrenderer=vulkan\n  corefonts \n\n",
    )
    .unwrap();
    let verbs = installed(root.path()).unwrap();
    assert_eq!(verbs.len(), 4);
    for verb in ["vcrun2022_arm", "win10", "renderer=vulkan", "corefonts"] {
        assert!(verbs.contains(verb));
    }
    assert!(!verbs.contains("vcrun2022"));
}

#[test]
fn missing_keeps_order_without_duplicates_or_prefix_matches() {
    let installed = HashSet::from(["vcrun2022_arm".to_owned(), "win10".to_owned()]);
    let wanted = [
        "win10",
        "vcrun2022",
        "corefonts",
        "vcrun2022",
        "vcrun2022_arm",
    ]
    .map(str::to_owned);
    assert_eq!(missing(&wanted, &installed), ["vcrun2022", "corefonts"]);
    let installed = HashSet::from(["vcrun2022".to_owned()]);
    assert_eq!(
        missing(&["vcrun2022_arm".to_owned()], &installed),
        ["vcrun2022_arm"]
    );
}

#[test]
fn verb_shape() {
    for verb in ["vcrun2022", "win10", "renderer=vulkan"] {
        assert!(validate_verb(verb).is_ok());
    }
    for verb in ["", "two words", "line\nbreak", "--force", "-q"] {
        assert!(validate_verb(verb).is_err(), "{verb:?}");
    }
}

fn verbs(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn step(verb: &str, position: usize, total: usize) -> Step {
    Step {
        verb: verb.to_owned(),
        position,
        total,
    }
}

#[test]
fn progress_follows_requested_verbs_in_order() {
    let mut progress = Progress::new(&verbs(&["corefonts", "gdiplus", "vcrun2022"]));
    assert_eq!(
        progress.observe("Using winetricks 20250102 with wine-10.0"),
        None
    );
    assert_eq!(
        progress.observe("Executing w_do_call corefonts"),
        Some(step("corefonts", 1, 3))
    );
    assert_eq!(progress.observe("Executing wine cabextract -q x.exe"), None);
    assert_eq!(
        progress.observe("Executing w_do_call gdiplus\r"),
        Some(step("gdiplus", 2, 3))
    );
    assert_eq!(
        progress.observe("Executing w_do_call vcrun2022"),
        Some(step("vcrun2022", 3, 3))
    );
}

#[test]
fn progress_ignores_dependencies_that_were_not_requested() {
    let mut progress = Progress::new(&verbs(&["dotnet48"]));
    assert_eq!(progress.observe("Executing w_do_call dotnet40"), None);
    assert_eq!(
        progress.observe("Executing w_do_call dotnet_verifier"),
        None
    );
    assert_eq!(
        progress.observe("Executing w_do_call dotnet48"),
        Some(step("dotnet48", 1, 1))
    );
}

#[test]
fn progress_does_not_repeat_or_go_backwards() {
    let mut progress = Progress::new(&verbs(&["dotnet48", "gdiplus", "corefonts"]));
    assert_eq!(
        progress.observe("Executing w_do_call dotnet48"),
        Some(step("dotnet48", 1, 3))
    );
    // A dependency of an earlier verb that is also requested counts once.
    assert_eq!(
        progress.observe("Executing w_do_call corefonts"),
        Some(step("corefonts", 3, 3))
    );
    assert_eq!(progress.observe("Executing w_do_call gdiplus"), None);
    assert_eq!(progress.observe("Executing w_do_call corefonts"), None);
    assert_eq!(progress.observe("Executing w_do_call dotnet48"), None);
}

#[test]
fn progress_matches_whole_verb_names_only() {
    let mut progress = Progress::new(&verbs(&["vcrun2022", "renderer=vulkan"]));
    assert_eq!(progress.observe("Executing w_do_call vcrun2022_arm"), None);
    assert_eq!(progress.observe("Executing w_do_call vcrun"), None);
    assert_eq!(
        progress.observe("running Executing w_do_call vcrun2022"),
        None
    );
    assert_eq!(
        progress.observe("Executing w_do_call renderer=vulkan"),
        Some(step("renderer=vulkan", 2, 2))
    );
}

/// Runs `child` in a subprocess whose PATH starts with a fake winetricks.
fn with_fake_winetricks(script_body: &str, child: &str) {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let script = bin.join("winetricks");
    write_script(&script, script_body);
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", child, "--nocapture"])
        .env("WINEO_WINETRICKS_PROGRESS_ROOT", root.path())
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn install_reports_progress_from_the_log() {
    with_fake_winetricks(
        "#!/bin/sh\nfor verb in \"$@\"; do\n  case \"$verb\" in -q) ;; *)\n    echo \"Executing w_do_call dep\" >&2\n    echo \"Executing w_do_call $verb\" >&2 ;; esac\ndone\n",
        "winetricks::tests::install_progress_child",
    );
}

#[test]
fn a_silent_install_still_announces_its_log() {
    with_fake_winetricks(
        "#!/bin/sh\nsleep 0.3\nprintf 'no newline'\n",
        "winetricks::tests::silent_install_child",
    );
}

#[test]
fn silent_install_child() {
    let Some(root) = std::env::var_os("WINEO_WINETRICKS_PROGRESS_ROOT") else {
        return;
    };
    let prefix = std::path::PathBuf::from(root).join("prefix");
    fs::create_dir(&prefix).unwrap();
    let log = prefix.join("install.log");
    // A previous run's text must be gone by the time the log is announced.
    fs::write(&log, "stale\n").unwrap();
    let mut seen = Vec::new();
    install_unattended(
        "game",
        &Runner::fake("test"),
        &prefix,
        &BTreeMap::new(),
        &verbs(&["corefonts"]),
        1,
        &Token::new(),
        Output::Log(&log),
        |event| match event {
            WinetricksEvent::LogStarted => {
                assert!(fs::read(&log).unwrap().is_empty());
                seen.push("started");
            }
            WinetricksEvent::OutputLine(_) => seen.push("line"),
            WinetricksEvent::Progress(_) => seen.push("progress"),
            _ => {}
        },
    )
    .unwrap();
    assert_eq!(seen, ["started"]);
}

/// Feeds `chunks` to a tail one read at a time and returns the lines.
fn pump_chunks(chunks: &[&[u8]]) -> Vec<String> {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("install.log");
    fs::write(&path, "").unwrap();
    let mut tail = LineTail::open(&path).unwrap();
    let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
    let mut lines = Vec::new();
    for chunk in chunks {
        io::Write::write_all(&mut writer, chunk).unwrap();
        tail.pump(&mut |event| {
            if let WinetricksEvent::OutputLine(line) = event {
                lines.push(line);
            }
        });
    }
    lines
}

#[test]
fn a_line_of_exactly_the_cap_is_kept() {
    let line = "x".repeat(LINE_CAP);
    assert_eq!(
        pump_chunks(&[line.as_bytes(), b"\n", b"next\n"]),
        [line.as_str(), "next"]
    );
    assert_eq!(
        pump_chunks(&[format!("{line}\nnext\n").as_bytes()]),
        [line.as_str(), "next"]
    );
}

#[test]
fn a_line_one_over_the_cap_is_dropped_even_if_its_newline_comes_later() {
    let line = "x".repeat(LINE_CAP + 1);
    assert_eq!(
        pump_chunks(&[line.as_bytes(), b"\n", b"Executing w_do_call gdiplus\n"]),
        ["Executing w_do_call gdiplus"]
    );
    assert_eq!(
        pump_chunks(&[format!("{line}\nnext\n").as_bytes()]),
        ["next"]
    );
}

#[test]
fn carriage_returns_stay_in_the_line_and_progress_ignores_them() {
    // Lines are raw apart from the newline, so a CRLF split across reads
    // arrives as one line ending in the carriage return.
    let lines = pump_chunks(&[b"abc\r", b"\n", b"Executing w_do_call gdiplus\r\n"]);
    assert_eq!(lines, ["abc\r", "Executing w_do_call gdiplus\r"]);
    let mut progress = Progress::new(&verbs(&["gdiplus"]));
    assert_eq!(
        lines.iter().find_map(|line| progress.observe(line)),
        Some(step("gdiplus", 1, 1))
    );
}

#[test]
fn the_rest_of_an_oversized_line_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("install.log");
    fs::write(&path, "").unwrap();
    let mut tail = LineTail::open(&path).unwrap();
    let mut lines = Vec::new();
    let mut collect = |event: WinetricksEvent| {
        if let WinetricksEvent::OutputLine(line) = event {
            lines.push(line);
        }
    };
    let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
    // Multibyte text so any cut inside a character would show up.
    let long = "\u{3042}".repeat(LINE_CAP);
    io::Write::write_all(&mut writer, b"before\n").unwrap();
    io::Write::write_all(&mut writer, long.as_bytes()).unwrap();
    tail.pump(&mut collect);
    io::Write::write_all(
        &mut writer,
        b"suffix of the long line\nExecuting w_do_call gdiplus\n",
    )
    .unwrap();
    tail.pump(&mut collect);
    assert_eq!(lines, ["before", "Executing w_do_call gdiplus"]);
    let mut progress = Progress::new(&verbs(&["gdiplus"]));
    assert_eq!(
        lines.iter().find_map(|line| progress.observe(line)),
        Some(step("gdiplus", 1, 1))
    );
}

#[test]
fn install_progress_child() {
    let Some(root) = std::env::var_os("WINEO_WINETRICKS_PROGRESS_ROOT") else {
        return;
    };
    let prefix = std::path::PathBuf::from(root).join("prefix");
    fs::create_dir(&prefix).unwrap();
    let log = prefix.join("install.log");
    let mut steps = Vec::new();
    let mut lines = 0;
    install_unattended(
        "game",
        &Runner::fake("test"),
        &prefix,
        &BTreeMap::new(),
        &verbs(&["corefonts", "gdiplus"]),
        1,
        &Token::new(),
        Output::Log(&log),
        |event| match event {
            WinetricksEvent::Progress(step) => steps.push(step),
            WinetricksEvent::OutputLine(_) => lines += 1,
            _ => {}
        },
    )
    .unwrap();
    assert_eq!(lines, 4);
    assert_eq!(steps, [step("corefonts", 1, 2), step("gdiplus", 2, 2)]);
}

#[test]
fn token_cancels_from_another_thread_and_cleans_partial_work() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let script = bin.join("winetricks");
    write_script(
        &script,
        // The trap goes first: cancellation wakes as soon as the marker
        // exists, before a later trap line would have run.
        "#!/bin/sh\ntrap 'rm -f \"$WINEPREFIX/partial\"; exit 130' INT\ntouch \"$WINEPREFIX/partial\"\nwhile :; do sleep 0.05; done\n",
    );
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "winetricks::tests::token_cancel_child",
            "--nocapture",
        ])
        .env("WINEO_WINETRICKS_CANCEL_ROOT", root.path())
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn token_cancel_child() {
    let Some(root) = std::env::var_os("WINEO_WINETRICKS_CANCEL_ROOT") else {
        return;
    };
    let prefix = std::path::PathBuf::from(root).join("prefix");
    fs::create_dir(&prefix).unwrap();
    let token = Token::new();
    let request = token.clone();
    let marker = prefix.join("partial");
    let waiter = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "winetricks never started");
            thread::sleep(Duration::from_millis(10));
        }
        request.cancel();
    });
    let mut events = Vec::new();
    let result = run(
        &Runner::fake("test"),
        &prefix,
        &BTreeMap::new(),
        &["-q".to_owned(), "corefonts".to_owned()],
        1,
        &token,
        Output::Log(&prefix.join("output.log")),
        |event| events.push(event),
    );
    waiter.join().unwrap();
    assert!(
        result
            .unwrap_err()
            .downcast_ref::<Error>()
            .is_some_and(|err| matches!(err, Error::Canceled("winetricks")))
    );
    assert!(!prefix.join("partial").exists());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, WinetricksEvent::Stopping))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, WinetricksEvent::Done(_)))
    );
}
