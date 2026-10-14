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
/// How long cleanup after a failure waits for a killed group, so a process
/// stuck in uninterruptible sleep can't hang the caller.
pub const CLEANUP_BOUND: Duration = Duration::from_secs(10);
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
    read_boot_id().ok()
}

pub(crate) fn read_boot_id() -> io::Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
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
/// from it kills the group, and comes back with how the group ended. Returns
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
) -> io::Result<Result<Ended, (E, Ended)>> {
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
) -> io::Result<Result<Ended, (E, Ended)>> {
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
) -> io::Result<Result<Ended, (E, Ended)>> {
    let wake = wake.ok();
    let mut settle_poll = SETTLE_POLL;
    loop {
        if let Err(err) = on(None) {
            return abandon(leader, &members, err, CLEANUP_BOUND);
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
                return abandon(leader, &members, err, CLEANUP_BOUND);
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

/// Ends a stop whose caller failed: kills the whole group and waits, up to
/// `bound`, until nothing in it is left running. Returns the error with how
/// the group ended, `Unconfirmed` if that couldn't be seen in time, since a
/// process in uninterruptible sleep can outlast SIGKILL. The leader stays
/// unreaped.
fn abandon<E>(
    leader: &Child,
    members: &impl Fn(u32) -> Option<bool>,
    err: E,
    bound: Duration,
) -> io::Result<Result<Ended, (E, Ended)>> {
    signal_led_group(leader, Signal::KILL)?;
    let deadline = Instant::now() + bound;
    let mut poll = SETTLE_POLL;
    loop {
        if leader_exited(leader)? {
            match members(leader.id()) {
                Some(false) => return Ok(Err((err, Ended::Stopped))),
                None => return Ok(Err((err, Ended::Unconfirmed))),
                Some(true) => {}
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Err((err, Ended::Unconfirmed)));
        }
        std::thread::sleep(poll.min(left));
        poll = (poll * 2).min(SETTLE_POLL_MAX);
    }
}

/// Kills the group `leader` leads and reaps the leader, for a caller giving
/// up on a stop. Only for a child nothing has reaped yet. Gives up after
/// `CLEANUP_BOUND`, leaving a leader stuck in uninterruptible sleep unreaped
/// rather than hanging; false then.
pub fn kill_and_reap(leader: &mut Child) -> bool {
    let _ = signal_led_group(leader, Signal::KILL);
    reap_within(leader, CLEANUP_BOUND, |leader| {
        leader.try_wait().map(|status| status.is_some())
    })
}

/// A failed stop's error, saying `unconfirmed` too unless its group was seen
/// to stop and its leader reaped.
pub(crate) fn failed_stop(
    err: crate::error::Error,
    settled: bool,
    unconfirmed: &str,
) -> crate::error::Error {
    if settled {
        return err;
    }
    crate::error::Error::Unsettled(format!(
        "{unconfirmed} after: {}",
        crate::error::chain(&err)
    ))
}

/// Reaps `leader` through `reap` once it can be, for up to `bound`.
fn reap_within(
    leader: &mut Child,
    bound: Duration,
    mut reap: impl FnMut(&mut Child) -> io::Result<bool>,
) -> bool {
    let deadline = Instant::now() + bound;
    let mut poll = SETTLE_POLL;
    loop {
        if reap(leader).unwrap_or(false) {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(poll.min(left));
        poll = (poll * 2).min(SETTLE_POLL_MAX);
    }
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

/// A pidfd can watch a process wineo did not spawn; waitid cannot.
pub fn pidfd_exited(pidfd: &OwnedFd) -> io::Result<bool> {
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
mod tests;
