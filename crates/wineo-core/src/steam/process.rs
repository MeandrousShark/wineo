//! The Steam reaper is the identity of a running game. Its process group also
//! contains the Steam client, so every signal here goes through a verified
//! pidfd for a member of the reaper's process tree.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::OwnedFd;
use rustix::param::clock_ticks_per_second;
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use super::Install;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Reaper {
    pub pid: u32,
    pub start_time: u64,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Baseline {
    pub boot_id: String,
    pub start_ticks: u64,
    pub existing: Vec<Reaper>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcStat {
    pid: u32,
    ppid: u32,
    start_time: u64,
    state: char,
}

/// Find the reapers for one app in the active install. Steam's real reaper
/// uses exactly `reaper SteamLaunch AppId=<id> -- ...` and is a direct child
/// of the client. A path through `~/.steam/root` and its canonical target
/// identify the same install.
pub fn matching_reapers(install: &Install, app_id: &str) -> io::Result<Vec<Reaper>> {
    let boot_id = current_boot_id()?;
    let expected = install.root().join("ubuntu12_32/reaper");
    let canonical = fs::canonicalize(&expected).unwrap_or_else(|_| expected.clone());
    let app_arg = format!("AppId={app_id}");
    let mut found = Vec::new();

    for pid in proc_pids()? {
        let Some(arguments) = read_arguments(pid) else {
            continue;
        };
        if !matches_arguments(&arguments, &app_arg, &expected, &canonical) {
            continue;
        }
        let Some(stat) = read_stat(pid) else {
            continue;
        };
        if !live(stat.state) || stat.ppid <= 1 {
            continue;
        }
        let Some(parent) = read_stat(stat.ppid) else {
            continue;
        };
        if !live(parent.state) {
            continue;
        }
        // A reparented orphan is no longer anchored to Steam. Reading again
        // also catches a parent change while cmdline and stat were inspected.
        if read_stat(pid) != Some(stat) {
            continue;
        }
        found.push(Reaper {
            pid,
            start_time: stat.start_time,
            boot_id: boot_id.clone(),
        });
    }
    found.sort_by_key(|reaper| reaper.pid);
    Ok(found)
}

/// Capture the time before the first running check, plus every reaper seen at
/// that check. The latter protects against accepting an old reaper that was
/// missed by a clock tick's limited precision.
pub fn baseline(install: &Install, app_id: &str) -> io::Result<(Baseline, Vec<Reaper>)> {
    let boot_id = current_boot_id()?;
    let start_ticks = uptime_ticks()?;
    let existing = matching_reapers(install, app_id)?;
    let baseline = Baseline {
        boot_id,
        start_ticks,
        existing: existing.clone(),
    };
    Ok((baseline, existing))
}

/// Candidates that appeared after the first running check. A process can
/// start in the same clock tick as that check, so equality is eligible.
pub fn eligible_reapers(
    install: &Install,
    app_id: &str,
    baseline: &Baseline,
) -> io::Result<Vec<Reaper>> {
    if current_boot_id()? != baseline.boot_id {
        return Ok(Vec::new());
    }
    Ok(matching_reapers(install, app_id)?
        .into_iter()
        .filter(|reaper| {
            reaper.start_time >= baseline.start_ticks && !baseline.existing.contains(reaper)
        })
        .collect())
}

/// Open a pidfd only while the recorded pid still has its recorded identity.
pub fn open_reaper(reaper: &Reaper) -> io::Result<Option<OwnedFd>> {
    if current_boot_id()? != reaper.boot_id {
        return Ok(None);
    }
    let Some(stat) = read_stat(reaper.pid) else {
        return Ok(None);
    };
    if stat.start_time != reaper.start_time || !live(stat.state) {
        return Ok(None);
    }
    let fd = match open_pidfd(reaper.pid) {
        Ok(fd) => fd,
        Err(error) if error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if !read_stat(reaper.pid)
        .is_some_and(|current| current.start_time == reaper.start_time && live(current.state))
        || pidfd_exited(&fd)?
    {
        return Ok(None);
    }
    Ok(Some(fd))
}

pub fn reaper_alive(reaper: &Reaper) -> bool {
    open_reaper(reaper).is_ok_and(|fd| fd.is_some())
}

/// A pidfd can watch a process wineo did not spawn; waitid cannot.
pub fn pidfd_exited(pidfd: &OwnedFd) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(pidfd, PollFlags::IN)];
    poll(&mut descriptors, Some(&Timespec::default()))?;
    Ok(!descriptors[0].revents().is_empty())
}

/// Stop only processes descended from the verified reaper. New members get
/// SIGTERM during the grace period, and every member ever seen keeps its
/// pidfd so reparented survivors still get SIGKILL. `escalate` lets a second
/// Ctrl+C skip the rest of the grace period.
pub fn stop_tree(
    reaper: &Reaper,
    timeout: Duration,
    force: bool,
    mut escalate: impl FnMut() -> bool,
) -> io::Result<()> {
    let Some(root_fd) = open_reaper(reaper)? else {
        return Ok(());
    };
    let mut seen = BTreeMap::new();
    seen.insert(
        (reaper.pid, reaper.start_time),
        Member {
            fd: root_fd,
            term_sent: false,
        },
    );
    capture_tree(reaper, &mut seen)?;
    if force {
        signal_members(&mut seen, Signal::KILL, false)?;
        return wait_for_kill(reaper, &mut seen);
    }

    let deadline = Instant::now() + timeout;
    loop {
        capture_tree(reaper, &mut seen)?;
        signal_members(&mut seen, Signal::TERM, true)?;
        if all_exited(&seen)? {
            return Ok(());
        }
        if escalate() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    capture_tree(reaper, &mut seen)?;
    signal_members(&mut seen, Signal::KILL, false)?;
    wait_for_kill(reaper, &mut seen)
}

fn wait_for_kill(reaper: &Reaper, seen: &mut BTreeMap<(u32, u64), Member>) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        capture_tree(reaper, seen)?;
        if all_exited(seen)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Steam game's process tree did not stop",
            ));
        }
        signal_members(seen, Signal::KILL, false)?;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn all_exited(seen: &BTreeMap<(u32, u64), Member>) -> io::Result<bool> {
    for member in seen.values() {
        if !pidfd_exited(&member.fd)? {
            return Ok(false);
        }
    }
    Ok(true)
}

