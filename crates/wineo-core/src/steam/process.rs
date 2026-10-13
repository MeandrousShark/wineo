//! The Steam reaper is the identity of a running game. Its process group also
//! contains the Steam client, so every signal here goes through a verified
//! pidfd for a member of the reaper's process tree.

use std::collections::{BTreeMap, HashMap, VecDeque};
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

use super::{Install, InstallKind};

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
    Ok(reapers_by_app(install, &[app_id.to_owned()])?
        .remove(app_id)
        .unwrap_or_default())
}

/// `matching_reapers` for several apps from one sweep of /proc, which costs
/// more than any single app's check. Every requested app has an entry, empty
/// when nothing matched.
pub fn reapers_by_app(
    install: &Install,
    app_ids: &[String],
) -> io::Result<HashMap<String, Vec<Reaper>>> {
    if app_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let boot_id = current_boot_id()?;
    scan_reapers(&Proc, proc_pids()?, install, app_ids, &boot_id)
}

fn scan_reapers<R: ProcessReader>(
    reader: &R,
    pids: Vec<u32>,
    install: &Install,
    app_ids: &[String],
    boot_id: &str,
) -> io::Result<HashMap<String, Vec<Reaper>>> {
    let expected = install.root().join("ubuntu12_32/reaper");
    let canonical = fs::canonicalize(install.root())
        .unwrap_or_else(|_| install.root().to_owned())
        .join("ubuntu12_32/reaper");
    let app_args: Vec<String> = app_ids.iter().map(|id| format!("AppId={id}")).collect();
    let mut found: HashMap<String, Vec<Reaper>> = app_ids
        .iter()
        .map(|app_id| (app_id.clone(), Vec::new()))
        .collect();
    if app_ids.is_empty() {
        return Ok(found);
    }

    for pid in pids {
        let Some(process) =
            reaper_candidate(reader, pid, app_args.as_slice(), &expected, &canonical)?
        else {
            continue;
        };
        let Some(parent) = client_candidate(reader, process.stat.ppid)? else {
            continue;
        };
        if !matches_client(install, &parent)
            || !current_child(reader, &process, parent.stat, &parent.fd)?
        {
            continue;
        }
        // The candidate check proved argument 2 is one of the requested
        // arguments, so the prefix is there and the id is valid text.
        let Some(app_id) = process
            .arguments
            .get(2)
            .and_then(|argument| std::str::from_utf8(argument).ok())
            .and_then(|argument| argument.strip_prefix("AppId="))
        else {
            continue;
        };
        if let Some(reapers) = found.get_mut(app_id) {
            reapers.push(Reaper {
                pid,
                start_time: process.stat.start_time,
                boot_id: boot_id.to_owned(),
            });
        }
    }
    for reapers in found.values_mut() {
        reapers.sort_by_key(|reaper| reaper.pid);
    }
    Ok(found)
}

/// The `AppId=<id>` arguments a scan accepts.
trait WantedApps {
    fn wants(&self, argument: &[u8]) -> bool;
}

impl WantedApps for str {
    fn wants(&self, argument: &[u8]) -> bool {
        self.as_bytes() == argument
    }
}

impl WantedApps for [String] {
    fn wants(&self, argument: &[u8]) -> bool {
        self.iter().any(|wanted| wanted.as_bytes() == argument)
    }
}

