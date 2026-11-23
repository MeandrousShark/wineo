//! The sysfs check on made-up trees, and one `--ignored` test that takes a
//! real adapter with someone at the pad.

use std::fs;
use std::os::unix::fs::symlink;

use super::*;

/// An interface folder bound to `driver`, if any.
fn interface(driver: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let interface = root.path().join("devices/1-4/1-4:1.0");
    fs::create_dir_all(&interface).unwrap();
    if let Some(driver) = driver {
        symlink(
            format!("../../../bus/usb/drivers/{driver}"),
            interface.join("driver"),
        )
        .unwrap();
    }
    // What `usbhid` makes for the adapter: a hidraw node and no input device.
    fs::create_dir_all(interface.join("0003:057E:0337.0001/hidraw/hidraw3")).unwrap();
    (root, interface)
}

#[test]
fn usbhid_or_no_driver_with_no_input_device_is_free() {
    let (_root, dir) = interface(Some("usbhid"));
    assert_eq!(check(&dir), Sysfs::Free);
    // After Dolphin, or a take that died with wineo, no driver is bound.
    let (_root, dir) = interface(None);
    assert_eq!(check(&dir), Sysfs::Free);
}

#[test]
fn a_programs_claim_shows_as_usbfs() {
    let (_root, dir) = interface(Some("usbfs"));
    assert_eq!(check(&dir), Sysfs::Program);
}

#[test]
fn another_driver_or_an_input_device_is_the_kernels() {
    let (_root, dir) = interface(Some("gcadapter_oc"));
    assert_eq!(check(&dir), Sysfs::Kernel);

    // `usbhid` making gamepads from it, as a later kernel might.
    let (_root, dir) = interface(Some("usbhid"));
    fs::create_dir_all(dir.join("0003:057E:0337.0001/input/input23")).unwrap();
    assert_eq!(check(&dir), Sysfs::Kernel);
    let (_root, dir) = interface(None);
    fs::create_dir_all(dir.join("0003:057E:0337.0001/input/input23/event5")).unwrap();
    assert_eq!(check(&dir), Sysfs::Kernel);
}

#[test]
fn links_under_the_interface_are_not_followed() {
    let (root, dir) = interface(Some("usbhid"));
    // A link that leads to an input device elsewhere, like `subsystem`.
    let elsewhere = root.path().join("class/input/input9");
    fs::create_dir_all(&elsewhere).unwrap();
    symlink(root.path().join("class"), dir.join("subsystem")).unwrap();
    symlink(&elsewhere, dir.join("input9")).unwrap();
    assert_eq!(check(&dir), Sysfs::Free);
}

#[test]
fn the_interface_is_under_the_devices_folder() {
    assert_eq!(
        interface_dir(Path::new("/sys/bus/usb/devices/1-4")),
        Path::new("/sys/bus/usb/devices/1-4/1-4:1.0")
    );
}

#[test]
fn only_an_unplug_is_an_unplug() {
    assert_eq!(lost(TransferError::Disconnected), Lost::Unplugged);
    for error in [
        TransferError::Cancelled,
        TransferError::Stall,
        TransferError::Fault,
        TransferError::Unknown(5),
    ] {
        assert_eq!(lost(error), Lost::Failed, "{error:?}");
    }
}

/// How long each prompt waits before giving up.
const PROMPT: Duration = Duration::from_secs(20);
/// How long a read may take before the adapter counts as silent.
const READ: Duration = Duration::from_secs(2);
const PORT: usize = 0;

/// What one prompt saw of port 1.
#[derive(Debug, Default)]
struct Ranges {
    min: [u8; 6],
    max: [u8; 6],
}

impl Ranges {
    fn new() -> Self {
        Self {
            min: [u8::MAX; 6],
            max: [0; 6],
        }
    }

