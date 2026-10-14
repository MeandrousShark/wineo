//! Process and process-group identities for conservative recovery.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll};
use rustix::fd::OwnedFd;
use rustix::io::Errno;
use rustix::process::{
    Pid, PidfdFlags, Resource, Rlimit, Signal, WaitId, WaitIdOptions, getrlimit,
    kill_process_group, pidfd_open, pidfd_send_signal, prlimit, waitid,
};

use crate::cancel::{Subscription, Token};

/// How often a stopped group's members are checked once its leader is gone,
/// since they have nothing to wait on. Backs off to the max.
const SETTLE_POLL: Duration = Duration::from_millis(20);
const SETTLE_POLL_MAX: Duration = Duration::from_millis(250);
/// How often a stop looks around when it has nothing to block on.
const FALLBACK_POLL: Duration = Duration::from_millis(20);

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

/// A process group as it was when it started: its number, the boot it ran
/// in, and when its leader started, so a later group that reuses the number
/// is never mistaken for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub group: u32,
    pub boot_id: String,
    pub leader_start: u64,
}

impl Identity {
    /// The group `leader` leads, read now. None once either part is unreadable.
    pub fn of(leader: u32) -> Option<Self> {
        Some(Self {
            group: leader,
            boot_id: boot_id()?,
            leader_start: start_time(leader)?,
        })
    }

