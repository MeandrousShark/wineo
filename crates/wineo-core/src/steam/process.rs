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

use rustix::event::{PollFd, PollFlags, poll};
use rustix::fd::OwnedFd;
use rustix::param::clock_ticks_per_second;
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use super::{Install, InstallKind};
use crate::process::{pidfd_exited, read_boot_id};

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
    let boot_id = read_boot_id()?;
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
        if !launches_wanted_app(reader, pid, app_args.as_slice()) {
            continue;
        }
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

/// A one-read rejection for the pids of a sweep, most of which are not a
/// Steam launch at all. It only skips; a pid that passes (or whose command
/// line could not be judged) still goes through the full pidfd-verified
/// `reaper_candidate`, which reads the arguments again under the pidfd.
fn launches_wanted_app<R: ProcessReader, W: WantedApps + ?Sized>(
    reader: &R,
    pid: u32,
    wanted: &W,
) -> bool {
    match reader.arguments(pid) {
        Ok(Some(arguments)) => {
            arguments.len() >= 3 && arguments[1] == b"SteamLaunch" && wanted.wants(&arguments[2])
        }
        Ok(None) => false,
        Err(error) if permission_denied(&error) => false,
        // Leave anything unexpected to the full check, which reports it.
        Err(_) => true,
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
    let boot_id = read_boot_id()?;
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
    if read_boot_id()? != baseline.boot_id {
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
    if read_boot_id()? != reaper.boot_id {
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
    if read_boot_id()? != reaper.boot_id {
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
mod tests;
