//! The tray on a real bus: each test starts its own `dbus-daemon` and runs
//! the tray service in a child copy of this test binary, pointed at that bus
//! through `DBUS_SESSION_BUS_ADDRESS`. Setting the variable in this process
//! would need `unsafe`, and the test plays the tray host itself, so nothing
//! here reaches the real session bus.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::zvariant::{OwnedValue, Value};

use super::super::{Event, Menu, Row, events};
use crate::test_script::KillOnDrop;

/// How long a test waits for the bus or the child to answer.
const PATIENCE: Duration = Duration::from_secs(15);
const CHILD: &str = "WINEO_TRAY_TEST_CHILD";
const CHILD_TEST: &str = "tray::tests::bus::child_serves_the_tray";
const EVENT_LINE: &str = "tray-event: ";
const STOPPED: &str = "stopped";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

fn child_menu() -> Menu {
    Menu {
        play: vec![
            Row::play("stardew", "Stardew Valley"),
            Row::play("snake", "snake_game"),
        ],
        stop: vec![Row::stop("hollow", "Hollow Knight")],
        more: false,
    }
}

/// Not a test of its own: the child the others start. It serves the tray
/// with a fixed menu, prints each event, and on stdin swaps the menu for an
/// empty one or stops the service the way an exit does.
#[test]
fn child_serves_the_tray() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (menu, receiver) = tokio::sync::watch::channel(child_menu());
    let running = super::super::Running::default();
    let stopping = running.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            match line.as_deref() {
                Ok("swap") => {
                    menu.send_replace(Menu::default());
                }
                Ok("stop") => {
                    if let Some(stopped) = stopping.stop() {
                        iced::futures::executor::block_on(stopped);
                        println!("{EVENT_LINE}{STOPPED}");
                    }
                }
                _ => {}
            }
        }
    });
    runtime.block_on(async move {
        use iced::futures::StreamExt;
        let mut events = std::pin::pin!(events(receiver, crate::tray_icons(), running,));
        while let Some(event) = events.next().await {
            println!("{EVENT_LINE}{event:?}");
        }
    });
}

/// A session bus of the test's own, with no service files, so nothing on it
/// can start a real program.
pub(crate) struct Bus {
    _dir: tempfile::TempDir,
    _daemon: KillOnDrop,
    pub(crate) address: String,
}

impl Bus {
    /// None, with a note, where `dbus-daemon` isn't installed.
    pub(crate) fn start() -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("session.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                dir.path().join("bus").display()
            ),
        )
        .unwrap();
        let spawned = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipped: dbus-daemon isn't installed");
                return None;
            }
            Err(error) => panic!("dbus-daemon didn't start: {error}"),
        };
        let stdout = child.stdout.take().unwrap();
        let daemon = KillOnDrop::new(child);
        let mut address = String::new();
        BufReader::new(stdout).read_line(&mut address).unwrap();
        let address = address.trim().to_owned();
        assert!(address.starts_with("unix:"), "{address:?}");
        Some(Self {
            _dir: dir,
            _daemon: daemon,
            address,
        })
    }

    pub(crate) async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .method_timeout(PATIENCE)
            .build()
            .await
            .unwrap()
    }
}

/// The child running the tray service, with its printed events.
struct Child {
    process: KillOnDrop,
    stdin: std::process::ChildStdin,
    lines: std_mpsc::Receiver<String>,
    _home: tempfile::TempDir,
}

