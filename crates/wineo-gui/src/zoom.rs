//! The library grid's zoom: the keys, the wheel, and the slider that change
//! it, the tween between steps, keeping the reader's place, and saving.

use std::time::{Duration, Instant};

use iced::keyboard::{self, Modifiers};
use iced::mouse::ScrollDelta;
use iced::widget::scrollable;
use iced::{Subscription, Task, window};
use wineo_core::library::{ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

use crate::sort_control::Field;
use crate::{App, Message, SCROLL_ID};

/// Keys and wheel notches move to the next multiple of this.
const STEP: u16 = 10;
/// The details fade shares it, so they land with the zoom.
pub(crate) const TWEEN: Duration = Duration::from_millis(160);
/// A drag snaps to 100% within this many points of it...
const SNAP_IN: f32 = 8.0;
/// ...and lets go only past this many, so it doesn't flicker at the edge.
/// At 11 or more, a drag would hold through 90% and 110% without showing them.
const SNAP_OUT: f32 = 10.0;
/// Trackpad pixels that make one step: what iced scrolls for a wheel line.
const PIXELS_PER_STEP: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    In,
    Out,
    Reset,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Key(Key),
    /// The slider's arrow keys, up when true.
    Step(bool),
    /// A Ctrl+wheel turn over the grid or the slider.
    Wheel(ScrollDelta),
    /// The pointer left the grid, which drops a partial trackpad step.
    PointerLeft,
    /// The slider's thumb is at this zoom, before snapping.
    Drag(f32),
    DragEnded,
    TogglePosterOnly,
    /// A scroll or click on the grid, which ends the gesture's anchoring.
    Interrupted,
    Frame(u64, Instant),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // A drag must always be able to end, or it would never save.
        !matches!(
            self,
            Self::Frame(..) | Self::DragEnded | Self::PointerLeft | Self::Interrupted
        )
    }
}

/// What a zoom key does, if it is one: Ctrl with `=` or `+` (Shift and the
/// keypad give the same key) zooms in, with `-` out, and with `0` resets.
pub(crate) fn key(key: &keyboard::Key, modifiers: Modifiers) -> Option<Key> {
    if !modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character("=" | "+") => Some(Key::In),
        keyboard::Key::Character("-") => Some(Key::Out),
        keyboard::Key::Character("0") => Some(Key::Reset),
        _ => None,
    }
}