struct Member {
    fd: OwnedFd,
    term_sent: bool,
}

fn capture_tree(reaper: &Reaper, seen: &mut BTreeMap<(u32, u64), Member>) -> io::Result<()> {
    if current_boot_id()? != reaper.boot_id {
        return Ok(());
    }
    let stats = proc_pids()?
        .into_iter()
        .filter_map(read_stat)
        .filter(|stat| live(stat.state))
        .collect::<Vec<_>>();
    let seeds = seen
        .iter()
        .filter(|(_, member)| !pidfd_exited(&member.fd).unwrap_or(true))
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    for stat in tree_members_from_seeds(&stats, &seeds) {
        let key = (stat.pid, stat.start_time);
        if seen.contains_key(&key) {
            continue;
        }
        let Some(current) = read_stat(stat.pid) else {
            continue;
        };
        if current != stat {
            continue;
        }
        let fd = match open_pidfd(stat.pid) {
            Ok(fd) => fd,
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) => {
                continue;
            }
            Err(error) => return Err(error),
        };
        if read_stat(stat.pid)
            .is_some_and(|current| current.start_time == stat.start_time && live(current.state))
            && !pidfd_exited(&fd)?
        {
            seen.insert(
                key,
                Member {
                    fd,
                    term_sent: false,
                },
            );
        }
    }
    Ok(())
}

fn signal_members(
    seen: &mut BTreeMap<(u32, u64), Member>,
    signal: Signal,
    only_new: bool,
) -> io::Result<()> {
    for member in seen.values_mut() {
        if only_new && member.term_sent {
            continue;
        }
        if pidfd_exited(&member.fd)? {
            continue;
        }
        match pidfd_send_signal(&member.fd, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => return Err(error.into()),
        }
        member.term_sent = true;
    }
    Ok(())
}

#[cfg(test)]
fn tree_members(stats: &[ProcStat], root: u32, start_time: u64) -> Vec<ProcStat> {
    tree_members_from_seeds(stats, &[(root, start_time)])
}

fn tree_members_from_seeds(stats: &[ProcStat], seeds: &[(u32, u64)]) -> Vec<ProcStat> {
    let mut result = Vec::new();
    let mut queue = VecDeque::new();
    let mut visited = BTreeSet::new();
    for &(pid, start_time) in seeds {
        if let Some(stat) = stats
            .iter()
            .find(|stat| stat.pid == pid && stat.start_time == start_time)
            && visited.insert(pid)
        {
            result.push(*stat);
            queue.push_back(pid);
        }
    }
    while let Some(parent) = queue.pop_front() {
        for stat in stats.iter().filter(|stat| stat.ppid == parent) {
            if visited.insert(stat.pid) {
                result.push(*stat);
                queue.push_back(stat.pid);
            }
        }
    }
    result
}

fn matches_arguments(args: &[Vec<u8>], app_arg: &str, expected: &Path, canonical: &Path) -> bool {
    args.len() >= 3 && args[1] == b"SteamLaunch" && args[2] == app_arg.as_bytes() && {
        let path = PathBuf::from(OsStr::from_bytes(&args[0]));
        path == expected || fs::canonicalize(path).is_ok_and(|path| path == canonical)
    }
}

fn read_arguments(pid: u32) -> Option<Vec<Vec<u8>>> {
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let mut args = bytes
        .split(|byte| *byte == 0)
        .map(Vec::from)
        .collect::<Vec<_>>();
    if args.last().is_some_and(Vec::is_empty) {
        args.pop();
    }
    Some(args)
}

