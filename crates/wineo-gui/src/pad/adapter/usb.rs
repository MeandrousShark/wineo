//! The adapter over USB, through `nusb`, the only file that names it.
//!
//! Opening and claiming are plain calls (`.wait()`): they take a third of a
//! millisecond, and awaiting them would need `nusb`'s `tokio` feature, which
//! runs them through `spawn_blocking`, the kind of task quit waits on (see
//! `pad/reader.rs`). Reads, writes, and hotplug are `nusb`'s async forms, so
//! the reader is never parked in a blocking call and can always be dropped.

use std::ffi::OsStr;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::Subscription;
use iced::futures::{SinkExt, Stream, StreamExt};
use nusb::hotplug::{HotplugEvent, HotplugWatch};
use nusb::transfer::{
    Buffer, ControlOut, ControlType, In, Interrupt, Out, Recipient, TransferError,
};
use nusb::{DeviceId, DeviceInfo, Endpoint, ErrorKind, Interface, MaybeFuture};
use tokio::sync::{mpsc, watch};

use super::{Device, Hotplug, Look, Lost, PRODUCT, Refused, State, Transport, VENDOR};
use crate::Message;

const INTERFACE: u8 = 0;
const REPORTS: u8 = 0x81;
const COMMANDS: u8 = 0x02;
/// Starts the reports.
const START: u8 = 0x13;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
/// How deep under the interface an input device can sit: `usbhid` puts one
/// at `<interface>/<hid device>/input/inputN`.
const INPUT_DEPTH: u8 = 3;

/// Who has the interface, by sysfs alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sysfs {
    /// `usbhid` or no driver, and no input device: wineo may take it.
    Free,
    /// A program claimed it, which sysfs shows as the `usbfs` driver.
    Program,
    /// Any other driver, or an input device under it: gilrs reads its pads,
    /// and a take would disconnect a kernel driver.
    Kernel,
}

/// The interface's folder under the device's.
pub(crate) fn interface_dir(device: &Path) -> PathBuf {
    let name = device
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    device.join(format!("{name}:1.{INTERFACE}"))
}

/// Who has the interface at `interface`. `usbhid` gives the adapter only a
/// root-only hidraw node, so taking it from there loses nothing.
pub(crate) fn check(interface: &Path) -> Sysfs {
    let driver = std::fs::read_link(interface.join("driver"))
        .ok()
        .and_then(|link| link.file_name().map(OsStr::to_owned));
    match driver.as_deref().and_then(OsStr::to_str) {
        Some("usbfs") => Sysfs::Program,
        None | Some("usbhid") if !has_input(interface, INPUT_DEPTH) => Sysfs::Free,
        _ => Sysfs::Kernel,
    }
}

/// Whether an `input*` folder sits under `dir`, within `depth` more levels.
/// Links aren't followed, so `driver` and `subsystem` don't lead away.
fn has_input(dir: &Path, depth: u8) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_type().is_ok_and(|kind| kind.is_dir())
            && (entry.file_name().to_string_lossy().starts_with("input")
                || (depth > 0 && has_input(&entry.path(), depth - 1)))
    })
}

/// The bus through `nusb`.
pub(crate) struct Usb {
    watch: Option<HotplugWatch>,
    /// The adapter last found, to know when it leaves.
    adapter: Option<DeviceId>,
}

impl Usb {
    pub(crate) fn new() -> Self {
        Self {
            // Without a hotplug watch, the adapter is looked for on each want.
            watch: nusb::watch_devices().ok(),
            adapter: None,
        }
    }

    pub(crate) fn find() -> Option<DeviceInfo> {
        nusb::list_devices()
            .wait()
            .ok()?
            .find(|info| info.vendor_id() == VENDOR && info.product_id() == PRODUCT)
    }
}

impl Transport for Usb {
    type Found = DeviceInfo;
    type Device = Held;

    fn look(&mut self) -> Look<DeviceInfo> {
        let Some(info) = Self::find() else {
            self.adapter = None;
            return Look::Absent;
        };
        self.adapter = Some(info.id());
        match check(&interface_dir(info.sysfs_path())) {
            Sysfs::Free => Look::Found(info),
            Sysfs::Program => Look::Program,
            Sysfs::Kernel => Look::Kernel,
        }
    }

    fn claim(&mut self, info: DeviceInfo) -> Result<Held, Refused> {
        Held::claim(&info)
    }