impl Child {
    fn start(bus: &Bus) -> Self {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env_clear()
            .env(CHILD, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = std_mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                // The first one shares its line with the harness's own.
                if let Some((_, event)) = line.split_once(EVENT_LINE)
                    && send.send(event.to_owned()).is_err()
                {
                    return;
                }
            }
        });
        Self {
            process: KillOnDrop::new(child),
            stdin,
            lines,
            _home: home,
        }
    }

    /// Waits for the child to print `event`, failing on any other.
    async fn expect(&mut self, event: &Event) {
        self.expect_line(&format!("{event:?}")).await;
    }

    async fn expect_line(&mut self, wanted: &str) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.lines.try_recv() {
                Ok(line) => {
                    assert_eq!(line, wanted);
                    return;
                }
                Err(std_mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    assert!(
                        self.process.try_wait().unwrap().is_none(),
                        "the child exited before {wanted}"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(_) => panic!("no {wanted} from the child"),
            }
        }
    }

    fn swap_menu(&mut self) {
        writeln!(self.stdin, "swap").unwrap();
    }

    fn stop(&mut self) {
        writeln!(self.stdin, "stop").unwrap();
    }
}

/// A stand-in for a bar's StatusNotifierWatcher, which records each item
/// that registers.
#[derive(Clone, Default)]
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(&self, service: &str) {
        self.items.lock().unwrap().push(service.to_owned());
    }

    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }
}

impl Watcher {
    /// Serves the watcher under its well-known name.
    async fn serve(&self, connection: &zbus::Connection) {
        connection
            .object_server()
            .at(WATCHER_PATH, self.clone())
            .await
            .unwrap();
        connection.request_name(WATCHER).await.unwrap();
    }

