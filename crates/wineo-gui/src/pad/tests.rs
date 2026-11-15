use std::time::{Duration, Instant};

use wineo_core::library::PadButton;

use super::reader::Read;
use super::*;

fn info(name: &str, vendor: u16, product: u16) -> PadInfo {
    PadInfo {
        name: name.to_owned(),
        vendor: Some(vendor),
        product: Some(product),
    }
}

fn xbox() -> PadInfo {
    info("Xbox Wireless Controller", 0x045e, 0x0b13)
}

fn switch_pro() -> PadInfo {
    info("Nintendo Switch Pro Controller", 0x057e, 0x2009)
}

fn steam_virtual() -> PadInfo {
    info("Steam Virtual Gamepad", 0x28de, 0x11ff)
}

fn pads(connected: &[(PadId, PadInfo)]) -> Pads {
    let mut pads = Pads::default();
    for (id, info) in connected {
        pads.event(PadEvent::Connected(*id, info.clone()), Instant::now());
    }
    pads
}

fn press(pads: &mut Pads, id: PadId, raw: Raw) -> Option<Input> {
    pads.event(PadEvent::Pressed(id, raw), Instant::now())
}

#[test]
fn each_pad_takes_the_labels_of_its_own_vendor() {
    let mut pads = pads(&[(0, xbox()), (1, switch_pro())]);
    // The bottom button is A on an Xbox pad and B on a Nintendo one.
    assert_eq!(
        press(&mut pads, 0, Raw::South),
        Some(Input::Button(PadButton::A, Style::Xbox))
    );
    assert_eq!(
        press(&mut pads, 1, Raw::South),
        Some(Input::Button(PadButton::B, Style::Nintendo))
    );
    assert_eq!(
        press(&mut pads, 1, Raw::East),
        Some(Input::Button(PadButton::A, Style::Nintendo))
    );
    assert_eq!(
        press(&mut pads, 0, Raw::East),
        Some(Input::Button(PadButton::B, Style::Xbox))
    );
    // X sits north on a Nintendo pad and west on an Xbox one.
    assert_eq!(
        press(&mut pads, 1, Raw::North),
        Some(Input::Button(PadButton::X, Style::Nintendo))
    );
    assert_eq!(
        press(&mut pads, 1, Raw::West),
        Some(Input::Button(PadButton::Y, Style::Nintendo))
    );
    assert_eq!(
        press(&mut pads, 0, Raw::North),
        Some(Input::Button(PadButton::Y, Style::Xbox))
    );
    assert_eq!(
        press(&mut pads, 0, Raw::West),
        Some(Input::Button(PadButton::X, Style::Xbox))
    );
}

#[test]
fn only_the_face_buttons_swap_on_a_nintendo_pad() {
    for raw in [
        Raw::LeftBumper,
        Raw::RightBumper,
        Raw::LeftTrigger,
        Raw::RightTrigger,
        Raw::Select,
        Raw::Start,
        Raw::LeftStick,
        Raw::RightStick,
    ] {
        assert_eq!(
            label(Style::Nintendo, raw),
            label(Style::Xbox, raw),
            "{raw:?}"
        );
    }
    assert_eq!(label(Style::Xbox, Raw::LeftTrigger), Some(PadButton::Lt));
    assert_eq!(label(Style::Xbox, Raw::LeftBumper), Some(PadButton::Lb));
    assert_eq!(label(Style::PlayStation, Raw::South), Some(PadButton::A));
    assert_eq!(label(Style::Xbox, Raw::DPadUp), None);
}

#[test]
fn vendors_pick_the_label_style() {
    assert_eq!(xbox().style(), Style::Xbox);
    assert_eq!(switch_pro().style(), Style::Nintendo);
    assert_eq!(
        info("DualSense Wireless Controller", 0x054c, 0x0ce6).style(),
        Style::PlayStation
    );
    let unknown = PadInfo {
        name: "Generic".to_owned(),
        vendor: None,
        product: None,
    };
    assert_eq!(unknown.style(), Style::Xbox);
}

