//! The poster star's and the poster-only overlay's fades. Each card's star
//! follows its hover and its overlay follows its hover or selection, over
//! the grid's card fade. Only cards that are still fading are stored: a
//! settled card's values are what its hover and selection say.

use std::collections::HashMap;
use std::time::Instant;

use crate::grid_motion::FADE;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Fade {
    star: f32,
    overlay: f32,
}

impl Fade {
    fn of(name: &str, hovered: Option<&str>, selected: Option<&str>) -> Self {
        let hovered = hovered == Some(name);
        let selected = selected == Some(name);
        Self {
            star: if hovered { 1.0 } else { 0.0 },
            overlay: if hovered || selected { 1.0 } else { 0.0 },
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct HoverFades {
    fades: HashMap<String, Fade>,
    /// The hover and selection last seen, which the fades head for. Kept
    /// apart from the app's own so a fade still knows where it was going
    /// after they change, and so a change by any path retargets.
    hovered: Option<String>,
    selected: Option<String>,
    last_tick: Option<Instant>,
    generation: u64,
}

impl HoverFades {
    /// Retargets the cards whose hover or selection changed since the last
    /// call, each from where it's drawn now.
    pub(crate) fn sync(&mut self, hovered: Option<&str>, selected: Option<&str>) {
        if hovered == self.hovered.as_deref() && selected == self.selected.as_deref() {
            return;
        }
        let was_active = self.active();
        let old_hovered = self.hovered.take();
        let old_selected = self.selected.take();
        let names = [
            old_hovered.as_deref(),
            old_selected.as_deref(),
            hovered,
            selected,
        ];
        for name in names.into_iter().flatten() {
            let before = Fade::of(name, old_hovered.as_deref(), old_selected.as_deref());
            if before != Fade::of(name, hovered, selected) {
                self.fades.entry(name.to_owned()).or_insert(before);
            }
        }
        self.hovered = hovered.map(str::to_owned);
        self.selected = selected.map(str::to_owned);
        // The first frame starts the clock, so a fade sets out from what's
        // on screen.
        if !was_active && self.active() {
            self.last_tick = None;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Puts `name`'s star at full strength: a hovered favorite's filled
    /// star hides it, so an unfavorite under the pointer shows the outline
    /// star at once.
    pub(crate) fn fill_star(&mut self, name: &str) {
        if let Some(fade) = self.fades.get_mut(name) {
            fade.star = 1.0;
        }
        self.drop_settled();
    }

    pub(crate) fn fading(&self, name: &str) -> bool {
        self.fades.contains_key(name)
    }

    /// Drops the fades of cards `shown` no longer has.
    pub(crate) fn retain(&mut self, shown: impl Fn(&str) -> bool) {
        self.fades.retain(|name, _| shown(name));
        if !self.active() {
            self.last_tick = None;
        }
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation || !self.active() {
            return;
        }
        let previous = self.last_tick.unwrap_or(now);
        self.last_tick = Some(now.max(previous));
        let step = now.saturating_duration_since(previous).as_secs_f32() / FADE.as_secs_f32();
        for (name, fade) in &mut self.fades {
            let target = Fade::of(name, self.hovered.as_deref(), self.selected.as_deref());
            fade.star = toward(fade.star, target.star, step);
            fade.overlay = toward(fade.overlay, target.overlay, step);
        }
        self.drop_settled();
    }

    fn drop_settled(&mut self) {
        let (hovered, selected) = (self.hovered.as_deref(), self.selected.as_deref());
        self.fades
            .retain(|name, fade| *fade != Fade::of(name, hovered, selected));
        if !self.active() {
            self.last_tick = None;
        }
    }

    /// The star's and the overlay's strength on `name`, 0 to 1.
    pub(crate) fn progress(&self, name: &str, hovered: bool, selected: bool) -> (f32, f32) {
        self.fades.get(name).map_or(
            (
                if hovered { 1.0 } else { 0.0 },
                if hovered || selected { 1.0 } else { 0.0 },
            ),
            |fade| (fade.star, fade.overlay),
        )
    }

    pub(crate) fn active(&self) -> bool {
        !self.fades.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Frames only while something fades. It isn't held back while the grid
    /// is covered, so a fade a flow or dialog opens over still finishes.
    pub(crate) fn subscription(&self) -> iced::Subscription<crate::Message> {
        if self.active() {
            iced::window::frames()
                .with(self.generation)
                .map(|(generation, now)| crate::Message::HoverTick(generation, now))
        } else {
            iced::Subscription::none()
        }
    }
}

/// Linear, like the cards' own fade.
fn toward(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}
