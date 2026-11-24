//! The adapter on made-up reports and a fake bus. Nothing here opens USB.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::Notify;
use wineo_core::library::PadButton;

use super::*;
use crate::pad::{Input, Pads, Style};

/// Two of the probe's first reports from the maintainer's adapter, with one
/// wired controller at rest in port 1 and the grey plug in
/// (`gc-probe/reports-1.txt`).
const PROBE_FIRST: &str =
    "2114000083838484221c040000000000000000040000000000000000040000000000000000";
const PROBE_SECOND: &str =
    "2114000083838484221d040000000000000000040000000000000000040000000000000000";

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

/// A controller's bytes in a made-up report.
#[derive(Debug, Clone, Copy)]
struct Pad {
    status: u8,
    buttons: u16,
    stick: [u8; 2],
    c: [u8; 2],
    triggers: [u8; 2],
}

/// The probe's pad at rest.
const REST: Pad = Pad {
    status: 0x14,
    buttons: 0,
    stick: [131, 131],
    c: [132, 132],
    triggers: [34, 29],
};

const A: u16 = 0x0001;

impl Pad {
    fn buttons(self, buttons: u16) -> Self {
        Self { buttons, ..self }
    }
    fn stick(self, x: u8, y: u8) -> Self {
        Self {
            stick: [x, y],
            ..self
        }
    }
    fn c(self, x: u8, y: u8) -> Self {
        Self { c: [x, y], ..self }
    }
    fn l(self, l: u8) -> Self {
        Self {
            triggers: [l, self.triggers[1]],
            ..self
        }
    }
    fn r(self, r: u8) -> Self {
        Self {
            triggers: [self.triggers[0], r],
            ..self
        }
    }
}

/// A report with `ports` filled in; an empty port has only the grey plug's
/// bit, as on the probe.
fn report(ports: [Option<Pad>; PORTS]) -> Vec<u8> {
    let mut bytes = vec![REPORT_TAG];
    for port in ports {
        match port {
            Some(pad) => {
                let [low, high] = pad.buttons.to_le_bytes();
                bytes.extend([pad.status, low, high]);
                bytes.extend(pad.stick);
                bytes.extend(pad.c);
                bytes.extend(pad.triggers);
            }
            None => bytes.extend([0x04, 0, 0, 0, 0, 0, 0, 0, 0]),
        }
    }
    bytes
}

fn one(pad: Pad) -> Vec<u8> {
    report([Some(pad), None, None, None])
}

const PORT1: PadId = PadId::Adapter(0);

/// A decoder that has seen `first` from port 1, and what that sent.
fn taken(first: Pad) -> (Decoder, Vec<PadEvent>) {
    let mut decoder = Decoder::default();
    let events = decoder.report(&one(first));
    (decoder, events)
}

/// The decoder after a resting first report.
fn rested() -> Decoder {
    taken(REST).0
}

fn feed(decoder: &mut Decoder, pad: Pad) -> Vec<PadEvent> {
    decoder.report(&one(pad))
}

fn pressed(raw: Raw) -> Vec<PadEvent> {
    vec![PadEvent::Pressed(PORT1, raw)]
}

fn released(raw: Raw) -> Vec<PadEvent> {
    vec![PadEvent::Released(PORT1, raw)]
}

#[test]
fn the_parser_reads_the_probes_real_reports() {
    for text in [PROBE_FIRST, PROBE_SECOND] {
        let bytes = hex(text);
        assert_eq!(bytes.len(), REPORT_LEN);
        let ports = parse(&bytes).unwrap();
        let port = ports[0].unwrap();
        assert_eq!(port.buttons, 0);
        assert_eq!(port.stick, [131, 131]);
        assert_eq!(port.c_stick, [132, 132]);
        assert_eq!(port.triggers[0], 34);
        // The grey plug's bit alone is no controller.
        assert_eq!(ports[1..], [None, None, None]);
    }
    let mut decoder = Decoder::default();
    assert_eq!(
        decoder.report(&hex(PROBE_FIRST)),
        [PadEvent::Connected(PORT1, port_info(0))]
    );
    // R's byte moving 28 to 29 at rest sends nothing.
    assert!(decoder.report(&hex(PROBE_SECOND)).is_empty());
}

#[test]
fn a_short_report_or_a_wrong_first_byte_is_no_report() {
    let bytes = hex(PROBE_FIRST);
    assert!(parse(&bytes[..REPORT_LEN - 1]).is_none());
    assert!(parse(&[]).is_none());
    let mut wrong = bytes.clone();
    wrong[0] = 0x22;
    assert!(parse(&wrong).is_none());
    let mut decoder = Decoder::default();
    assert!(decoder.report(&wrong).is_empty());
    assert!(decoder.report(&bytes[..20]).is_empty());
}

#[test]
fn each_port_comes_and_goes_wired_or_wavebird() {
    for port in 0..PORTS {
        for status in [0x10, 0x14, 0x20, 0x24] {
            let mut ports = [None; PORTS];
            ports[port] = Some(Pad { status, ..REST });
            let id = PadId::Adapter(port as u8);
            let mut decoder = Decoder::default();
            assert_eq!(
                decoder.report(&report(ports)),
                [PadEvent::Connected(id, port_info(port))],
                "port {port} status {status:#x}"
            );
            assert_eq!(
                decoder.report(&report([None; PORTS])),
                [PadEvent::Disconnected(id)]
            );
        }
    }
    assert_eq!(port_info(1).name, "GameCube controller (port 2)");
}

