//! `wineo runners update`, against a fake release catalog.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use wineo_core::download::InstallLock;

mod common;
use common::releases_api::{ReleasesApi, Repo, component_archive, proton_archive};
use common::{Fixture, KillOnDrop, assert_success, stderr, stdout, write_script};

const CACHYOS: &str = "proton-cachyos-10.0-20261101-slr-x86_64";

/// A catalog and a library for the plan's example: GE-Proton and DXVK have
/// something newer, Proton-CachyOS is current, and vkd3d-proton isn't installed.
fn example() -> (Fixture, ReleasesApi) {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    f.install_proton("GE-Proton10-20");
    f.install_proton(CACHYOS);
    f.install_build("dxvk", "dxvk-2.6", &["x64", "x32"]);
    api.announce(
        Repo::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
        498 * 1024 * 1024,
    );
    api.announce(
        Repo::ProtonCachyos,
        "cachyos-10.0-20261101-slr",
        &format!("{CACHYOS}.tar.xz"),
        1024,
    );
    api.announce(Repo::Dxvk, "v2.7.1", "dxvk-2.7.1.tar.gz", 9 * 1024 * 1024);
    (f, api)
}

fn update(f: &Fixture, api: &ReleasesApi, args: &[&str]) -> Output {
    f.releases_command(api)
        .args(["runners", "update"])
        .args(args)
        .output()
        .expect("wineo should start")
}

fn runner_root(f: &Fixture) -> std::path::PathBuf {
    f.root.path().join("data/wineo/runners")
}

/// A GE-Proton build and a DXVK build, each with a newer release ready to
/// download.
fn downloadable(f: &Fixture, api: &ReleasesApi) {
    f.install_proton("GE-Proton10-20");
    f.install_build("dxvk", "dxvk-2.6", &["x64", "x32"]);
    api.publish(
        Repo::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
        proton_archive("GE-Proton10-27"),
    );
    api.publish(
        Repo::Dxvk,
        "v2.7.1",
        "dxvk-2.7.1.tar.gz",
        component_archive("dxvk-2.7.1", true),
    );
}

#[test]
fn check_prints_the_table_and_downloads_nothing() {
    let (f, api) = example();
    let output = update(&f, &api, &["--check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "GE-Proton       GE-Proton10-20 -> GE-Proton10-27 (498 MiB)\n\
             Proton-CachyOS  up to date ({CACHYOS})\n\
             DXVK            dxvk-2.6 -> dxvk-2.7.1 (9 MiB)\n"
        )
    );
    assert_eq!(stderr(&output), "");
    assert_eq!(api.catalog_requests().len(), 3, "{:?}", api.requests());
    assert!(api.downloads().is_empty());
}