#[test]
fn gilrs_bumpers_and_triggers_keep_their_places() {
    // gilrs calls the bumper `LeftTrigger` and the trigger `LeftTrigger2`.
    assert_eq!(
        reader::raw(gilrs::Button::LeftTrigger),
        Some(Raw::LeftBumper)
    );
    assert_eq!(
        reader::raw(gilrs::Button::LeftTrigger2),
        Some(Raw::LeftTrigger)
    );
    assert_eq!(
        reader::raw(gilrs::Button::RightTrigger),
        Some(Raw::RightBumper)
    );
    assert_eq!(
        reader::raw(gilrs::Button::RightTrigger2),
        Some(Raw::RightTrigger)
    );
    assert_eq!(reader::raw(gilrs::Button::South), Some(Raw::South));
    assert_eq!(reader::raw(gilrs::Button::Mode), None);
}

#[test]
fn steams_virtual_pad_is_read_only_without_a_real_one() {
    let mut pads = pads(&[(0, steam_virtual())]);
    assert_eq!(
        press(&mut pads, 0, Raw::South),
        Some(Input::Button(PadButton::A, Style::Xbox))
    );
    assert_eq!(pads.names(), ["Steam Virtual Gamepad"]);

    pads.event(PadEvent::Connected(1, xbox()), Instant::now());
    assert_eq!(press(&mut pads, 0, Raw::South), None);
    assert_eq!(press(&mut pads, 0, Raw::DPadLeft), None);
    assert_eq!(
        pads.event(PadEvent::Stick(0, Axis::X, 1.0), Instant::now()),
        None
    );
    assert_eq!(
        press(&mut pads, 1, Raw::South),
        Some(Input::Button(PadButton::A, Style::Xbox))
    );
    assert_eq!(pads.names(), ["Xbox Wireless Controller"]);

    pads.event(PadEvent::Disconnected(1), Instant::now());
    assert!(press(&mut pads, 0, Raw::South).is_some());
}

#[test]
fn a_real_pad_arriving_stops_the_virtual_pads_repeat() {
    let mut pads = pads(&[(0, steam_virtual())]);
    assert!(press(&mut pads, 0, Raw::DPadRight).is_some());
    assert!(pads.repeat_due().is_some());
    pads.event(PadEvent::Connected(1, xbox()), Instant::now());
    assert_eq!(pads.repeat_due(), None);
}

#[test]
fn a_held_direction_repeats_after_a_pause_then_steadily() {
    let mut pads = pads(&[(0, xbox())]);
    let start = Instant::now();
    assert_eq!(
        pads.event(PadEvent::Pressed(0, Raw::DPadDown), start),
        Some(Input::Move(Direction::Down, Style::Xbox))
    );
    assert_eq!(pads.repeat_due(), Some(start + REPEAT_AFTER));
    assert_eq!(
        pads.repeat(start + REPEAT_AFTER - Duration::from_millis(1)),
        None
    );
    let first = start + REPEAT_AFTER;
    assert_eq!(
        pads.repeat(first),
        Some(Input::Move(Direction::Down, Style::Xbox))
    );
    assert_eq!(pads.repeat_due(), Some(first + REPEAT_EVERY));
    // A late wakeup repeats once, from when it came.
    let late = first + 3 * REPEAT_EVERY;
    assert!(pads.repeat(late).is_some());
    assert_eq!(pads.repeat_due(), Some(late + REPEAT_EVERY));
    pads.event(PadEvent::Released(0, Raw::DPadDown), late);
    assert_eq!(pads.repeat_due(), None);
    assert_eq!(pads.repeat(late + Duration::from_secs(1)), None);
}

