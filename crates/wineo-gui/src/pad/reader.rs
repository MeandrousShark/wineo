//! The one thread that reads controllers, for the life of the process.
//!
//! It's a plain `std::thread` parked in gilrs's blocking read, so an idle
//! pad costs no CPU. Never `spawn_blocking`: iced's runtime drop waits on
//! blocking tasks with no limit (see `exit_watchdog`), so every quit would
//! hit the watchdog. It can't be stopped without an event to wake it, so it
//! stays once started and the app ignores what it sends while the switch
//! is off.

use std::time::{Duration, Instant};

use iced::Subscription;
use iced::futures::{SinkExt, Stream};

use super::{Axis, PadEvent, PadInfo, Raw};
use crate::Message;

/// Where events come from: gilrs in the app, made-up ones in tests.
pub(crate) trait Source {
    /// Blocks until the next event. `None` is often normal: gilrs returns
    /// it once a batch of events is drained, after a hotplug that isn't a
    /// pad, and for events its filters drop. Only a run of them right away
    /// means the read itself is failing.
    fn next(&mut self) -> Option<PadEvent>;
}

/// The wait once reads keep failing, so a broken device can't spin a core.
pub(crate) const RETRY_AFTER: Duration = Duration::from_secs(1);
/// A `None` back sooner than this didn't wait for anything.
const QUICK: Duration = Duration::from_millis(50);
/// Quick `None`s in a row, with no event between, before the reader backs
/// off. A drained batch gives one per pad at most.
const QUICK_NONES: u32 = 8;

/// Sends what `source` reads until the receiving side is gone. A `None`
/// reads again at once, and only a run of quick ones waits `retry` before
/// each further read.
pub(crate) fn pump(mut source: impl Source, retry: Duration, send: impl Fn(PadEvent) -> bool) {
    let mut quick_nones = 0;
    loop {
        let asked = Instant::now();
        match source.next() {
            Some(event) => {
                quick_nones = 0;
                if !send(event) {
                    return;
                }
            }
            // It waited in the read first, so the read works.
            None if asked.elapsed() >= QUICK => quick_nones = 0,
            None => {
                quick_nones += 1;
                if quick_nones > QUICK_NONES {
                    std::thread::sleep(retry);
                }
            }
        }
    }
}

/// The reader's events for as long as the app runs.
pub(crate) fn subscription() -> Subscription<Message> {
    Subscription::run(events).map(Message::Pad)
}

fn events() -> impl Stream<Item = PadEvent> {
    iced::stream::channel(16, async |mut output| {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let started = std::thread::Builder::new()
            .name("wineo-pad".to_owned())
            .spawn(move || {
                // No pad support here is the same as no pad.
                let Some(source) = Gilrs::new() else {
                    return;
                };
                for event in source.connected() {
                    if send.send(event).is_err() {
                        return;
                    }
                }
                pump(source, RETRY_AFTER, |event| send.send(event).is_ok());
            });
        if started.is_err() {
            return;
        }
        while let Some(event) = receive.recv().await {
            if output.send(event).await.is_err() {
                return;
            }
        }
    })
}

struct Gilrs(gilrs::Gilrs);

impl Gilrs {
    fn new() -> Option<Self> {
        gilrs::Gilrs::new().ok().map(Self)
    }

    /// The pads already there at start, which gilrs reports no event for.
    fn connected(&self) -> Vec<PadEvent> {
        self.0
            .gamepads()
            .map(|(id, pad)| PadEvent::Connected(id.into(), info(&pad)))
            .collect()
    }
}

impl Source for Gilrs {
    fn next(&mut self) -> Option<PadEvent> {
        loop {
            let event = self.0.next_event_blocking(None)?;
            let id = usize::from(event.id);
            let translated = match event.event {
                gilrs::EventType::Connected => {
                    Some(PadEvent::Connected(id, info(&self.0.gamepad(event.id))))
                }
                gilrs::EventType::Disconnected => Some(PadEvent::Disconnected(id)),
                gilrs::EventType::ButtonPressed(button, _) => {
                    raw(button).map(|raw| PadEvent::Pressed(id, raw))
                }
                gilrs::EventType::ButtonReleased(button, _) => {
                    raw(button).map(|raw| PadEvent::Released(id, raw))
                }
                gilrs::EventType::AxisChanged(gilrs::Axis::LeftStickX, value, _) => {
                    Some(PadEvent::Stick(id, Axis::X, value))
                }
                gilrs::EventType::AxisChanged(gilrs::Axis::LeftStickY, value, _) => {
                    Some(PadEvent::Stick(id, Axis::Y, value))
                }
                _ => None,
            };
            // Events nothing reads, like the right stick or a trigger's
            // travel, don't wake the app.
            if translated.is_some() {
                return translated;
            }
        }
    }
}

fn info(pad: &gilrs::Gamepad<'_>) -> PadInfo {
    PadInfo {
        name: pad.name().to_owned(),
        vendor: pad.vendor_id(),
        product: pad.product_id(),
    }
}

/// gilrs's names by position. Its `LeftTrigger` is the bumper and
/// `LeftTrigger2` the trigger, which reaches here as a press once gilrs's
/// own threshold is crossed, so analog triggers need none of ours.
pub(crate) fn raw(button: gilrs::Button) -> Option<Raw> {
    use gilrs::Button;
    Some(match button {
        Button::South => Raw::South,
        Button::East => Raw::East,
        Button::North => Raw::North,
        Button::West => Raw::West,
        Button::LeftTrigger => Raw::LeftBumper,
        Button::RightTrigger => Raw::RightBumper,
        Button::LeftTrigger2 => Raw::LeftTrigger,
        Button::RightTrigger2 => Raw::RightTrigger,
        Button::Select => Raw::Select,
        Button::Start => Raw::Start,
        Button::LeftThumb => Raw::LeftStick,
        Button::RightThumb => Raw::RightStick,
        Button::DPadUp => Raw::DPadUp,
        Button::DPadDown => Raw::DPadDown,
        Button::DPadLeft => Raw::DPadLeft,
        Button::DPadRight => Raw::DPadRight,
        Button::C | Button::Z | Button::Mode | Button::Unknown => return None,
    })
}
