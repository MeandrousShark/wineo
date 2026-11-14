//! Game controllers: what the reader thread reports, and the pure rules
//! that turn it into presses and moves. Nothing here knows gilrs, so tests
//! drive it with made-up events and never touch `/dev/input`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use wineo_core::library::PadButton;

use crate::keys::Direction;

pub(crate) mod glyph;
pub(crate) mod reader;

/// The id the reader gives a pad, stable while it stays connected.
pub(crate) type PadId = usize;

/// A button by where it sits, the way the SDL mapping database names it:
/// `South` is the bottom face button on every pad, whatever is printed on
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Raw {
    South,
    East,
    North,
    West,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl Raw {
    fn direction(self) -> Option<Direction> {
        match self {
            Self::DPadUp => Some(Direction::Up),
            Self::DPadDown => Some(Direction::Down),
            Self::DPadLeft => Some(Direction::Left),
            Self::DPadRight => Some(Direction::Right),
            _ => None,
        }
    }
}

/// The left stick's axes. Up and right are positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PadInfo {
    pub(crate) name: String,
    pub(crate) vendor: Option<u16>,
    pub(crate) product: Option<u16>,
}

impl PadInfo {
    pub(crate) fn style(&self) -> Style {
        match self.vendor {
            Some(NINTENDO) => Style::Nintendo,
            Some(SONY) => Style::PlayStation,
            _ => Style::Xbox,
        }
    }

    fn steam_virtual(&self) -> bool {
        self.vendor == Some(VALVE) && self.product == Some(STEAM_VIRTUAL)
    }
}

/// What the reader thread saw.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PadEvent {
    Connected(PadId, PadInfo),
    Disconnected(PadId),
    Pressed(PadId, Raw),
    Released(PadId, Raw),
    /// The left stick moved, from -1 to 1.
    Stick(PadId, Axis, f32),
}

const NINTENDO: u16 = 0x057e;
const SONY: u16 = 0x054c;
const VALVE: u16 = 0x28de;
/// The Xbox pad Steam Input adds beside the real one.
const STEAM_VIRTUAL: u16 = 0x11ff;

/// Whose labels a pad's buttons carry, for the hint bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Style {
    #[default]
    Xbox,
    PlayStation,
    Nintendo,
}

/// The printed label of a raw button. A Nintendo pad prints A on the east
/// button and X on the north one, so those swap with B and Y there; the
/// SDL database maps by position, so this is the only swap.
pub(crate) fn label(style: Style, raw: Raw) -> Option<PadButton> {
    let nintendo = style == Style::Nintendo;
    Some(match raw {
        Raw::South if nintendo => PadButton::B,
        Raw::East if nintendo => PadButton::A,
        Raw::North if nintendo => PadButton::X,
        Raw::West if nintendo => PadButton::Y,
        Raw::South => PadButton::A,
        Raw::East => PadButton::B,
        Raw::North => PadButton::Y,
        Raw::West => PadButton::X,
        Raw::LeftBumper => PadButton::Lb,
        Raw::RightBumper => PadButton::Rb,
        Raw::LeftTrigger => PadButton::Lt,
        Raw::RightTrigger => PadButton::Rt,
        Raw::Select => PadButton::View,
        Raw::Start => PadButton::Start,
        Raw::LeftStick => PadButton::Ls,
        Raw::RightStick => PadButton::Rs,
        Raw::DPadUp | Raw::DPadDown | Raw::DPadLeft | Raw::DPadRight => return None,
    })
}

/// The stick presses a direction at half its travel and lets go below
/// this, so resting near the threshold doesn't flicker.
pub(crate) const STICK_PRESS: f32 = 0.5;
pub(crate) const STICK_RELEASE: f32 = 0.35;
/// A held direction repeats after this, then every `REPEAT_EVERY`.
pub(crate) const REPEAT_AFTER: Duration = Duration::from_millis(250);
pub(crate) const REPEAT_EVERY: Duration = Duration::from_millis(100);

/// What a pad event comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Input {
    /// A button by its printed label, and the labels of the pad it's on.
    Button(PadButton, Style),
    /// The D-pad or stick, once on the press and again on each repeat.
    Move(Direction, Style),
}