    /// The name the item registered with, once it has.
    async fn registered(&self) -> String {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(item) = self.items.lock().unwrap().first() {
                return item.clone();
            }
            assert!(Instant::now() < deadline, "nothing registered");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn run(test: impl AsyncFnOnce(Bus)) {
    let Some(bus) = Bus::start() else {
        return;
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(test(bus));
}

async fn property(connection: &zbus::Connection, item: &str, name: &str) -> OwnedValue {
    let reply = connection
        .call_method(
            Some(item),
            ITEM_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(ITEM_INTERFACE, name),
        )
        .await
        .unwrap();
    reply.body().deserialize::<OwnedValue>().unwrap()
}

type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// The menu's rows as (id, label), with separators as "-".
async fn layout(connection: &zbus::Connection, item: &str) -> Vec<(i32, String)> {
    let reply = connection
        .call_method(
            Some(item),
            MENU_PATH,
            Some(MENU_INTERFACE),
            "GetLayout",
            &(0i32, -1i32, Vec::<String>::new()),
        )
        .await
        .unwrap();
    let (_revision, (_, _, children)): (u32, Layout) = reply.body().deserialize().unwrap();
    children
        .into_iter()
        .map(|child| {
            let (id, properties, _): Layout = child.try_into().unwrap();
            let text = |key: &str| {
                properties
                    .get(key)
                    .and_then(|value| String::try_from(value.clone()).ok())
            };
            let label = if text("type").as_deref() == Some("separator") {
                "-".to_owned()
            } else {
                text("label").unwrap_or_default()
            };
            (id, label)
        })
        .collect()
}

fn labels(rows: &[(i32, String)]) -> Vec<&str> {
    rows.iter().map(|(_, label)| label.as_str()).collect()
}

#[test]
fn the_tray_registers_its_icon_with_the_watcher() {
    run(async |bus| {
        let host = bus.connect().await;
        let watcher = Watcher::default();
        watcher.serve(&host).await;
        let mut child = Child::start(&bus);
        child.expect(&Event::Available(true)).await;
        let item = watcher.registered().await;
        assert!(item.starts_with("org.kde.StatusNotifierItem-"), "{item}");
        let text = |value: OwnedValue| String::try_from(value).unwrap();
        assert_eq!(text(property(&host, &item, "Id").await), "wineo-gui");
        assert_eq!(text(property(&host, &item, "Title").await), "wineo");
        assert_eq!(text(property(&host, &item, "IconName").await), "wineo-gui");
        let pixmaps: Vec<(i32, i32, Vec<u8>)> = property(&host, &item, "IconPixmap")
            .await
            .try_into()
            .unwrap();
        let sizes: Vec<(i32, i32, usize)> = pixmaps
            .iter()
            .map(|(width, height, data)| (*width, *height, data.len()))
            .collect();
        assert_eq!(sizes, [(32, 32, 32 * 32 * 4), (64, 64, 64 * 64 * 4)]);
    });
}

#[test]
fn a_watcher_that_starts_late_still_gets_the_icon() {
    run(async |bus| {
        let mut child = Child::start(&bus);
        child.expect(&Event::Available(true)).await;
        // Nothing to register with yet, so the item counts as offline.
        child.expect(&Event::Available(false)).await;
        let host = bus.connect().await;
        let watcher = Watcher::default();
        watcher.serve(&host).await;
        child.expect(&Event::Available(true)).await;
        let item = watcher.registered().await;
        assert!(item.starts_with("org.kde.StatusNotifierItem-"), "{item}");
    });
}

#[test]
fn the_menu_layout_reads_as_the_app_built_it_and_clicks_come_back() {
    run(async |bus| {
        let host = bus.connect().await;
        let watcher = Watcher::default();
        watcher.serve(&host).await;
        let mut child = Child::start(&bus);
        child.expect(&Event::Available(true)).await;
        let item = watcher.registered().await;
        let rows = layout(&host, &item).await;
        assert_eq!(
            labels(&rows),
            [
                "Open wineo",
                "Add to library...",
                "-",
                "Play Stardew Valley",
                "Play snake__game",
                "-",
                "Stop Hollow Knight",
                "-",
                "Quit",
            ]
        );

        let click = |label: &str| {
            let id = rows
                .iter()
                .find(|(_, shown)| shown == label)
                .map(|(id, _)| *id)
                .unwrap();
            let host = host.clone();
            let item = item.clone();
            async move {
                host.call_method(
                    Some(item.as_str()),
                    MENU_PATH,
                    Some(MENU_INTERFACE),
                    "Event",
                    &(id, "clicked", Value::from(0i32), 0u32),
                )
                .await
                .unwrap();
            }
        };
        click("Play Stardew Valley").await;
        child.expect(&Event::Play("stardew".into())).await;
        click("Stop Hollow Knight").await;
        child.expect(&Event::Stop("hollow".into())).await;
        click("Add to library...").await;
        child.expect(&Event::Add).await;
        click("Quit").await;
        child.expect(&Event::Quit).await;
        host.call_method(
            Some(item.as_str()),
            ITEM_PATH,
            Some(ITEM_INTERFACE),
            "Activate",
            &(0i32, 0i32),
        )
        .await
        .unwrap();
        child.expect(&Event::Open).await;

        // A new menu from the app reaches the host.
        child.swap_menu();
        let deadline = Instant::now() + PATIENCE;
        loop {
            let rows = layout(&host, &item).await;
            if labels(&rows) == ["Open wineo", "Add to library...", "-", "Quit"] {
                break;
            }
            assert!(Instant::now() < deadline, "{rows:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

#[test]
fn an_exit_takes_the_icon_off_the_bus() {
    run(async |bus| {
        let host = bus.connect().await;
        let watcher = Watcher::default();
        watcher.serve(&host).await;
        let mut child = Child::start(&bus);
        child.expect(&Event::Available(true)).await;
        let item = watcher.registered().await;
        let owner = zbus::fdo::DBusProxy::new(&host)
            .await
            .unwrap()
            .get_name_owner(item.as_str().try_into().unwrap())
            .await
            .unwrap();
        child.stop();
        child.expect_line(STOPPED).await;
        // The stop ends once the service has closed its connection, so the
        // bus drops the item while the process lives on.
        let proxy = zbus::fdo::DBusProxy::new(&host).await.unwrap();
        let deadline = Instant::now() + PATIENCE;
        while proxy.name_has_owner(owner.as_ref().into()).await.unwrap() {
            assert!(Instant::now() < deadline, "{item} stayed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(child.process.try_wait().unwrap().is_none());
    });
}
