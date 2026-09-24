use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};

/// umu-launcher, which runs Proton the way Steam does.
const UMU: &str = "umu-run";
/// How often `wait_settled` checks whether a Proton process group is gone.
const GROUP_POLL: Duration = Duration::from_millis(100);
/// How long a killed group that lost its leader first gets to finish exiting.
/// Long enough for killed members to become zombies, and too short to wait
/// out anything still running, which stays unconfirmed.
const LEADERLESS_GRACE: Duration = Duration::from_millis(200);

/// What ending a lost run found to kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Killed {
    /// Whatever the run left behind has been killed.
    Done,
    /// The run's process group was already gone.
    Gone,
    /// The run recorded no process group, so there is nothing to signal.
    Untracked,
}

/// The wine build an environment runs with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runner {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selection: Option<String>,
    kind: Kind,
}

/// Proton is driven entirely through umu, so it shares no executables with a
/// plain wine build rather than being another way to reach the same ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Wine { wine: PathBuf, wineserver: PathBuf },
    Proton { root: PathBuf },
}

impl Runner {
    pub fn selection(&self) -> &str {
        self.selection.as_deref().unwrap_or(&self.name)
    }

    pub(crate) fn with_selection(mut self, selection: &str) -> Self {
        self.selection = Some(selection.to_owned());
        self
    }

    /// The reserved name meaning whatever wine is on `PATH`.
    pub const SYSTEM: &'static str = "wine";