    /// A recorded identity, which is no identity at all with a part missing.
    pub fn recorded(
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Option<Self> {
        Some(Self {
            group: group?,
            boot_id: boot_id?.to_owned(),
            leader_start: leader_start?,
        })
    }

    /// Checks whether the recorded leader still identifies the recorded group.
    pub fn ownership(&self) -> GroupOwnership {
        let Some(current_boot) = boot_id() else {
            return GroupOwnership::Unknown;
        };
        classify_group(
            Some(&self.boot_id),
            Some(&current_boot),
            Some(self.leader_start),
            read_stat(self.group),
            group_exists(self.group),
            self.group,
        )
    }

    /// Signals each verified member by pidfd. The caller never signals a
    /// process by a group number alone, which could be reused between checking
    /// and killing.
    pub fn signal_if_owned(&self, signal: Signal) -> io::Result<GroupOwnership> {
        let group = self.group;
        match self.ownership() {
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
            .find(|(pid, start, _)| *pid == group && *start == self.leader_start)
        else {
            return Ok(GroupOwnership::Unknown);
        };
        if !self.leader_is_recorded() || pidfd_exited(leader_fd).unwrap_or(true) {
            return Ok(match self.ownership() {
                GroupOwnership::Gone => GroupOwnership::Gone,
                GroupOwnership::Owned | GroupOwnership::Unknown => GroupOwnership::Unknown,
            });
        }

        let mut signaled = false;
        let mut leader = None;
        for (pid, start_time, pidfd) in members {
            if pid == group {
                leader = Some((start_time, pidfd));
                continue;
            }
            if !self.leader_is_recorded() {
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
        if self.leader_is_recorded()
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
            Ok(self.ownership())
        }
    }

    /// Signals members of a group whose ownership was established while its
    /// leader was live and whose caller still holds the active operation lock.
    /// This can continue a cleanup after that leader has exited, since an
    /// extant group with a dead leader still needs settling.
    pub fn signal_if_live(&self, signal: Signal) -> io::Result<GroupOwnership> {
        let ownership = self.ownership();
        if ownership != GroupOwnership::Owned {
            return Ok(ownership);
        }
        let members = match open_group_members(self.group) {
            Ok(members) => members,
            Err(()) => return Ok(GroupOwnership::Unknown),
        };
        if self.ownership() != GroupOwnership::Owned {
            return Ok(GroupOwnership::Unknown);
        }
        let mut signaled = false;
        for (pid, start_time, pidfd) in members {
            if self.ownership() != GroupOwnership::Owned {
                return Ok(GroupOwnership::Unknown);
            }
            if !member_is_recorded(pid, self.group, start_time) || pidfd_exited(&pidfd)? {
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
            Ok(self.ownership())
        }
    }

    /// Kills the group if its leader still proves it, or, failing that, if it
    /// is still the live group the caller established.
    pub fn kill_now(&self) -> io::Result<GroupOwnership> {
        let ownership = self.signal_if_owned(Signal::KILL)?;
        if ownership != GroupOwnership::Unknown {
            return Ok(ownership);
        }
        self.signal_if_live(Signal::KILL)
    }

    fn leader_is_recorded(&self) -> bool {
        boot_id().as_deref() == Some(self.boot_id.as_str())
            && matches!(read_stat(self.group), ProcRead::Found(stat) if stat.start_time == self.leader_start && stat.group == self.group)
    }
}

/// How a group stop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The leader exited without a stop being requested.
    Exited,
    /// Stopped, with nothing in the group left running.
    Stopped,
    /// Stopped and killed, but the group couldn't be inspected to confirm it.
    Unconfirmed,
}

/// What a stop does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Ask programs to close.
    Ask,
    /// Kill what is left, the first time.
    Kill,
    /// Kill again, for a kill that can miss a program still starting.
    Rekill,
}

/// How far a stop has gone: the first request asks programs to close, and a
/// forced or repeated one, or the end of `grace`, kills them.
#[derive(Debug)]
pub struct Escalation {
    grace: Duration,
    retry: Option<Duration>,
    asked: bool,
    killed: Option<Instant>,
    due: Option<Instant>,
    /// The token level already acted on.
    seen: u8,
}

impl Escalation {
    pub fn new(grace: Duration, retry: Option<Duration>) -> Self {
        Self {
            grace,
            retry,
            asked: false,
            killed: None,
            due: None,
            seen: 0,
        }
    }

    pub fn request(&mut self, forced: bool, now: Instant) -> Option<Step> {
        if forced || self.stopping() {
            let step = if self.killed.is_some() {
                Step::Rekill
            } else {
                Step::Kill
            };
            self.killed.get_or_insert(now);
            self.due = self.retry.map(|retry| now + retry);
            return Some(step);
        }
        self.asked = true;
        self.due = Some(now + self.grace);
        Some(Step::Ask)
    }

    /// The kill that falls due at the end of the grace period or a retry.
    pub fn poll(&mut self, now: Instant) -> Option<Step> {
        let due = self.due.is_some_and(|due| due <= now);
        due.then(|| self.request(true, now)).flatten()
    }

    /// A request for each new token level, or else `poll`.
    pub fn follow(&mut self, token: &Token, now: Instant) -> Option<Step> {
        let level = u8::from(token.is_canceled()) + u8::from(token.is_forced());
        if level <= self.seen {
            return self.poll(now);
        }
        self.seen = level;
        self.request(level == 2, now)
    }

    pub fn stopping(&self) -> bool {
        self.asked || self.killed.is_some()
    }

    pub fn killed_at(&self) -> Option<Instant> {
        self.killed
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }
}

/// Signals the group `leader` leads. Only for an unreaped child: even as a
/// zombie it keeps the group number from being reused, so this needs no
/// `/proc` check and still works when processes can't be inspected.
pub fn signal_led_group(leader: &Child, signal: Signal) -> io::Result<()> {
    let group = i32::try_from(leader.id())
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(Errno::INVAL)?;
    match kill_process_group(group, signal) {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Whether a child has exited, leaving it unreaped. A reaped one is an error.
pub fn leader_exited(leader: &Child) -> io::Result<bool> {
    let pid = Pid::from_raw(leader.id() as i32).ok_or(Errno::INVAL)?;
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    // Unlike std's `try_wait`, rustix doesn't retry after a signal handler.
    loop {
        match waitid(WaitId::Pid(pid), options) {
            Ok(status) => return Ok(status.is_some()),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Waits for `leader`, an unreaped child leading its own group, stopping the
/// group as `token` and `escalation` say: `ask` first, then SIGKILL. `on`
/// hears `None` each time round and each step once it is signaled; an error
/// from it kills the group before it is returned. Returns
/// once the leader exits and, after a stop, nothing in the group is left
/// running or that can't be inspected after a kill. The leader stays unreaped
/// for the caller. The outer error is the helper's own, the inner one `on`'s.
///
/// It sleeps until the leader exits, the token rises, or a stop deadline
/// passes. `tick` adds a wakeup for a caller with work of its own, such as
/// reading output.
pub fn stop_group<E>(
    leader: &Child,
    ask: Signal,
    escalation: &mut Escalation,
    token: &Token,
    tick: Option<Duration>,
    on: impl FnMut(Option<Step>) -> Result<(), E>,
) -> io::Result<Result<Ended, E>> {
    stop_group_probed(
        leader,
        ask,
        escalation,
        token,
        tick,
        on,
        group_has_live_members,
    )
}

pub(crate) fn stop_group_probed<E>(
    leader: &Child,
    ask: Signal,
    escalation: &mut Escalation,
    token: &Token,
    tick: Option<Duration>,
    on: impl FnMut(Option<Step>) -> Result<(), E>,
    members: impl Fn(u32) -> Option<bool>,
) -> io::Result<Result<Ended, E>> {
    let wake = Wake::open(leader, token);
    stop_group_with(leader, ask, escalation, token, tick, on, members, wake)
}

/// What wakes a waiting stop: the leader's pidfd, and an eventfd the token
/// writes to when it rises.
pub(crate) struct Wake {
    exit: OwnedFd,
    raised: Arc<OwnedFd>,
    _subscription: Subscription,
}

impl Wake {
    fn open(leader: &Child, token: &Token) -> io::Result<Self> {
        let pid = Pid::from_raw(leader.id() as i32).ok_or(Errno::INVAL)?;
        let exit = pidfd_open(pid, PidfdFlags::empty())?;
        let raised = Arc::new(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?);
        // Subscribed before the first look at the token, so no rise is missed.
        let subscription = {
            let raised = raised.clone();
            token.subscribe(move || {
                let _ = rustix::io::write(&*raised, &1u64.to_ne_bytes());
            })
        };
        Ok(Self {
            exit,
            raised,
            _subscription: subscription,
        })
    }
}

/// `stop_group_probed` with its wake source given. Without one it polls,
/// so a spawned leader is never left behind over a missing pidfd or eventfd.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stop_group_with<E>(
    leader: &Child,
    ask: Signal,
    escalation: &mut Escalation,
    token: &Token,
    tick: Option<Duration>,
    mut on: impl FnMut(Option<Step>) -> Result<(), E>,
    members: impl Fn(u32) -> Option<bool>,
    wake: io::Result<Wake>,
) -> io::Result<Result<Ended, E>> {
    let wake = wake.ok();
    let mut settle_poll = SETTLE_POLL;
    loop {
        if let Err(err) = on(None) {
            return abandon(leader, &members, err);
        }
        // Checked before signaling, so a reaped leader is an error, not a
        // signal to whatever reused its number.
        let exited = leader_exited(leader)?;
        let now = Instant::now();
        let mut next = escalation.follow(token, now);
        while let Some(step) = next {
            // Signaled before `on` runs, so a failing cleanup there can never
            // keep the group alive.
            signal_led_group(leader, if step == Step::Ask { ask } else { Signal::KILL })?;
            if let Err(err) = on(Some(step)) {
                return abandon(leader, &members, err);
            }
            // A zero grace period kills along with the ask.
            next = (step == Step::Ask).then(|| escalation.poll(now)).flatten();
        }
        let mut timeout = tick;
        let mut shorten = |by: Duration| timeout = Some(timeout.map_or(by, |t| t.min(by)));
        if exited {
            if !escalation.stopping() {
                return Ok(Ok(Ended::Exited));
            }
            match members(leader.id()) {
                Some(false) => return Ok(Ok(Ended::Stopped)),
                // A delivered SIGKILL doesn't prove a member in uninterruptible
                // sleep is gone, so the caller decides what that means.
                None if escalation.killed_at().is_some() => return Ok(Ok(Ended::Unconfirmed)),
                _ => {}
            }
            shorten(settle_poll);
            settle_poll = (settle_poll * 2).min(SETTLE_POLL_MAX);
        }
        if let Some(due) = escalation.deadline() {
            shorten(due.saturating_duration_since(now));
        }
        match &wake {
            // An exited leader's pidfd stays readable, so it is left out then.
            Some(wake) => wait_either((!exited).then_some(&wake.exit), &wake.raised, timeout)?,
            None => token.wait_timeout(timeout.map_or(FALLBACK_POLL, |t| t.min(FALLBACK_POLL))),
        }
    }
}

/// Ends a stop whose caller failed: kills the whole group and waits until
/// nothing in it is left running, as far as it can be inspected, so an error
/// never leaves the group behind. The leader stays unreaped.
fn abandon<E>(
    leader: &Child,
    members: &impl Fn(u32) -> Option<bool>,
    err: E,
) -> io::Result<Result<Ended, E>> {
    signal_led_group(leader, Signal::KILL)?;
    let mut poll = SETTLE_POLL;
    while !leader_exited(leader)? || members(leader.id()) == Some(true) {
        std::thread::sleep(poll);
        poll = (poll * 2).min(SETTLE_POLL_MAX);
    }
    Ok(Err(err))
}

/// Kills the group `leader` leads and reaps it, for a caller giving up on a
/// stop. Only for a child nothing has reaped yet.
pub fn kill_and_reap(leader: &mut Child) {
    let _ = signal_led_group(leader, Signal::KILL);
    let _ = leader.wait();
}

/// Sleeps until `exit` or `raised` is readable or `timeout` passes, then
/// clears `raised`.
fn wait_either(
    exit: Option<&OwnedFd>,
    raised: &OwnedFd,
    timeout: Option<Duration>,
) -> io::Result<()> {
    let timeout = timeout.map(|timeout| Timespec::try_from(timeout).unwrap_or(Timespec::default()));
    let mut descriptors = vec![PollFd::new(raised, PollFlags::IN)];
    descriptors.extend(exit.map(|exit| PollFd::new(exit, PollFlags::IN)));
    match poll(&mut descriptors, timeout.as_ref()) {
        // A signal handler only cuts the wait short; the caller looks again.
        Ok(_) | Err(Errno::INTR) => {}
        Err(error) => return Err(error.into()),
    }
    let mut count = [0u8; 8];
    match rustix::io::read(raised, &mut count) {
        Ok(_) | Err(Errno::AGAIN) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The ownership of a recorded identity. Missing identity is always unknown,
/// even if the numeric group is absent.
pub fn group_ownership(identity: Option<&Identity>) -> GroupOwnership {
    identity.map_or(GroupOwnership::Unknown, Identity::ownership)
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
    group_live_in(&proc_pids()?, &Probe::REAL, group)
}

/// How processes are looked at, so tests can stand in for `/proc`.
struct Probe<'a> {
    read: &'a dyn Fn(u32) -> io::Result<String>,
    group_of: &'a dyn Fn(u32) -> io::Result<u32>,
}

impl Probe<'static> {
    const REAL: Self = Self {
        read: &read_proc_stat,
        group_of: &group_of,
    };
}

impl Probe<'_> {
    /// Whether a process whose stat is denied is in `group`. With `/proc`
    /// mounted `hidepid`, that's another user's process, or one of ours
    /// that made itself undumpable, so it's asked of the kernel instead,
    /// which needs no `/proc`. Its state and parents stay unknown, so one in
    /// the group has to count as live. `None` for an unexpected answer.
    fn denied_in(&self, pid: u32, group: u32) -> Option<bool> {
        match (self.group_of)(pid) {
            Ok(member) => Some(member == group),
            Err(err) if err.raw_os_error() == Some(Errno::SRCH.raw_os_error()) => Some(false),
            Err(_) => None,
        }
    }
}

fn group_of(pid: u32) -> io::Result<u32> {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::from_raw_os_error(Errno::SRCH.raw_os_error()))?;
    let group = rustix::process::getpgid(Some(pid))?;
    Ok(group.as_raw_nonzero().get() as u32)
}

fn group_live_in(pids: &[u32], probe: &Probe<'_>, group: u32) -> Option<bool> {
    for &pid in pids {
        match (probe.read)(pid) {
            Err(err) if denied(&err) => {
                if probe.denied_in(pid, group)? {
                    return Some(true);
                }
            }
            stat => match parse_stat_read(stat) {
                ProcRead::Found(stat)
                    if stat.group == group && !matches!(stat.state, 'Z' | 'X' | 'x') =>
                {
                    return Some(true);
                }
                ProcRead::Found(_) | ProcRead::Gone => {}
                ProcRead::Unknown => return None,
            },
        }
    }
    Some(false)
}

fn proc_pids() -> Option<Vec<u32>> {
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc").ok()? {
        if let Some(pid) = entry
            .ok()?
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        {
            pids.push(pid);
        }
    }
    Some(pids)
}

fn read_proc_stat(pid: u32) -> io::Result<String> {
    fs::read_to_string(format!("/proc/{pid}/stat"))
}

fn denied(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(code) if code == Errno::ACCESS.raw_os_error() || code == Errno::PERM.raw_os_error()
    )
}

fn pidfd_exited(pidfd: &OwnedFd) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(pidfd, PollFlags::IN)];
    // poll is never restarted after a signal handler runs, even with a zero
    // timeout, so a Ctrl+C mid-scan would otherwise surface as an error.
    loop {
        match poll(&mut descriptors, Some(&Timespec::default())) {
            Ok(_) => return Ok(!descriptors[0].revents().is_empty()),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
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

pub(crate) fn group_exists(group: u32) -> Option<bool> {
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
                group_of: Box::new(move |pid| {
                    match groups.iter().find(|(entry, _)| *entry == pid) {
                        Some((_, Ok(group))) => Ok(*group),
                        Some((_, Err(errno))) => Err(error(*errno)),
                        None => Err(error(Errno::SRCH)),
                    }
                }),
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
        while id(group, boot, start).ownership() == GroupOwnership::Owned
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
        assert_eq!(answer.unwrap(), Err("output read failed"));
        assert_eq!(live, Some(false));
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
        assert_eq!(answer.unwrap(), Err("cleanup failed"));
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
}