#[test]
fn each_button_bit_is_its_own_label() {
    for (bit, raw) in [
        (0x0001, Raw::South),
        (0x0002, Raw::East),
        (0x0004, Raw::West),
        (0x0008, Raw::North),
        (0x0010, Raw::DPadLeft),
        (0x0020, Raw::DPadRight),
        (0x0040, Raw::DPadDown),
        (0x0080, Raw::DPadUp),
        (0x0100, Raw::Start),
        (0x0200, Raw::Select),
        (0x0400, Raw::RightBumper),
        (0x0800, Raw::LeftBumper),
    ] {
        let mut decoder = rested();
        assert_eq!(
            feed(&mut decoder, REST.buttons(bit)),
            pressed(raw),
            "{bit:#x}"
        );
        assert_eq!(feed(&mut decoder, REST), released(raw), "{bit:#x}");
    }
    // The labels are the GameCube's own: no Nintendo swap.
    let label = |raw| crate::pad::label(Style::GameCube, raw);
    assert_eq!(label(Raw::South), Some(PadButton::A));
    assert_eq!(label(Raw::East), Some(PadButton::B));
    assert_eq!(label(Raw::West), Some(PadButton::X));
    assert_eq!(label(Raw::North), Some(PadButton::Y));
    assert_eq!(label(Raw::Select), Some(PadButton::View));
}

#[test]
fn an_off_center_rest_reads_as_zero() {
    let resting = REST.stick(150, 110);
    let (mut decoder, events) = taken(resting);
    assert_eq!(events, [PadEvent::Connected(PORT1, port_info(0))]);
    assert!(feed(&mut decoder, resting).is_empty());
    // Half its travel from where it rested, not from 128.
    assert_eq!(
        feed(&mut decoder, REST.stick(195, 110)),
        [PadEvent::Stick(PORT1, Axis::X, 0.5)]
    );
}

#[test]
fn a_first_report_outside_the_band_or_of_zeros_centers_on_128() {
    let (mut decoder, events) = taken(REST.stick(200, 128));
    assert_eq!(events[1..], [PadEvent::Stick(PORT1, Axis::X, 72.0 / 90.0)]);
    assert_eq!(
        feed(&mut decoder, REST.stick(128, 128)),
        [PadEvent::Stick(PORT1, Axis::X, 0.0)]
    );

    let (mut decoder, events) = decoder_zeros();
    assert_eq!(
        events[1..],
        [
            PadEvent::Stick(PORT1, Axis::X, -1.0),
            PadEvent::Stick(PORT1, Axis::Y, -1.0)
        ]
    );
    assert_eq!(
        feed(&mut decoder, REST.stick(128, 128)),
        [
            PadEvent::Stick(PORT1, Axis::X, 0.0),
            PadEvent::Stick(PORT1, Axis::Y, 0.0)
        ]
    );
}

#[test]
fn the_center_band_takes_32_from_128_and_not_33() {
    for x in [160, 96] {
        let (_, events) = taken(REST.stick(x, 128));
        assert_eq!(events, [PadEvent::Connected(PORT1, port_info(0))], "{x}");
    }
    for (x, value) in [(161, 33.0 / 90.0), (95, -33.0 / 90.0)] {
        let (_, events) = taken(REST.stick(x, 128));
        assert_eq!(events[1..], [PadEvent::Stick(PORT1, Axis::X, value)], "{x}");
    }
    // The same on y.
    let (_, events) = taken(REST.stick(128, 160));
    assert_eq!(events, [PadEvent::Connected(PORT1, port_info(0))]);
    let (_, events) = taken(REST.stick(128, 95));
    assert_eq!(events[1..], [PadEvent::Stick(PORT1, Axis::Y, -33.0 / 90.0)]);
}

fn decoder_zeros() -> (Decoder, Vec<PadEvent>) {
    taken(Pad {
        status: 0x14,
        buttons: 0,
        stick: [0, 0],
        c: [0, 0],
        triggers: [0, 0],
    })
}

#[test]
fn full_travel_reaches_one_and_no_further() {
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.stick(255, 0)),
        [
            PadEvent::Stick(PORT1, Axis::X, 1.0),
            PadEvent::Stick(PORT1, Axis::Y, -1.0)
        ]
    );
    // 131 + 90 is full travel already.
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.stick(221, 131)),
        [PadEvent::Stick(PORT1, Axis::X, 1.0)]
    );
}

#[test]
fn the_c_stick_presses_lt_left_and_rt_right_at_the_thresholds() {
    let mut decoder = rested();
    // 132 rests; half of 90 is 45.
    assert!(feed(&mut decoder, REST.c(88, 132)).is_empty());
    assert_eq!(
        feed(&mut decoder, REST.c(87, 132)),
        pressed(Raw::LeftTrigger)
    );
    // Kept down to 32 steps out, past the release threshold of 31.5.
    assert!(feed(&mut decoder, REST.c(100, 132)).is_empty());
    assert_eq!(
        feed(&mut decoder, REST.c(101, 132)),
        released(Raw::LeftTrigger)
    );
    assert_eq!(
        feed(&mut decoder, REST.c(177, 132)),
        pressed(Raw::RightTrigger)
    );
    assert_eq!(feed(&mut decoder, REST), released(Raw::RightTrigger));
}

#[test]
fn the_c_stick_up_down_and_a_mostly_vertical_diagonal_press_nothing() {
    let mut decoder = rested();
    for c in [[132, 255], [132, 0], [82, 200], [190, 60]] {
        assert!(feed(&mut decoder, REST.c(c[0], c[1])).is_empty(), "{c:?}");
    }
    assert!(feed(&mut decoder, REST).is_empty());
}

