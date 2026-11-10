//! Launches on a real bus: each test starts its own `dbus-daemon` and runs
//! each launch in a child copy of this test binary, which claims the name
//! the way `run()` does but never opens a window. The bus is reached only
//! through the child's `DBUS_SESSION_BUS_ADDRESS`, so nothing here touches
//! the real session bus.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use super::super::{Claim, NAME, bus_name, claim};
use crate::test_script::KillOnDrop;
use crate::tray::tests::bus::Bus;

/// How long a test waits for the bus or a child to answer.
const PATIENCE: Duration = Duration::from_secs(15);
const CHILD: &str = "WINEO_INSTANCE_TEST_CHILD";
const CHILD_TEST: &str = "instance::tests::bus::child_launches";
const LINE: &str = "instance: ";

/// Not a test of its own: the launch the others start. It claims the name
/// for its environment's root and prints what it got. An owner then prints
/// each `Show` and takes "refuse" and "leave" on stdin.
#[test]
fn child_launches() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let paths = wineo_core::paths::Paths::from_env(None).unwrap();
    let instance = match claim(&paths) {
        Claim::Owner(instance) => instance,
        Claim::Shown => {
            println!("{LINE}shown");
            return;
        }
        Claim::Alone => {
            println!("{LINE}alone");
            return;
        }
    };
    println!("{LINE}owner");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let commands = instance.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            match line.as_deref() {
                Ok("refuse") => {
                    commands.refuse_shows();
                    println!("{LINE}refused");
                }
                Ok("leave") => {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(commands.leave());
                    println!("{LINE}left");
                }
                _ => {}
            }
        }
        std::process::exit(0);
    });
    runtime.block_on(async move {
        use iced::futures::StreamExt;
        let shows = instance.shows();
        let mut stream = std::pin::pin!(super::super::shows(&shows));
        while stream.next().await.is_some() {
            println!("{LINE}show");
        }
    });
}

/// The roots a launch can have: the default one under its home, a
/// portable folder, or a custom data dir.
#[derive(Clone, Copy)]
enum Root<'a> {
    Default,
    Portable(&'a Path),
    Data(&'a Path),
}

/// One launch, with its printed lines.
struct Launch {
    process: KillOnDrop,
    stdin: std::process::ChildStdin,
    lines: std_mpsc::Receiver<String>,
}

impl Launch {
    /// A launch on `bus`, or with no session bus at all when it's None.
    fn start(bus: Option<&Bus>, home: &Path, root: Root) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env_clear()
            .env(CHILD, "1")
            .env("HOME", home)
            // Where a missing DBUS_SESSION_BUS_ADDRESS sends zbus, so the
            // real bus under /run/user is never the fallback.
            .env("XDG_RUNTIME_DIR", home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(bus) = bus {
            command.env("DBUS_SESSION_BUS_ADDRESS", &bus.address);
        }
        match root {
            Root::Default => {}
            Root::Portable(folder) => {
                command.env(wineo_core::paths::PORTABLE_VAR, folder);
            }
            Root::Data(dir) => {
                command.env("XDG_DATA_HOME", dir);
            }
        }
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = std_mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                // The first one shares its line with the harness's own.
                if let Some((_, said)) = line.split_once(LINE)
                    && send.send(said.to_owned()).is_err()
                {
                    return;
                }
            }
        });
        Self {
            process: KillOnDrop::new(child),
            stdin,
            lines,
        }
    }

    /// The next line the launch prints, within `PATIENCE`.
    fn next(&mut self) -> String {
        self.lines
            .recv_timeout(PATIENCE)
            .unwrap_or_else(|_| panic!("the launch said nothing"))
    }

    fn expect(&mut self, wanted: &str) {
        assert_eq!(self.next(), wanted);
    }

    /// Nothing more within a short while.
    fn quiet(&mut self) {
        if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(300)) {
            panic!("unexpected {line:?}");
        }
    }

    fn send(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").unwrap();
    }

    /// Waits for the launch to end on its own.
    fn exit_status(&mut self) -> ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the launch didn't exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Whether `name` has an owner on `bus`, asked as a host would.
fn owned(bus: &Bus, name: &str) -> bool {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let connection = bus.connect().await;
            zbus::fdo::DBusProxy::new(&connection)
                .await
                .unwrap()
                .name_has_owner(name.try_into().unwrap())
                .await
                .unwrap()
        })
}

/// The name a launch with `vars` claims, worked out here the same way.
fn name_for(vars: &[(&str, &Path)]) -> String {
    let vars: Vec<(String, std::ffi::OsString)> = vars
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.as_os_str().to_owned()))
        .collect();
    let paths = wineo_core::paths::Paths::from_lookup(|key| {
        vars.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    })
    .unwrap();
    bus_name(&paths)
}

