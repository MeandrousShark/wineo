//! The library grid's zoom: the keys, the wheel, and the slider that change
//! it, the tween between steps, keeping the reader's place, and saving.

use std::time::{Duration, Instant};

use iced::keyboard::{self, Modifiers};
use iced::mouse::ScrollDelta;
use iced::widget::scrollable;
use iced::{Subscription, Task, window};
use wineo_core::library::{ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

use crate::sort_control::Field;
use crate::{App, Message, SCROLL_ID, grid, grid_motion};

/// Keys and wheel notches move to the next multiple of this.
const STEP: u16 = 10;
const TWEEN: Duration = Duration::from_millis(160);
/// A drag snaps to 100% within this many points of it...
const SNAP_IN: f32 = 5.0;
/// ...and lets go only past this many, so it doesn't flicker at the edge.
const SNAP_OUT: f32 = 7.0;
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

    /// Settles a tween at its target, as when the Library is left.
    pub(crate) fn finish(&mut self) {
        if self.tween.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.moved();
        }
        self.gesture = None;
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

impl App {
    /// Zoom input counts while the Library shows with nothing over it: no
    /// menu, popup, flow, or dialog.
    pub(crate) fn zoom_allowed(&self) -> bool {
        self.library_menu_may_open()
    }

    /// Each card's height, top, and the grid's height at the shown layout.
    fn grid_rows(&self) -> (Vec<f32>, Vec<f32>, f32) {
        let layout = self.grid_layout();
        let heights: Vec<f32> = self
            .filtered()
            .iter()
            .map(|card| grid::card_height(card, layout))
            .collect();
        let (tops, height) = grid_motion::card_tops(&heights, self.columns());
        (heights, tops, height)
    }

    /// Starts a gesture unless one is running: the selected card anchors it
    /// when any of it is on screen, otherwise the first card in view.
    fn begin_gesture(&mut self) {
        if self.zooming.gesture.is_some() {
            return;
        }
        let (heights, tops, _) = self.grid_rows();
        let top = self.scroll_offset;
        let bottom = top + self.viewport_height;
        let in_view = |index: usize| tops[index] < bottom && tops[index] + heights[index] > top;
        let filtered = self.filtered();
        let selected = self
            .selected
            .as_ref()
            .and_then(|name| filtered.iter().position(|card| &card.name == name));
        let (anchor, reveal) = match selected {
            Some(index) if in_view(index) => (Some(index), false),
            selected => (
                (0..filtered.len()).find(|&index| in_view(index)),
                selected.is_some(),
            ),
        };
        let anchor = anchor.map(|index| (filtered[index].name.clone(), tops[index] - top));
        self.zooming.gesture = Some(Gesture { anchor, reveal });
    }

    /// Puts the anchor card's top back where the gesture found it, within
    /// the scroll range. A card taller than the viewport shows its top.
    fn hold_anchor(&mut self) -> Task<Message> {
        let Some((name, offset)) = self
            .zooming
            .gesture
            .as_ref()
            .and_then(|gesture| gesture.anchor.clone())
        else {
            return Task::none();
        };
        let Some(index) = self.filtered().iter().position(|card| card.name == name) else {
            return Task::none();
        };
        let (heights, tops, height) = self.grid_rows();
        let wanted = if heights[index] > self.viewport_height {
            tops[index]
        } else {
            tops[index] - offset
        };
        let offset = wanted.clamp(0.0, (height - self.viewport_height).max(0.0));
        self.on_scroll_offset(offset);
        iced::widget::operation::scroll_to(
            iced::widget::Id::new(SCROLL_ID),
            scrollable::AbsoluteOffset { x: 0.0, y: offset },
        )
    }

    /// Ends the gesture, bringing an off-screen selection into view like
    /// an arrow key would.
    fn end_gesture(&mut self) -> Task<Message> {
        let reveal = self
            .zooming
            .gesture
            .take()
            .is_some_and(|gesture| gesture.reveal);
        let art = self.load_visible_art();
        if reveal && self.selected_shown() {
            Task::batch([art, self.scroll_selection()])
        } else {
            art
        }
    }

    /// Ends the gesture's anchoring, for input that moves the grid by hand.
    pub(crate) fn interrupt_zoom(&mut self) {
        self.zooming.gesture = None;
    }

    /// Tweens to `target` from whatever the grid shows now and saves it.
    fn zoom_to(&mut self, target: u16, now: Instant) -> Task<Message> {
        if self.zooming.dragging() || target == self.zoom {
            return Task::none();
        }
        self.begin_gesture();
        let from = self.zooming.shown().unwrap_or_else(|| f32::from(self.zoom));
        self.zoom = target;
        self.zooming.tween = Some(Tween {
            from,
            to: target,
            start: now,
            shown: from,
        });
        self.zooming.generation = self.zooming.generation.wrapping_add(1);
        self.save_field(Field::Zoom(target))
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
        let hold = self.hold_anchor();
        if done {
            // A selection that moved during the tween waited for it.
            let moved = match self.grid_scroll_after_settle.take() {
                Some(name)
                    if !self.grid_motion.active() && self.selected.as_deref() == Some(&name) =>
                {
                    self.scroll_selection()
                }
                other => {
                    self.grid_scroll_after_settle = other;
                    Task::none()
                }
            };
            Task::batch([hold, self.end_gesture(), moved])
        } else {
            hold
        }
    }

    fn zoom_drag(&mut self, raw: f32) -> Task<Message> {
        self.begin_gesture();
        // A drag takes over from a tween at once, keeping its gesture.
        if self.zooming.tween.take().is_some() {
            self.zooming.generation = self.zooming.generation.wrapping_add(1);
            self.zooming.moved();
        }
        let (zoom, held) = snap(raw, self.zooming.drag.unwrap_or(false));
        self.zooming.drag = Some(held);
        if zoom == self.zoom {
            return Task::none();
        }
        self.zoom = zoom;
        self.zooming.moved();
        Task::batch([self.hold_anchor(), self.load_visible_art()])
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
        Event::TogglePosterOnly => {
            app.poster_only = !app.poster_only;
            app.save_field(Field::PosterOnly(app.poster_only))
        }
        Event::Interrupted => {
            app.interrupt_zoom();
            Task::none()
        }
        Event::Frame(generation, now) => app.zoom_frame(generation, now),
    }
}

#[cfg(test)]
mod tests;