#[test]
fn a_held_c_stick_sends_one_press() {
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.c(40, 132)),
        pressed(Raw::LeftTrigger)
    );
    for _ in 0..20 {
        assert!(feed(&mut decoder, REST.c(40, 132)).is_empty());
    }
}

#[test]
fn wobbling_the_c_stick_across_a_diagonal_sends_no_second_press() {
    let mut decoder = rested();
    // Up and left, wobbling which is the stronger.
    assert_eq!(
        feed(&mut decoder, REST.c(70, 187)),
        pressed(Raw::LeftTrigger)
    );
    for c in [[77, 194], [70, 187], [77, 194], [72, 190]] {
        assert!(feed(&mut decoder, REST.c(c[0], c[1])).is_empty(), "{c:?}");
    }
    // Straight up from there lets go, since left fell under the release.
    assert_eq!(
        feed(&mut decoder, REST.c(132, 255)),
        released(Raw::LeftTrigger)
    );
}

#[test]
fn a_side_still_past_the_release_keeps_its_press_while_up_dominates() {
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.c(70, 187)),
        pressed(Raw::LeftTrigger)
    );
    // Left at 36 of 90, 0.4, between the thresholds; up at 0.64 is stronger.
    assert!(feed(&mut decoder, REST.c(96, 190)).is_empty());
    // At 31 of 90, under the release, left lets go.
    assert_eq!(
        feed(&mut decoder, REST.c(101, 190)),
        released(Raw::LeftTrigger)
    );
}

#[test]
fn a_c_stick_tie_goes_sideways() {
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.c(182, 182)),
        pressed(Raw::RightTrigger)
    );
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.c(82, 82)),
        pressed(Raw::LeftTrigger)
    );
}

#[test]
fn the_c_sticks_center_follows_the_band_rule() {
    // Inside the band, its rest is its center.
    let (mut decoder, _) = taken(REST.c(150, 132));
    assert!(feed(&mut decoder, REST.c(190, 132)).is_empty());
    assert_eq!(
        feed(&mut decoder, REST.c(195, 132)),
        pressed(Raw::RightTrigger)
    );
    // Outside it, 128 is: 170 is under half, 173 is half.
    let (mut decoder, events) = taken(REST.c(170, 132));
    assert_eq!(events, [PadEvent::Connected(PORT1, port_info(0))]);
    assert_eq!(
        feed(&mut decoder, REST.c(173, 132)),
        pressed(Raw::RightTrigger)
    );
}

#[test]
fn a_trigger_presses_halfway_and_the_release_threshold_lets_go() {
    let mut decoder = rested();
    // L rests at 34: half of 140 is 70.
    assert!(feed(&mut decoder, REST.l(103)).is_empty());
    assert_eq!(feed(&mut decoder, REST.l(104)), pressed(Raw::LeftBumper));
    assert!(feed(&mut decoder, REST.l(83)).is_empty());
    assert_eq!(feed(&mut decoder, REST.l(82)), released(Raw::LeftBumper));
    // R rests at 29.
    assert_eq!(feed(&mut decoder, REST.r(99)), pressed(Raw::RightBumper));
    assert_eq!(feed(&mut decoder, REST), released(Raw::RightBumper));
}

#[test]
fn the_click_presses_with_the_byte_at_rest() {
    let mut decoder = rested();
    assert_eq!(
        feed(&mut decoder, REST.buttons(L_CLICK)),
        pressed(Raw::LeftBumper)
    );
    assert_eq!(feed(&mut decoder, REST), released(Raw::LeftBumper));
    assert_eq!(
        feed(&mut decoder, REST.buttons(R_CLICK)),
        pressed(Raw::RightBumper)
    );
}

#[test]
fn a_first_trigger_byte_above_64_or_below_8_rests_at_32() {
    // Rest 70 would need 140 to press; at 32, 102 is half.
    let (mut decoder, _) = taken(REST.l(70));
    assert!(feed(&mut decoder, REST.l(101)).is_empty());
    assert_eq!(feed(&mut decoder, REST.l(102)), pressed(Raw::LeftBumper));
    // Rest 5 would press at 75; at 32 it takes 102.
    let (mut decoder, _) = taken(REST.l(5));
    assert!(feed(&mut decoder, REST.l(75)).is_empty());
    assert_eq!(feed(&mut decoder, REST.l(102)), pressed(Raw::LeftBumper));
    // The edges of the range are a rest.
    let (mut decoder, _) = taken(REST.l(64));
    assert!(feed(&mut decoder, REST.l(133)).is_empty());
    assert_eq!(feed(&mut decoder, REST.l(134)), pressed(Raw::LeftBumper));
    let (mut decoder, _) = taken(REST.l(8));
    assert_eq!(feed(&mut decoder, REST.l(78)), pressed(Raw::LeftBumper));
}

#[test]
fn a_byte_below_rest_is_no_travel() {
    let (mut decoder, _) = taken(REST.l(64));
    assert!(feed(&mut decoder, REST.l(0)).is_empty());
    assert_eq!(feed(&mut decoder, REST.l(134)), pressed(Raw::LeftBumper));
    // Far below rest lets go, rather than reading as travel the other way.
    assert_eq!(feed(&mut decoder, REST.l(0)), released(Raw::LeftBumper));
}

