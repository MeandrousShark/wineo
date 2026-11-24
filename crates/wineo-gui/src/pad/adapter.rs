//! Nintendo's GameCube USB adapter (`057e:0337`): its report format, the
//! rules that turn reports into pad events, and when wineo holds it. Pure
//! like `pad.rs`: the USB side is behind `Transport`, and only `usb.rs`
//! names `nusb`, so tests drive all of this with a fake one.

use std::collections::BTreeSet;
use std::future::Future;
use std::ops::RangeInclusive;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::{Axis, PadEvent, PadId, PadInfo, Raw, STICK_PRESS, STICK_RELEASE, Stick};
use crate::keys::Direction;

pub(crate) mod usb;

pub(crate) const VENDOR: u16 = 0x057e;
pub(crate) const PRODUCT: u16 = 0x0337;

/// A report is a tag byte, then nine bytes for each of the four ports.
const REPORT_LEN: usize = 37;
const REPORT_TAG: u8 = 0x21;
const PORT_LEN: usize = 9;
pub(crate) const PORTS: usize = 4;
/// A port's status bits for a controller. Neither is an empty port; the
/// `0x04` bit beside them is the grey plug.
const WIRED: u8 = 0x10;
const WAVEBIRD: u8 = 0x20;

/// A stick's travel each way, and a trigger's full travel above its rest,
/// in report steps. Measured on a real adapter with a wired controller: the
/// triggers rest at 28 to 32 and reach 227 to 241 at the click (199 to 209
/// of travel), the C-stick 89 to 94 from its center and the main stick 93
/// to 105. The stick's 90 sits at the smallest reach, so every direction
/// gets to full travel or within a step of it.
const STICK_TRAVEL: f32 = 90.0;
const TRIGGER_TRAVEL: f32 = 200.0;
/// A stick's first report is its center only this close to the middle, so
/// a thumb on it or a report of zeros can't set a center that reads as a
/// direction held forever.
const CENTER_BAND: u8 = 32;
const MIDDLE: u8 = 128;
/// A trigger's first report is its rest only in this range, for the same
/// reason. The probe's pad rested at 34 and 29.
const TRIGGER_REST: RangeInclusive<u8> = 8..=64;
const DEFAULT_REST: u8 = 32;
/// The main stick reports a move once it has gone this much of its travel
/// since the last one.
const STEP: f32 = 1.0 / 20.0;

/// How often a busy adapter or a failed read is tried again while wanted.
pub(crate) const RETRY: Duration = Duration::from_secs(1);
/// How long a start waits for the reader to let go.
pub(crate) const LET_GO_WAIT: Duration = Duration::from_millis(250);

/// The second and third bytes of a port, read as one: A, B, X, Y, then the
/// D-pad's left, right, down, and up from the low bit, then Start, Z, R,
/// and L. Each goes out as the `Raw` whose Xbox label is the GameCube
/// button's own, so here `Raw` names a label, not a position, and
/// `label(GameCube, ..)` needs no swap.
const BUTTONS: [(u16, Raw); 10] = [
    (0x0001, Raw::South),
    (0x0002, Raw::East),
    (0x0004, Raw::West),
    (0x0008, Raw::North),
    (0x0010, Raw::DPadLeft),
    (0x0020, Raw::DPadRight),
    (0x0040, Raw::DPadDown),
    (0x0080, Raw::DPadUp),
    (0x0100, Raw::Start),
    // Z
    (0x0200, Raw::Select),
];
/// The triggers' clicks at the end of their travel. L and R press as
/// `LeftBumper` and `RightBumper`, from the click or the analog travel.
const R_CLICK: u16 = 0x0400;
const L_CLICK: u16 = 0x0800;
const TRIGGERS: [(u16, Raw); 2] = [(L_CLICK, Raw::LeftBumper), (R_CLICK, Raw::RightBumper)];

/// One port's controller in a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PortReport {
    pub(crate) buttons: u16,
    pub(crate) stick: [u8; 2],
    pub(crate) c_stick: [u8; 2],
    /// L, then R.
    pub(crate) triggers: [u8; 2],
}

