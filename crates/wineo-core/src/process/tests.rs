use rustix::process::{WaitId, WaitIdOptions, waitid};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// A process's state, parent, and group, or how reading its stat fails.
type FakeStat = Result<(char, u32, u32), Errno>;

/// A stand-in for `/proc` and `getpgid`. A denied entry's group is
/// given separately, as only the kernel would know it.
struct FakeProc {
    pids: Vec<u32>,
    read: Box<dyn Fn(u32) -> io::Result<String>>,
    group_of: Box<dyn Fn(u32) -> io::Result<u32>>,
}

impl FakeProc {
    fn new(entries: Vec<(u32, FakeStat)>, groups: Vec<(u32, Result<u32, Errno>)>) -> Self {
        let error = |errno: Errno| io::Error::from_raw_os_error(errno.raw_os_error());
        Self {
            pids: entries.iter().map(|(pid, _)| *pid).collect(),
            read: Box::new(move |pid| {
                match entries
                    .iter()
                    .find(|(entry, _)| *entry == pid)
                    .map(|(_, stat)| *stat)
                {
                    Some(Ok((state, parent, group))) => Ok(format!(
                        "{pid} (fake) {state} {parent} {group} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 42 0\n"
                    )),
                    Some(Err(errno)) => Err(error(errno)),
                    None => Err(error(Errno::NOENT)),
                }
            }),
            group_of: Box::new(
                move |pid| match groups.iter().find(|(entry, _)| *entry == pid) {
                    Some((_, Ok(group))) => Ok(*group),
                    Some((_, Err(errno))) => Err(error(*errno)),
                    None => Err(error(Errno::SRCH)),
                },
            ),
        }
    }

    fn probe(&self) -> Probe<'_> {
        Probe {
            read: &*self.read,
            group_of: &*self.group_of,
        }
    }
}

#[test]
fn group_scans_skip_vanished_entries_and_others_groups() {
    let fake = FakeProc::new(
        vec![
            (10, Err(Errno::ACCESS)),
            (11, Err(Errno::PERM)),
            (12, Err(Errno::NOENT)),
            (13, Err(Errno::SRCH)),
            (14, Ok(('Z', 1, 55))),
            (15, Ok(('S', 1, 66))),
            (18, Err(Errno::PERM)),
        ],
        // 10 is in another group, and 11 and 18 have exited meanwhile.
        vec![(10, Ok(66)), (11, Err(Errno::SRCH))],
    );
    assert_eq!(group_live_in(&fake.pids, &fake.probe(), 55), Some(false));
    let fake = FakeProc::new(
        vec![(10, Err(Errno::ACCESS)), (16, Ok(('S', 1, 55)))],
        vec![(10, Ok(66))],
    );
    assert_eq!(group_live_in(&fake.pids, &fake.probe(), 55), Some(true));
    let fake = FakeProc::new(vec![(17, Err(Errno::IO))], vec![]);
    assert_eq!(group_live_in(&fake.pids, &fake.probe(), 55), None);
}

#[test]
fn a_denied_entry_in_the_group_counts_as_live() {
    // One of ours that made itself undumpable, under `hidepid`.
    let fake = FakeProc::new(vec![(10, Err(Errno::PERM))], vec![(10, Ok(55))]);
    assert_eq!(group_live_in(&fake.pids, &fake.probe(), 55), Some(true));
    let fake = FakeProc::new(vec![(10, Err(Errno::PERM))], vec![(10, Err(Errno::IO))]);
    assert_eq!(group_live_in(&fake.pids, &fake.probe(), 55), None);
}

#[test]
fn vanished_proc_stat_is_gone_but_unreadable_or_malformed_stat_is_unknown() {
    for errno in [Errno::NOENT, Errno::SRCH] {
        assert!(matches!(
            parse_stat_read(Err(io::Error::from_raw_os_error(errno.raw_os_error()))),
            ProcRead::Gone
        ));
    }
    assert!(matches!(
        parse_stat_read(Err(io::Error::from_raw_os_error(
            Errno::ACCESS.raw_os_error()
        ))),
        ProcRead::Unknown
    ));
    assert!(matches!(
        parse_stat_read(Ok("incomplete stat".to_owned())),
        ProcRead::Unknown
    ));
}