#[test]
fn a_trigger_wobbling_at_or_between_the_thresholds_is_one_press() {
    let mut decoder = rested();
    let mut events = Vec::new();
    for l in [103, 104, 103, 104, 103, 90, 84, 99, 84, 104, 83] {
        events.extend(feed(&mut decoder, REST.l(l)));
    }
    assert_eq!(events, pressed(Raw::LeftBumper));
    // Between the thresholds from below never presses.
    let mut decoder = rested();
    for l in [83, 90, 103, 90, 85] {
        assert!(feed(&mut decoder, REST.l(l)).is_empty());
    }
}

#[test]
fn analog_and_click_together_are_one_press_and_one_release() {
    let one_press = [
        PadEvent::Pressed(PORT1, Raw::LeftBumper),
        PadEvent::Released(PORT1, Raw::LeftBumper),
    ];
    let mut decoder = rested();
    let mut events = Vec::new();
    for pad in [
        REST.l(104),
        REST.l(200),
        REST.l(255).buttons(L_CLICK),
        REST.l(60).buttons(L_CLICK),
        REST.buttons(L_CLICK),
        REST,
    ] {
        events.extend(feed(&mut decoder, pad));
    }
    assert_eq!(events, one_press);
    // The click first, then the travel.
    let mut decoder = rested();
    let mut events = Vec::new();
    for pad in [
        REST.buttons(L_CLICK),
        REST.l(255).buttons(L_CLICK),
        REST.l(255),
        REST,
    ] {
        events.extend(feed(&mut decoder, pad));
    }
    assert_eq!(events, one_press);
}

/// `held` down in the first report presses nothing, staying or going, and
/// presses `raw` once pressed again.
fn held_at_first_presses_after_a_release(held: Pad, raw: Raw) {
    let (mut decoder, events) = taken(held);
    assert_eq!(
        events,
        [PadEvent::Connected(PORT1, port_info(0))],
        "{raw:?}"
    );
    assert!(feed(&mut decoder, held).is_empty(), "{raw:?}");
    assert!(feed(&mut decoder, REST).is_empty(), "{raw:?}");
    assert_eq!(feed(&mut decoder, held), pressed(raw), "{raw:?}");
    assert_eq!(feed(&mut decoder, REST), released(raw), "{raw:?}");
}

#[test]
fn what_is_down_at_a_take_fires_nothing_until_pressed_again() {
    held_at_first_presses_after_a_release(REST.buttons(A), Raw::South);
    held_at_first_presses_after_a_release(REST.buttons(0x0010), Raw::DPadLeft);
    held_at_first_presses_after_a_release(REST.l(200), Raw::LeftBumper);
    held_at_first_presses_after_a_release(REST.buttons(R_CLICK), Raw::RightBumper);
    held_at_first_presses_after_a_release(REST.c(20, 132), Raw::LeftTrigger);
}

#[test]
fn what_is_down_when_a_controller_appears_fires_nothing_either() {
    let mut decoder = rested();
    let held = REST.buttons(A).l(200).c(240, 132);
    let both = |second| report([Some(REST), Some(second), None, None]);
    let second = PadId::Adapter(1);
    assert_eq!(
        decoder.report(&both(held)),
        [PadEvent::Connected(second, port_info(1))]
    );
    assert!(decoder.report(&both(held)).is_empty());
    assert!(decoder.report(&both(REST)).is_empty());
    assert_eq!(
        decoder.report(&both(held)),
        [
            PadEvent::Pressed(second, Raw::South),
            PadEvent::Pressed(second, Raw::LeftBumper),
            PadEvent::Pressed(second, Raw::RightTrigger),
        ]
    );
    // Port 1, there all along, still presses at once.
    assert_eq!(
        decoder.report(&report([Some(REST.buttons(A)), Some(held), None, None])),
        pressed(Raw::South)
    );
}

#[test]
fn a_held_main_stick_still_moves_at_a_take() {
    let (_, events) = taken(REST.stick(250, 128));
    assert_eq!(
        events,
        [
            PadEvent::Connected(PORT1, port_info(0)),
            PadEvent::Stick(PORT1, Axis::X, 1.0)
        ]
    );
    let mut pads = Pads::default();
    let moves: Vec<_> = events
        .into_iter()
        .filter_map(|event| pads.event(event, Instant::now()))
        .collect();
    assert_eq!(moves, [Input::Move(Direction::Right, Style::GameCube)]);
}

#[test]
fn report_bytes_reach_pads_as_lt_from_the_c_stick_and_lb_from_a_half_trigger() {
    let mut pads = Pads::default();
    let mut decoder = Decoder::default();
    let mut inputs = Vec::new();
    for bytes in [
        hex(PROBE_FIRST),
        one(REST.c(30, 132)),
        one(REST),
        one(REST.l(110)),
    ] {
        for event in decoder.report(&bytes) {
            inputs.extend(pads.event(event, Instant::now()));
        }
    }
    assert_eq!(
        inputs,
        [
            Input::Button(PadButton::Lt, Style::GameCube),
            Input::Button(PadButton::Lb, Style::GameCube),
        ]
    );
    assert_eq!(pads.names(), ["GameCube controller (port 1)"]);
}

#[test]
fn jitter_at_rest_sends_nothing() {
    let mut decoder = rested();
    for step in [1u8, 2, 3, 2, 1, 0] {
        for pad in [
            REST.stick(131 + step, 131 - step),
            REST.c(132 - step, 132 + step),
            REST.l(34 + step).r(29 + step),
            REST.l(34 - step).r(29 - step),
        ] {
            assert!(feed(&mut decoder, pad).is_empty(), "{pad:?}");
        }
    }
}