fn read_stat(pid: u32) -> Option<ProcStat> {
    parse_stat(pid, &fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

fn parse_stat(pid: u32, text: &str) -> Option<ProcStat> {
    let (_, fields) = text.rsplit_once(") ")?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    Some(ProcStat {
        pid,
        state: fields.first()?.chars().next()?,
        ppid: fields.get(1)?.parse().ok()?,
        start_time: fields.get(19)?.parse().ok()?,
    })
}

fn live(state: char) -> bool {
    !matches!(state, 'Z' | 'X' | 'x')
}

fn proc_pids() -> io::Result<Vec<u32>> {
    Ok(fs::read_dir("/proc")?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect())
}

fn current_boot_id() -> io::Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}

fn uptime_ticks() -> io::Result<u64> {
    let text = fs::read_to_string("/proc/uptime")?;
    let first = text
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty /proc/uptime"))?;
    let (whole, fraction) = first.split_once('.').unwrap_or((first, ""));
    let seconds = whole
        .parse::<u64>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let ticks = clock_ticks_per_second();
    let fractional_ticks = fraction
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold((0u128, 1u128), |(value, scale), digit| {
            (value * 10 + u128::from(digit - b'0'), scale * 10)
        });
    let result = u128::from(seconds) * u128::from(ticks)
        + fractional_ticks.0 * u128::from(ticks) / fractional_ticks.1;
    u64::try_from(result).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn open_pidfd(pid: u32) -> io::Result<OwnedFd> {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid pid"))?;
    pidfd_open(pid, PidfdFlags::empty()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    #[test]
    fn argument_match_is_positional_and_app_id_is_whole() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ubuntu12_32/reaper");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        let args = |app: &str| {
            vec![
                path.as_os_str().as_bytes().to_vec(),
                b"SteamLaunch".to_vec(),
                format!("AppId={app}").into_bytes(),
                b"--".to_vec(),
            ]
        };
        assert!(matches_arguments(
            &args("289070"),
            "AppId=289070",
            &path,
            &path
        ));
        assert!(!matches_arguments(
            &args("289070"),
            "AppId=2890",
            &path,
            &path
        ));
        let mut misplaced = args("289070");
        misplaced.swap(1, 2);
        assert!(!matches_arguments(&misplaced, "AppId=289070", &path, &path));
        let other = root.path().join("other/ubuntu12_32/reaper");
        assert!(!matches_arguments(
            &args("289070"),
            "AppId=289070",
            &other,
            &other
        ));
        let alias = root.path().join("steam-alias");
        symlink(root.path(), &alias).unwrap();
        let mut aliased = args("289070");
        aliased[0] = alias
            .join("ubuntu12_32/reaper")
            .as_os_str()
            .as_bytes()
            .to_vec();
        assert!(matches_arguments(&aliased, "AppId=289070", &path, &path));
    }

    #[test]
    fn tree_walk_follows_parent_chain_without_process_groups() {
        let stat = |pid, ppid, start_time| ProcStat {
            pid,
            ppid,
            start_time,
            state: 'S',
        };
        let stats = [
            stat(10, 1, 9),
            stat(11, 10, 10),
            stat(12, 11, 11),
            stat(13, 1, 12),
            stat(14, 12, 13),
        ];
        assert_eq!(
            tree_members(&stats, 11, 10),
            vec![stats[1], stats[2], stats[4]]
        );
        assert!(tree_members(&stats, 11, 999).is_empty());
    }

    #[test]
    fn uptime_ticks_track_current_process_start() {
        let ticks = uptime_ticks().unwrap();
        let own = read_stat(std::process::id()).unwrap().start_time;
        assert!(ticks >= own);
    }

    #[test]
    fn stop_kills_reparented_child_but_spares_same_group_foreign_process() {
        let temp = tempfile::tempdir().unwrap();
        let child_file = temp.path().join("child.pid");
        let mut root = Command::new("sh")
            .arg("-c")
            .arg("sh -c 'trap \"\" TERM; exec sleep 30' & echo $! > \"$CHILD_PID_FILE\"; wait")
            .env("CHILD_PID_FILE", &child_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut foreign = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(root.id() as i32)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let child_pid = loop {
            if let Ok(text) = fs::read_to_string(&child_file)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "child did not start");
            std::thread::sleep(Duration::from_millis(10));
        };
        let child_fd = open_pidfd(child_pid).unwrap();
        let reaper = Reaper {
            pid: root.id(),
            start_time: read_stat(root.id()).unwrap().start_time,
            boot_id: current_boot_id().unwrap(),
        };
        stop_tree(&reaper, Duration::from_millis(100), false, || false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pidfd_exited(&child_fd).unwrap() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pidfd_exited(&child_fd).unwrap());
        assert!(foreign.try_wait().unwrap().is_none());
        let _ = foreign.kill();
        let _ = foreign.wait();
        let _ = root.wait();
    }
}