    fn add(&mut self, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            self.min[index] = self.min[index].min(*byte);
            self.max[index] = self.max[index].max(*byte);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Port 1's nine bytes, or `None` for a report that isn't one.
fn port(report: &[u8]) -> Option<&[u8]> {
    (report.len() >= 37 && report[0] == 0x21).then(|| &report[1 + 9 * PORT..10 + 9 * PORT])
}

fn controller(bytes: &[u8]) -> bool {
    bytes[0] & 0x30 != 0
}

async fn next(held: &mut Held) -> Vec<u8> {
    match tokio::time::timeout(READ, held.read()).await {
        Ok(Ok(report)) => report,
        Ok(Err(lost)) => panic!("the read failed: {lost:?}"),
        Err(_) => panic!("no report for {READ:?}"),
    }
}

fn say(prompt: &str) {
    println!();
    println!(">>> {prompt} (gives up after {} s)", PROMPT.as_secs());
}

/// Reads until `done` says so or the prompt gives up, and returns whether
/// it finished.
async fn until(held: &mut Held, mut done: impl FnMut(&[u8]) -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + PROMPT;
    while tokio::time::Instant::now() < deadline {
        if done(&next(held).await) {
            return true;
        }
    }
    println!("    (gave up)");
    false
}

/// One trigger: its rest, the byte the click bit sets at, and the most a
/// full press reaches. `at` is 7 for L and 8 for R in the port's bytes.
async fn trigger(held: &mut Held, name: &str, at: usize, click: u8) {
    say(&format!(
        "Press {name} slowly all the way to the click, hold it a moment, then let go"
    ));
    let mut rest = None;
    let mut clicked_at = None;
    let mut most = 0u8;
    let mut released = false;
    let finished = until(held, |report| {
        let Some(bytes) = port(report) else {
            return false;
        };
        let byte = bytes[at];
        rest.get_or_insert(byte);
        most = most.max(byte);
        let click_down = bytes[2] & click != 0;
        if click_down && clicked_at.is_none() {
            clicked_at = Some(byte);
        }
        released |= clicked_at.is_some() && !click_down;
        released && byte <= rest.unwrap_or(0).saturating_add(10)
    })
    .await;
    println!(
        "- {name}: rest {rest:?}, click bit set at {clicked_at:?}, most on a full press {most}{}",
        if finished { "" } else { " (prompt gave up)" }
    );
}

/// A stick's reach each way from its first report. `x` is 0 for the main
/// stick and 2 for the C-stick, from the port's first stick byte.
async fn stick(held: &mut Held, name: &str, x: usize, center: [u8; 2]) {
    say(&format!(
        "Push the {name} all the way left, right, up, and down, then around the rim twice, then let go"
    ));
    let mut ranges = Ranges::new();
    let deadline = tokio::time::Instant::now() + PROMPT;
    while tokio::time::Instant::now() < deadline {
        if let Some(bytes) = port(&next(held).await) {
            ranges.add(&bytes[3..9]);
        }
    }
    let y = x + 1;
    println!(
        "- {name}: center {},{}; x {}..{} (left {}, right {}); y {}..{} (down {}, up {})",
        center[0],
        center[1],
        ranges.min[x],
        ranges.max[x],
        center[0].saturating_sub(ranges.min[x]),
        ranges.max[x].saturating_sub(center[0]),
        ranges.min[y],
        ranges.max[y],
        center[1].saturating_sub(ranges.min[y]),
        ranges.max[y].saturating_sub(center[1]),
    );
}

async fn record() {
    let info = Usb::find().expect("no GameCube adapter (057e:0337) is plugged in");
    let dir = interface_dir(info.sysfs_path());
    let driver = fs::read_link(dir.join("driver")).ok();
    println!("GameCube adapter recording, plan 0072 step 3");
    println!("- interface {}, driver {driver:?}", dir.display());
    assert_eq!(
        check(&dir),
        Sysfs::Free,
        "the adapter is in use by a program or a kernel driver; not taking it"
    );
    println!();
    println!(">>> Hands off the controller in port 1 until the next prompt");
    std::thread::sleep(Duration::from_secs(3));
    let mut held = Held::claim(&info).expect("the claim failed");
    println!("- control request: {:?}", held.control().await);
    println!("- start byte: {:?}", held.start().await);

    let mut first = Vec::new();
    for _ in 0..10 {
        first.push(next(&mut held).await);
    }
    println!("- first reports after the take:");
    for report in &first {
        println!("  `{}`", hex(report));
    }
    let Some(rest) = first.first().and_then(|report| port(report)) else {
        panic!("the first report isn't one: {}", hex(&first[0]));
    };
    assert!(
        controller(rest),
        "no controller in port 1; plug one in and run again"
    );
    let main_center = [rest[3], rest[4]];
    let c_center = [rest[5], rest[6]];

    trigger(&mut held, "L", 7, 0x08).await;
    trigger(&mut held, "R", 8, 0x04).await;
    stick(&mut held, "C-stick", 2, c_center).await;
    stick(&mut held, "main stick", 0, main_center).await;

    say("Unplug the controller from port 1, wait a second, and plug it back in");
    let mut empty_status = None;
    let finished = until(&mut held, |report| {
        let Some(bytes) = port(report) else {
            return false;
        };
        if !controller(bytes) {
            empty_status.get_or_insert(bytes[0]);
        }
        empty_status.is_some() && controller(bytes)
    })
    .await;
    if finished {
        let mut after = Vec::new();
        for _ in 0..10 {
            after.push(next(&mut held).await);
        }
        println!("- port 1's status while empty: {empty_status:02x?}");
        println!("- first reports after the controller came back:");
        // The report that showed it back was read inside `until`, so these
        // start one later.
        for report in &after {
            println!("  `{}`", hex(report));
        }
    } else {
        println!("- unplug and replug: not seen");
    }

    drop(held);
    println!();
    println!("Let go of the adapter.");
    std::thread::sleep(Duration::from_millis(1500));
    println!(
        "- driver 1.5 s later: {:?}",
        fs::read_link(dir.join("driver")).ok()
    );
}

#[test]
#[ignore = "takes a real GameCube adapter; needs WINEO_TEST_GC_ADAPTER=1 and someone at the pad"]
fn a_real_adapter_records_its_reports_and_ranges() {
    if std::env::var_os("WINEO_TEST_GC_ADAPTER").as_deref() != Some(OsStr::new("1")) {
        eprintln!("WINEO_TEST_GC_ADAPTER=1 isn't set, so the adapter is left alone");
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(record());
}