#[test]
fn a_slow_release_of_the_main_stick_ends_the_hold() {
    let mut pads = Pads::default();
    let mut decoder = Decoder::default();
    let mut sent = Vec::new();
    for x in (131..=190).chain((131..190).rev()) {
        for event in decoder.report(&one(REST.stick(x, 131))) {
            sent.push(event.clone());
            pads.event(event, Instant::now());
        }
    }
    assert!(pads.repeat_due().is_none());
    assert_eq!(sent.last(), Some(&PadEvent::Stick(PORT1, Axis::X, 0.0)));
    // A stop just past the release threshold still holds; a step under it
    // lets go.
    let mut pads = Pads::default();
    let mut decoder = Decoder::default();
    for x in (131..=190).chain((163..190).rev()) {
        for event in decoder.report(&one(REST.stick(x, 131))) {
            pads.event(event, Instant::now());
        }
    }
    assert!(pads.repeat_due().is_some());
    for event in decoder.report(&one(REST.stick(162, 131))) {
        pads.event(event, Instant::now());
    }
    assert!(pads.repeat_due().is_none());
}

#[test]
fn a_let_go_disconnects_every_port() {
    let mut decoder = Decoder::default();
    decoder.report(&report([Some(REST), None, Some(REST), None]));
    assert_eq!(
        decoder.release(),
        [
            PadEvent::Disconnected(PadId::Adapter(0)),
            PadEvent::Disconnected(PadId::Adapter(2))
        ]
    );
    assert!(decoder.release().is_empty());
    // The next take is a first report again.
    assert_eq!(
        decoder.report(&one(REST.buttons(A))),
        [PadEvent::Connected(PORT1, port_info(0))]
    );
}

// The hold, on a fake bus and tokio's paused clock.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Absent,
    Kernel,
    Program,
    Found,
}

/// The fake bus, which tests change between steps.
struct Bus {
    seen: Seen,
    /// What the next claims answer; past the end, they succeed.
    claims: VecDeque<Refused>,
    claimed_at: Vec<tokio::time::Instant>,
    control_fails: bool,
    start_fails: bool,
    /// A device is claimed and not yet dropped.
    held: bool,
    let_go: usize,
    reads: VecDeque<Result<Vec<u8>, Lost>>,
    looks: usize,
    /// Runs inside the device's drop, while the reader is letting go.
    on_let_go: Option<Box<dyn FnOnce() + Send>>,
}

#[derive(Clone)]
struct Shared(Arc<Mutex<Bus>>, Arc<Notify>);

impl Shared {
    fn with<R>(&self, change: impl FnOnce(&mut Bus) -> R) -> R {
        change(&mut self.0.lock().unwrap())
    }

    /// A report or a failure for the held device's next read.
    fn push(&self, read: Result<Vec<u8>, Lost>) {
        self.with(|bus| bus.reads.push_back(read));
        self.1.notify_one();
    }
}

struct Fake {
    shared: Shared,
    hotplug: mpsc::UnboundedReceiver<Hotplug>,
}

struct FakeDevice(Shared);

impl Drop for FakeDevice {
    fn drop(&mut self) {
        let on_let_go = self.0.with(|bus| {
            bus.held = false;
            bus.let_go += 1;
            bus.on_let_go.take()
        });
        if let Some(on_let_go) = on_let_go {
            on_let_go();
        }
    }
}

impl Transport for Fake {
    type Found = ();
    type Device = FakeDevice;

    fn look(&mut self) -> Look<()> {
        let seen = self.shared.with(|bus| {
            bus.looks += 1;
            bus.seen
        });
        match seen {
            Seen::Absent => Look::Absent,
            Seen::Kernel => Look::Kernel,
            Seen::Program => Look::Program,
            Seen::Found => Look::Found(()),
        }
    }

    fn claim(&mut self, (): ()) -> Result<FakeDevice, Refused> {
        self.shared.with(|bus| {
            bus.claimed_at.push(tokio::time::Instant::now());
            match bus.claims.pop_front() {
                Some(refused) => Err(refused),
                None => {
                    assert!(!bus.held, "claimed twice");
                    bus.held = true;
                    bus.reads.clear();
                    Ok(())
                }
            }
        })?;
        Ok(FakeDevice(self.shared.clone()))
    }

    async fn hotplug(&mut self) -> Hotplug {
        match self.hotplug.recv().await {
            Some(hotplug) => hotplug,
            None => std::future::pending().await,
        }
    }
}

impl Device for FakeDevice {
    fn control(&mut self) -> impl Future<Output = Result<(), Lost>> + Send {
        let fails = self.0.with(|bus| bus.control_fails);
        async move { if fails { Err(Lost::Failed) } else { Ok(()) } }
    }

    fn start(&mut self) -> impl Future<Output = Result<(), Lost>> + Send {
        let fails = self.0.with(|bus| bus.start_fails);
        async move { if fails { Err(Lost::Failed) } else { Ok(()) } }
    }

    async fn read(&mut self) -> Result<Vec<u8>, Lost> {
        loop {
            if let Some(read) = self.0.with(|bus| bus.reads.pop_front()) {
                return read;
            }
            self.0.1.notified().await;
        }
    }
}

struct Harness {
    shared: Shared,
    wanted: watch::Sender<bool>,
    state: watch::Sender<State>,
    out: mpsc::UnboundedReceiver<Report>,
    hotplug: mpsc::UnboundedSender<Hotplug>,
    task: tokio::task::JoinHandle<()>,
    start: tokio::time::Instant,
}

