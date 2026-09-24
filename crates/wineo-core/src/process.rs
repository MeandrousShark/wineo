//! Process and process-group identities for conservative recovery.

use std::collections::BTreeMap;
use std::fs;
use std::io;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::OwnedFd;
use rustix::io::Errno;
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

/// Telling a recorded process group apart from one that reused its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupOwnership {
    /// The recorded leader is still present in the recorded process group.
    Owned,
    /// The original group is gone, or the machine has rebooted since it ran.
    Gone,
    /// Its identity cannot be proven, so it must stay protected.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcStat {
    state: char,
    start_time: u64,
    group: u32,
}

enum ProcRead {
    Found(ProcStat),
    Gone,
    Unknown,
}

/// This boot's id. A process recorded under another one is long gone.
pub fn boot_id() -> Option<String> {
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(id.trim().to_owned())
}

/// When a process started, in clock ticks since boot, as `/proc/<pid>/stat`
/// records it. None once it is gone or unreadable.
pub fn start_time(pid: u32) -> Option<u64> {
    match read_stat(pid) {
        ProcRead::Found(stat) => Some(stat.start_time),
        ProcRead::Gone | ProcRead::Unknown => None,
    }
}

/// Checks whether a recorded leader still identifies the recorded group.
/// Missing identity is always unknown, even if the numeric group is absent.
pub fn group_ownership(
    group: Option<u32>,
    recorded_boot: Option<&str>,
    leader_start: Option<u64>,
) -> GroupOwnership {
    let (Some(group), Some(recorded_boot), Some(leader_start)) =
        (group, recorded_boot, leader_start)
    else {
        return GroupOwnership::Unknown;
    };
    let Some(current_boot) = boot_id() else {
        return GroupOwnership::Unknown;
    };
    classify_group(
        Some(recorded_boot),
        Some(&current_boot),
        Some(leader_start),
        read_stat(group),
        group_exists(group),
        group,
    )
}

fn classify_group(
    recorded_boot: Option<&str>,
    current_boot: Option<&str>,
    leader_start: Option<u64>,
    leader: ProcRead,
    group_exists: Option<bool>,
    group: u32,
) -> GroupOwnership {
    let (Some(recorded_boot), Some(current_boot), Some(leader_start)) =
        (recorded_boot, current_boot, leader_start)
    else {
        return GroupOwnership::Unknown;
    };
    if recorded_boot != current_boot {
        return GroupOwnership::Gone;
    }
    match leader {
        ProcRead::Found(stat) if stat.start_time != leader_start => GroupOwnership::Gone,
        ProcRead::Found(stat) if stat.group != group => GroupOwnership::Unknown,
        ProcRead::Found(_) => match group_exists {
            Some(true) => GroupOwnership::Owned,
            Some(false) => GroupOwnership::Gone,
            None => GroupOwnership::Unknown,
        },
        ProcRead::Gone => match group_exists {
            Some(true) => GroupOwnership::Unknown,
            Some(false) => GroupOwnership::Gone,
            None => GroupOwnership::Unknown,
        },
        ProcRead::Unknown => GroupOwnership::Unknown,
    }
}

/// Signals each verified member by pidfd. The caller never signals a process
/// by a group number alone, which could be reused between checking and killing.
pub fn signal_group_if_owned(
    group: Option<u32>,
    recorded_boot: Option<&str>,
    leader_start: Option<u64>,
    signal: Signal,
) -> io::Result<GroupOwnership> {
    let (Some(group), Some(recorded_boot), Some(leader_start)) =
        (group, recorded_boot, leader_start)
    else {
        return Ok(GroupOwnership::Unknown);
    };
    match group_ownership(Some(group), Some(recorded_boot), Some(leader_start)) {
        GroupOwnership::Owned => {}
        other => return Ok(other),
    }

    let members = match open_group_members(group) {
        Ok(members) => members,
        Err(()) => return Ok(GroupOwnership::Unknown),
    };
    match group_has_live_members(group) {
        Some(false) => return Ok(GroupOwnership::Owned),
        Some(true) => {}
        None => return Ok(GroupOwnership::Unknown),
    }
    let Some((_, _, leader_fd)) = members
        .iter()
        .find(|(pid, start, _)| *pid == group && *start == leader_start)
    else {
        return Ok(GroupOwnership::Unknown);
    };
    if !leader_is_recorded(group, recorded_boot, leader_start)
        || pidfd_exited(leader_fd).unwrap_or(true)
    {
        return Ok(
            match group_ownership(Some(group), Some(recorded_boot), Some(leader_start)) {
                GroupOwnership::Gone => GroupOwnership::Gone,
                GroupOwnership::Owned | GroupOwnership::Unknown => GroupOwnership::Unknown,
            },
        );
    }

    let mut signaled = false;
    let mut leader = None;
    for (pid, start_time, pidfd) in members {
        if pid == group {
            leader = Some((start_time, pidfd));
            continue;
        }
        if !leader_is_recorded(group, recorded_boot, leader_start) {
            return Ok(GroupOwnership::Unknown);
        }
        if !member_is_recorded(pid, group, start_time) {
            continue;
        }
        if pidfd_exited(&pidfd)? {
            continue;
        }
        match pidfd_send_signal(&pidfd, signal) {
            Ok(()) => signaled = true,
            Err(Errno::SRCH) => {}
            Err(err) => return Err(io::Error::from(err)),
        }
    }
    let Some((start_time, pidfd)) = leader else {
        return Ok(GroupOwnership::Unknown);
    };
    if leader_is_recorded(group, recorded_boot, leader_start)
        && member_is_recorded(group, group, start_time)
        && !pidfd_exited(&pidfd)?
    {
        match pidfd_send_signal(&pidfd, signal) {
            Ok(()) => signaled = true,
            Err(Errno::SRCH) => {}
            Err(err) => return Err(io::Error::from(err)),
        }
    }

    if signaled {
        Ok(GroupOwnership::Owned)
    } else {
        Ok(group_ownership(
            Some(group),
            Some(recorded_boot),
            Some(leader_start),
        ))
    }
}