#[test]
fn the_dpad_and_stick_share_one_repeat_clock() {
    let mut pads = pads(&[(0, xbox())]);
    let start = Instant::now();
    pads.event(PadEvent::Pressed(0, Raw::DPadRight), start);
    let later = start + Duration::from_millis(200);
    assert_eq!(
        pads.event(PadEvent::Stick(0, Axis::Y, 0.9), later),
        Some(Input::Move(Direction::Up, Style::Xbox))
    );
    // The stick took over, and its clock starts from its own press.
    assert_eq!(pads.repeat_due(), Some(later + REPEAT_AFTER));
    // Letting go of the D-pad leaves the stick's repeat.
    pads.event(PadEvent::Released(0, Raw::DPadRight), later);
    assert_eq!(pads.repeat_due(), Some(later + REPEAT_AFTER));
    assert_eq!(
        pads.repeat(later + REPEAT_AFTER),
        Some(Input::Move(Direction::Up, Style::Xbox))
    );
    pads.event(PadEvent::Stick(0, Axis::Y, 0.0), later);
    assert_eq!(pads.repeat_due(), None);

    // The same direction from both: letting go of the D-pad's leaves the
    // stick's.
    pads.event(PadEvent::Pressed(0, Raw::DPadRight), later);
    pads.event(PadEvent::Stick(0, Axis::X, 0.9), later);
    pads.event(PadEvent::Released(0, Raw::DPadRight), later);
    assert_eq!(pads.repeat_due(), Some(later + REPEAT_AFTER));
}

#[test]
fn the_stick_presses_at_half_and_lets_go_below_35_percent() {
    let mut pads = pads(&[(0, xbox())]);
    let now = Instant::now();
    let mut stick = |axis, value| pads.event(PadEvent::Stick(0, axis, value), now);
    assert_eq!(stick(Axis::X, 0.49), None);
    assert_eq!(
        stick(Axis::X, 0.5),
        Some(Input::Move(Direction::Right, Style::Xbox))
    );
    // Holding between the two thresholds keeps it pressed, with no new
    // move.
    assert_eq!(stick(Axis::X, 0.4), None);
    assert_eq!(stick(Axis::X, 0.36), None);
    assert!(pads.repeat_due().is_some());
    let mut stick = |axis, value| pads.event(PadEvent::Stick(0, axis, value), now);
    assert_eq!(stick(Axis::X, 0.34), None);
    assert_eq!(pads.repeat_due(), None);
    let mut stick = |axis, value| pads.event(PadEvent::Stick(0, axis, value), now);
    // Back above the release line but under the press line is nothing.
    assert_eq!(stick(Axis::X, 0.45), None);
    assert_eq!(
        stick(Axis::X, -0.8),
        Some(Input::Move(Direction::Left, Style::Xbox))
    );
}

#[test]
fn a_sloppy_diagonal_moves_only_along_its_dominant_axis() {
    let mut pads = pads(&[(0, xbox())]);
    let now = Instant::now();
    let mut stick = |axis, value| pads.event(PadEvent::Stick(0, axis, value), now);
    assert_eq!(
        stick(Axis::X, 0.7),
        Some(Input::Move(Direction::Right, Style::Xbox))
    );
    // Up past the press line, but still less than right: no section jump.
    assert_eq!(stick(Axis::Y, 0.6), None);
    assert_eq!(stick(Axis::Y, -0.65), None);
    // Down taking over counts once it's the stronger.
    assert_eq!(
        stick(Axis::Y, -0.8),
        Some(Input::Move(Direction::Down, Style::Xbox))
    );
    // Starting from rest on a diagonal picks one direction.
    let mut pads = pads_with_xbox();
    let mut stick = |axis, value| pads.event(PadEvent::Stick(0, axis, value), now);
    assert_eq!(
        stick(Axis::Y, 0.55),
        Some(Input::Move(Direction::Up, Style::Xbox))
    );
    assert_eq!(stick(Axis::X, 0.5), None);
}

fn pads_with_xbox() -> Pads {
    pads(&[(0, xbox())])
}