/// The next multiple of 10 from `from` in the step's direction, so steps
/// from a dragged zoom always pass through 100.
pub(crate) fn step(from: u16, up: bool) -> u16 {
    let next = if up {
        (from / STEP + 1) * STEP
    } else {
        from.div_ceil(STEP).saturating_sub(1) * STEP
    };
    next.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The zoom a drag at `raw` shows, and whether it's held at 100%.
pub(crate) fn snap(raw: f32, held: bool) -> (u16, bool) {
    let distance = (raw - f32::from(ZOOM_DEFAULT)).abs();
    if distance <= SNAP_IN || (held && distance <= SNAP_OUT) {
        (ZOOM_DEFAULT, true)
    } else {
        let zoom = raw.round().clamp(f32::from(ZOOM_MIN), f32::from(ZOOM_MAX));
        (zoom as u16, false)
    }
}

#[derive(Debug, Clone, Copy)]
struct Tween {
    from: f32,
    to: u16,
    start: Instant,
    shown: f32,
}

/// A drag or a run of steps. The anchor is a card's name and its top's
/// offset in the viewport, found once when the gesture starts.
#[derive(Debug, Clone, Default)]
struct Gesture {
    anchor: Option<(String, f32)>,
    /// The selection was off screen, so it scrolls into view at the end.
    reveal: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Zoom {
    tween: Option<Tween>,
    generation: u64,
    /// Whether a drag in progress is held at 100%.
    drag: Option<bool>,
    gesture: Option<Gesture>,
    /// Trackpad pixels toward the next step, signed by direction.
    pixels: f32,
    /// Grows each time the zoom changes the drawn geometry.
    revision: u64,
}

impl Zoom {
    /// The zoom a tween frame shows, while one runs.
    pub(crate) fn shown(&self) -> Option<f32> {
        self.tween.map(|tween| tween.shown)
    }

    pub(crate) fn active(&self) -> bool {
        self.tween.is_some()
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    fn moved(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// A tween or a drag is changing the grid, so a card's place isn't
    /// final yet.
    pub(crate) fn moving(&self) -> bool {
        self.tween.is_some() || self.drag.is_some()
    }

    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A gesture is keeping a card in place.
    pub(crate) fn anchoring(&self) -> bool {
        self.gesture
            .as_ref()
            .is_some_and(|gesture| gesture.anchor.is_some())
    }

    /// Frames only while a tween runs, so an idle grid asks for none.
    pub(crate) fn subscription(&self) -> Subscription<Message> {
        if self.active() {
            window::frames()
                .with(self.generation)
                .map(|(generation, now)| Message::Zoom(Event::Frame(generation, now)))
        } else {
            Subscription::none()
        }
    }

    /// Ctrl came up: a partial trackpad step doesn't carry over.
    pub(crate) fn modifiers_changed(&mut self, modifiers: Modifiers) {
        if !modifiers.control() {
            self.pixels = 0.0;
        }
    }

    /// Whole steps from a wheel turn, keeping what's left of a trackpad's.
    fn wheel_steps(&mut self, delta: ScrollDelta) -> i32 {
        let amount = match delta {
            ScrollDelta::Lines { y, .. } => y * PIXELS_PER_STEP,
            ScrollDelta::Pixels { y, .. } => y,
        };
        if amount == 0.0 {
            return 0;
        }
        if self.pixels != 0.0 && amount.signum() != self.pixels.signum() {
            self.pixels = 0.0;
        }
        self.pixels += amount;
        let steps = (self.pixels / PIXELS_PER_STEP).trunc();
        self.pixels -= steps * PIXELS_PER_STEP;
        steps as i32
    }

    /// Settles a tween at its target.
    fn finish(&mut self) {
        if self.tween.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.moved();
        }
    }
}

pub(crate) fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

impl App {
    /// Zoom input counts while the Library shows with nothing over it: no
    /// menu, popup, flow, or dialog.
    pub(crate) fn zoom_allowed(&self) -> bool {
        self.library_menu_may_open()
    }

    /// The card that keeps the place: the selected one when any of it is on
    /// screen, otherwise the first card in view, with its top's offset in
    /// the viewport, read from where cards are drawn now, so a place kept
    /// mid-motion is the one on screen. Also whether the selection is off
    /// screen.
    fn find_anchor(&self) -> (Option<(String, f32)>, bool) {
        let places = self.grid_places();
        let shown = self.grid_motion.shown(&places);
        let top = self.scroll_offset;
        let bottom = top + self.viewport_height;
        let in_view = |index: usize| {
            let card: iced::Rectangle = shown[index];
            card.y < bottom && card.y + card.height > top
        };
        let selected = self.selected.as_ref().and_then(|name| places.index(name));
        let (anchor, reveal) = match selected {
            Some(index) if in_view(index) => (Some(index), false),
            selected => (
                (0..shown.len()).find(|&index| in_view(index)),
                selected.is_some(),
            ),
        };
        let anchor = anchor.map(|index| (places.names[index].clone(), shown[index].y - top));
        (anchor, reveal)
    }

    /// Starts a gesture unless one is running.
    fn begin_gesture(&mut self) {
        if self.zooming.gesture.is_some() {
            return;
        }
        let (anchor, reveal) = self.find_anchor();
        self.grid_motion.anchor = None;
        self.zooming.gesture = Some(Gesture { anchor, reveal });
    }

    /// Puts the anchor card's top back where the gesture found it, within
    /// the scroll range. A card taller than the viewport shows its top.
    /// After the zoom stops, the cards it set moving keep the anchor too.
    pub(crate) fn hold_anchor(&mut self) -> Task<Message> {
        let Some((name, offset)) = self
            .zooming
            .gesture
            .as_ref()
            .and_then(|gesture| gesture.anchor.clone())
            .or_else(|| self.grid_motion.anchor.clone())
        else {
            return Task::none();
        };
        let places = self.grid_places();
        let Some(index) = places.index(&name) else {
            return Task::none();
        };
        let card = self.grid_motion.drawn(&places, index);
        let height = self.grid_motion.content_height(&places);
        let wanted = if card.height > self.viewport_height {
            card.y
        } else {
            card.y - offset
        };
        let offset = wanted.clamp(0.0, (height - self.viewport_height).max(0.0));
        self.on_scroll_offset(offset);
        iced::widget::operation::scroll_to(
            iced::widget::Id::new(SCROLL_ID),
            scrollable::AbsoluteOffset { x: 0.0, y: offset },
        )
    }

    /// Gives the cards their slots at the zoom now shown: they spring when
    /// the column count changed and only scale otherwise.
    fn zoom_grid(&mut self, now: Instant) {
        let places = self.grid_places();
        self.grid_motion.zoom_to(&places, now);
    }

    /// Where every way a zoom stops moving ends: a tween's last frame, a
    /// drag's release, closing, or leaving the Library. It brings the
    /// selection into view, once, if the gesture found it off screen or a
    /// key moved it while the zoom was moving. The dock's motion keeps a
    /// scroll it is still waiting for.
    fn end_gesture(&mut self) -> Task<Message> {
        let gesture = self.zooming.gesture.take().unwrap_or_default();
        let reveal = gesture.reveal;
        if self.grid_motion.active() {
            self.grid_motion.anchor = gesture.anchor;
        }
        // An off-screen selection joins the scroll a key may have left
        // waiting, so it happens once, now or when the dock settles.
        if reveal {
            self.grid_scroll_after_settle = self.selected.clone();
        }
        let waited = self.motion_ended(true);
        if self.panel_hidden() {
            return Task::none();
        }
        Task::batch([self.load_visible_art(), waited])
    }

    /// Stops the zoom where it's heading, as when the Library is covered
    /// or left.
    pub(crate) fn stop_zoom(&mut self) -> Task<Message> {
        if !self.zooming.moving() {
            return Task::none();
        }
        self.zooming.finish();
        let places = self.grid_places();
        if self.panel_hidden() {
            self.grid_motion.snap_to(&places);
        } else {
            self.grid_motion.zoom_to(&places, Instant::now());
        }
        if self.zooming.dragging() {
            self.end_zoom_drag()
        } else {
            self.end_gesture()
        }
    }

    /// Ends the gesture's anchoring, for input that moves the grid by hand.
    pub(crate) fn interrupt_zoom(&mut self) {
        self.zooming.gesture = None;
        self.grid_motion.anchor = None;
    }

    /// Tweens to `target` from whatever the grid shows now and saves it.
    fn zoom_to(&mut self, target: u16, now: Instant) -> Task<Message> {
        if self.zooming.dragging() || target == self.zoom {
            return Task::none();
        }
        self.begin_gesture();
        let from = self.zooming.shown().unwrap_or_else(|| f32::from(self.zoom));
        let text = self.grid_layout().text();
        self.zoom = target;
        self.zooming.tween = Some(Tween {
            from,
            to: target,
            start: now,
            shown: from,
        });
        self.zooming.generation = self.zooming.generation.wrapping_add(1);
        let save = self.save_field(Field::Zoom(target));
        // Crossing 90% changes the cards' text, and so their rows, as the
        // tween sets out: they spring there now, like the poster-only
        // toggle, rather than snap before the first frame.
        if self.grid_layout().text() != text {
            self.zooming.moved();
            self.zoom_grid(now);
            return Task::batch([save, self.hold_anchor()]);
        }
        save
    }

    fn zoom_frame(&mut self, generation: u64, now: Instant) -> Task<Message> {
        if generation != self.zooming.generation {
            return Task::none();
        }
        let Some(tween) = &mut self.zooming.tween else {
            return Task::none();
        };
        let t = (now.saturating_duration_since(tween.start).as_secs_f32() / TWEEN.as_secs_f32())
            .clamp(0.0, 1.0);
        tween.shown = tween.from + (f32::from(tween.to) - tween.from) * ease_out(t);
        let done = t >= 1.0;
        self.zooming.moved();
        if done {
            self.zooming.tween = None;
            self.zooming.generation = self.zooming.generation.wrapping_add(1);
        }
        self.zoom_grid(now);
        let hold = self.hold_anchor();
        if done {
            Task::batch([hold, self.end_gesture()])
        } else {
            hold
        }
    }

    fn zoom_drag(&mut self, raw: f32) -> Task<Message> {
        self.begin_gesture();
        // A drag takes over from a tween at once, keeping its gesture.
        let now = Instant::now();
        if self.zooming.tween.take().is_some() {
            self.zooming.generation = self.zooming.generation.wrapping_add(1);
            self.zooming.moved();
            self.zoom_grid(now);
        }
        let (zoom, held) = snap(raw, self.zooming.drag.unwrap_or(false));
        self.zooming.drag = Some(held);
        if zoom == self.zoom {
            return Task::none();
        }
        self.zoom = zoom;
        self.zooming.moved();
        self.zoom_grid(now);
        Task::batch([self.hold_anchor(), self.load_visible_art()])
    }

    /// Flips poster-only and moves the cards to the rows it gives them,
    /// keeping the place the way a zoom does, and saves it.
    fn toggle_poster_only(&mut self) -> Task<Message> {
        // Found in the rows shown before the flip. A zoom in progress keeps
        // its own anchor.
        let anchor = if self.zooming.gesture.is_some() {
            None
        } else {
            self.find_anchor().0
        };
        self.poster_only = !self.poster_only;
        self.details_fade.snap(self.details_full());
        let save = self.save_field(Field::PosterOnly(self.poster_only));
        let waited = self.move_grid();
        if !self.grid_motion.active() {
            return Task::batch([save, waited]);
        }
        if anchor.is_some() {
            self.grid_motion.anchor = anchor;
        }
        Task::batch([save, self.hold_anchor(), self.load_visible_art()])
    }

    /// Ends a drag, saving where it let go. Closing the window ends one too.
    pub(crate) fn end_zoom_drag(&mut self) -> Task<Message> {
        if self.zooming.drag.take().is_none() {
            return Task::none();
        }
        let save = self.save_field(Field::Zoom(self.zoom));
        Task::batch([save, self.end_gesture()])
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    if event.is_input() && !app.zoom_allowed() {
        return Task::none();
    }
    let now = Instant::now();
    match event {
        Event::Key(Key::In) => app.zoom_to(step(app.zoom, true), now),
        Event::Key(Key::Out) => app.zoom_to(step(app.zoom, false), now),
        Event::Key(Key::Reset) => app.zoom_to(ZOOM_DEFAULT, now),
        Event::Step(up) => app.zoom_to(step(app.zoom, up), now),
        Event::Wheel(delta) => {
            let steps = app.zooming.wheel_steps(delta);
            let mut target = app.zoom;
            for _ in 0..steps.unsigned_abs() {
                target = step(target, steps > 0);
            }
            app.zoom_to(target, now)
        }
        Event::PointerLeft => {
            app.zooming.pixels = 0.0;
            Task::none()
        }
        Event::Drag(raw) => app.zoom_drag(raw),
        Event::DragEnded => app.end_zoom_drag(),
        Event::TogglePosterOnly => app.toggle_poster_only(),
        Event::Interrupted => {
            app.interrupt_zoom();
            Task::none()
        }
        Event::Frame(generation, now) => app.zoom_frame(generation, now),
    }
}

#[cfg(test)]
mod tests;
