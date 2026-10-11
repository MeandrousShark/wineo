//! Process and process-group identities for conservative recovery.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::process::Child;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::OwnedFd;
use rustix::io::Errno;
use rustix::process::{
    Pid, PidfdFlags, Resource, Rlimit, Signal, getrlimit, pidfd_open, pidfd_send_signal, prlimit,
};

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

/// Lowers a freshly spawned child's soft core size limit to exactly 1 byte, so
/// the Wine processes it starts inherit it. Canceling tears Wine down with
/// SIGQUIT, and one caught before it installs its handler would dump core and
/// raise a desktop notification.
///
/// It is 1 and not 0 on purpose: when `core_pattern` pipes to a handler such as
/// systemd-coredump, the kernel ignores a 0 limit and still runs the handler,
/// but a limit of exactly 1 is a special case that skips it entirely. The hard
/// limit is left alone. Only for children wineo can cancel; programs the user
/// plays keep the normal limit so real crashes stay debuggable.
///
/// This runs after `spawn` since the crate forbids unsafe code, so a process
/// that crashes in its first instants can still dump. A failure is ignored: the
/// operation matters more than a quiet crash.
pub fn quiet_core_dumps(child: &Child) {
    let _ = try_quiet_core_dumps(child.id());
}

fn try_quiet_core_dumps(pid: u32) -> io::Result<()> {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(Errno::INVAL)?;
    // The child inherited our hard limit and prlimit sets both, so pass ours
    // back to leave it as it was. A hard limit of 0 can't hold a soft limit of 1.
    let hard = getrlimit(Resource::Core).maximum;
    if hard == Some(0) {
        return Ok(());
    }
    prlimit(
        Some(pid),
        Resource::Core,
        Rlimit {
            current: Some(1),
            maximum: hard,
        },
    )?;
    Ok(())
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

/// Whether this process's own group still has a live member descended from
/// it, for a program that shares wineo's group, as an attached exec does.
/// Others in the group, like the rest of a shell pipeline, don't count.
/// Orphaned descendants are only seen once `become_subreaper` has made
/// them wineo's. `None` means the processes couldn't be inspected.
pub fn own_group_has_live_descendants() -> Option<bool> {
    let me = std::process::id();
    let group = u32::try_from(rustix::process::getpgrp().as_raw_nonzero().get()).ok()?;
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
        if pid == me {
            continue;
        }
        match read_family(pid) {
            Family::Found {
                state,
                group: member,
                ..
            } if member == group && !matches!(state, 'Z' | 'X' | 'x') => {
                if descends_from(pid, me)? {
                    return Some(true);
                }
            }
            Family::Found { .. } | Family::Gone => {}
            Family::Unknown => return None,
        }
    }
    Some(false)
}

/// Makes orphaned descendants wineo's children instead of init's, so
/// `own_group_has_live_descendants` still sees them.
pub fn become_subreaper() -> io::Result<()> {
    rustix::process::set_child_subreaper(Some(rustix::process::getpid())).map_err(io::Error::from)
}

fn descends_from(pid: u32, ancestor: u32) -> Option<bool> {
    let mut current = pid;
    // A parent chain can't be longer than the pid space, but stop early on
    // anything odd rather than loop.
    for _ in 0..4096 {
        match read_family(current) {
            Family::Found { parent, .. } if parent == ancestor => return Some(true),
            Family::Found { parent, .. } if parent > 1 => current = parent,
            Family::Found { .. } | Family::Gone => return Some(false),
            Family::Unknown => return None,
        }
    }
    None
}

enum Family {
    Found {
        state: char,
        parent: u32,
        group: u32,
    },
    Gone,
    Unknown,
}

fn read_family(pid: u32) -> Family {
    let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(err)
            if err.kind() == io::ErrorKind::NotFound
                || err.raw_os_error() == Some(Errno::SRCH.raw_os_error()) =>
        {
            return Family::Gone;
        }
        Err(_) => return Family::Unknown,
    };
    let Some((_, rest)) = text.rsplit_once(')') else {
        return Family::Unknown;
    };
    let fields: Vec<_> = rest.split_whitespace().collect();
    let state = fields.first().and_then(|field| field.chars().next());
    let parent = fields.get(1).and_then(|field| field.parse::<u32>().ok());
    let group = fields.get(2).and_then(|field| field.parse::<u32>().ok());
    match (state, parent, group) {
        (Some(state), Some(parent), Some(group)) => Family::Found {
            state,
            parent,
            group,
        },
        // A dead task can linger with fields that no longer parse.
        (Some('X' | 'x'), _, _) => Family::Gone,
        _ => Family::Unknown,
    }
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
    parse_stat_read(fs::read_to_string(format!("/proc/{pid}/stat")))
}

fn parse_stat_read(read: io::Result<String>) -> ProcRead {
    let text = match read {
        Ok(text) => text,
        Err(err)
            if err.kind() == io::ErrorKind::NotFound
                || err.raw_os_error() == Some(Errno::SRCH.raw_os_error()) =>
        {
            return ProcRead::Gone;
        }
        Err(_) => return ProcRead::Unknown,
    };
    let Some(fields) = text
        .rsplit_once(')')
        .map(|(_, fields)| fields.split_whitespace())
    else {
        return ProcRead::Unknown;
    };
    let fields: Vec<_> = fields.collect();
    let state = fields.first().and_then(|field| field.chars().next());
    let start_time = fields.get(19).and_then(|field| field.parse::<u64>().ok());
    // A dead task can remain in /proc briefly with no process group.
    if matches!(state, Some('X' | 'x'))
        && fields
            .get(2)
            .and_then(|field| field.parse::<i32>().ok())
            .is_some()
        && start_time.is_some()
    {
        return ProcRead::Gone;
    }
    let parsed = state
        .zip(fields.get(2).and_then(|field| field.parse::<u32>().ok()))
        .zip(start_time);
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