/// A reader on a bus that shows `seen`, wanted or not from the start.
fn harness(seen: Seen, wanted: bool, change: impl FnOnce(&mut Bus)) -> Harness {
    let mut bus = Bus {
        seen,
        claims: VecDeque::new(),
        claimed_at: Vec::new(),
        control_fails: false,
        start_fails: false,
        held: false,
        let_go: 0,
        reads: VecDeque::new(),
        looks: 0,
        on_let_go: None,
    };
    change(&mut bus);
    let shared = Shared(Arc::new(Mutex::new(bus)), Arc::new(Notify::new()));
    let (hotplug, hotplug_rx) = mpsc::unbounded_channel();
    let (send, out) = mpsc::unbounded_channel();
    let wanted = watch::Sender::new(wanted);
    let state = watch::Sender::new(State::Absent);
    let task = tokio::spawn(run(
        Fake {
            shared: shared.clone(),
            hotplug: hotplug_rx,
        },
        wanted.subscribe(),
        state.clone(),
        send,
    ));
    Harness {
        shared,
        wanted,
        state,
        out,
        hotplug,
        task,
        start: tokio::time::Instant::now(),
    }
}

impl Harness {
    /// Lets the reader run until it waits, with the paused clock moved on
    /// by `millis`.
    async fn wait(&self, millis: u64) {
        tokio::time::sleep(Duration::from_millis(millis)).await;
    }

    async fn settle(&self) {
        self.wait(1).await;
    }

    fn sent(&mut self) -> Vec<Report> {
        let mut sent = Vec::new();
        while let Ok(report) = self.out.try_recv() {
            sent.push(report);
        }
        sent
    }

    fn states(&mut self) -> Vec<State> {
        self.sent()
            .into_iter()
            .filter_map(|report| match report {
                Report::State(state) => Some(state),
                Report::Pad(_) => None,
            })
            .collect()
    }

    fn held(&self) -> bool {
        self.shared.with(|bus| bus.held)
    }

    fn claims(&self) -> usize {
        self.shared.with(|bus| bus.claimed_at.len())
    }

    fn now(&self) -> u128 {
        self.start.elapsed().as_millis()
    }

    /// When each claim came, in milliseconds from the start.
    fn claim_times(&self) -> Vec<u128> {
        self.shared.with(|bus| {
            bus.claimed_at
                .iter()
                .map(|at| at.duration_since(self.start).as_millis())
                .collect()
        })
    }

    fn want(&self, wanted: bool) {
        self.wanted.send_replace(wanted);
    }
}