/// Signals members of a group whose ownership was established while its
/// leader was live and whose caller still holds the active operation lock.
/// This can continue a cleanup after that leader has exited.
pub fn signal_group_if_live(
    group: Option<u32>,
    recorded_boot: Option<&str>,
    leader_start: Option<u64>,
    signal: Signal,
) -> io::Result<GroupOwnership> {
    let (Some(group), Some(recorded_boot), Some(leader_start)) =
        (group, recorded_boot, leader_start)
    else {
        return Ok(GroupOwnership::Unknown);
    };
    if live_group_ownership(group, recorded_boot, leader_start) != GroupOwnership::Owned {
        return Ok(live_group_ownership(group, recorded_boot, leader_start));
    }
    let members = match open_group_members(group) {
        Ok(members) => members,
        Err(()) => return Ok(GroupOwnership::Unknown),
    };
    if live_group_ownership(group, recorded_boot, leader_start) != GroupOwnership::Owned {
        return Ok(GroupOwnership::Unknown);
    }
    let mut signaled = false;
    for (pid, start_time, pidfd) in members {
        if live_group_ownership(group, recorded_boot, leader_start) != GroupOwnership::Owned {
            return Ok(GroupOwnership::Unknown);
        }
        if !member_is_recorded(pid, group, start_time) || pidfd_exited(&pidfd)? {
            continue;
        }
        match pidfd_send_signal(&pidfd, signal) {
            Ok(()) => signaled = true,
            Err(Errno::SRCH) => {}
            Err(err) => return Err(io::Error::from(err)),
        }
    }
    if signaled {
        Ok(GroupOwnership::Owned)
    } else {
        Ok(live_group_ownership(group, recorded_boot, leader_start))
    }
}

/// Wait-side ownership retains the identity established by the live owner.
/// With a dead leader, an extant group still needs settling, not a signal.
pub fn live_group_ownership(group: u32, recorded_boot: &str, leader_start: u64) -> GroupOwnership {
    group_ownership(Some(group), Some(recorded_boot), Some(leader_start))
}

fn leader_is_recorded(group: u32, recorded_boot: &str, leader_start: u64) -> bool {
    boot_id().as_deref() == Some(recorded_boot)
        && matches!(read_stat(group), ProcRead::Found(stat) if stat.start_time == leader_start && stat.group == group)
}

fn member_is_recorded(pid: u32, group: u32, start_time: u64) -> bool {
    matches!(read_stat(pid), ProcRead::Found(stat) if stat.start_time == start_time && stat.group == group)
}

/// Whether a group whose leader has exited has nothing left that can run.
/// Killing a group can reap its leader before the rest, and a group number
/// is not reused while any member, zombie or not, remains. `None` means the
/// leader is not gone or the group could not be inspected.
pub fn leaderless_group_settled(group: u32) -> Option<bool> {
    if !matches!(read_stat(group), ProcRead::Gone) {
        return None;
    }
    match group_exists(group)? {
        false => Some(true),
        true => group_has_live_members(group).map(|live| !live),
    }
}

/// Returns whether a group has a process that can still run or fork.
pub fn group_has_live_members(group: u32) -> Option<bool> {
    let processes = fs::read_dir("/proc").ok()?;
    for entry in processes {
        let entry = entry.ok()?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        match read_stat(pid) {
            ProcRead::Found(stat)
                if stat.group == group && !matches!(stat.state, 'Z' | 'X' | 'x') =>
            {
                return Some(true);
            }
            ProcRead::Found(_) | ProcRead::Gone => {}
            ProcRead::Unknown => return None,
        }
    }
    Some(false)
}