#[test]
fn dead_proc_stat_without_a_group_is_gone_but_a_zombie_keeps_its_identity() {
    let dead = "55 (wineo) X 0 -1 -1 0 -1 4228108 869 488 0 0 0 0 0 0 20 0 0 0 13843914 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 17 7 0 0 0 0 0 0 0 0 0 0 0 0 9\n";
    assert!(matches!(
        parse_stat_read(Ok(dead.to_owned())),
        ProcRead::Gone
    ));
    assert!(matches!(
        parse_stat_read(Ok(dead.replacen(") X ", ") x ", 1))),
        ProcRead::Gone
    ));
    let zombie = dead.replacen(") X 0 -1", ") Z 0 55", 1);
    assert!(matches!(
        parse_stat_read(Ok(zombie)),
        ProcRead::Found(ProcStat {
            state: 'Z',
            group: 55,
            start_time: 13843914,
        })
    ));
    assert!(matches!(
        parse_stat_read(Ok("55 (wineo) X 0 -1".to_owned())),
        ProcRead::Unknown
    ));
}

fn id(group: u32, boot: &str, leader_start: u64) -> Identity {
    Identity {
        group,
        boot_id: boot.to_owned(),
        leader_start,
    }
}

fn process_group(command: &mut Command) -> &mut Command {
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

fn kill_group(child: &mut Child, group: u32, boot: &str, start: u64) {
    let _ = id(group, boot, start).signal_if_owned(Signal::KILL);
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(2);
    while id(group, boot, start).ownership() == GroupOwnership::Owned && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

/// The soft and hard "Max core file size" of a process, as /proc prints them.
fn core_limits(pid: u32) -> (String, String) {
    let limits = fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
    let line = limits
        .lines()
        .find(|line| line.starts_with("Max core file size"))
        .unwrap();
    let mut fields = line.split_whitespace().skip(4);
    (
        fields.next().unwrap().to_owned(),
        fields.next().unwrap().to_owned(),
    )
}

#[test]
fn quieting_core_dumps_sets_the_soft_limit_to_one_and_keeps_the_hard_limit() {
    let own = core_limits(std::process::id());
    if own.1 == "0" {
        return;
    }
    let mut child = process_group(&mut Command::new("sleep"))
        .arg("30")
        .spawn()
        .unwrap();
    quiet_core_dumps(&child);
    let limits = core_limits(child.id());
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(limits, ("1".to_owned(), own.1));
}

#[test]
fn a_child_left_alone_keeps_the_inherited_core_limit() {
    let mut child = process_group(&mut Command::new("sleep"))
        .arg("30")
        .spawn()
        .unwrap();
    let limits = core_limits(child.id());
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(limits, core_limits(std::process::id()));
}

#[test]
fn a_running_process_has_a_start_time_and_the_boot_an_id() {
    let own = start_time(std::process::id()).unwrap();
    assert!(own > 0);
    assert_eq!(start_time(std::process::id()), Some(own));
    assert!(!boot_id().unwrap().is_empty());
    assert_eq!(start_time(u32::MAX), None);
}

#[test]
fn a_live_group_is_signaled_without_touching_a_foreign_group() {
    let mut owned = process_group(&mut Command::new("sh"))
        .args(["-c", "sleep 30 >/dev/null 2>&1 & echo ready; wait"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    use std::io::BufRead;
    std::io::BufReader::new(owned.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    let mut foreign = process_group(&mut Command::new("sleep"))
        .arg("30")
        .spawn()
        .unwrap();
    let group = owned.id();
    let boot = boot_id().unwrap();
    let leader_start = start_time(group).unwrap();

    assert_eq!(
        id(group, &boot, leader_start).ownership(),
        GroupOwnership::Owned
    );
    assert_eq!(
        id(group, &boot, leader_start)
            .signal_if_owned(Signal::KILL)
            .unwrap(),
        GroupOwnership::Owned
    );
    owned.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while group_has_live_members(group) != Some(false) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(group_has_live_members(group), Some(false));
    assert_ne!(
        id(group, &boot, leader_start).ownership(),
        GroupOwnership::Owned
    );
    assert_eq!(
        Identity::of(foreign.id()).unwrap().ownership(),
        GroupOwnership::Owned
    );

    let foreign_id = foreign.id();
    let foreign_boot = boot_id().unwrap();
    let foreign_start = start_time(foreign_id).unwrap();
    kill_group(&mut foreign, foreign_id, &foreign_boot, foreign_start);
}

#[test]
fn a_leaderless_group_is_unknown_and_keeps_its_children_alive() {
    let mut leader = process_group(&mut Command::new("sh"))
        .args(["-c", "sleep 30 >/dev/null 2>&1 & echo $!; exit 0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let group = leader.id();
    let boot = boot_id().unwrap();
    let leader_start = start_time(group).unwrap();
    let mut output = String::new();
    use std::io::Read;
    leader
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    leader.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while start_time(group).is_some() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        id(group, &boot, leader_start).ownership(),
        GroupOwnership::Unknown
    );
    assert_eq!(
        id(group, &boot, leader_start)
            .signal_if_owned(Signal::KILL)
            .unwrap(),
        GroupOwnership::Unknown
    );
    let child_pid = output.trim().parse::<u32>().unwrap();
    assert!(start_time(child_pid).is_some());
    let child_pid = Pid::from_raw(child_pid as i32).unwrap();
    let _ = rustix::process::kill_process(child_pid, Signal::KILL);
}

#[test]
fn a_leaderless_group_settles_once_only_zombies_are_left() {
    let mut leader = process_group(&mut Command::new("sleep"))
        .arg("30")
        .spawn()
        .unwrap();
    let group = leader.id();
    let mut member = Command::new("sleep")
        .arg("30")
        .process_group(group as i32)
        .spawn()
        .unwrap();
    assert_eq!(leaderless_group_settled(group), None);

    leader.kill().unwrap();
    leader.wait().unwrap();
    assert_eq!(leaderless_group_settled(group), Some(false));

    // Killed but not yet reaped, the member is a zombie still in the group.
    let member_pid = Pid::from_raw(member.id() as i32).unwrap();
    rustix::process::kill_process(member_pid, Signal::KILL).unwrap();
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    let deadline = Instant::now() + Duration::from_secs(2);
    while waitid(WaitId::Pid(member_pid), options).unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(group_exists(group), Some(true));
    assert_eq!(leaderless_group_settled(group), Some(true));

    member.wait().unwrap();
    assert_eq!(leaderless_group_settled(group), Some(true));
}

#[test]
fn a_bystander_whose_name_is_not_utf8_does_not_stop_a_group_scan() {
    let dir = tempfile::tempdir().unwrap();
    // The kernel keeps 15 bytes of a name, which splits the final `é` here.
    let script = dir.path().join("aaaaaaaaaaaaaa\u{e9}");
    crate::test_script::write_script(&script, "#!/bin/sh\nsleep 30\nexit 0\n");
    let mut bystander = process_group(&mut Command::new(&script)).spawn().unwrap();
    let comm = format!("/proc/{}/comm", bystander.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fs::read(&comm).unwrap().contains(&0xc3) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(std::str::from_utf8(&fs::read(&comm).unwrap()).is_err());
    assert!(matches!(
        read_stat(bystander.id()),
        ProcRead::Found(stat) if stat.group == bystander.id()
    ));

    let mut leader = process_group(&mut Command::new("sleep"))
        .arg("30")
        .spawn()
        .unwrap();
    let group = leader.id();
    assert_eq!(group_has_live_members(group), Some(true));
    leader.kill().unwrap();
    leader.wait().unwrap();
    assert_eq!(group_has_live_members(group), Some(false));

    let _ = rustix::process::kill_process_group(
        Pid::from_raw(bystander.id() as i32).unwrap(),
        Signal::KILL,
    );
    let _ = bystander.wait();
}

#[test]
fn a_live_owner_keeps_its_zombie_leader_as_a_witness_until_children_stop() {
    let mut child = process_group(&mut Command::new("sh"))
        .args(["-c", "sleep 2 >/dev/null 2>&1 & exit 0"])
        .spawn()
        .unwrap();
    let group = child.id();
    let boot = boot_id().unwrap();
    let leader_start = start_time(group).unwrap();
    let pid = Pid::from_raw(group as i32).unwrap();
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut exited = false;
    while Instant::now() < deadline {
        if waitid(WaitId::Pid(pid), options).unwrap().is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let ownership_with_zombie_leader = id(group, &boot, leader_start).ownership();
    let has_live_child = group_has_live_members(group);
    let signaled = id(group, &boot, leader_start).signal_if_live(Signal::KILL);
    let _ = rustix::process::kill_process_group(pid, Signal::KILL);
    let deadline = Instant::now() + Duration::from_secs(2);
    while group_has_live_members(group) != Some(false) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let settled_members = group_has_live_members(group);
    child.wait().unwrap();
    assert!(exited, "leader should exit while remaining unreaped");
    assert_eq!(ownership_with_zombie_leader, GroupOwnership::Owned);
    assert_eq!(has_live_child, Some(true));
    assert_eq!(signaled.unwrap(), GroupOwnership::Owned);
    assert_eq!(settled_members, Some(false));
}

/// Starts `script` leading its own group and waits for it to print a line.
fn ready_group(script: &str) -> Child {
    let mut child = process_group(&mut Command::new("sh"))
        .args(["-c", script])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    use std::io::BufRead;
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    child
}

/// Runs `stop_group` on `child`, with `stop` applied to the token from
/// another thread, and returns its answer, the steps taken, and how long
/// it took.
fn stopped(
    mut child: Child,
    grace: Duration,
    stop: impl FnOnce(&Token) + Send + 'static,
) -> (Ended, Vec<Step>, Duration) {
    let token = Token::new();
    let stopper = token.clone();
    let started = Instant::now();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        stop(&stopper);
    });
    let mut steps = Vec::new();
    let mut escalation = Escalation::new(grace, None);
    let answer = stop_group(
        &child,
        Signal::TERM,
        &mut escalation,
        &token,
        None,
        |step| {
            steps.extend(step);
            Ok::<_, ()>(())
        },
    );
    let elapsed = started.elapsed();
    let live = group_has_live_members(child.id());
    let _ = signal_led_group(&child, Signal::KILL);
    child.wait().unwrap();
    assert_eq!(live, Some(false), "the group outlived the stop");
    (answer.unwrap().unwrap(), steps, elapsed)
}

#[test]
fn a_group_that_closes_when_asked_is_never_killed() {
    let child = ready_group("sleep 30 & echo ready; wait");
    let (stopped, steps, elapsed) = stopped(child, Duration::from_secs(30), Token::cancel);
    assert_eq!(stopped, Ended::Stopped);
    assert_eq!(steps, [Step::Ask]);
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn a_group_that_ignores_the_ask_is_killed_after_the_grace_period() {
    let grace = Duration::from_millis(300);
    let child = ready_group("trap '' TERM; sleep 30 & echo ready; wait");
    let (stopped, steps, elapsed) = stopped(child, grace, Token::cancel);
    assert_eq!(stopped, Ended::Stopped);
    assert_eq!(steps, [Step::Ask, Step::Kill]);
    assert!(elapsed >= grace);
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn a_zero_grace_period_kills_along_with_the_ask() {
    // The leader would exit cleanly after the ask if given any time.
    let child = ready_group("sleep 30 & trap '' TERM; echo ready; wait; exit 0");
    let (stopped, steps, _) = stopped(child, Duration::ZERO, Token::cancel);
    assert_eq!(stopped, Ended::Stopped);
    assert_eq!(steps, [Step::Ask, Step::Kill]);
}

#[test]
fn forcing_a_stop_kills_without_asking_or_waiting() {
    let child = ready_group("trap '' TERM; sleep 30 & echo ready; wait");
    let (stopped, steps, elapsed) = stopped(child, Duration::from_secs(30), Token::force);
    assert_eq!(stopped, Ended::Stopped);
    assert_eq!(steps, [Step::Kill]);
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn a_stop_waits_for_the_whole_group_after_the_leader_exits() {
    // The leader exits at once, but its child ignores the ask.
    let grace = Duration::from_millis(300);
    let child =
        ready_group("trap 'exit 0' TERM; sh -c 'trap \"\" TERM; echo ready; sleep 30' & wait");
    let (stopped, steps, elapsed) = stopped(child, grace, Token::cancel);
    assert_eq!(stopped, Ended::Stopped);
    assert_eq!(steps, [Step::Ask, Step::Kill]);
    assert!(elapsed >= grace);
}

#[test]
fn a_leader_exiting_on_its_own_leaves_its_group_alone() {
    let mut child = ready_group("sleep 30 & echo ready; exit 0");
    let token = Token::new();
    let mut escalation = Escalation::new(Duration::ZERO, None);
    let stopped = stop_group(
        &child,
        Signal::TERM,
        &mut escalation,
        &token,
        None,
        |step| {
            assert_eq!(step, None);
            Ok::<_, ()>(())
        },
    );
    let live = group_has_live_members(child.id());
    let _ = signal_led_group(&child, Signal::KILL);
    child.wait().unwrap();
    assert_eq!(stopped.unwrap().unwrap(), Ended::Exited);
    assert_eq!(live, Some(true));
}

#[test]
fn a_wait_with_nothing_happening_sleeps_until_the_leader_exits() {
    let mut child = process_group(&mut Command::new("sleep"))
        .arg("1")
        .spawn()
        .unwrap();
    let token = Token::new();
    let mut escalation = Escalation::new(Duration::ZERO, None);
    let mut rounds = 0;
    let stopped = stop_group(&child, Signal::TERM, &mut escalation, &token, None, |_| {
        rounds += 1;
        Ok::<_, ()>(())
    });
    child.wait().unwrap();
    assert_eq!(stopped.unwrap().unwrap(), Ended::Exited);
    // Once on the way in and once when the exit wakes it.
    assert!(rounds <= 3, "woke {rounds} times");
}

#[test]
fn a_stop_without_pidfd_or_eventfd_still_stops_and_settles_the_group() {
    let mut child = ready_group("trap '' TERM; sleep 30 & echo ready; wait");
    let token = Token::new();
    let stopper = token.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        stopper.cancel();
    });
    let grace = Duration::from_millis(200);
    let mut escalation = Escalation::new(grace, None);
    let mut steps = Vec::new();
    let answer = stop_group_with(
        &child,
        Signal::TERM,
        &mut escalation,
        &token,
        None,
        |step| {
            steps.extend(step);
            Ok::<_, ()>(())
        },
        group_has_live_members,
        Err(io::Error::from_raw_os_error(Errno::NOSYS.raw_os_error())),
    );
    let live = group_has_live_members(child.id());
    let _ = signal_led_group(&child, Signal::KILL);
    child.wait().unwrap();
    assert_eq!(answer.unwrap().unwrap(), Ended::Stopped);
    assert_eq!(steps, [Step::Ask, Step::Kill]);
    assert_eq!(live, Some(false));
}

#[test]
fn a_killed_group_that_cant_be_inspected_is_never_called_stopped() {
    let mut child = ready_group("trap '' TERM; sleep 30 & echo ready; wait");
    let token = Token::new();
    token.force();
    let mut escalation = Escalation::new(Duration::from_secs(30), None);
    let answer = stop_group_probed(
        &child,
        Signal::TERM,
        &mut escalation,
        &token,
        None,
        |_| Ok::<_, ()>(()),
        |_| None,
    );
    let _ = signal_led_group(&child, Signal::KILL);
    child.wait().unwrap();
    assert_eq!(answer.unwrap().unwrap(), Ended::Unconfirmed);
}

#[test]
fn a_failing_output_read_after_the_leader_exits_still_kills_its_group() {
    // The leader hands off to a descendant and exits, and then reading
    // its output fails, with no stop requested.
    let mut child = ready_group("sleep 30 & echo ready; exit 0");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !leader_exited(&child).unwrap() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let token = Token::new();
    let mut escalation = Escalation::new(Duration::from_secs(30), None);
    let answer = stop_group(
        &child,
        Signal::TERM,
        &mut escalation,
        &token,
        None,
        |step| step.map_or(Err("output read failed"), |_| Ok(())),
    );
    // Checked before reaping, while the group number is still pinned.
    let live = group_has_live_members(child.id());
    let _ = signal_led_group(&child, Signal::KILL);
    child.wait().unwrap();
    assert_eq!(answer.unwrap(), Err(("output read failed", Ended::Stopped)));
    assert_eq!(live, Some(false));
}

/// Runs `cleanup` on another thread and fails rather than hangs if it
/// outlasts `limit`.
fn finishes_within<T: Send + 'static>(
    limit: Duration,
    cleanup: impl FnOnce() -> T + Send + 'static,
) -> (T, Duration) {
    let (sender, finished) = std::sync::mpsc::channel();
    let started = Instant::now();
    thread::spawn(move || {
        let _ = sender.send(cleanup());
    });
    let value = finished
        .recv_timeout(limit)
        .expect("cleanup did not finish within its bound");
    (value, started.elapsed())
}

#[test]
fn cleanup_of_a_group_that_never_settles_gives_up_unconfirmed() {
    // Stands in for a member stuck in uninterruptible sleep after SIGKILL.
    let child = ready_group("echo ready; sleep 30");
    let bound = Duration::from_millis(300);
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let leader = child.clone();
    let (answer, elapsed) = finishes_within(bound + Duration::from_secs(3), move || {
        let leader = leader.lock().unwrap();
        abandon(&leader, &|_| Some(true), "read failed", bound).map_err(|err| err.kind())
    });
    child.lock().unwrap().wait().unwrap();
    assert_eq!(answer, Ok(Err(("read failed", Ended::Unconfirmed))));
    assert!(elapsed >= bound);
}

#[test]
fn reaping_a_leader_that_never_becomes_waitable_gives_up() {
    let bound = Duration::from_millis(300);
    let (reaped, elapsed) = finishes_within(bound + Duration::from_secs(3), move || {
        let mut leader = process_group(&mut Command::new("sleep"))
            .arg("30")
            .spawn()
            .unwrap();
        // A reap that never succeeds, as for a leader in uninterruptible sleep.
        let reaped = reap_within(&mut leader, bound, |_| Ok(false));
        let _ = leader.kill();
        let _ = leader.wait();
        reaped
    });
    assert!(!reaped);
    assert!(elapsed >= bound);
}

#[test]
fn a_failing_step_callback_never_keeps_the_group_from_its_signal() {
    let mut child = ready_group("trap '' TERM; echo ready; sleep 30");
    let token = Token::new();
    token.force();
    let mut escalation = Escalation::new(Duration::from_secs(30), None);
    let answer = stop_group(
        &child,
        Signal::TERM,
        &mut escalation,
        &token,
        None,
        |step| step.map_or(Ok(()), |_| Err("cleanup failed")),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let status = child.try_wait().unwrap();
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert_eq!(answer.unwrap(), Err(("cleanup failed", Ended::Stopped)));
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.and_then(|status| status.signal()), Some(9));
}

#[test]
fn a_reused_group_number_is_never_signaled() {
    let mut child = process_group(&mut Command::new("sleep"))
        .arg("30")
        .spawn()
        .unwrap();
    let real = Identity::of(child.id()).unwrap();
    for stale in [
        Identity {
            leader_start: real.leader_start + 1,
            ..real.clone()
        },
        Identity {
            boot_id: "another-boot".to_owned(),
            ..real.clone()
        },
    ] {
        assert_eq!(stale.ownership(), GroupOwnership::Gone);
        assert_eq!(stale.kill_now().unwrap(), GroupOwnership::Gone);
        assert_eq!(
            stale.signal_if_live(Signal::KILL).unwrap(),
            GroupOwnership::Gone
        );
    }
    let running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    child.wait().unwrap();
    assert!(running);
}

#[test]
fn escalation_asks_then_kills_and_retries_only_when_told_to() {
    let start = Instant::now();
    let grace = Duration::from_secs(5);
    let mut once = Escalation::new(grace, None);
    assert_eq!(once.poll(start), None);
    assert_eq!(once.request(false, start), Some(Step::Ask));
    assert_eq!(once.deadline(), Some(start + grace));
    assert_eq!(once.poll(start + grace / 2), None);
    assert_eq!(once.poll(start + grace), Some(Step::Kill));
    assert_eq!(once.poll(start + grace * 4), None);
    assert_eq!(once.request(false, start + grace * 4), Some(Step::Rekill));

    let retry = Duration::from_secs(1);
    let mut repeating = Escalation::new(grace, Some(retry));
    assert_eq!(repeating.request(true, start), Some(Step::Kill));
    assert!(repeating.stopping());
    assert_eq!(repeating.killed_at(), Some(start));
    assert_eq!(repeating.poll(start + retry), Some(Step::Rekill));
    assert_eq!(repeating.killed_at(), Some(start));

    let token = Token::new();
    let mut following = Escalation::new(grace, None);
    assert_eq!(following.follow(&token, start), None);
    token.cancel();
    assert_eq!(following.follow(&token, start), Some(Step::Ask));
    assert_eq!(following.follow(&token, start), None);
    token.force();
    assert_eq!(following.follow(&token, start), Some(Step::Kill));
    assert_eq!(following.follow(&token, start), None);
}

#[test]
fn stale_group_classification_requires_a_complete_matching_identity() {
    let group = 1234;
    assert_eq!(
        classify_group(
            None,
            Some("boot"),
            Some(7),
            ProcRead::Gone,
            Some(false),
            group
        ),
        GroupOwnership::Unknown
    );
    assert_eq!(
        classify_group(
            Some("old"),
            Some("new"),
            Some(7),
            ProcRead::Unknown,
            None,
            group
        ),
        GroupOwnership::Gone
    );
    assert_eq!(
        classify_group(
            Some("boot"),
            Some("boot"),
            Some(7),
            ProcRead::Found(ProcStat {
                state: 'S',
                start_time: 8,
                group,
            }),
            Some(true),
            group,
        ),
        GroupOwnership::Gone
    );
    assert_eq!(
        classify_group(
            Some("boot"),
            Some("boot"),
            Some(7),
            ProcRead::Gone,
            Some(true),
            group,
        ),
        GroupOwnership::Unknown
    );
    assert_eq!(
        classify_group(
            Some("boot"),
            Some("boot"),
            Some(7),
            ProcRead::Unknown,
            Some(true),
            group,
        ),
        GroupOwnership::Unknown
    );
    assert_eq!(
        classify_group(
            Some("boot"),
            Some("boot"),
            Some(7),
            ProcRead::Found(ProcStat {
                state: 'S',
                start_time: 7,
                group,
            }),
            Some(true),
            group,
        ),
        GroupOwnership::Owned
    );
}