fn reaper_candidate<R: ProcessReader, W: WantedApps + ?Sized>(
    reader: &R,
    pid: u32,
    wanted: &W,
    expected: &Path,
    canonical: &Path,
) -> io::Result<Option<Inspected<R::Fd>>> {
    let Some(mut process) = inspect_stat(reader, pid)? else {
        return Ok(None);
    };
    let arguments = match reader.arguments(pid) {
        Ok(arguments) => arguments,
        Err(error) if permission_denied(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(arguments) = arguments else {
        return Ok(None);
    };
    if process.stat.ppid <= 1
        || arguments.len() < 3
        || arguments[1] != b"SteamLaunch"
        || !wanted.wants(&arguments[2])
    {
        return Ok(None);
    }
    // Equal to a wanted argument, which is valid text.
    let app_arg = String::from_utf8_lossy(&arguments[2]).into_owned();
    process.arguments = arguments;
    if !matches_arguments(&process.arguments, &app_arg, expected, canonical) {
        process.exe = match reader.exe(pid) {
            Ok(exe) => exe,
            Err(error) if permission_denied(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
    }
    if !matches_reaper(&process, &app_arg, expected, canonical) || !still_current(reader, &process)?
    {
        return Ok(None);
    }
    Ok(Some(process))
}

fn client_candidate<R: ProcessReader>(
    reader: &R,
    pid: u32,
) -> io::Result<Option<Inspected<R::Fd>>> {
    let Some(mut process) = inspect_stat(reader, pid)? else {
        return Ok(None);
    };
    process.exe = match reader.exe(pid) {
        Ok(exe) => exe,
        Err(error) if permission_denied(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    process.arguments = match reader.arguments(pid) {
        Ok(arguments) => arguments.unwrap_or_default(),
        Err(error) if permission_denied(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if !still_current(reader, &process)? {
        return Ok(None);
    }
    Ok(Some(process))
}

fn permission_denied(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code)
            if code == rustix::io::Errno::ACCESS.raw_os_error()
                || code == rustix::io::Errno::PERM.raw_os_error()
    )
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
    let Some(process) = inspect_stat(&Proc, reaper.pid)? else {
        return Ok(None);
    };
    if process.stat.start_time != reaper.start_time {
        return Ok(None);
    }
    Ok(Some(process.fd))
}

pub fn reaper_alive(reaper: &Reaper) -> bool {
    open_reaper(reaper).is_ok_and(|fd| fd.is_some())
}

/// A pidfd can watch a process wineo did not spawn; waitid cannot.
pub fn pidfd_exited(pidfd: &OwnedFd) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(pidfd, PollFlags::IN)];
    // poll is never restarted after a signal handler runs, even with a zero
    // timeout, so a Ctrl+C mid-scan would otherwise surface as an error.
    loop {
        match poll(&mut descriptors, Some(&Timespec::default())) {
            Ok(_) => return Ok(!descriptors[0].revents().is_empty()),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Blocks until the process behind the pidfd exits.
pub fn wait_pidfd_exit(pidfd: &OwnedFd) -> io::Result<()> {
    let mut descriptors = [PollFd::new(pidfd, PollFlags::IN)];
    loop {
        match poll(&mut descriptors, None) {
            Ok(_) if !descriptors[0].revents().is_empty() => return Ok(()),
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
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

struct Member<F = OwnedFd> {
    fd: F,
    term_sent: bool,
}

fn capture_tree(reaper: &Reaper, seen: &mut BTreeMap<(u32, u64), Member>) -> io::Result<()> {
    if current_boot_id()? != reaper.boot_id {
        return Ok(());
    }
    let mut processes = Vec::new();
    for pid in proc_pids()? {
        if let Some(stat) = Proc.stat(pid)? {
            processes.push(stat);
        }
    }
    capture_members(&Proc, processes, seen)
}

fn capture_members<R: ProcessReader>(
    reader: &R,
    processes: Vec<ProcStat>,
    seen: &mut BTreeMap<(u32, u64), Member<R::Fd>>,
) -> io::Result<()> {
    let mut queue = VecDeque::new();
    for (&key, member) in seen.iter() {
        if !reader.exited(&member.fd)? {
            queue.push_back(key);
        }
    }
    let mut candidates = BTreeMap::<u32, Vec<ProcStat>>::new();
    for stat in processes {
        if live(stat.state) {
            candidates.entry(stat.ppid).or_default().push(stat);
        }
    }
    while let Some((pid, start_time)) = queue.pop_front() {
        for candidate in candidates.remove(&pid).unwrap_or_default() {
            let key = (candidate.pid, candidate.start_time);
            if seen.contains_key(&key) || candidate.start_time < start_time {
                continue;
            }
            let Some(process) = inspect_stat(reader, candidate.pid)? else {
                continue;
            };
            if process.stat.start_time != candidate.start_time
                || process.stat.ppid != candidate.ppid
            {
                continue;
            }
            let parent = ProcStat {
                pid,
                ppid: 0,
                start_time,
                state: 'S',
            };
            // Exited members retain their pidfds for signalling survivors, but
            // their numeric pids can never anchor another ancestry edge.
            if !current_child(reader, &process, parent, &seen[&(pid, start_time)].fd)? {
                continue;
            }
            seen.insert(
                key,
                Member {
                    fd: process.fd,
                    term_sent: false,
                },
            );
            queue.push_back(key);
        }
    }
    Ok(())
}

fn current_child<R: ProcessReader>(
    reader: &R,
    child: &Inspected<R::Fd>,
    parent: ProcStat,
    parent_fd: &R::Fd,
) -> io::Result<bool> {
    Ok(child.stat.ppid == parent.pid
        && child.stat.start_time >= parent.start_time
        && !reader.exited(&child.fd)?
        && reader.stat(child.stat.pid)?.is_some_and(|stat| {
            stat.start_time == child.stat.start_time && stat.ppid == parent.pid && live(stat.state)
        })
        && !reader.exited(parent_fd)?)
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

// All metadata reads belong to the process held by this pidfd. Checking
// again after the reads rejects exits and PID reuse during a /proc scan.
trait ProcessReader {
    type Fd;

    fn open(&self, pid: u32) -> io::Result<Self::Fd>;
    fn stat(&self, pid: u32) -> io::Result<Option<ProcStat>>;
    fn arguments(&self, pid: u32) -> io::Result<Option<Vec<Vec<u8>>>>;
    fn exe(&self, pid: u32) -> io::Result<Option<PathBuf>>;
    fn exited(&self, fd: &Self::Fd) -> io::Result<bool>;
}

struct Proc;

impl ProcessReader for Proc {
    type Fd = OwnedFd;

    fn open(&self, pid: u32) -> io::Result<OwnedFd> {
        open_pidfd(pid)
    }

    fn stat(&self, pid: u32) -> io::Result<Option<ProcStat>> {
        read_stat(pid)
    }

    fn arguments(&self, pid: u32) -> io::Result<Option<Vec<Vec<u8>>>> {
        read_arguments(pid)
    }

    fn exe(&self, pid: u32) -> io::Result<Option<PathBuf>> {
        match fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(path) => Ok(Some(path)),
            Err(error) if process_gone(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn exited(&self, fd: &OwnedFd) -> io::Result<bool> {
        pidfd_exited(fd)
    }
}

struct Inspected<F> {
    fd: F,
    stat: ProcStat,
    arguments: Vec<Vec<u8>>,
    exe: Option<PathBuf>,
}

#[cfg(test)]
fn inspect<R: ProcessReader>(reader: &R, pid: u32) -> io::Result<Option<Inspected<R::Fd>>> {
    inspect_with_metadata(reader, pid, true)
}

fn inspect_stat<R: ProcessReader>(reader: &R, pid: u32) -> io::Result<Option<Inspected<R::Fd>>> {
    inspect_with_metadata(reader, pid, false)
}

fn inspect_with_metadata<R: ProcessReader>(
    reader: &R,
    pid: u32,
    metadata: bool,
) -> io::Result<Option<Inspected<R::Fd>>> {
    let fd = match reader.open(pid) {
        Ok(fd) => fd,
        Err(error) if process_gone(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(stat) = reader.stat(pid)?.filter(|stat| live(stat.state)) else {
        return Ok(None);
    };
    let arguments = if metadata {
        reader.arguments(pid)?.unwrap_or_default()
    } else {
        Vec::new()
    };
    let exe = if metadata { reader.exe(pid)? } else { None };
    let process = Inspected {
        fd,
        stat,
        arguments,
        exe,
    };
    if !still_current(reader, &process)? {
        return Ok(None);
    }
    Ok(Some(process))
}

fn still_current<R: ProcessReader>(reader: &R, process: &Inspected<R::Fd>) -> io::Result<bool> {
    Ok(!reader.exited(&process.fd)?
        && reader.stat(process.stat.pid)?.is_some_and(|current| {
            current.start_time == process.stat.start_time
                && current.ppid == process.stat.ppid
                && live(current.state)
        }))
}

fn matches_client<F>(install: &Install, process: &Inspected<F>) -> bool {
    let expected = install.root().join("ubuntu12_32/steam");
    let canonical = fs::canonicalize(install.root())
        .unwrap_or_else(|_| install.root().to_owned())
        .join("ubuntu12_32/steam");
    let argv_matches = || {
        process.arguments.first().is_some_and(|argument| {
            matches_path(
                Path::new(OsStr::from_bytes(argument)),
                &expected,
                &canonical,
            )
        })
    };
    match &process.exe {
        Some(exe) => {
            matches_path(exe, &expected, &canonical)
                || (install.kind() == InstallKind::Flatpak && argv_matches())
        }
        None => argv_matches(),
    }
}

fn matches_path(path: &Path, expected: &Path, canonical: &Path) -> bool {
    path == expected
        || path == canonical
        || fs::canonicalize(path).is_ok_and(|path| path == canonical)
}

fn matches_arguments(args: &[Vec<u8>], app_arg: &str, expected: &Path, canonical: &Path) -> bool {
    args.len() >= 3 && args[1] == b"SteamLaunch" && args[2] == app_arg.as_bytes() && {
        let path = PathBuf::from(OsStr::from_bytes(&args[0]));
        matches_path(&path, expected, canonical)
    }
}

fn matches_reaper<F>(
    process: &Inspected<F>,
    app_arg: &str,
    expected: &Path,
    canonical: &Path,
) -> bool {
    matches_arguments(&process.arguments, app_arg, expected, canonical)
        || (process.arguments.len() >= 3
            && process.arguments[1] == b"SteamLaunch"
            && process.arguments[2] == app_arg.as_bytes()
            && process
                .exe
                .as_deref()
                .is_some_and(|path| matches_path(path, expected, canonical)))
}

fn read_arguments(pid: u32) -> io::Result<Option<Vec<Vec<u8>>>> {
    let bytes = match fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(bytes) => bytes,
        Err(error) if process_gone(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut args = bytes
        .split(|byte| *byte == 0)
        .map(Vec::from)
        .collect::<Vec<_>>();
    if args.last().is_some_and(Vec::is_empty) {
        args.pop();
    }
    Ok(Some(args))
}

fn read_stat(pid: u32) -> io::Result<Option<ProcStat>> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => parse_stat(pid, &text)
            .map(Some)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid process stat")),
        Err(error) if process_gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn process_gone(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == rustix::io::Errno::SRCH.raw_os_error()
            || code == rustix::io::Errno::NOENT.raw_os_error()
    )
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
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if process_gone(&error) => continue,
            Err(error) => return Err(error),
        };
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        {
            pids.push(pid);
        }
    }
    Ok(pids)
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
    use std::cell::RefCell;
    use std::collections::BTreeSet;
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
    fn reaper_match_accepts_verified_exe_when_argv_path_differs() {
        let root = tempfile::tempdir().unwrap();
        let expected = root.path().join("ubuntu12_32/reaper");
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::write(&expected, "").unwrap();
        let mut process = Inspected {
            fd: (),
            stat: stat(10, 1, 20),
            arguments: vec![
                b"reaper".to_vec(),
                b"SteamLaunch".to_vec(),
                b"AppId=289070".to_vec(),
            ],
            exe: Some(expected.clone()),
        };
        assert!(matches_reaper(
            &process,
            "AppId=289070",
            &expected,
            &expected
        ));
        assert!(!matches_reaper(
            &process,
            "AppId=2890",
            &expected,
            &expected
        ));
        process.exe = Some(root.path().join("other/reaper"));
        assert!(!matches_reaper(
            &process,
            "AppId=289070",
            &expected,
            &expected
        ));
    }

    struct FakeProc {
        stats: RefCell<BTreeMap<u32, VecDeque<ProcStat>>>,
        exited: BTreeSet<u32>,
        calls: RefCell<Vec<&'static str>>,
        open_error: Option<u32>,
        stat_error: Option<u32>,
        arguments_error: Option<(u32, i32)>,
        exe_error: Option<(u32, i32)>,
        missing_arguments: BTreeSet<u32>,
        missing_exe: BTreeSet<u32>,
        arguments_by_pid: BTreeMap<u32, Vec<Vec<u8>>>,
        exe_by_pid: BTreeMap<u32, PathBuf>,
    }

    impl FakeProc {
        fn new(stats: &[ProcStat]) -> Self {
            Self {
                stats: RefCell::new(
                    stats
                        .iter()
                        .map(|stat| (stat.pid, VecDeque::from([*stat])))
                        .collect(),
                ),
                exited: BTreeSet::new(),
                calls: RefCell::new(Vec::new()),
                open_error: None,
                stat_error: None,
                arguments_error: None,
                exe_error: None,
                missing_arguments: BTreeSet::new(),
                missing_exe: BTreeSet::new(),
                arguments_by_pid: BTreeMap::new(),
                exe_by_pid: BTreeMap::new(),
            }
        }
    }

    impl ProcessReader for FakeProc {
        type Fd = u32;

        fn open(&self, pid: u32) -> io::Result<u32> {
            self.calls.borrow_mut().push("open");
            if self.open_error == Some(pid) {
                return Err(io::Error::from_raw_os_error(
                    rustix::io::Errno::MFILE.raw_os_error(),
                ));
            }
            Ok(pid)
        }

        fn stat(&self, pid: u32) -> io::Result<Option<ProcStat>> {
            self.calls.borrow_mut().push("stat");
            if self.stat_error == Some(pid) {
                return Err(io::Error::from_raw_os_error(
                    rustix::io::Errno::MFILE.raw_os_error(),
                ));
            }
            let mut stats = self.stats.borrow_mut();
            let Some(reads) = stats.get_mut(&pid) else {
                return Ok(None);
            };
            Ok(if reads.len() > 1 {
                reads.pop_front()
            } else {
                reads.front().copied()
            })
        }

        fn arguments(&self, pid: u32) -> io::Result<Option<Vec<Vec<u8>>>> {
            self.calls.borrow_mut().push("cmdline");
            if let Some((_, error)) = self
                .arguments_error
                .filter(|(error_pid, _)| *error_pid == pid)
            {
                return Err(io::Error::from_raw_os_error(error));
            }
            if self.missing_arguments.contains(&pid) {
                return Ok(None);
            }
            if let Some(arguments) = self.arguments_by_pid.get(&pid) {
                return Ok(Some(arguments.clone()));
            }
            Ok(Some(vec![
                b"reaper".to_vec(),
                b"SteamLaunch".to_vec(),
                b"AppId=289070".to_vec(),
            ]))
        }

        fn exe(&self, pid: u32) -> io::Result<Option<PathBuf>> {
            self.calls.borrow_mut().push("exe");
            if let Some((_, error)) = self.exe_error.filter(|(error_pid, _)| *error_pid == pid) {
                return Err(io::Error::from_raw_os_error(error));
            }
            if self.missing_exe.contains(&pid) {
                return Ok(None);
            }
            if let Some(exe) = self.exe_by_pid.get(&pid) {
                return Ok(Some(exe.clone()));
            }
            Ok(Some(PathBuf::from("reaper")))
        }

        fn exited(&self, pid: &u32) -> io::Result<bool> {
            self.calls.borrow_mut().push("exited");
            Ok(self.exited.contains(pid))
        }
    }

    fn stat(pid: u32, ppid: u32, start_time: u64) -> ProcStat {
        ProcStat {
            pid,
            ppid,
            start_time,
            state: 'S',
        }
    }

    fn tree_members(
        reader: &FakeProc,
        stats: &[ProcStat],
        root: u32,
        start_time: u64,
    ) -> Vec<ProcStat> {
        let processes = stats.to_vec();
        let mut seen = BTreeMap::new();
        if let Some(process) = inspect(reader, root).unwrap()
            && process.stat.start_time == start_time
        {
            seen.insert(
                (root, start_time),
                Member {
                    fd: process.fd,
                    term_sent: false,
                },
            );
        }
        capture_members(reader, processes, &mut seen).unwrap();
        stats
            .iter()
            .filter(|stat| seen.contains_key(&(stat.pid, stat.start_time)))
            .copied()
            .collect()
    }

    #[test]
    fn inspection_opens_pidfd_before_metadata_and_confirms_identity_afterwards() {
        let reader = FakeProc::new(&[stat(11, 10, 20)]);
        assert!(inspect(&reader, 11).unwrap().is_some());
        assert_eq!(
            *reader.calls.borrow(),
            ["open", "stat", "cmdline", "exe", "exited", "stat"]
        );
    }

    #[test]
    fn inspection_rejects_pid_reused_between_metadata_reads() {
        let original = stat(11, 10, 20);
        let stranger = stat(11, 10, 30);
        let reader = FakeProc::new(&[original]);
        reader
            .stats
            .borrow_mut()
            .insert(11, VecDeque::from([original, stranger]));
        assert!(inspect(&reader, 11).unwrap().is_none());
    }

    #[test]
    fn inspection_rejects_an_exited_pidfd_even_if_stat_still_matches() {
        let mut reader = FakeProc::new(&[stat(11, 10, 20)]);
        reader.exited.insert(11);
        assert!(inspect(&reader, 11).unwrap().is_none());
    }

    #[test]
    fn tree_walk_follows_parent_chain_without_process_groups() {
        let stats = [
            stat(10, 1, 9),
            stat(11, 10, 10),
            stat(12, 11, 11),
            stat(13, 1, 12),
            stat(14, 12, 13),
        ];
        let reader = FakeProc::new(&stats);
        assert_eq!(
            tree_members(&reader, &stats, 11, 10),
            vec![stats[1], stats[2], stats[4]]
        );
        assert!(tree_members(&reader, &stats, 11, 999).is_empty());
    }

    #[test]
    fn tree_walk_rejects_stale_edge_when_child_predates_parent() {
        let stats = [stat(10, 1, 20), stat(11, 10, 10), stat(12, 11, 30)];
        let reader = FakeProc::new(&stats);
        assert_eq!(tree_members(&reader, &stats, 10, 20), vec![stats[0]]);
    }

    #[test]
    fn tree_walk_never_anchors_children_to_an_exited_retained_pidfd() {
        let stats = [stat(10, 1, 20), stat(11, 10, 30)];
        let mut reader = FakeProc::new(&stats);
        let processes = stats.to_vec();
        let mut seen = BTreeMap::from([(
            (10, 20),
            Member {
                fd: 10,
                term_sent: true,
            },
        )]);
        // Numeric PID 10 may now belong to a stranger. The retained pidfd
        // remains exited, even if a later /proc snapshot has the same ppid.
        reader.exited.insert(10);
        capture_members(&reader, processes, &mut seen).unwrap();
        assert_eq!(seen.keys().copied().collect::<Vec<_>>(), [(10, 20)]);
    }

    #[test]
    fn tree_walk_rechecks_parent_pidfd_when_linking_a_child() {
        let stats = [stat(10, 1, 20), stat(11, 10, 30)];
        let mut reader = FakeProc::new(&stats);
        let child = inspect_stat(&reader, 11).unwrap().unwrap();
        reader.exited.insert(10);
        assert!(!current_child(&reader, &child, stats[0], &10).unwrap());
    }

    #[test]
    fn tree_walk_opens_only_descendants_and_propagates_pidfd_errors() {
        let stats = [stat(10, 1, 20), stat(11, 10, 30), stat(12, 1, 40)];
        let mut reader = FakeProc::new(&stats);
        reader.open_error = Some(12);
        let mut seen = BTreeMap::from([(
            (10, 20),
            Member {
                fd: 10,
                term_sent: false,
            },
        )]);
        capture_members(&reader, stats.to_vec(), &mut seen).unwrap();
        assert!(seen.contains_key(&(11, 30)));
        assert!(!seen.contains_key(&(12, 40)));
        reader.open_error = Some(11);
        let mut seen = BTreeMap::from([(
            (10, 20),
            Member {
                fd: 10,
                term_sent: false,
            },
        )]);
        let error = capture_members(&reader, stats.to_vec(), &mut seen).unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::MFILE.raw_os_error())
        );
    }

    #[test]
    fn stat_errors_are_not_treated_as_process_exits() {
        let mut reader = FakeProc::new(&[stat(10, 1, 20)]);
        reader.stat_error = Some(10);
        let error = inspect_stat(&reader, 10).err().unwrap();
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::MFILE.raw_os_error())
        );
    }

    #[test]
    fn reaper_candidate_reads_exe_only_after_positional_match() {
        let path = Path::new("/steam/ubuntu12_32/reaper");
        let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
        reader.exe_error = Some((10, rustix::io::Errno::MFILE.raw_os_error()));
        assert!(
            reaper_candidate(&reader, 10, "AppId=2890", path, path)
                .unwrap()
                .is_none()
        );
        assert!(!reader.calls.borrow().contains(&"exe"));
        let error = reaper_candidate(&reader, 10, "AppId=289070", path, path)
            .err()
            .unwrap();
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::MFILE.raw_os_error())
        );
    }

    #[test]
    fn candidate_metadata_errors_propagate() {
        let path = Path::new("/steam/ubuntu12_32/reaper");
        let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
        reader.arguments_error = Some((10, rustix::io::Errno::MFILE.raw_os_error()));
        let error = reaper_candidate(&reader, 10, "AppId=289070", path, path)
            .err()
            .unwrap();
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::MFILE.raw_os_error())
        );
        reader.arguments_error = None;
        reader.exe_error = Some((10, rustix::io::Errno::MFILE.raw_os_error()));
        let error = client_candidate(&reader, 10).err().unwrap();
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::MFILE.raw_os_error())
        );
    }

    #[test]
    fn candidate_permission_errors_are_skipped() {
        let path = Path::new("/steam/ubuntu12_32/reaper");
        let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
        reader.arguments_error = Some((10, rustix::io::Errno::ACCESS.raw_os_error()));
        assert!(
            reaper_candidate(&reader, 10, "AppId=289070", path, path)
                .unwrap()
                .is_none()
        );

        reader.arguments_error = None;
        reader.exe_error = Some((10, rustix::io::Errno::PERM.raw_os_error()));
        assert!(
            reaper_candidate(&reader, 10, "AppId=289070", path, path)
                .unwrap()
                .is_none()
        );

        reader.exe_error = Some((10, rustix::io::Errno::ACCESS.raw_os_error()));
        assert!(client_candidate(&reader, 10).unwrap().is_none());

        reader.exe_error = None;
        reader.arguments_error = Some((10, rustix::io::Errno::PERM.raw_os_error()));
        assert!(client_candidate(&reader, 10).unwrap().is_none());
    }

    /// A fake process table: a client (pid 5), reapers for apps 10 (two), 20,
    /// and 30 under it, a reaper for app 30 under a foreign parent, and bystanders.
    fn reaper_table(root: &Path) -> (FakeProc, Vec<u32>) {
        let reaper = root.join("ubuntu12_32/reaper");
        let client = root.join("ubuntu12_32/steam");
        let launch = |app: &str| {
            vec![
                reaper.as_os_str().as_bytes().to_vec(),
                b"SteamLaunch".to_vec(),
                format!("AppId={app}").into_bytes(),
                b"--".to_vec(),
            ]
        };
        let mut reader = FakeProc::new(&[
            stat(5, 2, 100),
            stat(6, 5, 110),
            stat(7, 5, 120),
            stat(8, 5, 130),
            stat(9, 4, 140),
            stat(11, 5, 150),
            stat(12, 1, 160),
            stat(13, 5, 170),
            stat(4, 2, 90),
        ]);
        reader.exe_by_pid.insert(5, client.clone());
        reader.exe_by_pid.insert(4, PathBuf::from("/usr/bin/other"));
        reader
            .arguments_by_pid
            .insert(5, vec![client.as_os_str().as_bytes().to_vec()]);
        reader.arguments_by_pid.insert(4, vec![b"other".to_vec()]);
        for (pid, app) in [(6, "10"), (7, "20"), (8, "30"), (9, "30")] {
            reader.arguments_by_pid.insert(pid, launch(app));
            reader.exe_by_pid.insert(pid, reaper.clone());
        }
        reader
            .arguments_by_pid
            .insert(11, vec![b"bash".to_vec(), b"-c".to_vec()]);
        reader.arguments_by_pid.insert(12, launch("10"));
        // A second game process for app 10, listed ahead of the first so the
        // result has to be sorted.
        reader.arguments_by_pid.insert(13, launch("10"));
        reader.exe_by_pid.insert(13, reaper.clone());
        (reader, vec![13, 4, 5, 6, 7, 8, 9, 11, 12])
    }

    #[test]
    fn one_scan_gives_each_app_what_a_scan_for_it_alone_gives() {
        let root = tempfile::tempdir().unwrap();
        let install = Install {
            kind: InstallKind::Native,
            root: root.path().to_owned(),
        };
        let (reader, pids) = reaper_table(root.path());
        let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        let all = ids(&["10", "20", "30", "40"]);
        let together = scan_reapers(&reader, pids.clone(), &install, &all, "boot").unwrap();

        // The parent of 9 is not the client, and 12 is orphaned to init.
        let pids_of = |app: &str| {
            together[app]
                .iter()
                .map(|reaper| reaper.pid)
                .collect::<Vec<_>>()
        };
        assert_eq!(pids_of("10"), [6, 13]);
        assert_eq!(pids_of("20"), [7]);
        assert_eq!(pids_of("30"), [8]);
        assert_eq!(pids_of("40"), Vec::<u32>::new());
        assert_eq!(together["10"][0].start_time, 110);
        assert_eq!(together["10"][0].boot_id, "boot");

        for app in &all {
            let (reader, pids) = reaper_table(root.path());
            let alone =
                scan_reapers(&reader, pids, &install, std::slice::from_ref(app), "boot").unwrap();
            assert_eq!(alone[app], together[app], "app {app}");
        }
    }

    #[test]
    fn one_scan_opens_each_process_once_however_many_apps_it_serves() {
        let root = tempfile::tempdir().unwrap();
        let install = Install {
            kind: InstallKind::Native,
            root: root.path().to_owned(),
        };
        let opens = |ids: &[&str]| {
            let (reader, pids) = reaper_table(root.path());
            let ids = ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
            scan_reapers(&reader, pids, &install, &ids, "boot").unwrap();
            let calls = reader.calls.borrow();
            calls.iter().filter(|call| **call == "open").count()
        };
        // Nine processes, plus the parent opened again for each reaper of a
        // requested app that has a live parent (6, 7, 8, 9, 13).
        assert_eq!(opens(&["10", "20", "30"]), 9 + 5);
        // Three sweeps for the same apps would repeat the nine each time.
        assert!(opens(&["10"]) + opens(&["20"]) + opens(&["30"]) > opens(&["10", "20", "30"]));
    }

    #[test]
    fn missing_client_exe_keeps_the_argv_fallback() {
        let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
        reader.missing_exe.insert(10);
        let client = client_candidate(&reader, 10).unwrap().unwrap();
        assert!(client.exe.is_none());
        assert!(!client.arguments.is_empty());
    }

    #[test]
    fn client_match_requires_install_exe_with_limited_argv_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let mut install = Install {
            kind: InstallKind::Native,
            root: temp.path().to_owned(),
        };
        let path = install.root().join("ubuntu12_32/steam");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        let mut client = Inspected {
            fd: (),
            stat: stat(10, 1, 10),
            arguments: vec![path.as_os_str().as_bytes().to_vec()],
            exe: Some(path.clone()),
        };
        assert!(matches_client(&install, &client));
        client.exe = Some(PathBuf::from("/unrelated/steam"));
        assert!(!matches_client(&install, &client));
        client.exe = None;
        assert!(matches_client(&install, &client));
        client.arguments[0] = b"/unrelated/steam".to_vec();
        assert!(!matches_client(&install, &client));
        install.kind = InstallKind::Flatpak;
        client.exe = Some(PathBuf::from("/sandbox/steam"));
        client.arguments[0] = path.as_os_str().as_bytes().to_vec();
        assert!(matches_client(&install, &client));
        let alias = temp.path().join("alias");
        symlink(temp.path(), &alias).unwrap();
        install.kind = InstallKind::Native;
        install.root = alias;
        client.exe = Some(path);
        assert!(matches_client(&install, &client));
    }

    #[test]
    fn uptime_ticks_track_current_process_start() {
        let ticks = uptime_ticks().unwrap();
        let own = read_stat(std::process::id()).unwrap().unwrap().start_time;
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
            start_time: read_stat(root.id()).unwrap().unwrap().start_time,
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