fn pidfd_exited(pidfd: &OwnedFd) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(pidfd, PollFlags::IN)];
    poll(&mut descriptors, Some(&Timespec::default()))?;
    Ok(!descriptors[0].revents().is_empty())
}

fn open_group_members(group: u32) -> Result<Vec<(u32, u64, OwnedFd)>, ()> {
    let processes = fs::read_dir("/proc").map_err(|_| ())?;
    let mut members = BTreeMap::new();
    for entry in processes {
        let entry = entry.map_err(|_| ())?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = match read_stat(pid) {
            ProcRead::Found(stat) => stat,
            ProcRead::Gone => continue,
            ProcRead::Unknown => return Err(()),
        };
        if stat.group != group {
            continue;
        }
        let pid_number = pid;
        let Some(pid) = i32::try_from(pid_number).ok().and_then(Pid::from_raw) else {
            return Err(());
        };
        let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
            Ok(pidfd) => pidfd,
            Err(Errno::SRCH) => continue,
            Err(_) => return Err(()),
        };
        if member_is_recorded(pid_number, group, stat.start_time) {
            if pidfd_exited(&pidfd).map_err(|_| ())? {
                continue;
            }
            members.insert(pid_number, (stat.start_time, pidfd));
        }
    }
    Ok(members
        .into_iter()
        .map(|(pid, (start_time, pidfd))| (pid, start_time, pidfd))
        .collect())
}

fn read_stat(pid: u32) -> ProcRead {
    let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return ProcRead::Gone,
        Err(_) => return ProcRead::Unknown,
    };
    let Some(fields) = text
        .rsplit_once(')')
        .map(|(_, fields)| fields.split_whitespace())
    else {
        return ProcRead::Unknown;
    };
    let fields: Vec<_> = fields.collect();
    let parsed = fields
        .first()
        .and_then(|field| field.chars().next())
        .zip(fields.get(2).and_then(|field| field.parse::<u32>().ok()))
        .zip(fields.get(19).and_then(|field| field.parse::<u64>().ok()));
    match parsed {
        Some(((state, group), start_time)) => ProcRead::Found(ProcStat {
            state,
            group,
            start_time,
        }),
        None => ProcRead::Unknown,
    }
}

fn group_exists(group: u32) -> Option<bool> {
    let pid = i32::try_from(group).ok().and_then(Pid::from_raw)?;
    match rustix::process::test_kill_process_group(pid) {
        Ok(()) | Err(Errno::PERM) => Some(true),
        Err(Errno::SRCH) => Some(false),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    fn process_group(command: &mut Command) -> &mut Command {
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
    }

    fn stop_group(child: &mut Child, group: u32, boot: &str, start: u64) {
        let _ = signal_group_if_owned(Some(group), Some(boot), Some(start), Signal::KILL);
        let _ = child.wait();
        let deadline = Instant::now() + Duration::from_secs(2);
        while group_ownership(Some(group), Some(boot), Some(start)) == GroupOwnership::Owned
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
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
            group_ownership(Some(group), Some(&boot), Some(leader_start)),
            GroupOwnership::Owned
        );
        assert_eq!(
            signal_group_if_owned(Some(group), Some(&boot), Some(leader_start), Signal::KILL)
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
            group_ownership(Some(group), Some(&boot), Some(leader_start)),
            GroupOwnership::Owned
        );
        assert_eq!(
            group_ownership(Some(foreign.id()), Some(&boot), start_time(foreign.id())),
            GroupOwnership::Owned
        );

        let foreign_id = foreign.id();
        let foreign_boot = boot_id().unwrap();
        let foreign_start = start_time(foreign_id).unwrap();
        stop_group(&mut foreign, foreign_id, &foreign_boot, foreign_start);
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
            group_ownership(Some(group), Some(&boot), Some(leader_start)),
            GroupOwnership::Unknown
        );
        assert_eq!(
            signal_group_if_owned(Some(group), Some(&boot), Some(leader_start), Signal::KILL)
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
        while waitid(WaitId::Pid(member_pid), options).unwrap().is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(group_exists(group), Some(true));
        assert_eq!(leaderless_group_settled(group), Some(true));

        member.wait().unwrap();
        assert_eq!(leaderless_group_settled(group), Some(true));
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
        let ownership_with_zombie_leader =
            group_ownership(Some(group), Some(&boot), Some(leader_start));
        let has_live_child = group_has_live_members(group);
        let signaled =
            signal_group_if_live(Some(group), Some(&boot), Some(leader_start), Signal::KILL);
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
}