/// Each port's controller, or `None` for an empty port. A report that is
/// short or doesn't start with the tag is no report.
pub(crate) fn parse(report: &[u8]) -> Option<[Option<PortReport>; PORTS]> {
    if report.len() < REPORT_LEN || report[0] != REPORT_TAG {
        return None;
    }
    Some(std::array::from_fn(|port| {
        let bytes = &report[1 + PORT_LEN * port..1 + PORT_LEN * (port + 1)];
        (bytes[0] & (WIRED | WAVEBIRD) != 0).then(|| PortReport {
            buttons: u16::from_le_bytes([bytes[1], bytes[2]]),
            stick: [bytes[3], bytes[4]],
            c_stick: [bytes[5], bytes[6]],
            triggers: [bytes[7], bytes[8]],
        })
    }))
}

/// The pad a port's controller is.
pub(crate) fn port_info(port: usize) -> PadInfo {
    PadInfo {
        name: format!("GameCube controller (port {})", port + 1),
        vendor: Some(VENDOR),
        product: Some(PRODUCT),
    }
}

/// A stick axis from -1 to 1 around `center`.
fn stick_travel(byte: u8, center: u8) -> f32 {
    ((f32::from(byte) - f32::from(center)) / STICK_TRAVEL).clamp(-1.0, 1.0)
}

/// A trigger from 0 at rest to 1 at full travel. Below rest is no travel.
fn trigger_travel(byte: u8, rest: u8) -> f32 {
    ((f32::from(byte) - f32::from(rest)) / TRIGGER_TRAVEL).clamp(0.0, 1.0)
}

/// A stick's center from its first report: there when both axes are
/// within the band, else the middle.
fn center(first: [u8; 2]) -> [u8; 2] {
    if first
        .iter()
        .all(|axis| axis.abs_diff(MIDDLE) <= CENTER_BAND)
    {
        first
    } else {
        [MIDDLE; 2]
    }
}

fn rest(first: u8) -> u8 {
    if TRIGGER_REST.contains(&first) {
        first
    } else {
        DEFAULT_REST
    }
}

/// Which of `Pads`' stick thresholds a value is past, each way.
fn sides(value: f32) -> [bool; 4] {
    [
        value >= STICK_PRESS,
        value >= STICK_RELEASE,
        value <= -STICK_RELEASE,
        value <= -STICK_PRESS,
    ]
}

/// Whether the main stick has changed enough since `last` to say so: a
/// twentieth of its travel, a threshold crossed, or back at rest. The
/// thresholds are in the rule so a slow release can't stop just short of
/// one and leave a direction repeating.
fn stick_moved(last: f32, value: f32) -> bool {
    (value - last).abs() >= STEP || sides(last) != sides(value) || (value == 0.0 && last != 0.0)
}

/// What one port's controller has said, and what `Pads` has been told.
#[derive(Debug, Clone)]
struct Port {
    stick_center: [u8; 2],
    /// The main stick's axes as last sent.
    stick_sent: [f32; 2],
    c_center: [u8; 2],
    /// The C-stick, settled like a stick into left, right, or neither.
    c_stick: Stick,
    rests: [u8; 2],
    /// Each trigger's travel past the press threshold, held until it drops
    /// below the release one.
    latched: [bool; 2],
    /// What `Pads` has been told is down.
    sent: BTreeSet<Raw>,
    /// Down in the first report, so it presses only once let go and
    /// pressed again.
    blocked: BTreeSet<Raw>,
}

impl Port {
    fn new(first: &PortReport) -> Self {
        Self {
            stick_center: center(first.stick),
            stick_sent: [0.0; 2],
            c_center: center(first.c_stick),
            c_stick: Stick::default(),
            rests: first.triggers.map(rest),
            latched: [false; 2],
            sent: BTreeSet::new(),
            blocked: BTreeSet::new(),
        }
    }

    /// Takes a report and adds what changed to `events`. On the first,
    /// whatever is down counts as held from before.
    fn update(&mut self, id: PadId, report: &PortReport, first: bool, events: &mut Vec<PadEvent>) {
        for (index, axis) in [Axis::X, Axis::Y].into_iter().enumerate() {
            let value = stick_travel(report.stick[index], self.stick_center[index]);
            if stick_moved(self.stick_sent[index], value) {
                self.stick_sent[index] = value;
                events.push(PadEvent::Stick(id, axis, value));
            }
        }
        let down = self.down(report);
        if first {
            self.blocked = down.clone();
        }
        self.blocked.retain(|raw| down.contains(raw));
        for raw in self.sent.difference(&down) {
            events.push(PadEvent::Released(id, *raw));
        }
        for raw in down.difference(&self.sent) {
            if !self.blocked.contains(raw) {
                events.push(PadEvent::Pressed(id, *raw));
            }
        }
        self.sent = down
            .into_iter()
            .filter(|raw| !self.blocked.contains(raw))
            .collect();
    }