#[test]
fn a_disconnect_or_release_all_stops_the_repeat() {
    let mut pads = pads(&[(0, xbox())]);
    press(&mut pads, 0, Raw::DPadUp);
    pads.event(PadEvent::Disconnected(0), Instant::now());
    assert_eq!(pads.repeat_due(), None);
    assert!(pads.names().is_empty());

    let mut pads = pads_with_xbox();
    pads.event(PadEvent::Stick(0, Axis::X, 1.0), Instant::now());
    pads.release_all();
    assert_eq!(pads.repeat_due(), None);
    // The stick still held counts as a new press once it moves again.
    assert_eq!(
        pads.event(PadEvent::Stick(0, Axis::X, 0.95), Instant::now()),
        Some(Input::Move(Direction::Right, Style::Xbox))
    );
}

/// Made-up time for the pump: it moves only when a read says it took a
/// while or the pump sleeps, so nothing here really waits.
#[derive(Clone)]
struct FakeClock(std::rc::Rc<std::cell::Cell<Instant>>);

impl FakeClock {
    fn new() -> Self {
        Self(std::rc::Rc::new(std::cell::Cell::new(Instant::now())))
    }

    fn advance(&self, duration: Duration) {
        self.0.set(self.0.get() + duration);
    }
}

impl reader::Clock for FakeClock {
    fn now(&self) -> Instant {
        self.0.get()
    }

    fn sleep(&self, duration: Duration) {
        self.advance(duration);
    }
}

/// A made-up source: each read takes its next entry, and the reads are
/// counted.
struct Fake {
    reads: std::rc::Rc<std::cell::Cell<usize>>,
    next: Box<dyn FnMut(usize) -> reader::Read>,
}

impl reader::Source for Fake {
    fn next(&mut self) -> reader::Read {
        let read = self.reads.get();
        self.reads.set(read + 1);
        (self.next)(read)
    }
}

fn fake(next: impl FnMut(usize) -> reader::Read + 'static) -> Fake {
    Fake {
        reads: std::rc::Rc::default(),
        next: Box::new(next),
    }
}

#[test]
fn a_none_between_events_reads_on_at_once_and_the_pump_stops_with_the_app() {
    // Quick empty reads between events never add up to a back-off, as long as
    // an event comes between: ten here, but never more than five in a row.
    let mut events = vec![Read::Event(PadEvent::Pressed(0, Raw::DPadRight))];
    events.extend([
        Read::Nothing,
        Read::Nothing,
        Read::Nothing,
        Read::Nothing,
        Read::Nothing,
    ]);
    events.push(Read::Event(PadEvent::Released(0, Raw::DPadRight)));
    events.extend([
        Read::Nothing,
        Read::Nothing,
        Read::Nothing,
        Read::Nothing,
        Read::Nothing,
    ]);
    events.push(Read::Event(PadEvent::Disconnected(0)));
    events.push(Read::Event(PadEvent::Connected(1, xbox())));
    let clock = FakeClock::new();
    let start = reader::Clock::now(&clock);
    let sent = std::cell::RefCell::new(Vec::new());
    reader::pump(
        fake(move |read| events[read].clone()),
        reader::RETRY_AFTER,
        &clock,
        |event| {
            sent.borrow_mut().push((event, reader::Clock::now(&clock)));
            sent.borrow().len() < 3
        },
    );
    let sent = sent.into_inner();
    // The tap's release comes with no wait at all, as gilrs says nothing
    // after every drained batch, so no repeat can fire in between.
    assert_eq!(sent[1].1, sent[0].1);
    assert_eq!(reader::Clock::now(&clock), start);
    // It stops once the app is gone, leaving the rest unread.
    assert_eq!(
        sent.into_iter().map(|(event, _)| event).collect::<Vec<_>>(),
        [
            PadEvent::Pressed(0, Raw::DPadRight),
            PadEvent::Released(0, Raw::DPadRight),
            PadEvent::Disconnected(0),
        ]
    );
}