/// Where a held direction comes from, so letting go of one source doesn't
/// stop a direction another started since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    DPad(PadId),
    Stick(PadId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    direction: Direction,
    source: Source,
    style: Style,
    next: Instant,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Stick {
    x: f32,
    y: f32,
    pressed: Option<Direction>,
}

/// The connected pads and what's held on them.
#[derive(Debug, Default)]
pub(crate) struct Pads {
    connected: BTreeMap<PadId, PadInfo>,
    sticks: BTreeMap<PadId, Stick>,
    /// The one direction that repeats. A new press of any direction takes
    /// over, so the D-pad and stick share one clock.
    held: Option<Held>,
    /// The labels of the pad pressed last.
    last_style: Option<Style>,
}

impl Pads {
    /// The connected pads' names, in the order they came, Steam's virtual
    /// pad left out while a real one is there.
    pub(crate) fn names(&self) -> Vec<String> {
        self.connected
            .iter()
            .filter(|(id, _)| self.read(**id))
            .map(|(_, info)| info.name.clone())
            .collect()
    }

    /// Whether events from `id` count. With Steam's virtual pad and a real
    /// one, only the real one is read.
    fn read(&self, id: PadId) -> bool {
        let Some(info) = self.connected.get(&id) else {
            // Events can come before Connected for a pad open at start.
            return true;
        };
        !info.steam_virtual() || self.connected.values().all(PadInfo::steam_virtual)
    }

    /// Whose labels to show away from a press: the pad pressed last, else
    /// the first one read.
    pub(crate) fn recent_style(&self) -> Style {
        self.last_style.unwrap_or_else(|| {
            self.connected
                .iter()
                .find(|(id, _)| self.read(**id))
                .map(|(_, info)| info.style())
                .unwrap_or_default()
        })
    }

    fn style(&self, id: PadId) -> Style {
        self.connected
            .get(&id)
            .map(PadInfo::style)
            .unwrap_or_default()
    }

    /// Takes one event and says what it comes to.
    pub(crate) fn event(&mut self, event: PadEvent, now: Instant) -> Option<Input> {
        match event {
            PadEvent::Connected(id, info) => {
                self.connected.insert(id, info);
                // A real pad arriving silences a virtual one mid-hold.
                if self.held.is_some_and(|held| !self.read(held.source.pad())) {
                    self.held = None;
                }
                None
            }
            PadEvent::Disconnected(id) => {
                self.connected.remove(&id);
                self.sticks.remove(&id);
                if self.held.is_some_and(|held| held.source.pad() == id) {
                    self.held = None;
                }
                None
            }
            _ if !self.read(event.pad()) => None,
            PadEvent::Pressed(id, raw) => {
                let style = self.style(id);
                self.last_style = Some(style);
                match raw.direction() {
                    Some(direction) => Some(self.hold(direction, Source::DPad(id), style, now)),
                    None => label(style, raw).map(|button| Input::Button(button, style)),
                }
            }
            PadEvent::Released(id, raw) => {
                if let Some(direction) = raw.direction() {
                    self.let_go(direction, Source::DPad(id));
                }
                None
            }
            PadEvent::Stick(id, axis, value) => {
                let style = self.style(id);
                let stick = self.sticks.entry(id).or_default();
                match axis {
                    Axis::X => stick.x = value,
                    Axis::Y => stick.y = value,
                }
                let (was, now_pressed) = stick.settle();
                if was == now_pressed {
                    return None;
                }
                if let Some(direction) = was {
                    self.let_go(direction, Source::Stick(id));
                }
                now_pressed.map(|direction| self.hold(direction, Source::Stick(id), style, now))
            }
        }
    }

    fn hold(&mut self, direction: Direction, source: Source, style: Style, now: Instant) -> Input {
        self.held = Some(Held {
            direction,
            source,
            style,
            next: now + REPEAT_AFTER,
        });
        Input::Move(direction, style)
    }

    fn let_go(&mut self, direction: Direction, source: Source) {
        if self
            .held
            .is_some_and(|held| held.direction == direction && held.source == source)
        {
            self.held = None;
        }
    }

    /// When the held direction repeats next.
    pub(crate) fn repeat_due(&self) -> Option<Instant> {
        self.held.map(|held| held.next)
    }

    /// The held direction once its time comes. A late wakeup repeats once,
    /// from now, rather than catching up.
    pub(crate) fn repeat(&mut self, now: Instant) -> Option<Input> {
        let held = self.held.as_mut()?;
        if now < held.next {
            return None;
        }
        held.next = now + REPEAT_EVERY;
        Some(Input::Move(held.direction, held.style))
    }

    /// Stops any repeat, for when the pad gate closes or the switch goes
    /// off.
    pub(crate) fn release_all(&mut self) {
        self.held = None;
        for stick in self.sticks.values_mut() {
            stick.pressed = None;
        }
    }
}

impl Source {
    fn pad(self) -> PadId {
        match self {
            Self::DPad(id) | Self::Stick(id) => id,
        }
    }
}

impl PadEvent {
    fn pad(&self) -> PadId {
        match self {
            Self::Connected(id, _)
            | Self::Disconnected(id)
            | Self::Pressed(id, _)
            | Self::Released(id, _)
            | Self::Stick(id, _, _) => *id,
        }
    }
}

impl Stick {
    /// Settles which direction the stick holds after a move, and returns it
    /// before and after. Only the dominant axis counts, so a sloppy
    /// diagonal stays one direction.
    fn settle(&mut self) -> (Option<Direction>, Option<Direction>) {
        let was = self.pressed;
        let along = |direction| match direction {
            Direction::Right => self.x,
            Direction::Left => -self.x,
            Direction::Up => self.y,
            Direction::Down => -self.y,
        };
        let dominant = if self.x.abs() >= self.y.abs() {
            if self.x >= 0.0 {
                Direction::Right
            } else {
                Direction::Left
            }
        } else if self.y >= 0.0 {
            Direction::Up
        } else {
            Direction::Down
        };
        let strong = along(dominant) >= STICK_PRESS;
        self.pressed = match was {
            // Turning to another axis or the other way counts once that
            // way is pressed in its own right.
            Some(held) if dominant != held && strong => Some(dominant),
            Some(held) if along(held) >= STICK_RELEASE => Some(held),
            _ if strong => Some(dominant),
            _ => None,
        };
        (was, self.pressed)
    }
}

#[cfg(test)]
mod tests;