    /// Every button down in `report`, the derived ones included.
    fn down(&mut self, report: &PortReport) -> BTreeSet<Raw> {
        let mut down: BTreeSet<Raw> = BUTTONS
            .into_iter()
            .filter(|(bit, _)| report.buttons & bit != 0)
            .map(|(_, raw)| raw)
            .collect();
        for (index, (click, raw)) in TRIGGERS.into_iter().enumerate() {
            let travel = trigger_travel(report.triggers[index], self.rests[index]);
            let threshold = if self.latched[index] {
                STICK_RELEASE
            } else {
                STICK_PRESS
            };
            self.latched[index] = travel >= threshold;
            // The click presses whatever the byte says, so a trigger with a
            // short analog range still pages.
            if self.latched[index] || report.buttons & click != 0 {
                down.insert(raw);
            }
        }
        match self.c_direction(report) {
            Some(Direction::Left) => {
                down.insert(Raw::LeftTrigger);
            }
            Some(Direction::Right) => {
                down.insert(Raw::RightTrigger);
            }
            _ => {}
        }
        down
    }

    /// The C-stick's direction, by `Pads`' stick rule. Up and down do
    /// nothing, so they never take over from a side still past the release
    /// threshold: wobbling across a diagonal can't press a side twice.
    fn c_direction(&mut self, report: &PortReport) -> Option<Direction> {
        self.c_stick.x = stick_travel(report.c_stick[0], self.c_center[0]);
        self.c_stick.y = stick_travel(report.c_stick[1], self.c_center[1]);
        let (was, now) = self.c_stick.settle();
        if let Some(side @ (Direction::Left | Direction::Right)) = was
            && matches!(now, Some(Direction::Up | Direction::Down))
        {
            let along = match side {
                Direction::Left => -self.c_stick.x,
                _ => self.c_stick.x,
            };
            if along >= STICK_RELEASE {
                self.c_stick.pressed = Some(side);
            }
        }
        self.c_stick.pressed
    }
}

/// Turns the adapter's reports into pad events, one per change. Each take
/// starts a new one, so the first report after it is every controller's
/// first.
#[derive(Debug, Default)]
pub(crate) struct Decoder {
    ports: [Option<Port>; PORTS],
}

impl Decoder {
    pub(crate) fn report(&mut self, bytes: &[u8]) -> Vec<PadEvent> {
        let mut events = Vec::new();
        let Some(reports) = parse(bytes) else {
            return events;
        };
        for (index, (port, report)) in self.ports.iter_mut().zip(reports).enumerate() {
            let id = PadId::Adapter(index as u8);
            match (port.as_mut(), report) {
                (None, Some(report)) => {
                    events.push(PadEvent::Connected(id, port_info(index)));
                    port.insert(Port::new(&report))
                        .update(id, &report, true, &mut events);
                }
                (Some(_), None) => {
                    *port = None;
                    events.push(PadEvent::Disconnected(id));
                }
                (Some(state), Some(report)) => state.update(id, &report, false, &mut events),
                (None, None) => {}
            }
        }
        events
    }

    /// Disconnects every port, for a let-go.
    pub(crate) fn release(&mut self) -> Vec<PadEvent> {
        self.ports
            .iter_mut()
            .enumerate()
            .filter_map(|(index, port)| {
                port.take()
                    .map(|_| PadEvent::Disconnected(PadId::Adapter(index as u8)))
            })
            .collect()
    }
}

/// Where the adapter stands, for the Settings list and the start's wait.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// None plugged in, or the reader isn't running.
    #[default]
    Absent,
    /// There, and wineo isn't holding it.
    Free,
    Held,
    /// Another program holds it.
    Busy,
    /// The user may not open it: no udev rule.
    NoAccess,
    /// A kernel driver uses it, so gilrs reads its pads.
    Kernel,
}

/// What the reader sends the app.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Report {
    Pad(PadEvent),
    State(State),
}

/// What a look at the bus found, without opening anything.
#[derive(Debug)]
pub(crate) enum Look<F> {
    Absent,
    /// A kernel driver other than `usbhid` has it, or makes input devices
    /// from it.
    Kernel,
    /// Another program has claimed it.
    Program,
    Found(F),
}

