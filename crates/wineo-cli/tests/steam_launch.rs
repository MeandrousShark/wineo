mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::steam::SteamFixture;
use common::{assert_success, stderr, stdout, wait_for};

fn signal(pid: u32, kind: &str) {
    assert!(
        Command::new("kill")
            .args([kind, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}

fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.to_owned()))
        .is_some_and(|fields| !fields.starts_with('Z'))
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Waits for the supervisor to record its claim, since the reaper's child can
/// exist while the run state is still in its starting phase.
fn claimed_pid(s: &SteamFixture) -> u32 {
    let mut pid = 0;
    wait_for("the run to claim its reaper", || {
        let state = s.f.state("civ");
        if state.contains("phase = \"starting\"") {
            return false;
        }
        pid = state
            .lines()
            .find_map(|line| line.strip_prefix("pid = "))
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        pid != 0
    });
    pid
}

#[test]
fn start_claims_reaper_and_stop_leaves_steam_client_alive() {
    let s = SteamFixture::new();
    s.config("");
    let started = s.run(&["start", "civ", "steam", "--", "--windowed"]);
    assert_success(&started);
    assert!(
        stdout(&started).contains("Asked Steam to start civ."),
        "{}",
        stdout(&started)
    );
    assert_eq!(s.dispatch_count(), 1);
    assert!(
        fs::read_to_string(&s.dispatches)
            .unwrap()
            .contains("-applaunch 289070 --windowed")
    );
    wait_for("Steam reaper claim", || {
        let state = s.f.state("civ");
        state.contains("target = \"steam\"") && state.contains("running_at = ")
    });
    let status = s.run(&["status", "civ"]);
    assert_success(&status);
    assert!(
        stdout(&status).contains("civ: running"),
        "{}",
        stdout(&status)
    );
    let listed = s.run(&["list"]);
    assert_success(&listed);
    assert!(stdout(&listed).contains("running"), "{}", stdout(&listed));
    assert_success(&s.run(&["stop", "civ"]));
    s.f.wait_for_run_end("civ", "stopped");
    assert!(alive(s.client_pid()), "Stop signaled the fake Steam client");
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn run_waits_for_natural_exit_and_sets_hook_variables() {
    let s = SteamFixture::new();
    s.set_mode("exit");
    let hook_log = s.f.root.path().join("hooks.log");
    let pfx = s.root.join("steamapps/compatdata/289070/pfx");
    fs::create_dir_all(&pfx).unwrap();
    s.config(&format!(
        "[hooks]\nbefore = 'printf \"before:%s:%s:%s:%s:%s:%s\\n\" \"$WINEO_ENVIRONMENT\" \"$WINEO_TARGET\" \"$WINEO_LAUNCHER\" \"$WINEO_APP_ID\" \"$WINEO_PREFIX\" \"${{WINEO_EXE-unset}}\" >> {}'\nafter = 'printf \"after:%s:%s:%s:%s\\n\" \"$WINEO_END\" \"${{WINEO_EXIT_CODE-unset}}\" \"$WINEO_SECONDS\" \"$WINEO_HOOK\" >> {}'\n",
        hook_log.display(), hook_log.display()
    ));
    let ran = s.run(&["run", "civ"]);
    assert_eq!(ran.status.code(), Some(0), "{}", stderr(&ran));
    let hooks = fs::read_to_string(&hook_log).unwrap();
    assert!(
        hooks.contains(&format!(
            "before:civ:steam:steam:289070:{}:unset",
            pfx.display()
        )),
        "{hooks}"
    );
    assert!(hooks.contains("after:exited::"), "{hooks}");
    assert!(hooks.contains(":after"), "{hooks}");
    assert_eq!(hooks.lines().count(), 2, "{hooks}");
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn steam_started_game_is_visible_and_stop_runs_no_hooks() {
    let s = SteamFixture::new();
    let marker = s.f.root.path().join("hooks");
    s.config(&format!(
        "[hooks]\nbefore = 'touch {}'\nafter = 'touch {}'\n",
        marker.display(),
        marker.display()
    ));
    s.launch_from_steam("289070");
    wait_for("Steam started game status", || {
        stdout(&s.run(&["status", "civ"])).contains("started from Steam")
    });
    let started = s.run(&["start", "civ"]);
    assert_success(&started);
    assert!(
        stderr(&started).contains("already running (started from Steam)"),
        "{}",
        stderr(&started)
    );
    let run = s.run(&["run", "civ"]);
    assert!(!run.status.success());
    assert_eq!(s.dispatch_count(), 0);
    assert!(!marker.exists());
    assert_success(&s.run(&["stop", "civ"]));
    wait_for("Steam reaper stopped", || {
        stdout(&s.run(&["status", "civ"])).contains("inactive")
    });
    assert!(alive(s.client_pid()));
    assert!(!marker.exists());
}

#[test]
fn game_started_during_before_is_adopted_without_dispatch() {
    let s = SteamFixture::new();
    let marker = s.f.root.path().join("after");
    s.config(&format!(
        "[hooks]\nbefore = 'printf \"AppId=289070 hold\\n\" > {}; sleep 0.3'\nafter = 'touch {}'\n",
        s.fifo.display(),
        marker.display()
    ));
    let started = s.run(&["start", "civ"]);
    assert_success(&started);
    assert!(
        stdout(&started).contains("Steam had already launched it"),
        "{}",
        stdout(&started)
    );
    assert_eq!(s.dispatch_count(), 0);
    assert!(!marker.exists(), "after ran while the game was alive");
    assert_success(&s.run(&["stop", "civ"]));
    wait_for("after hook", || marker.exists());
}

#[test]
fn a_missing_reaper_fails_after_short_test_timeout_and_runs_after() {
    let s = SteamFixture::new();
    s.set_mode("none");
    let marker = s.f.root.path().join("after");
    s.config(&format!(
        "[hooks]\nafter = 'printf \"%s\" \"$WINEO_END\" > {}'\n",
        marker.display()
    ));
    assert_success(&s.run(&["start", "civ"]));
    s.f.wait_for_run_end("civ", "failed");
    wait_for("failed after hook", || marker.exists());
    assert_eq!(fs::read_to_string(marker).unwrap(), "failed");
    assert!(
        s.f.log("civ").contains("Steam didn't start civ."),
        "{}",
        s.f.log("civ")
    );
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn attached_run_exits_one_when_steam_never_starts_the_game() {
    let s = SteamFixture::new();
    s.set_mode("none");
    s.config("");
    let ran = s.run(&["run", "civ"]);
    assert_eq!(ran.status.code(), Some(1), "{}", stderr(&ran));
    assert!(stderr(&ran).contains("Steam didn't start civ."));
    s.f.wait_for_run_end("civ", "failed");
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn staging_progress_keeps_a_steam_launch_starting_past_the_normal_limit() {
    let s = SteamFixture::new();
    s.set_game_progress("289070", "Civilization VI", 1030, 100, 0);
    s.set_mode("none");
    s.config("");
    let started = s
        .wineo()
        .env("WINEO_TEST_STEAM_START_TIMEOUT_MS", "250")
        .env("WINEO_TEST_STEAM_STALL_TIMEOUT_MS", "800")
        .args(["start", "civ"])
        .output()
        .unwrap();
    assert_success(&started);
    wait_for("updating game starting", || {
        s.f.state("civ").contains("phase = \"starting\"")
    });
    let status = s.run(&["status", "civ"]);
    assert!(
        stdout(&status).contains("civ: starting"),
        "{}",
        stdout(&status)
    );
    let listed = s.run(&["list"]);
    assert!(stdout(&listed).contains("starting"), "{}", stdout(&listed));
    for staged in 1..=6 {
        thread::sleep(Duration::from_millis(150));
        s.set_game_progress("289070", "Civilization VI", 1030, 100, staged);
        assert!(
            !s.f.state("civ").contains("status = \"failed\""),
            "staging progress was treated as a stall"
        );
    }
    s.launch_from_steam("289070");
    wait_for("reaper claimed after staging", || {
        s.f.state("civ").contains("running_at = ")
    });
    let status = s.run(&["status", "civ"]);
    assert!(
        stdout(&status).contains("civ: running"),
        "{}",
        stdout(&status)
    );
    let listed = s.run(&["list"]);
    assert!(stdout(&listed).contains("running"), "{}", stdout(&listed));
    assert_success(&s.run(&["stop", "civ"]));
    s.f.wait_for_run_end("civ", "stopped");
}

#[test]
fn stalled_update_fails_even_though_steam_still_says_updating() {
    let s = SteamFixture::new();
    s.add_game("289070", "Civilization VI", 1030);
    s.set_mode("none");
    s.config("");
    assert_success(&s.run(&["start", "civ"]));
    s.f.wait_for_run_end("civ", "failed");
    assert!(s.f.log("civ").contains("Steam didn't start civ."));
    assert_eq!(s.dispatch_count(), 1);
}

#[test]
fn a_failed_steam_spawn_fails_the_run_and_runs_after() {
    let s = SteamFixture::new();
    let marker = s.f.root.path().join("after");
    s.config(&format!(
        "[hooks]\nafter = 'printf \"%s\" \"$WINEO_END\" > {}'\n",
        marker.display()
    ));
    let ran = s
        .wineo()
        .env("PATH", &s.f.sandbox)
        .args(["run", "civ"])
        .output()
        .unwrap();
    assert_eq!(ran.status.code(), Some(1), "{}", stderr(&ran));
    assert!(
        stderr(&ran).contains("failed to start Steam"),
        "{}",
        stderr(&ran)
    );
    assert_eq!(fs::read_to_string(marker).unwrap(), "failed");
    assert_eq!(s.dispatch_count(), 0);
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn stop_during_starting_does_not_claim_a_later_reaper() {
    let s = SteamFixture::new();
    s.set_mode("none");
    s.config("");
    assert_success(&s.run(&["start", "civ"]));
    wait_for("starting state", || {
        s.f.state("civ").contains("phase = \"starting\"")
    });
    assert_success(&s.run(&["stop", "civ"]));
    s.f.wait_for_run_end("civ", "stopped");
    s.launch_from_steam("289070");
    wait_for("later Steam launch", || {
        stdout(&s.run(&["status", "civ"])).contains("started from Steam")
    });
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn launch_request_that_never_exits_does_not_block_start() {
    let s = SteamFixture::new();
    s.config("");
    fs::write(s.f.root.path().join("steam-linger"), "").unwrap();
    let began = Instant::now();
    let started = s.run(&["start", "civ"]);
    assert_success(&started);
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "handoff waited for the Steam request"
    );
    wait_for("reaper claim", || {
        s.f.state("civ").contains("running_at = ")
    });
}

#[test]
fn wrong_app_id_is_not_claimed() {
    let s = SteamFixture::new();
    s.config("");
    s.set_mode("none");
    s.launch_from_steam("2890");
    assert_success(&s.run(&["start", "civ"]));
    s.f.wait_for_run_end("civ", "failed");
    assert!(alive(s.client_pid()));
}

#[test]
fn a_reaper_from_the_other_install_is_not_claimed() {
    let mut s = SteamFixture::new();
    s.config("");
    s.set_mode("none");
    s.launch_from_other_install("289070");
    let installs = wineo_core::steam::installs(s.f.root.path());
    let other = installs
        .iter()
        .find(|install| install.kind() == wineo_core::steam::InstallKind::Flatpak)
        .unwrap();
    wait_for("other install reaper", || {
        !wineo_core::steam::matching_reapers(other, "289070")
            .unwrap()
            .is_empty()
    });
    assert!(stdout(&s.run(&["status", "civ"])).contains("inactive"));
    assert_success(&s.run(&["start", "civ"]));
    s.f.wait_for_run_end("civ", "failed");
    assert_eq!(s.dispatch_count(), 1);
    assert!(
        !wineo_core::steam::matching_reapers(other, "289070")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn force_stop_kills_reparented_child_but_not_steam_client() {
    let s = SteamFixture::new();
    s.f.global_config("stop_timeout = 1\n");
    s.set_mode("child");
    s.config("");
    assert_success(&s.run(&["start", "civ"]));
    wait_for("reaper child", || s.child_pid.exists());
    let child: u32 = fs::read_to_string(&s.child_pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(child));
    assert_success(&s.run(&["stop", "--force", "civ"]));
    wait_for("reparented child killed", || !alive(child));
    assert!(alive(s.client_pid()), "Stop signaled the fake Steam client");
}

#[test]
fn polite_stop_kills_a_child_reparented_away_from_the_reaper() {
    let s = SteamFixture::new();
    s.f.global_config("stop_timeout = 1\n");
    s.set_mode("child");
    s.config("");
    assert_success(&s.run(&["start", "civ"]));
    wait_for("reaper child", || s.child_pid.exists());
    let child: u32 = fs::read_to_string(&s.child_pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let reaper = claimed_pid(&s);
    assert_eq!(parent_pid(child), Some(reaper));
    let stop = s
        .wineo()
        .args(["stop", "civ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("child reparented during polite Stop", || {
        alive(child) && parent_pid(child) != Some(reaper)
    });
    let stopped = stop.wait_with_output().unwrap();
    assert_success(&stopped);
    wait_for("reparented child killed", || !alive(child));
    assert!(alive(s.client_pid()), "Stop signaled the fake Steam client");
}

#[test]
fn external_stop_makes_attached_steam_run_exit_130() {
    let s = SteamFixture::new();
    s.config("");
    let child = s
        .wineo()
        .args(["run", "civ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("claimed run", || s.f.state("civ").contains("running_at = "));
    assert_success(&s.run(&["stop", "civ"]));
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    s.f.wait_for_run_end("civ", "stopped");
}

#[test]
fn ctrl_c_during_starting_stops_the_wait() {
    let s = SteamFixture::new();
    s.set_mode("none");
    s.config("");
    let child = s
        .wineo()
        .args(["run", "civ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("starting state", || {
        s.f.state("civ").contains("phase = \"starting\"")
    });
    signal(child.id(), "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    s.f.wait_for_run_end("civ", "stopped");
}

#[test]
fn ctrl_c_during_running_stops_the_reaper_tree() {
    let s = SteamFixture::new();
    s.config("");
    let child = s
        .wineo()
        .args(["run", "civ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("claimed run", || s.f.state("civ").contains("running_at = "));
    signal(child.id(), "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("press Ctrl+C again to force it"),
        "{}",
        stderr(&output)
    );
    s.f.wait_for_run_end("civ", "stopped");
    assert!(alive(s.client_pid()));
}

#[test]
fn second_ctrl_c_forces_a_steam_run_without_waiting_for_stop_timeout() {
    let s = SteamFixture::new();
    s.f.global_config("stop_timeout = 4\n");
    s.set_mode("child");
    s.config("");
    let child = s
        .wineo()
        .args(["run", "civ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("reaper child", || s.child_pid.exists());
    let survivor: u32 = fs::read_to_string(&s.child_pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let reaper = claimed_pid(&s);
    let began = Instant::now();
    signal(child.id(), "-INT");
    wait_for("first Ctrl+C started polite Stop", || {
        alive(survivor) && parent_pid(survivor) != Some(reaper)
    });
    signal(child.id(), "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "second Ctrl+C waited for stop_timeout"
    );
    assert!(
        stderr(&output).contains("press Ctrl+C again to force it"),
        "{}",
        stderr(&output)
    );
    wait_for("forced child kill", || !alive(survivor));
    assert!(alive(s.client_pid()));
}

#[test]
fn two_reapers_started_during_before_are_not_claimed() {
    let s = SteamFixture::new();
    s.config(&format!(
        "[hooks]\nbefore = 'printf \"AppId=289070 hold\\n\" > {0}; printf \"AppId=289070 hold\\n\" > {0}; sleep 0.3'\n",
        s.fifo.display()
    ));
    let start = s.run(&["start", "civ"]);
    assert!(!start.status.success(), "{}", stdout(&start));
    assert!(
        stderr(&start).contains("More than one Steam process matches civ."),
        "{}",
        stderr(&start)
    );
    assert_eq!(s.dispatch_count(), 0);
    assert!(alive(s.client_pid()));
}

#[test]
fn force_stop_escalates_a_detached_steam_run_after_the_reaper_exits() {
    let s = SteamFixture::new();
    s.f.global_config("stop_timeout = 4\n");
    s.set_mode("child");
    s.config("");
    assert_success(&s.run(&["start", "civ"]));
    wait_for("claimed run with reaper child", || {
        s.child_pid.exists() && s.f.state("civ").contains("running_at = ")
    });
    let survivor: u32 = fs::read_to_string(&s.child_pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let reaper = claimed_pid(&s);
    let stop = s
        .wineo()
        .args(["stop", "civ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("reaper exited during polite Stop", || {
        !alive(reaper) && alive(survivor) && parent_pid(survivor) != Some(reaper)
    });
    let began = Instant::now();
    assert_success(&s.run(&["stop", "--force", "civ"]));
    assert_success(&stop.wait_with_output().unwrap());
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "force Stop waited for stop_timeout"
    );
    wait_for("forced child kill", || !alive(survivor));
    s.f.wait_for_run_end("civ", "stopped");
    assert!(alive(s.client_pid()), "Stop signaled the fake Steam client");
    assert!(!s.f.stats_log("civ").exists());
}

#[test]
fn attached_run_exits_one_when_two_reapers_appear_during_before() {
    let s = SteamFixture::new();
    s.config(&format!(
        "[hooks]\nbefore = 'printf \"AppId=289070 hold\\n\" > {0}; printf \"AppId=289070 hold\\n\" > {0}; sleep 0.3'\n",
        s.fifo.display()
    ));
    let ran = s.run(&["run", "civ"]);
    assert_eq!(ran.status.code(), Some(1), "{}", stderr(&ran));
    assert!(
        stderr(&ran).contains("More than one Steam process matches civ."),
        "{}",
        stderr(&ran)
    );
    assert_eq!(s.dispatch_count(), 0);
    assert!(alive(s.client_pid()));
}

#[test]
fn lost_proxied_run_does_not_kill_a_live_group_or_write_a_session() {
    let s = SteamFixture::new();
    s.config("");
    let runtime = s.f.runtime("civ");
    fs::create_dir_all(runtime.dir()).unwrap();
    let pid = s.client_pid();
    let boot = wineo_core::process::boot_id().unwrap();
    let start = wineo_core::process::start_time(pid).unwrap();
    fs::write(
        runtime.dir().join("state.toml"),
        format!("status = \"running\"\nphase = \"starting\"\nmode = \"detached\"\ntarget = \"steam\"\nlauncher = \"steam\"\npid = {pid}\ngroup = {pid}\nboot_id = \"{boot}\"\nleader_start = {start}\nstarted_at = \"2026-10-07T00:00:00Z\"\nheartbeat_at = \"2026-10-07T00:01:00Z\"\n"),
    ).unwrap();
    assert!(runtime.status().unwrap().lost);
    let status = s.run(&["status", "civ"]);
    assert_success(&status);
    let state = s.f.state("civ");
    assert!(state.contains("status = \"stopped\""), "{state}");
    assert!(
        state.contains("ended_at = \"2026-10-07T00:01:00Z\""),
        "{state}"
    );
    assert!(
        alive(pid),
        "lost cleanup signaled the live fake Steam client group"
    );
    assert!(!s.f.stats_log("civ").exists());
    assert_success(&s.run(&["start", "civ"]));
}