    /// The system wine, found on `PATH`.
    pub fn system() -> Result<Self> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Ok(Self {
            name: Self::SYSTEM.to_owned(),
            selection: None,
            kind: Kind::Wine {
                wine: find_in_path(&path, "wine").ok_or(Error::ProgramNotFound("wine"))?,
                wineserver: find_in_path(&path, "wineserver")
                    .ok_or(Error::ProgramNotFound("wineserver"))?,
            },
        })
    }

    /// An installed wine build, with its executables already located.
    pub(crate) fn wine_build(name: String, wine: PathBuf, wineserver: PathBuf) -> Self {
        Self {
            name,
            selection: None,
            kind: Kind::Wine { wine, wineserver },
        }
    }

    /// An installed Proton build. Only its root matters, since umu takes that
    /// as `PROTONPATH` and runs everything itself.
    pub(crate) fn proton(name: String, root: PathBuf) -> Self {
        Self {
            name,
            selection: None,
            kind: Kind::Proton { root },
        }
    }

    /// A runner whose `wine` and `wineserver` both resolve to `true` on PATH,
    /// so tests can run real commands without wine installed.
    #[cfg(test)]
    pub(crate) fn fake(name: &str) -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let true_bin = find_in_path(&path, "true").expect("`true` must be on PATH");
        Self::wine_build(name.to_owned(), true_bin.clone(), true_bin)
    }

    /// A Proton runner pointing nowhere, for tests that only need its kind.
    #[cfg(test)]
    pub(crate) fn fake_proton(name: &str) -> Self {
        Self::proton(name.to_owned(), PathBuf::from("/nonexistent"))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::Wine { .. } => "wine",
            Kind::Proton { .. } => "proton",
        }
    }

    pub fn is_proton(&self) -> bool {
        matches!(self.kind, Kind::Proton { .. })
    }

    /// What wineo actually spawns, for naming it in an error.
    pub fn launcher(&self) -> &'static str {
        match self.kind {
            Kind::Wine { .. } => "wine",
            Kind::Proton { .. } => UMU,
        }
    }

    /// Whether other processes can outlive the launched program. Wine keeps a
    /// prefix alive after a launcher hands off; a Proton run is over when umu
    /// returns, because everything else is inside its container.
    pub fn outlives_program(&self) -> bool {
        !self.is_proton()
    }

    /// The resolved executables, for handing to a program that runs wine
    /// itself. Absolute, so a different PATH can't select another build.
    /// None for Proton, which has no wine wineo is meant to run directly.
    pub fn wine_path(&self) -> Option<&Path> {
        match &self.kind {
            Kind::Wine { wine, .. } => Some(wine),
            Kind::Proton { .. } => None,
        }
    }

    pub fn wineserver_path(&self) -> Option<&Path> {
        match &self.kind {
            Kind::Wine { wineserver, .. } => Some(wineserver),
            Kind::Proton { .. } => None,
        }
    }

    /// The wine prefix inside a configured prefix directory. Proton is given
    /// the compatdata directory and keeps its prefix in `pfx/` under it.
    pub fn wine_prefix(&self, prefix: &Path) -> PathBuf {
        match self.kind {
            Kind::Wine { .. } => prefix.to_owned(),
            Kind::Proton { .. } => prefix.join("pfx"),
        }
    }

    /// A command that runs a Windows program in `prefix`, through wine or
    /// through umu. The caller appends the program and its arguments. Also
    /// blocks wine's own menu builder, unless the inherited environment already
    /// opts back in. `prefix` is the configured directory, not `wine_prefix`:
    /// umu makes `pfx/` underneath it itself.
    pub fn command(&self, prefix: &Path, wrapper: Option<&Path>) -> Result<Command> {
        let mut command = match &self.kind {
            Kind::Wine { wine, .. } => wrapped(wine, wrapper),
            Kind::Proton { root } => {
                let umu = find_on_path(UMU).ok_or(Error::ProgramNotFound(UMU))?;
                let mut command = wrapped(&umu, wrapper);
                command.env("PROTONPATH", root);
                command
            }
        };
        let overrides =
            with_menu_builder_disabled(std::env::var("WINEDLLOVERRIDES").ok().as_deref());
        command
            .env("WINEPREFIX", prefix)
            .env("WINEDLLOVERRIDES", overrides);
        Ok(command)
    }

    /// A `wineserver` invocation bound to `prefix`. Proton has no wineserver
    /// wineo can reach, so its runs are ended through the process group instead.
    fn wineserver(&self, prefix: &Path) -> Option<Command> {
        let wineserver = self.wineserver_path()?;
        let mut command = Command::new(wineserver);
        command.env("WINEPREFIX", self.wine_prefix(prefix));
        Some(command)
    }

    /// Blocks until every process in the prefix has exited and wineserver has
    /// shut down. A Proton run is over when umu returns, so there is nothing
    /// to wait for.
    pub fn wait_idle(&self, prefix: &Path) -> Result<()> {
        let Some(mut command) = self.wineserver(prefix) else {
            return Ok(());
        };
        quietly(command.arg("-w"))
            .status()
            .context(|| "failed to run wineserver -w".to_owned())?;
        Ok(())
    }

    /// Like `wait_idle`, but only returns Ok once it is confirmed that nothing
    /// is left: wineserver has to report the prefix idle, and a Proton
    /// program's whole process group has to be gone.
    pub fn wait_settled(&self, prefix: &Path, group: Option<u32>) -> Result<()> {
        let Some(mut command) = self.wineserver(prefix) else {
            return wait_for_group(group);
        };
        let status = quietly(command.arg("-w"))
            .status()
            .context(|| "failed to run wineserver -w".to_owned())?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Unsettled(format!("wineserver -w failed ({status})")))
        }
    }

    /// Like `wait_settled`, but requires the recorded process-group identity
    /// before it can confirm that the group is gone.
    pub fn wait_settled_owned(
        &self,
        prefix: &Path,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<()> {
        if let Some(mut command) = self.wineserver(prefix) {
            let status = quietly(command.arg("-w"))
                .status()
                .context(|| "failed to run wineserver -w".to_owned())?;
            if !status.success() {
                return Err(Error::Unsettled(format!("wineserver -w failed ({status})")));
            }
        }
        wait_for_recorded_group(group, boot_id, leader_start)
    }

    /// Settlement for the owner that still holds the live operation lock.
    pub fn wait_settled_live(
        &self,
        prefix: &Path,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<()> {
        if let Some(mut command) = self.wineserver(prefix) {
            let status = quietly(command.arg("-w"))
                .status()
                .context(|| "failed to run wineserver -w".to_owned())?;
            if !status.success() {
                return Err(Error::Unsettled(format!("wineserver -w failed ({status})")));
            }
        }
        wait_for_live_group(group, boot_id, leader_start)
    }

    /// Asks running programs to close: a Windows logoff for wine, SIGTERM to
    /// umu's process group for Proton, which has no wineserver to talk to.
    pub fn end_session(&self, prefix: &Path, group: Option<u32>) -> Result<()> {
        if self.is_proton() {
            signal_group(group, Signal::TERM)?;
            return Ok(());
        }
        quietly(
            self.command(prefix, None)?
                .args(["wineboot", "--end-session"]),
        )
        .status()
        .context(|| "failed to run wineboot --end-session".to_owned())?;
        Ok(())
    }

    /// Ends the session only while the recorded leader still proves ownership.
    pub fn end_session_owned(
        &self,
        prefix: &Path,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<crate::process::GroupOwnership> {
        let ownership = crate::process::group_ownership(group, boot_id, leader_start);
        if ownership != crate::process::GroupOwnership::Owned {
            return Ok(ownership);
        }
        if self.is_proton() {
            return crate::process::signal_group_if_owned(
                group,
                boot_id,
                leader_start,
                Signal::TERM,
            )
            .context(|| "failed to signal tracked process group".to_owned());
        }
        quietly(
            self.command(prefix, None)?
                .args(["wineboot", "--end-session"]),
        )
        .status()
        .context(|| "failed to run wineboot --end-session".to_owned())?;
        Ok(crate::process::GroupOwnership::Owned)
    }

    /// Ends a session while the caller still owns the operation and prefix.
    pub fn end_session_live(
        &self,
        prefix: &Path,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<crate::process::GroupOwnership> {
        let (Some(group_id), Some(boot_id), Some(leader_start)) = (group, boot_id, leader_start)
        else {
            return Ok(crate::process::GroupOwnership::Unknown);
        };
        let ownership = crate::process::live_group_ownership(group_id, boot_id, leader_start);
        if ownership != crate::process::GroupOwnership::Owned {
            return Ok(ownership);
        }
        if self.is_proton() {
            return crate::process::signal_group_if_live(
                Some(group_id),
                Some(boot_id),
                Some(leader_start),
                Signal::TERM,
            )
            .context(|| "failed to signal tracked process group".to_owned());
        }
        quietly(
            self.command(prefix, None)?
                .args(["wineboot", "--end-session"]),
        )
        .status()
        .context(|| "failed to run wineboot --end-session".to_owned())?;
        Ok(crate::process::GroupOwnership::Owned)
    }

    /// Kills whatever is left without giving programs a chance to clean up.
    pub fn kill(&self, prefix: &Path, group: Option<u32>) -> Result<Killed> {
        let Some(mut command) = self.wineserver(prefix) else {
            return signal_group(group, Signal::KILL);
        };
        quietly(command.arg("-k"))
            .status()
            .context(|| "failed to run wineserver -k".to_owned())?;
        Ok(Killed::Done)
    }

    /// Kills only work whose recorded leader still proves ownership.
    pub fn kill_owned(
        &self,
        prefix: &Path,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<crate::process::GroupOwnership> {
        let ownership = crate::process::group_ownership(group, boot_id, leader_start);
        if ownership != crate::process::GroupOwnership::Owned {
            return Ok(ownership);
        }
        if self.is_proton() {
            return crate::process::signal_group_if_owned(
                group,
                boot_id,
                leader_start,
                Signal::KILL,
            )
            .context(|| "failed to signal tracked process group".to_owned());
        }
        self.kill(prefix, group)?;
        Ok(crate::process::GroupOwnership::Owned)
    }

    /// Kills tracked work while the caller still owns its active operation.
    pub fn kill_live(
        &self,
        prefix: &Path,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<crate::process::GroupOwnership> {
        let (Some(group_id), Some(boot_id), Some(leader_start)) = (group, boot_id, leader_start)
        else {
            return Ok(crate::process::GroupOwnership::Unknown);
        };
        let ownership = crate::process::live_group_ownership(group_id, boot_id, leader_start);
        if ownership != crate::process::GroupOwnership::Owned {
            return Ok(ownership);
        }
        if self.is_proton() {
            return crate::process::signal_group_if_live(
                Some(group_id),
                Some(boot_id),
                Some(leader_start),
                Signal::KILL,
            )
            .context(|| "failed to signal tracked process group".to_owned());
        }
        self.kill(prefix, Some(group_id))?;
        Ok(crate::process::GroupOwnership::Owned)
    }

    /// The version this runner reports: what wine prints, or what Proton
    /// records in its own `version` file.
    pub fn version(&self) -> Option<String> {
        match &self.kind {
            Kind::Wine { wine, .. } => {
                let output = Command::new(wine)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()?;
                let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (output.status.success() && !version.is_empty()).then_some(version)
            }
            // Proton writes a build timestamp and a name, like `1717451631 GE-Proton9-4`.
            Kind::Proton { root } => {
                let text = std::fs::read_to_string(root.join("version")).ok()?;
                let version = text.split_whitespace().next_back()?.to_owned();
                (!version.is_empty()).then_some(version)
            }
        }
    }
}

/// Signals a whole process group, so umu's container and everything under it
/// go together. A run recorded before wineo kept the group has none.
fn signal_group(group: Option<u32>, signal: Signal) -> Result<Killed> {
    let Some(pid) = group
        .and_then(|group| i32::try_from(group).ok())
        .and_then(Pid::from_raw)
    else {
        return Ok(Killed::Untracked);
    };
    match kill_process_group(pid, signal) {
        Ok(()) => Ok(Killed::Done),
        Err(Errno::SRCH) => Ok(Killed::Gone),
        Err(err) => {
            Err(io::Error::from(err)).context(|| format!("failed to signal process group {pid:?}"))
        }
    }
}

/// Polls until nothing is left in a process group. umu reaps what its
/// container starts before it exits, so this is normally over at once.
fn wait_for_group(group: Option<u32>) -> Result<()> {
    let Some(pid) = group
        .and_then(|group| i32::try_from(group).ok())
        .and_then(Pid::from_raw)
    else {
        return Err(Error::Unsettled(
            "no process group was recorded to check".to_owned(),
        ));
    };
    loop {
        match test_kill_process_group(pid) {
            Err(Errno::SRCH) => return Ok(()),
            // Something in the group belongs to someone else, which still
            // means the group is not gone.
            Ok(()) | Err(Errno::PERM) => std::thread::sleep(GROUP_POLL),
            Err(err) => {
                return Err(io::Error::from(err))
                    .context(|| format!("failed to check process group {pid:?}"));
            }
        }
    }
}

fn wait_for_recorded_group(
    group: Option<u32>,
    boot_id: Option<&str>,
    leader_start: Option<u64>,
) -> Result<()> {
    let Some(group) = group else {
        return Err(Error::Unsettled(
            "no process group was recorded to check".to_owned(),
        ));
    };
    let identified = boot_id.is_some() && leader_start.is_some();
    wait_for_owned_group(group, identified, || {
        crate::process::group_ownership(Some(group), boot_id, leader_start)
    })
}

fn wait_for_live_group(
    group: Option<u32>,
    boot_id: Option<&str>,
    leader_start: Option<u64>,
) -> Result<()> {
    let (Some(group), Some(boot_id), Some(leader_start)) = (group, boot_id, leader_start) else {
        return Err(Error::Unsettled(
            "no complete process identity was recorded".to_owned(),
        ));
    };
    wait_for_owned_group(group, true, || {
        crate::process::live_group_ownership(group, boot_id, leader_start)
    })
}

/// Polls until an owned group has nothing live left. A killed group can lose
/// its leader a moment before the rest, which leaves ownership unknown, so a
/// fully identified group gets `LEADERLESS_GRACE` to finish exiting.
fn wait_for_owned_group(
    group: u32,
    identified: bool,
    ownership: impl Fn() -> crate::process::GroupOwnership,
) -> Result<()> {
    let leaderless_deadline = Instant::now() + LEADERLESS_GRACE;
    loop {
        match ownership() {
            crate::process::GroupOwnership::Gone => return Ok(()),
            crate::process::GroupOwnership::Owned => {
                match crate::process::group_has_live_members(group) {
                    Some(false) => return Ok(()),
                    Some(true) => std::thread::sleep(GROUP_POLL),
                    None => {
                        return Err(Error::Unsettled(
                            "process-group members could not be inspected".to_owned(),
                        ));
                    }
                }
            }
            crate::process::GroupOwnership::Unknown => {
                match identified.then(|| crate::process::leaderless_group_settled(group)) {
                    Some(Some(true)) => return Ok(()),
                    Some(Some(false)) if Instant::now() < leaderless_deadline => {
                        std::thread::sleep(GROUP_POLL);
                    }
                    _ => {
                        return Err(Error::Unsettled(
                            "process-group ownership could not be confirmed".to_owned(),
                        ));
                    }
                }
            }
        }
    }
}

/// `program`, or `wrapper program` when something like gamemoderun wraps it.
fn wrapped(program: &Path, wrapper: Option<&Path>) -> Command {
    match wrapper {
        Some(wrapper) => {
            let mut command = Command::new(wrapper);
            command.arg(program);
            command
        }
        None => Command::new(program),
    }
}

/// Adds `winemenubuilder.exe=d` to a `WINEDLLOVERRIDES` value, unless it
/// already mentions winemenubuilder, so a user's own override always wins.
pub fn with_menu_builder_disabled(current: Option<&str>) -> String {
    const DISABLED: &str = "winemenubuilder.exe=d";
    match current {
        None | Some("") => DISABLED.to_owned(),
        Some(current) if mentions_menu_builder(current) => current.to_owned(),
        Some(current) => format!("{current};{DISABLED}"),
    }
}

/// Wine's `WINEDLLOVERRIDES` syntax is `name1,name2=order;name3=order2`.
fn mentions_menu_builder(overrides: &str) -> bool {
    overrides.split(';').any(|entry| {
        let names = entry.split_once('=').map_or(entry, |(names, _)| names);
        names.split(',').any(|name| {
            let name = name.trim();
            name.eq_ignore_ascii_case("winemenubuilder")
                || name.eq_ignore_ascii_case("winemenubuilder.exe")
        })
    })
}

fn quietly(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

/// Looks `program` up on the current PATH.
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    find_in_path(&std::env::var_os("PATH").unwrap_or_default(), program)
}

fn find_in_path(path_var: &OsStr, program: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
        .and_then(|candidate| std::fs::canonicalize(candidate).ok())
}

pub(crate) fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use super::*;

    #[test]
    fn finds_first_executable_on_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        // Not executable, so it must be skipped.
        fs::write(first.path().join("wine"), "").unwrap();
        let wine = second.path().join("wine");
        fs::write(&wine, "").unwrap();
        fs::set_permissions(&wine, fs::Permissions::from_mode(0o755)).unwrap();

        let path_var = std::env::join_paths([
            "relative",
            first.path().to_str().unwrap(),
            second.path().to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(find_in_path(&path_var, "wine"), Some(wine));
        assert_eq!(find_in_path(&path_var, "wineserver"), None);
        assert_eq!(find_in_path(&OsString::new(), "wine"), None);
    }

    #[test]
    fn menu_builder_is_disabled_when_overrides_are_unset() {
        assert_eq!(with_menu_builder_disabled(None), "winemenubuilder.exe=d");
        assert_eq!(
            with_menu_builder_disabled(Some("")),
            "winemenubuilder.exe=d"
        );
    }

    #[test]
    fn menu_builder_override_is_appended_to_existing_overrides() {
        assert_eq!(
            with_menu_builder_disabled(Some("foo.dll=n")),
            "foo.dll=n;winemenubuilder.exe=d"
        );
    }

    #[test]
    fn existing_menu_builder_mentions_are_left_alone() {
        for value in [
            "winemenubuilder.exe=n",
            "WineMenuBuilder=b",
            "foo.dll=n;winemenubuilder.exe=n",
            "foo,winemenubuilder.exe=n",
            " winemenubuilder.exe =n",
        ] {
            assert_eq!(with_menu_builder_disabled(Some(value)), value, "{value}");
        }
    }

    #[test]
    fn substring_matches_do_not_count_as_a_mention() {
        assert_eq!(
            with_menu_builder_disabled(Some("winemenubuilderextra.exe=n")),
            "winemenubuilderextra.exe=n;winemenubuilder.exe=d"
        );
        assert_eq!(
            with_menu_builder_disabled(Some("extrawinemenubuilder.exe=n")),
            "extrawinemenubuilder.exe=n;winemenubuilder.exe=d"
        );
    }

    #[test]
    fn a_killed_group_that_loses_its_leader_first_still_settles() {
        use std::os::unix::process::CommandExt;

        let mut leader = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let group = leader.id();
        let boot = crate::process::boot_id().unwrap();
        let leader_start = crate::process::start_time(group).unwrap();
        let mut member = Command::new("sleep")
            .arg("30")
            .process_group(group as i32)
            .spawn()
            .unwrap();
        leader.kill().unwrap();
        leader.wait().unwrap();
        member.kill().unwrap();

        let settled = wait_for_recorded_group(Some(group), Some(&boot), Some(leader_start));
        member.wait().unwrap();
        settled.unwrap();
    }
}