fn pad_events(reports: Vec<Report>) -> Vec<PadEvent> {
    reports
        .into_iter()
        .filter_map(|report| match report {
            Report::Pad(event) => Some(event),
            Report::State(_) => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn wanted_takes_the_adapter_and_its_reports_become_pads() {
    let mut reader = harness(Seen::Found, false, |_| {});
    reader.settle().await;
    assert!(!reader.held());
    assert_eq!(reader.states(), [State::Free]);

    reader.want(true);
    reader.settle().await;
    assert!(reader.held());
    assert_eq!(*reader.state.borrow(), State::Held);
    assert_eq!(reader.states(), [State::Held]);
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.shared.push(Ok(one(REST.buttons(A))));
    reader.settle().await;
    assert_eq!(
        pad_events(reader.sent()),
        [
            PadEvent::Connected(PORT1, port_info(0)),
            PadEvent::Pressed(PORT1, Raw::South)
        ]
    );

    // Wanted from the start, it takes at once.
    let reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    assert!(reader.held());
    assert_eq!(reader.claim_times(), [0]);
}

#[tokio::test(start_paused = true)]
async fn not_wanted_lets_go_and_disconnects_its_pads() {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.settle().await;
    reader.sent();

    reader.want(false);
    reader.settle().await;
    assert!(!reader.held());
    assert_eq!(
        reader.sent(),
        [
            Report::Pad(PadEvent::Disconnected(PORT1)),
            Report::State(State::Free)
        ]
    );
    assert_eq!(*reader.state.borrow(), State::Free);
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 1);
}

/// Waits for the reader to end, failing rather than hanging if it doesn't.
async fn ended(task: tokio::task::JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .expect("the reader should end")
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_app_going_away_ends_the_reader_and_lets_go() {
    let reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    assert!(reader.held());
    let Harness {
        shared,
        wanted,
        task,
        ..
    } = reader;
    drop(wanted);
    ended(task).await;
    assert!(!shared.with(|bus| bus.held));

    // The app no longer hearing it ends it too.
    let reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    let Harness {
        shared, out, task, ..
    } = reader;
    drop(out);
    shared.push(Ok(hex(PROBE_FIRST)));
    ended(task).await;
    assert!(!shared.with(|bus| bus.held));
}

#[tokio::test(start_paused = true)]
async fn busy_tries_again_each_second_until_not_wanted() {
    let mut reader = harness(Seen::Found, true, |bus| {
        bus.claims = [Refused::Busy, Refused::Busy].into();
    });
    reader.wait(5000).await;
    assert_eq!(reader.claim_times(), [0, 1000, 2000]);
    assert!(reader.held());
    assert_eq!(reader.states(), [State::Busy, State::Held]);

    let mut reader = harness(Seen::Found, true, |bus| {
        bus.claims = [Refused::Busy; 10].into();
    });
    reader.wait(1500).await;
    assert_eq!(reader.claims(), 2);
    assert!(!reader.held());
    reader.want(false);
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 2);
    assert_eq!(reader.states(), [State::Busy]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_read_lets_go_and_tries_again_a_second_later() {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.settle().await;
    let failed_at = reader.now();
    reader.shared.push(Err(Lost::Failed));
    reader.settle().await;
    assert!(!reader.held());
    let sent = reader.sent();
    assert!(sent.contains(&Report::Pad(PadEvent::Disconnected(PORT1))));
    assert_eq!(sent.last(), Some(&Report::State(State::Free)));
    reader.wait(1000).await;
    assert!(reader.held());
    assert_eq!(reader.claim_times(), [0, failed_at + 1000]);

    // It stops trying once not wanted.
    reader.shared.push(Err(Lost::Failed));
    reader.settle().await;
    reader.want(false);
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 2);

    // A start byte that fails is the same.
    let reader = harness(Seen::Found, true, |bus| bus.start_fails = true);
    reader.wait(2500).await;
    assert_eq!(reader.claim_times(), [0, 1000, 2000]);
    assert!(!reader.held());
}

#[tokio::test(start_paused = true)]
async fn a_failed_claim_tries_again_a_second_later() {
    let mut reader = harness(Seen::Found, true, |bus| {
        bus.claims = [Refused::Failed].into();
    });
    reader.wait(5000).await;
    assert_eq!(reader.claim_times(), [0, 1000]);
    assert!(reader.held());
    assert_eq!(reader.states(), [State::Free, State::Held]);
}

#[tokio::test(start_paused = true)]
async fn no_access_waits_for_a_replug_or_the_next_want() {
    let mut reader = harness(Seen::Found, true, |bus| {
        bus.claims = [Refused::NoAccess; 3].into();
    });
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 1);
    assert_eq!(reader.states(), [State::NoAccess]);

    reader.hotplug.send(Hotplug::Arrived).unwrap();
    reader.settle().await;
    assert_eq!(reader.claims(), 2);

    reader.want(false);
    reader.settle().await;
    reader.want(true);
    reader.settle().await;
    assert_eq!(reader.claims(), 3);
    // With the rule fixed, the next want takes it.
    reader.want(false);
    reader.settle().await;
    reader.want(true);
    reader.settle().await;
    assert!(reader.held());
}

#[tokio::test(start_paused = true)]
async fn a_kernel_driver_or_another_program_means_no_take() {
    let mut reader = harness(Seen::Kernel, true, |_| {});
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 0);
    assert_eq!(reader.states(), [State::Kernel]);

    // A program's claim reads as busy, and is tried again each second.
    let mut reader = harness(Seen::Program, true, |_| {});
    reader.wait(2500).await;
    assert_eq!(reader.claims(), 0);
    assert_eq!(reader.states(), [State::Busy]);
    reader.shared.with(|bus| bus.seen = Seen::Found);
    reader.wait(1000).await;
    assert!(reader.held());
    assert_eq!(reader.claim_times(), [3000]);

    // Not wanted, a look shows the same.
    let mut reader = harness(Seen::Program, false, |_| {});
    reader.settle().await;
    assert_eq!(reader.states(), [State::Busy]);
}

#[tokio::test(start_paused = true)]
async fn an_unplug_while_reading_lets_go_until_it_comes_back() {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.settle().await;
    reader.sent();
    // With the bus showing nothing, it really is gone, so nothing is tried.
    reader.shared.with(|bus| bus.seen = Seen::Absent);
    reader.shared.push(Err(Lost::Unplugged));
    reader.settle().await;
    assert!(!reader.held());
    assert_eq!(
        reader.sent(),
        [
            Report::Pad(PadEvent::Disconnected(PORT1)),
            Report::State(State::Absent)
        ]
    );
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 1);

    reader.shared.with(|bus| bus.seen = Seen::Found);
    reader.hotplug.send(Hotplug::Arrived).unwrap();
    reader.settle().await;
    assert!(reader.held());

    // Hotplug saying it left ends the hold too, and the look agrees.
    reader.shared.with(|bus| bus.seen = Seen::Absent);
    reader.hotplug.send(Hotplug::Left).unwrap();
    reader.settle().await;
    assert!(!reader.held());
    assert_eq!(*reader.state.borrow(), State::Absent);
}

/// A reader holding the adapter after one report, with its events drained.
async fn holding() -> Harness {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.settle().await;
    reader.sent();
    reader
}

#[tokio::test(start_paused = true)]
async fn a_disconnect_with_the_adapter_still_there_is_tried_again_a_second_later() {
    let mut reader = holding().await;
    let lost_at = reader.now();
    reader.shared.push(Err(Lost::Unplugged));
    reader.settle().await;
    assert!(!reader.held());
    assert_eq!(
        reader.sent(),
        [
            Report::Pad(PadEvent::Disconnected(PORT1)),
            Report::State(State::Free)
        ]
    );
    assert_eq!(reader.claims(), 1);
    reader.wait(1000).await;
    assert!(reader.held());
    assert_eq!(reader.claim_times(), [0, lost_at + 1000]);
    assert_eq!(*reader.state.borrow(), State::Held);
}

#[tokio::test(start_paused = true)]
async fn a_disconnect_that_a_look_confirms_is_not_tried_again() {
    let reader = holding().await;
    reader.shared.with(|bus| bus.seen = Seen::Absent);
    reader.shared.push(Err(Lost::Unplugged));
    reader.settle().await;
    // One look to confirm it, and none after: nothing is scheduled.
    let looks = reader.shared.with(|bus| bus.looks);
    reader.wait(5000).await;
    assert_eq!(reader.shared.with(|bus| bus.looks), looks);
    assert_eq!(reader.claims(), 1);
    assert_eq!(*reader.state.borrow(), State::Absent);
    assert!(!reader.held());
}