/// Why a claim failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    Busy,
    NoAccess,
    Gone,
    Failed,
}

/// Why a held adapter stopped answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lost {
    Unplugged,
    Failed,
}

/// The adapter coming or going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hotplug {
    Arrived,
    Left,
}

/// The bus: `usb.rs` in the app, a fake in tests. Looking and claiming are
/// plain calls; only hotplug waits.
pub(crate) trait Transport {
    type Found;
    type Device: Device;
    fn look(&mut self) -> Look<Self::Found>;
    fn claim(&mut self, found: Self::Found) -> Result<Self::Device, Refused>;
    /// The next time the adapter arrives or leaves.
    fn hotplug(&mut self) -> impl Future<Output = Hotplug> + Send;
}

/// A claimed adapter. Dropping it lets go.
pub(crate) trait Device: Send {
    /// The HID request third-party adapters need before they report.
    fn control(&mut self) -> impl Future<Output = Result<(), Lost>> + Send;
    /// The start byte, after which reports come.
    fn start(&mut self) -> impl Future<Output = Result<(), Lost>> + Send;
    fn read(&mut self) -> impl Future<Output = Result<Vec<u8>, Lost>> + Send;
}

/// Why a hold ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    NotWanted,
    Lost(Lost),
    /// The app is gone.
    Closed,
}

/// Holds the adapter while `wanted` says so, and sends what it reads and
/// where it stands to `out`, and the state to `state` too, for the start's
/// wait. Returns once the app is gone.
pub(crate) async fn run<T: Transport>(
    mut transport: T,
    mut wanted: watch::Receiver<bool>,
    state: watch::Sender<State>,
    out: mpsc::UnboundedSender<Report>,
) {
    let mut reader = Reader {
        state,
        out,
        current: None,
    };
    let mut want = *wanted.borrow_and_update();
    let mut try_now = want;
    let mut retry: Option<tokio::time::Instant> = None;
    if !want {
        reader.set(look_state(&transport.look()));
    }
    loop {
        if try_now && want {
            try_now = false;
            retry = None;
            match attempt(&mut transport) {
                Ok(device) => match reader.hold(device, &mut transport, &mut wanted).await {
                    Ended::Closed => return,
                    Ended::NotWanted => {
                        want = false;
                        reader.set(look_state(&transport.look()));
                    }
                    // A USB link reset (after a suspend) reads as a disconnect
                    // with the adapter still there, so look before believing it.
                    Ended::Lost(Lost::Unplugged) => {
                        let look = transport.look();
                        reader.set(look_state(&look));
                        if !matches!(look, Look::Absent) {
                            retry = Some(tokio::time::Instant::now() + RETRY);
                        }
                    }
                    Ended::Lost(Lost::Failed) => {
                        reader.set(look_state(&transport.look()));
                        retry = Some(tokio::time::Instant::now() + RETRY);
                    }
                },
                Err((state, again)) => {
                    reader.set(state);
                    if again {
                        retry = Some(tokio::time::Instant::now() + RETRY);
                    }
                }
            }
            if reader.out.is_closed() {
                return;
            }
            continue;
        }
        let retry_at = retry.unwrap_or_else(tokio::time::Instant::now);
        tokio::select! {
            changed = wanted.changed() => {
                if changed.is_err() {
                    return;
                }
                let now_wanted = *wanted.borrow_and_update();
                // A new want tries again, even after no access.
                try_now |= now_wanted && !want;
                want = now_wanted;
                if !want {
                    retry = None;
                }
            }
            hotplug = transport.hotplug() => match hotplug {
                // Gone, there is nothing to look at again until it's back.
                Hotplug::Left => {
                    retry = None;
                    reader.set(State::Absent);
                }
                Hotplug::Arrived if want => try_now = true,
                Hotplug::Arrived => reader.set(look_state(&transport.look())),
            },
            () = tokio::time::sleep_until(retry_at), if retry.is_some() && want => {
                try_now = true;
            }
        }
        if reader.out.is_closed() {
            return;
        }
    }
}

/// The state a look shows while nothing is held.
fn look_state<F>(look: &Look<F>) -> State {
    match look {
        Look::Absent => State::Absent,
        Look::Kernel => State::Kernel,
        Look::Program => State::Busy,
        Look::Found(_) => State::Free,
    }
}