#[test]
fn a_second_launch_shows_the_first_and_exits_zero() {
    let Some(bus) = Bus::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let mut first = Launch::start(Some(&bus), home.path(), Root::Default);
    first.expect("owner");
    assert!(owned(&bus, NAME));

    let mut second = Launch::start(Some(&bus), home.path(), Root::Default);
    second.expect("shown");
    assert!(second.exit_status().success());
    first.expect("show");

    // And again, for a launcher clicked twice.
    let mut third = Launch::start(Some(&bus), home.path(), Root::Default);
    third.expect("shown");
    assert!(third.exit_status().success());
    first.expect("show");
}

#[test]
fn two_roots_are_two_apps() {
    let Some(bus) = Bus::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut default = Launch::start(Some(&bus), home.path(), Root::Default);
    default.expect("owner");
    let mut portable = Launch::start(Some(&bus), home.path(), Root::Portable(folder.path()));
    portable.expect("owner");
    let mut custom = Launch::start(Some(&bus), home.path(), Root::Data(data.path()));
    custom.expect("owner");

    let portable_name = name_for(&[
        ("HOME", home.path()),
        (wineo_core::paths::PORTABLE_VAR, folder.path()),
    ]);
    let data_name = name_for(&[("HOME", home.path()), ("XDG_DATA_HOME", data.path())]);
    assert_ne!(portable_name, NAME);
    assert_ne!(data_name, NAME);
    assert!(owned(&bus, NAME));
    assert!(owned(&bus, &portable_name));
    assert!(owned(&bus, &data_name));

    // A relaunch reaches only the app for its own root.
    let mut again = Launch::start(Some(&bus), home.path(), Root::Portable(folder.path()));
    again.expect("shown");
    assert!(again.exit_status().success());
    portable.expect("show");
    default.quiet();
    custom.quiet();
}

#[test]
fn a_launch_after_the_first_left_starts_fresh() {
    let Some(bus) = Bus::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let mut first = Launch::start(Some(&bus), home.path(), Root::Default);
    first.expect("owner");
    first.send("leave");
    first.expect("left");
    assert!(!owned(&bus, NAME));

    let mut second = Launch::start(Some(&bus), home.path(), Root::Default);
    second.expect("owner");
    first.quiet();

    // One that's gone altogether leaves the name too.
    drop(second);
    let mut third = Launch::start(Some(&bus), home.path(), Root::Default);
    third.expect("owner");
}

#[test]
fn a_launch_during_a_slow_exit_waits_for_the_name() {
    let Some(bus) = Bus::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let mut first = Launch::start(Some(&bus), home.path(), Root::Default);
    first.expect("owner");
    // The exit has begun but the name hasn't gone yet.
    first.send("refuse");
    first.expect("refused");

    let mut second = Launch::start(Some(&bus), home.path(), Root::Default);
    std::thread::sleep(Duration::from_millis(300));
    first.send("leave");
    first.expect("left");
    second.expect("owner");
    first.quiet();
}

#[test]
fn a_launch_finding_a_stuck_owner_starts_anyway() {
    let Some(bus) = Bus::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let mut first = Launch::start(Some(&bus), home.path(), Root::Default);
    first.expect("owner");
    // Stopped, it holds the name and never answers.
    let pid = rustix::process::Pid::from_raw(first.process.id().try_into().unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::STOP).unwrap();

    let started = Instant::now();
    let mut second = Launch::start(Some(&bus), home.path(), Root::Default);
    second.expect("alone");
    let waited = started.elapsed();
    // One Show's wait, then the name is polled until the release wait runs
    // out, with a margin for starting the child.
    let bound = super::super::SHOW_WAIT + super::super::RELEASE_WAIT;
    assert!(waited >= bound, "{waited:?}");
    assert!(waited < bound + Duration::from_millis(1500), "{waited:?}");
    assert!(second.exit_status().success());
    rustix::process::kill_process(pid, rustix::process::Signal::CONT).unwrap();
}

#[test]
fn two_launches_at_once_make_one_app() {
    let Some(bus) = Bus::start() else { return };
    let home = tempfile::tempdir().unwrap();
    for _ in 0..5 {
        let mut a = Launch::start(Some(&bus), home.path(), Root::Default);
        let mut b = Launch::start(Some(&bus), home.path(), Root::Default);
        let (said_a, said_b) = (a.next(), b.next());
        let (owner, other) = match (said_a.as_str(), said_b.as_str()) {
            ("owner", "shown") => (&mut a, &mut b),
            ("shown", "owner") => (&mut b, &mut a),
            pair => panic!("{pair:?}"),
        };
        assert!(other.exit_status().success());
        owner.expect("show");
        drop(a);
        drop(b);
        // The owner's gone, so the bus takes the name back before the next
        // round.
        let deadline = Instant::now() + PATIENCE;
        while owned(&bus, NAME) {
            assert!(Instant::now() < deadline, "the name stayed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn no_session_bus_carries_on_alone() {
    let home = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut launch = Launch::start(None, home.path(), Root::Default);
    launch.expect("alone");
    assert!(launch.exit_status().success());
    assert!(started.elapsed() < PATIENCE);
}