#[tokio::test(start_paused = true)]
async fn a_hotplug_left_after_a_disconnect_cancels_its_retry() {
    let reader = holding().await;
    reader.shared.push(Err(Lost::Unplugged));
    reader.wait(300).await;
    reader.hotplug.send(Hotplug::Left).unwrap();
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 1);
    assert!(!reader.held());
    assert_eq!(*reader.state.borrow(), State::Absent);
}

#[tokio::test(start_paused = true)]
async fn losing_the_want_before_a_disconnects_retry_means_no_claim() {
    let reader = holding().await;
    reader.shared.push(Err(Lost::Unplugged));
    reader.wait(300).await;
    reader.want(false);
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 1);
    assert!(!reader.held());
}

#[tokio::test(start_paused = true)]
async fn an_unplug_drops_a_pending_retry() {
    let mut reader = harness(Seen::Program, true, |_| {});
    reader.wait(500).await;
    assert_eq!(reader.states(), [State::Busy]);
    let looks = reader.shared.with(|bus| bus.looks);
    reader.shared.with(|bus| bus.seen = Seen::Absent);
    reader.hotplug.send(Hotplug::Left).unwrap();
    reader.wait(5000).await;
    assert_eq!(reader.states(), [State::Absent]);
    assert_eq!(reader.shared.with(|bus| bus.looks), looks);

    // It still comes back on the next arrival.
    reader.shared.with(|bus| bus.seen = Seen::Found);
    reader.hotplug.send(Hotplug::Arrived).unwrap();
    reader.settle().await;
    assert!(reader.held());
}

#[tokio::test(start_paused = true)]
async fn with_none_plugged_in_it_waits_for_one_to_arrive() {
    let mut reader = harness(Seen::Absent, true, |_| {});
    reader.wait(5000).await;
    assert_eq!(reader.claims(), 0);
    assert_eq!(reader.states(), [State::Absent]);
    reader.shared.with(|bus| bus.seen = Seen::Found);
    reader.hotplug.send(Hotplug::Arrived).unwrap();
    reader.settle().await;
    assert!(reader.held());
    assert_eq!(reader.claim_times(), [5000]);
}

#[tokio::test(start_paused = true)]
async fn a_want_right_after_the_let_go_takes_it_again() {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.want(false);
    reader.settle().await;
    reader.want(true);
    reader.settle().await;
    assert!(reader.held());
    assert_eq!(reader.shared.with(|bus| bus.let_go), 1);
    assert_eq!(reader.claims(), 2);
    assert_eq!(*reader.state.borrow(), State::Held);
    assert_eq!(reader.states(), [State::Held, State::Free, State::Held]);
}

#[tokio::test(start_paused = true)]
async fn a_want_that_arrives_while_letting_go_takes_it_again() {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.settle().await;
    reader.sent();
    let wanted = reader.wanted.clone();
    reader.shared.with(|bus| {
        bus.on_let_go = Some(Box::new(move || {
            wanted.send_replace(true);
        }));
    });
    let let_go_at = reader.now();
    reader.want(false);
    reader.settle().await;
    assert_eq!(reader.shared.with(|bus| bus.let_go), 1);
    assert!(reader.held());
    // Taken again at once, not after a retry's second.
    assert_eq!(reader.claim_times(), [0, let_go_at]);
    assert_eq!(
        reader.sent(),
        [
            Report::Pad(PadEvent::Disconnected(PORT1)),
            Report::State(State::Free),
            Report::State(State::Held)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_want_back_before_the_reader_saw_it_go_keeps_the_hold() {
    let mut reader = harness(Seen::Found, true, |_| {});
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.settle().await;
    reader.sent();
    reader.want(false);
    reader.want(true);
    reader.settle().await;
    assert!(reader.held());
    assert_eq!(reader.claims(), 1);
    assert!(reader.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_control_request_failing_changes_nothing() {
    let mut reader = harness(Seen::Found, true, |bus| bus.control_fails = true);
    reader.settle().await;
    reader.shared.push(Ok(hex(PROBE_FIRST)));
    reader.wait(3000).await;
    assert!(reader.held());
    assert_eq!(reader.claims(), 1);
    assert_eq!(
        pad_events(reader.sent()),
        [PadEvent::Connected(PORT1, port_info(0))]
    );
}

#[tokio::test(start_paused = true)]
async fn a_start_waits_for_the_let_go_for_at_most_250_ms() {
    let state = watch::Sender::new(State::Held);
    let begun = tokio::time::Instant::now();
    let wait = tokio::spawn(let_go(Some(state.subscribe())));
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!wait.is_finished());
    state.send_replace(State::Free);
    wait.await.unwrap();
    assert_eq!(begun.elapsed(), Duration::from_millis(40));

    // No let-go: it runs at 250 ms.
    let state = watch::Sender::new(State::Held);
    let begun = tokio::time::Instant::now();
    let_go(Some(state.subscribe())).await;
    assert_eq!(begun.elapsed(), LET_GO_WAIT);
    assert_eq!(LET_GO_WAIT, Duration::from_millis(250));

    // Nothing held: no wait at all.
    let control = Control::default();
    assert!(control.held().is_none());
    let begun = tokio::time::Instant::now();
    let_go(control.held()).await;
    assert_eq!(begun.elapsed(), Duration::ZERO);
    control.state.send_replace(State::Held);
    assert!(control.held().is_some());
    control.state.send_replace(State::Busy);
    assert!(control.held().is_none());
}