/// One try at taking the adapter, or the state it left and whether to try
/// again in a second.
fn attempt<T: Transport>(transport: &mut T) -> Result<T::Device, (State, bool)> {
    match transport.look() {
        Look::Absent => Err((State::Absent, false)),
        Look::Kernel => Err((State::Kernel, false)),
        Look::Program => Err((State::Busy, true)),
        Look::Found(found) => transport.claim(found).map_err(|refused| match refused {
            Refused::Busy => (State::Busy, true),
            // Asking again can't help until a replug or the next want.
            Refused::NoAccess => (State::NoAccess, false),
            Refused::Gone => (State::Absent, false),
            Refused::Failed => (State::Free, true),
        }),
    }
}

struct Reader {
    state: watch::Sender<State>,
    out: mpsc::UnboundedSender<Report>,
    current: Option<State>,
}

impl Reader {
    /// Says where the adapter stands, only when that changed.
    fn set(&mut self, state: State) {
        if self.current == Some(state) {
            return;
        }
        self.current = Some(state);
        self.state.send_replace(state);
        let _ = self.out.send(Report::State(state));
    }

    /// Reads `device` until it isn't wanted or stops answering, then lets
    /// go and disconnects its pads.
    async fn hold<T: Transport>(
        &mut self,
        mut device: T::Device,
        transport: &mut T,
        wanted: &mut watch::Receiver<bool>,
    ) -> Ended {
        self.set(State::Held);
        let mut decoder = Decoder::default();
        let ended = {
            let session = session(&mut device, &mut decoder, &self.out);
            tokio::pin!(session);
            loop {
                tokio::select! {
                    ended = &mut session => break ended,
                    changed = wanted.changed() => {
                        if changed.is_err() {
                            break Ended::Closed;
                        }
                        if !*wanted.borrow_and_update() {
                            break Ended::NotWanted;
                        }
                    }
                    hotplug = transport.hotplug() => {
                        if hotplug == Hotplug::Left {
                            break Ended::Lost(Lost::Unplugged);
                        }
                    }
                }
            }
        };
        drop(device);
        for event in decoder.release() {
            let _ = self.out.send(Report::Pad(event));
        }
        ended
    }
}

/// Starts the adapter's reports and passes on what they say.
async fn session<D: Device>(
    device: &mut D,
    decoder: &mut Decoder,
    out: &mpsc::UnboundedSender<Report>,
) -> Ended {
    // Nyko's and some other adapters need it; Mayflash's refuse it, which
    // is harmless.
    let _ = device.control().await;
    if let Err(lost) = device.start().await {
        return Ended::Lost(lost);
    }
    loop {
        match device.read().await {
            Ok(bytes) => {
                for event in decoder.report(&bytes) {
                    if out.send(Report::Pad(event)).is_err() {
                        return Ended::Closed;
                    }
                }
            }
            Err(lost) => return Ended::Lost(lost),
        }
    }
}

/// The app's side of the reader.
#[derive(Debug)]
pub(crate) struct Control {
    /// Whether the app wants the adapter held. The reader follows it, and a
    /// new reader starts from it.
    pub(crate) wanted: watch::Sender<bool>,
    /// Where the reader says the adapter stands, for the start's wait.
    pub(crate) state: watch::Sender<State>,
    /// The same as the app last heard it, for Settings.
    pub(crate) shown: State,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            wanted: watch::Sender::new(false),
            state: watch::Sender::new(State::Absent),
            shown: State::Absent,
        }
    }
}

impl Control {
    pub(crate) fn link(&self) -> usb::Link {
        usb::Link {
            wanted: self.wanted.subscribe(),
            state: self.state.clone(),
        }
    }

    /// What a start waits on: the reader's state while it holds the
    /// adapter, else nothing.
    pub(crate) fn held(&self) -> Option<watch::Receiver<State>> {
        (*self.state.borrow() == State::Held).then(|| self.state.subscribe())
    }
}

/// A start's wait for the let-go: until `held` says the adapter isn't held,
/// for at most `LET_GO_WAIT`, and none at all with nothing held.
pub(crate) async fn let_go(held: Option<watch::Receiver<State>>) {
    let Some(mut state) = held else {
        return;
    };
    let _ = tokio::time::timeout(LET_GO_WAIT, state.wait_for(|state| *state != State::Held)).await;
}

#[cfg(test)]
mod tests;