    async fn hotplug(&mut self) -> Hotplug {
        let Some(watch) = self.watch.as_mut() else {
            return std::future::pending().await;
        };
        loop {
            match watch.next().await {
                Some(HotplugEvent::Connected(info))
                    if info.vendor_id() == VENDOR && info.product_id() == PRODUCT =>
                {
                    return Hotplug::Arrived;
                }
                Some(HotplugEvent::Disconnected(id)) if self.adapter == Some(id) => {
                    self.adapter = None;
                    return Hotplug::Left;
                }
                Some(_) => {}
                None => return std::future::pending().await,
            }
        }
    }
}

/// A claimed adapter. The endpoints come first, so they drop before the
/// interface, whose drop lets go and connects `usbhid` again.
pub(crate) struct Held {
    reports: Endpoint<Interrupt, In>,
    commands: Endpoint<Interrupt, Out>,
    interface: Interface,
}

impl Held {
    /// Opens and claims the adapter. Only `detach_and_claim_interface`,
    /// never a plain detach: it can't disconnect another program.
    pub(crate) fn claim(info: &DeviceInfo) -> Result<Self, Refused> {
        let device = info.open().wait().map_err(refused)?;
        let interface = device
            .detach_and_claim_interface(INTERFACE)
            .wait()
            .map_err(refused)?;
        let reports = interface
            .endpoint::<Interrupt, In>(REPORTS)
            .map_err(|_| Refused::Failed)?;
        let commands = interface
            .endpoint::<Interrupt, Out>(COMMANDS)
            .map_err(|_| Refused::Failed)?;
        Ok(Self {
            reports,
            commands,
            interface,
        })
    }
}

impl Device for Held {
    fn control(&mut self) -> impl Future<Output = Result<(), Lost>> + Send {
        let request = self.interface.control_out(
            ControlOut {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 11,
                value: 1,
                index: u16::from(INTERFACE),
                data: &[],
            },
            CONTROL_TIMEOUT,
        );
        async move { request.await.map_err(lost) }
    }

    async fn start(&mut self) -> Result<(), Lost> {
        self.commands.submit(Buffer::from(vec![START]));
        self.commands.next_complete().await.status.map_err(lost)
    }

    async fn read(&mut self) -> Result<Vec<u8>, Lost> {
        let size = self.reports.max_packet_size();
        if self.reports.pending() == 0 {
            self.reports.submit(Buffer::new(size));
        }
        let done = self.reports.next_complete().await;
        done.status.map_err(lost)?;
        // One stays in flight, so no report is missed between reads.
        self.reports.submit(Buffer::new(size));
        Ok(done.buffer.to_vec())
    }
}

fn refused(error: nusb::Error) -> Refused {
    match error.kind() {
        ErrorKind::Busy => Refused::Busy,
        ErrorKind::PermissionDenied => Refused::NoAccess,
        ErrorKind::Disconnected | ErrorKind::NotFound => Refused::Gone,
        _ => Refused::Failed,
    }
}

fn lost(error: TransferError) -> Lost {
    match error {
        TransferError::Disconnected => Lost::Unplugged,
        _ => Lost::Failed,
    }
}

/// The reader's subscription, which the app keeps only while both switches
/// are on and it isn't closing. Its identity never changes, so it runs
/// once for as long as it's subscribed; dropping it drops the device.
pub(crate) struct Link {
    pub(crate) wanted: watch::Receiver<bool>,
    pub(crate) state: watch::Sender<State>,
}

impl Hash for Link {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "gamecube-adapter".hash(state);
    }
}

pub(crate) fn subscription(link: Link) -> Subscription<Message> {
    Subscription::run_with(link, stream)
}

fn stream(link: &Link) -> impl Stream<Item = Message> + use<> {
    let wanted = link.wanted.clone();
    let state = link.state.clone();
    iced::stream::channel(16, async move |mut output| {
        let (send, mut receive) = mpsc::unbounded_channel();
        // Made here, not in `subscription`, so nothing watches USB until
        // the subscription starts.
        let reader = super::run(Usb::new(), wanted, state, send);
        let forward = async move {
            while let Some(report) = receive.recv().await {
                if output.send(Message::Adapter(report)).await.is_err() {
                    return;
                }
            }
        };
        tokio::pin!(reader, forward);
        tokio::select! {
            () = &mut reader => {}
            () = &mut forward => {}
        }
    })
}

#[cfg(test)]
mod tests;