#[test]
fn check_json_is_pinned_exactly() {
    let (f, api) = example();
    let output = update(&f, &api, &["--check", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "{{\"families\":[\
             {{\"provider\":\"ge-proton\",\"installed\":\"GE-Proton10-20\",\
             \"available\":\"GE-Proton10-27\",\"update\":\"available\",\
             \"download_size\":522190848}},\
             {{\"provider\":\"proton-cachyos\",\"installed\":\"{CACHYOS}\",\
             \"available\":null,\"update\":\"none\",\"download_size\":null}},\
             {{\"provider\":\"dxvk\",\"installed\":\"dxvk-2.6\",\
             \"available\":\"dxvk-2.7.1\",\"update\":\"available\",\
             \"download_size\":9437184}}],\
             \"errors\":[]}}\n"
        )
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn check_json_puts_a_family_that_fails_in_errors_and_exits_1() {
    let (f, api) = example();
    api.status(Repo::GeProton.releases_path(), 500);
    let output = update(
        &f,
        &api,
        &["--check", "--json", "ge-proton", "dxvk", "vkd3d"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    // The rest still print, and each failure names its family.
    assert!(
        text.starts_with(
            "{\"families\":[{\"provider\":\"dxvk\",\"installed\":\"dxvk-2.6\",\
             \"available\":\"dxvk-2.7.1\",\"update\":\"available\",\"download_size\":9437184}],\
             \"errors\":[{\"provider\":\"ge-proton\",\"message\":\"Couldn't check GE-Proton: "
        ),
        "{text}"
    );
    assert!(
        text.ends_with(
            ",{\"provider\":\"vkd3d\",\"message\":\"vkd3d-proton isn't installed. \
             Download it with wineo download vkd3d.\"}]}\n"
        ),
        "{text}"
    );
    // JSON mode leaves stderr to real problems, and the errors are in the JSON.
    assert_eq!(stderr(&output), "");
}

#[test]
fn check_reports_a_family_it_couldnt_check_and_exits_1() {
    let (f, api) = example();
    api.status(Repo::GeProton.releases_path(), 500);
    let output = update(&f, &api, &["--check"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "Proton-CachyOS  up to date ({CACHYOS})\n\
             DXVK            dxvk-2.6 -> dxvk-2.7.1 (9 MiB)\n"
        )
    );
    let errors = stderr(&output);
    assert!(
        errors.contains("error: Couldn't check GE-Proton: "),
        "{errors}"
    );
    assert!(
        errors.contains("Couldn't check 1 of 3 families: GE-Proton."),
        "{errors}"
    );
}

#[test]
fn json_without_check_is_refused() {
    let (f, api) = example();
    let output = update(&f, &api, &["--json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("--check"), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(api.requests().is_empty());
}

#[test]
fn check_exits_0_with_updates_waiting_and_changes_nothing() {
    let (f, api) = example();
    let output = update(&f, &api, &["--check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("->"), "{}", stdout(&output));
    assert!(api.downloads().is_empty());
    assert!(!runner_root(&f).join("GE-Proton10-27").exists());
    assert!(!f.root.path().join("data/wineo/dxvk/dxvk-2.7.1").exists());
}

#[test]
fn a_named_family_that_isnt_installed_is_an_error_with_a_hint() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    f.install_build("dxvk", "dxvk-2.6", &["x64", "x32"]);
    api.announce(Repo::Dxvk, "v2.7.1", "dxvk-2.7.1.tar.gz", 1024);
    let output = update(&f, &api, &["ge-proton"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains(
            "error: GE-Proton isn't installed. Download it with wineo download ge-proton."
        ),
        "{}",
        stderr(&output)
    );
    // Only what was named is asked about, and here that was nothing.
    assert!(api.requests().is_empty(), "{:?}", api.requests());
}

#[test]
fn a_family_named_twice_is_checked_once_and_unknown_names_are_refused() {
    let (f, api) = example();
    let output = update(&f, &api, &["--check", "dxvk", "dxvk"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "DXVK  dxvk-2.6 -> dxvk-2.7.1 (9 MiB)\n");
    assert_eq!(api.catalog_requests().len(), 1);

    let output = update(&f, &api, &["--check", "wine"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("ge-proton"), "{}", stderr(&output));
}

#[test]
fn nothing_installed_says_so_and_exits_0() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    let output = update(&f, &api, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "Nothing to update.\n");

    let output = update(&f, &api, &["--check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Nothing to update.\n");

    let output = update(&f, &api, &["--check", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "{\"families\":[],\"errors\":[]}\n");
    assert!(api.requests().is_empty(), "{:?}", api.requests());
}

#[test]
fn an_update_installs_the_newer_builds_beside_the_old_ones() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    downloadable(&f, &api);
    let output = update(&f, &api, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    // A run is progress, so stdout stays empty.
    assert_eq!(stdout(&output), "");
    let text = stderr(&output);
    assert!(
        text.contains("GE-Proton  GE-Proton10-20 -> GE-Proton10-27"),
        "{text}"
    );
    assert!(text.contains("Downloading GE-Proton10-27\n"), "{text}");
    assert!(text.contains("Downloading dxvk-2.7.1\n"), "{text}");
    assert!(
        text.contains("Installed GE-Proton10-27 and dxvk-2.7.1.\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "Nothing uses GE-Proton10-20 or dxvk-2.6 now. Remove them with \
             `wineo runners remove NAME` and `wineo dxvk remove NAME`.\n"
        ),
        "{text}"
    );
    assert!(runner_root(&f).join("GE-Proton10-27/proton").is_file());
    assert!(
        runner_root(&f)
            .join("GE-Proton10-27/.wineo-install.json")
            .is_file()
    );
    assert!(runner_root(&f).join("GE-Proton10-20/proton").is_file());
    let dxvk = f.root.path().join("data/wineo/dxvk");
    assert!(dxvk.join("dxvk-2.7.1/x64/dxgi.dll").is_file());
    assert!(dxvk.join("dxvk-2.6/x64/dxgi.dll").is_file());

    // Run again: nothing is newer, so nothing downloads.
    let again = update(&f, &api, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(stderr(&again).contains("GE-Proton  up to date (GE-Proton10-27)"));
    assert!(
        !stderr(&again).contains("Downloading"),
        "{}",
        stderr(&again)
    );
    assert_eq!(api.downloads().len(), 2);
}

#[test]
fn the_older_builds_line_names_every_unused_one_and_skips_those_in_use() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    f.install_proton("GE-Proton10-18");
    f.install_proton("GE-Proton10-20");
    // Something still uses 10-20, and 10-18 is older than the one just replaced.
    f.environment_with_runner("game", "GE-Proton10-20", "");
    api.publish(
        Repo::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
        proton_archive("GE-Proton10-27"),
    );
    let output = update(&f, &api, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("Installed GE-Proton10-27.\n"), "{text}");
    assert!(
        text.contains(
            "Nothing uses GE-Proton10-18 now. Remove it with `wineo runners remove NAME`.\n"
        ),
        "{text}"
    );
    assert!(!text.contains("GE-Proton10-20 now"), "{text}");
}

#[test]
fn the_older_builds_line_is_left_out_when_a_config_cant_be_read() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    f.install_proton("GE-Proton10-18");
    api.publish(
        Repo::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
        proton_archive("GE-Proton10-27"),
    );
    let broken = f.config_dir().join("environments");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("broken.toml"), "not valid toml = [").unwrap();
    let output = update(&f, &api, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("Installed GE-Proton10-27.\n"), "{text}");
    assert!(!text.contains("Nothing uses"), "{text}");
    // Which is what `remove` would say about it too.
    assert!(
        !f.run(&["runners", "remove", "GE-Proton10-18", "--yes"])
            .status
            .success()
    );
}

#[test]
fn the_older_builds_line_is_left_out_for_another_launchers_folder() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    let lutris = f.root.path().join("data/lutris/runners/wine");
    for name in ["GE-Proton10-18", "GE-Proton10-20"] {
        for file in ["proton", "files/bin/wine", "files/bin/wineserver"] {
            let path = lutris.join(name).join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            write_script(&path, "#!/bin/sh\nexit 0\n");
        }
    }
    f.global_config(&format!(
        "[defaults]\nrunner_root = \"{}\"\n",
        lutris.display()
    ));
    api.publish(
        Repo::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
        proton_archive("GE-Proton10-27"),
    );
    let output = update(&f, &api, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("Installed GE-Proton10-27.\n"), "{text}");
    assert!(!text.contains("Nothing uses"), "{text}");
}

#[test]
fn one_family_failing_leaves_the_rest_running_and_exits_1() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    downloadable(&f, &api);
    api.asset_status("dxvk-2.7.1.tar.gz", 404);
    let output = update(&f, &api, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    let text = stderr(&output);
    assert!(text.contains("Installed GE-Proton10-27.\n"), "{text}");
    assert!(
        text.contains("error: Couldn't download dxvk-2.7.1: "),
        "{text}"
    );
    assert!(
        text.contains("Couldn't update 1 of 2 families: DXVK.\n"),
        "{text}"
    );
    assert!(runner_root(&f).join("GE-Proton10-27/proton").is_file());
    assert!(!f.root.path().join("data/wineo/dxvk/dxvk-2.7.1").exists());
}

#[test]
fn a_family_another_run_is_downloading_fails_with_its_name() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    downloadable(&f, &api);
    // Another `runners update` is partway through the same build.
    let other = InstallLock::acquire(&runner_root(&f).join("GE-Proton10-27")).unwrap();
    let output = update(&f, &api, &[]);
    drop(other);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("Another download of GE-Proton10-27 is running."),
        "{text}"
    );
    assert!(text.contains("Installed dxvk-2.7.1.\n"), "{text}");
    assert!(!runner_root(&f).join("GE-Proton10-27").exists());
}

/// A process whose environment names this fixture's data folder, which only
/// a wineo the test started has.
fn leftovers(root: &Path) -> Vec<String> {
    let marker = format!("XDG_DATA_HOME={}", root.join("data").display());
    fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .filter(|entry| {
            fs::read(entry.path().join("environ"))
                .unwrap_or_default()
                .split(|byte| *byte == 0)
                .any(|var| var == marker.as_bytes())
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Runs `wineo runners update`, waits for `started` on stderr, sends Ctrl+C,
/// runs `after`, and returns the exit code with everything it printed.
fn interrupted(
    f: &Fixture,
    api: &ReleasesApi,
    started: &str,
    after: impl FnOnce(),
) -> (Option<i32>, String) {
    let mut child = f
        .releases_command(api)
        .args(["runners", "update"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pipe = child.stderr.take().unwrap();
    let mut child = KillOnDrop::new(child);
    let (lines, seen) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });

    // Wait until the archive is on its way, so the signal lands mid-download.
    let mut text = String::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !text.contains(started) {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = seen
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("the download never started:\n{text}"));
        text.push_str(&line);
        text.push('\n');
    }
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    let pidfd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
    pidfd_send_signal(&pidfd, Signal::INT).unwrap();
    after();

    let mut status = None;
    while status.is_none() {
        assert!(Instant::now() < deadline, "wineo ignored Ctrl+C:\n{text}");
        status = child.try_wait().unwrap();
        thread::sleep(Duration::from_millis(20));
    }
    reader.join().unwrap();
    text.extend(seen.try_iter().map(|line| line + "\n"));
    (status.unwrap().code(), text)
}

#[test]
fn ctrl_c_stops_the_download_and_exits_130() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    f.install_proton("GE-Proton10-20");
    api.drip(
        Repo::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
        4 << 20,
    );
    let (code, text) = interrupted(&f, &api, "Downloading GE-Proton10-27", || {});
    assert_eq!(code, Some(130), "{text}");
    assert!(text.contains("Update canceled"), "{text}");
    assert!(!text.contains("Installed"), "{text}");
    // Nothing half-installed, and the old build is where it was.
    let left: Vec<_> = fs::read_dir(runner_root(&f))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with(".wineo-lock-"))
        .collect();
    assert_eq!(left, ["GE-Proton10-20"], "{text}");
    assert_eq!(leftovers(f.root.path()), Vec::<String>::new());
}

#[test]
fn a_download_that_fails_after_ctrl_c_is_part_of_the_cancel_not_an_error() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    downloadable(&f, &api);
    // GE-Proton finishes first. DXVK's archive stalls, and the server hangs up
    // on it after the signal, so the download fails on its own.
    api.stall(Repo::Dxvk, "v2.7.1", "dxvk-2.7.1.tar.gz", 4 << 20);
    let (code, text) = interrupted(&f, &api, "Downloading dxvk-2.7.1", || {
        // Long enough for wineo's signal thread to have raised the token.
        thread::sleep(Duration::from_millis(500));
        api.release_stalls();
    });
    assert_eq!(code, Some(130), "{text}");
    assert!(text.contains("Installed GE-Proton10-27.\n"), "{text}");
    assert!(text.contains("Update canceled"), "{text}");
    assert!(!text.contains("error:"), "{text}");
    assert!(!text.contains("Couldn't"), "{text}");
    assert!(runner_root(&f).join("GE-Proton10-27/proton").is_file());
    assert!(!f.root.path().join("data/wineo/dxvk/dxvk-2.7.1").exists());
    assert_eq!(leftovers(f.root.path()), Vec::<String>::new());
}

#[test]
fn help_says_it_covers_the_graphics_components_too() {
    let f = Fixture::new();
    let output = f.run(&["runners", "update", "--help"]);
    assert_success(&output);
    let text = stdout(&output);
    for needed in ["DXVK", "vkd3d-proton", "--check", "--json", "FAMILY"] {
        assert!(text.contains(needed), "{needed}\n{text}");
    }
}