#[test]
fn reads_that_keep_failing_back_off() {
    let retry = reader::RETRY_AFTER;
    let window = Duration::from_millis(300);
    let clock = FakeClock::new();
    let start = reader::Clock::now(&clock);
    let time = clock.clone();
    // Says nothing at once until the window has passed, or after so many reads
    // that it was spinning, so a broken back-off ends too.
    let source = fake(move |read| {
        let passed = reader::Clock::now(&time).saturating_duration_since(start);
        if passed >= window || read >= 10_000 {
            Read::Event(PadEvent::Disconnected(0))
        } else {
            Read::Nothing
        }
    });
    let reads = source.reads.clone();
    reader::pump(source, retry, &clock, |_| false);
    // The quick ones, then one per retry until the window is over, and
    // the read that ends it.
    let expected = 8 + (window.as_millis() / retry.as_millis()) as usize + 1;
    assert_eq!(reads.get(), expected);
}

#[test]
fn a_press_after_dropped_axis_events_waits_at_most_one_short_retry() {
    // A thumb resting on the stick: gilrs's filters drop each axis event
    // and the read comes back empty, 1 ms apart, far more than the quick
    // count allows. The press read after them is sent within one retry.
    let clock = FakeClock::new();
    let start = reader::Clock::now(&clock);
    let ready = start + Duration::from_millis(40);
    let time = clock.clone();
    let read_at = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = read_at.clone();
    let source = fake(move |_| {
        let now = reader::Clock::now(&time);
        if now >= ready {
            seen.set(Some(now));
            Read::Event(PadEvent::Pressed(0, Raw::South))
        } else {
            time.advance(Duration::from_millis(1));
            Read::Nothing
        }
    });
    let sent = std::cell::Cell::new(None);
    reader::pump(source, reader::RETRY_AFTER, &clock, |event| {
        assert_eq!(event, PadEvent::Pressed(0, Raw::South));
        sent.set(Some(reader::Clock::now(&clock)));
        false
    });
    // At most 10 ms late, a literal so a longer retry fails here.
    let bound = Duration::from_millis(10);
    let late = sent.get().unwrap() - ready;
    assert!(late <= bound, "{late:?}");
    assert!(read_at.get().unwrap() - ready <= bound);
}

#[test]
fn a_slow_none_is_a_read_that_waited_and_never_backs_off() {
    // Each empty read comes after a wait, as after a hotplug that isn't a pad:
    // none of them count toward backing off, so the pump never sleeps.
    let clock = FakeClock::new();
    let start = reader::Clock::now(&clock);
    let time = clock.clone();
    let source = fake(move |read| {
        if read < 12 {
            time.advance(Duration::from_millis(60));
            Read::Nothing
        } else {
            Read::Event(PadEvent::Disconnected(0))
        }
    });
    reader::pump(source, Duration::from_secs(5), &clock, |_| false);
    assert_eq!(
        reader::Clock::now(&clock).saturating_duration_since(start),
        Duration::from_millis(12 * 60)
    );
}

#[test]
fn events_nothing_reads_show_the_device_works() {
    // A right-stick sweep: each event nothing reads is followed by gilrs's
    // drain read, ten times over, then the tap's release. That's more
    // empty reads than the back-off allows in a row, but each has an event
    // before it, so no fake time passes.
    let mut reads = vec![Read::Event(PadEvent::Pressed(0, Raw::South))];
    for _ in 0..10 {
        reads.extend([Read::Ignored, Read::Nothing]);
    }
    reads.push(Read::Event(PadEvent::Released(0, Raw::South)));
    let clock = FakeClock::new();
    let start = reader::Clock::now(&clock);
    let sent = std::cell::RefCell::new(Vec::new());
    reader::pump(
        fake(move |read| reads[read].clone()),
        reader::RETRY_AFTER,
        &clock,
        |event| {
            sent.borrow_mut().push(event);
            sent.borrow().len() < 2
        },
    );
    assert_eq!(
        sent.into_inner(),
        [
            PadEvent::Pressed(0, Raw::South),
            PadEvent::Released(0, Raw::South)
        ]
    );
    assert_eq!(reader::Clock::now(&clock), start);
}
