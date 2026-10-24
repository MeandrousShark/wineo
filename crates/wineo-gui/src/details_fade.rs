//! The cards' details fade: the identifier and the state and playtime row
//! fading and drifting into place when the zoom crosses into full text, and
//! out when it leaves. One progress for the whole grid, so a card that joins
//! mid-crossing reads the same value as the rest.

use std::time::Instant;

use crate::zoom::{TWEEN, ease_out};

/// How far below their place the details start on the way in, and end on
/// the way out.
pub(crate) const DRIFT: f32 = 8.0;

#[derive(Debug, Clone, Copy)]
struct Crossing {
    from: f32,
    /// Set by the first frame, so the fade sets out from what's on screen.
    start: Option<Instant>,
}

#[derive(Debug, Default)]
pub(crate) struct DetailsFade {
    /// Whether the cards show their details, as last seen. Kept apart from
    /// the zoom so a change by any path retargets.
    full: Option<bool>,
    crossing: Option<Crossing>,
    value: f32,
    generation: u64,
}

impl DetailsFade {
    /// Heads for `full` from where the details are drawn now. The first
    /// call only records it, so the saved zoom doesn't fade in at launch.
    pub(crate) fn sync(&mut self, full: bool) {
        let Some(was) = self.full else {
            self.full = Some(full);
            self.value = target(full);
            return;
        };
        if was == full {
            return;
        }
        self.full = Some(full);
        if self.crossing.is_none() {
            self.generation = self.generation.wrapping_add(1);
        }
        self.crossing = Some(Crossing {
            from: self.value,
            start: None,
        });
    }

    /// Puts the details at `full` at once, ending any crossing, for the
    /// poster-only toggle, which keeps its spring and nothing more.
    pub(crate) fn snap(&mut self, full: bool) {
        self.full = Some(full);
        self.value = target(full);
        self.crossing = None;
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation {
            return;
        }
        let (Some(crossing), Some(full)) = (&mut self.crossing, self.full) else {
            return;
        };
        let start = *crossing.start.get_or_insert(now);
        let t = (now.saturating_duration_since(start).as_secs_f32() / TWEEN.as_secs_f32())
            .clamp(0.0, 1.0);
        let to = target(full);
        self.value = crossing.from + (to - crossing.from) * ease_out(t);
        if t >= 1.0 {
            self.value = to;
            self.crossing = None;
        }
    }

    /// The details' strength, 0 to 1. At rest it's what `full` says, the
    /// cards' text as the grid is built now.
    pub(crate) fn progress(&self, full: bool) -> f32 {
        if self.crossing.is_some() {
            self.value
        } else {
            target(full)
        }
    }

    /// How far below their place the details draw at `progress`.
    pub(crate) fn drift(progress: f32) -> f32 {
        DRIFT * (1.0 - progress)
    }

    pub(crate) fn active(&self) -> bool {
        self.crossing.is_some()
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Frames only while crossing. Like the hover fades, it isn't held back
    /// while the grid is covered, so a crossing a flow opens over or the
    /// Library leaves still finishes.
    pub(crate) fn subscription(&self) -> iced::Subscription<crate::Message> {
        if self.active() {
            iced::window::frames()
                .with(self.generation)
                .map(|(generation, now)| crate::Message::DetailsTick(generation, now))
        } else {
            iced::Subscription::none()
        }
    }
}

fn target(full: bool) -> f32 {
    if full { 1.0 } else { 0.0 }
}
