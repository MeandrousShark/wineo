//! A presentation-only collapse of the Library grid: the shelves shake, the
//! visible cards drop into a heap at the bottom of the view, and a click puts
//! them back. Only drawn positions, rotation, and opacity change; the grid's
//! layout, scroll position, selection, and data never do, so any real change
//! to the grid can simply reset this to idle.

use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

use iced::widget::image;
use iced::{Point, Vector};

use crate::grid_motion::{GridMotion, bounded_axis};

const CLICKS: usize = 10;
const WINDOW: Duration = Duration::from_secs(3);
const SHAKE: Duration = Duration::from_millis(280);
const SHAKE_AMPLITUDE: f32 = 7.0;
const GRAVITY: f32 = 3200.0;
const RESTITUTION: f32 = 0.38;
const BOUNCES: usize = 3;
const MAX_DELAY: f32 = 0.12;
const MAX_DRIFT: f32 = 90.0;
const MAX_SPIN: f32 = 1.6;
const MAX_HOP: f32 = 420.0;
const STACK_STEP: f32 = 26.0;
pub(crate) const RESTORE_AFTER: Duration = Duration::from_secs(12);
const FADE_OUT: Duration = Duration::from_secs(3);
const FADE_IN: Duration = Duration::from_millis(800);
/// The poster's half size inside its 2 px selection frame.
const HALF_POSTER: Vector = Vector::new(78.0, 118.0);
const FRAME: f32 = 2.0;

/// Recent Library row clicks. Only a burst inside a short sliding window
/// counts, so ordinary navigation never gets near it.
#[derive(Debug, Default)]
pub(crate) struct Clicks(VecDeque<Instant>);

impl Clicks {
    /// Records a click and reports whether it completed a burst.
    pub(crate) fn record(&mut self, now: Instant) -> bool {
        self.0.push_back(now);
        while self
            .0
            .front()
            .is_some_and(|first| now.duration_since(*first) > WINDOW)
        {
            self.0.pop_front();
        }
        if self.0.len() >= CLICKS {
            self.0.clear();
            true
        } else {
            false
        }
    }
}

/// One falling card's randomness, picked once at the start.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fall {
    delay: f32,
    drift: f32,
    spin: f32,
    hop: f32,
    stack: f32,
}

/// The view the heap lands in: its bottom edge and width in grid content
/// coordinates, and how many columns the cards wrap at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Area {
    pub(crate) floor: f32,
    pub(crate) width: f32,
    pub(crate) columns: usize,
}

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    Down {
        started: Instant,
        area: Area,
        /// `None` for cards outside the view, which hide rather than fall.
        falls: Vec<Option<Fall>>,
        placeholder: image::Handle,
        resting: bool,
    },
    Rebuild {
        area: Area,
        falls: Vec<Option<Fall>>,
        placeholder: image::Handle,
        /// Seconds into the fall when the rebuild began, which fixes where
        /// each card flies back from.
        from: f32,
        motion: GridMotion,
    },
    FadeOut {
        started: Instant,
    },
    Gone,
    FadeIn {
        started: Instant,
        from: f32,
    },
}

/// How the grid draws one card.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Look {
    /// The real card, moved by `offset`.
    Normal(Vector),
    Hidden,
    /// Only the poster, drawn rotated with its node at `position`.
    Loose {
        position: Point,
        rotation: f32,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Collapse {
    pub(crate) clicks: Clicks,
    phase: Phase,
    now: Option<Instant>,
    generation: u64,
}

impl Collapse {
    /// Starts the drop. `visible` are the cards in view; higher rows land
    /// on top of lower ones.
    pub(crate) fn fall(
        &mut self,
        now: Instant,
        count: usize,
        visible: Range<usize>,
        area: Area,
        placeholder: image::Handle,
        seed: u64,
    ) {
        let columns = area.columns.max(1);
        let last_row = visible.end.saturating_sub(1) / columns;
        let max_stack = (area.floor * 0.5).max(0.0);
        let falls = (0..count)
            .map(|index| {
                visible.contains(&index).then(|| {
                    let mut random = Random::new(seed ^ (index as u64).wrapping_mul(0x9e37));
                    let rows_below = (last_row - index / columns) as f32;
                    Fall {
                        delay: random.unit() * MAX_DELAY,
                        drift: random.signed() * MAX_DRIFT,
                        spin: random.signed() * MAX_SPIN,
                        hop: random.unit() * MAX_HOP,
                        stack: (rows_below * STACK_STEP + random.unit() * 14.0).min(max_stack),
                    }
                })
            })
            .collect();
        self.begin(
            now,
            Phase::Down {
                started: now,
                area,
                falls,
                placeholder,
                resting: false,
            },
        );
    }

    /// The reduced motion version: the cards fade away instead of falling.
    pub(crate) fn fade(&mut self, now: Instant) {
        self.begin(now, Phase::FadeOut { started: now });
    }

    fn begin(&mut self, now: Instant, phase: Phase) {
        self.phase = phase;
        self.now = Some(now);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Puts everything back at once.
    pub(crate) fn reset(&mut self) {
        if !self.idle() {
            self.phase = Phase::Idle;
            self.now = None;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Starts putting the cards back. Ignored while they are already on
    /// their way.
    pub(crate) fn restore(&mut self, now: Instant) {
        let alpha = self.alpha();
        match std::mem::take(&mut self.phase) {
            Phase::Down {
                started,
                area,
                falls,
                placeholder,
                ..
            } => {
                let mut motion = GridMotion::default();
                motion.snap(false, falls.len());
                motion.retarget(true, falls.len(), now);
                self.phase = Phase::Rebuild {
                    area,
                    falls,
                    placeholder,
                    from: fall_seconds(started, now),
                    motion,
                };
            }
            Phase::FadeOut { .. } | Phase::Gone => {
                self.phase = Phase::FadeIn {
                    started: now,
                    from: alpha,
                };
            }
            phase => self.phase = phase,
        }
        self.now = Some(now);
    }

    pub(crate) fn tick(&mut self, generation: u64, now: Instant) {
        if generation != self.generation || !self.animating() {
            return;
        }
        self.now = Some(now);
        let finished = match &mut self.phase {
            Phase::Down {
                started,
                falls,
                resting,
                area,
                ..
            } => {
                let settle = falls
                    .iter()
                    .flatten()
                    .map(|fall| fall.delay + fall.settle(area.floor))
                    .fold(0.0, f32::max);
                if fall_seconds(*started, now) >= settle {
                    *resting = true;
                }
                false
            }
            Phase::Rebuild { motion, .. } => {
                motion.tick(motion.generation(), now);
                !motion.active()
            }
            Phase::FadeOut { started } => {
                if now.duration_since(*started) >= FADE_OUT {
                    self.phase = Phase::Gone;
                }
                false
            }
            Phase::FadeIn { started, .. } => now.duration_since(*started) >= FADE_IN,
            Phase::Idle | Phase::Gone => false,
        };
        if finished {
            self.reset();
        }
    }

    pub(crate) fn idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    /// True while frames are needed. A resting heap and an empty library
    /// stay still, so they don't keep the app busy.
    pub(crate) fn animating(&self) -> bool {
        match &self.phase {
            Phase::Idle | Phase::Gone => false,
            Phase::Down { resting, .. } => !resting,
            Phase::Rebuild { .. } | Phase::FadeOut { .. } | Phase::FadeIn { .. } => true,
        }
    }

    #[cfg(test)]
    pub(crate) fn resting(&self) -> bool {
        matches!(self.phase, Phase::Down { resting: true, .. })
    }

    #[cfg(test)]
    pub(crate) fn gone(&self) -> bool {
        matches!(self.phase, Phase::Gone)
    }

    /// Whether a click on the cards or the empty library brings them back.
    pub(crate) fn restorable(&self) -> bool {
        matches!(
            self.phase,
            Phase::Down { .. } | Phase::FadeOut { .. } | Phase::Gone
        )
    }

    /// Whether the empty library area restores, which only the faded
    /// version offers since its cards leave nothing to click.
    pub(crate) fn faded(&self) -> bool {
        matches!(self.phase, Phase::FadeOut { .. } | Phase::Gone)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn placeholder(&self) -> Option<&image::Handle> {
        match &self.phase {
            Phase::Down { placeholder, .. } | Phase::Rebuild { placeholder, .. } => {
                Some(placeholder)
            }
            _ => None,
        }
    }

    /// The whole card's opacity, for the faded version.
    pub(crate) fn alpha(&self) -> f32 {
        let now = self.now.unwrap_or_else(Instant::now);
        match &self.phase {
            Phase::FadeOut { started } => 1.0 - smooth(progress(*started, now, FADE_OUT)),
            Phase::Gone => 0.0,
            Phase::FadeIn { started, from } => {
                from + (1.0 - from) * smooth(progress(*started, now, FADE_IN))
            }
            _ => 1.0,
        }
    }

    /// The title and metadata's opacity. They fade during the shake, since
    /// only the poster can be drawn rotated.
    pub(crate) fn caption_alpha(&self) -> f32 {
        match &self.phase {
            Phase::Down { started, .. } => {
                1.0 - progress(*started, self.now.unwrap_or(*started), SHAKE)
            }
            Phase::Rebuild { .. } => 0.0,
            _ => 1.0,
        }
    }

    /// How card `index` is drawn, given where the grid really puts it.
    pub(crate) fn look(&self, index: usize, slot: Point) -> Look {
        match &self.phase {
            Phase::Down {
                started,
                area,
                falls,
                ..
            } => {
                let now = self.now.unwrap_or(*started);
                let elapsed = now.duration_since(*started);
                if elapsed < SHAKE {
                    return Look::Normal(shake(elapsed));
                }
                match falls.get(index).copied().flatten() {
                    Some(fall) => {
                        let (position, rotation) =
                            fall.pose(slot, *area, fall_seconds(*started, now));
                        Look::Loose { position, rotation }
                    }
                    None => Look::Hidden,
                }
            }
            Phase::Rebuild {
                area,
                falls,
                from,
                motion,
                ..
            } => match falls.get(index).copied().flatten() {
                Some(fall) => {
                    let (start, rotation) = fall.pose(slot, *area, *from);
                    let progress = motion.progress(index);
                    Look::Loose {
                        position: Point::new(
                            bounded_axis(start.x, slot.x, progress),
                            bounded_axis(start.y, slot.y, progress),
                        ),
                        rotation: rotation * (1.0 - progress.clamp(0.0, 1.0)),
                    }
                }
                None => Look::Normal(Vector::ZERO),
            },
            Phase::Gone => Look::Hidden,
            _ => Look::Normal(Vector::ZERO),
        }
    }
}

impl Fall {
    /// Where the card's node rests and the angle it lies at.
    fn rest(&self, slot: Point, area: Area) -> (Point, f32) {
        let rotation = self.spin;
        // The lowest point of the rotated poster touches the floor.
        let extent = HALF_POSTER.x * rotation.sin().abs() + HALF_POSTER.y * rotation.cos().abs();
        let center_y = area.floor - extent - self.stack;
        let max_x = (area.width - 2.0 * (HALF_POSTER.x + FRAME)).max(0.0);
        let x = (slot.x + self.drift).clamp(0.0, max_x);
        (Point::new(x, center_y - HALF_POSTER.y - FRAME), rotation)
    }

    /// The upward speed at release. Cards already below their resting
    /// height need enough to rise to it before they drop.
    fn launch(&self, height: f32) -> f32 {
        -(self.hop + (-2.0 * GRAVITY * height).max(0.0).sqrt())
    }

    fn landing(&self, height: f32) -> (f32, f32) {
        let v0 = self.launch(height);
        let time = (-v0 + (v0 * v0 + 2.0 * GRAVITY * height).max(0.0).sqrt()) / GRAVITY;
        (time, v0 + GRAVITY * time)
    }

    /// An upper bound on the seconds from release until the last bounce
    /// ends. The slot isn't known here, so this takes the worse of the
    /// longest drop (from the top of the content) and the longest climb
    /// (a card whose slot sits at the floor, below where it rests).
    fn settle(&self, floor: f32) -> f32 {
        let climb =
            -(2.0 * HALF_POSTER.y + 2.0 * FRAME + HALF_POSTER.x.hypot(HALF_POSTER.y) + self.stack);
        [floor.max(0.0), climb]
            .into_iter()
            .map(|height| {
                let (mut total, mut speed) = self.landing(height);
                for _ in 0..BOUNCES {
                    speed *= RESTITUTION;
                    total += 2.0 * speed / GRAVITY;
                }
                total
            })
            .fold(0.0, f32::max)
    }

    fn pose(&self, slot: Point, area: Area, seconds: f32) -> (Point, f32) {
        let t = seconds - self.delay;
        if t <= 0.0 {
            return (slot, 0.0);
        }
        let (rest, rotation) = self.rest(slot, area);
        let height = rest.y - slot.y;
        let v0 = self.launch(height);
        let (landing, impact) = self.landing(height);
        let travel = 1.0 - (1.0 - (t / landing).min(1.0)).powi(2);
        let x = slot.x + (rest.x - slot.x) * travel;
        let angle = rotation * travel;
        if t < landing {
            return (
                Point::new(x, slot.y + v0 * t + 0.5 * GRAVITY * t * t),
                angle,
            );
        }
        let mut t = t - landing;
        let mut speed = impact;
        for _ in 0..BOUNCES {
            speed *= RESTITUTION;
            let air = 2.0 * speed / GRAVITY;
            if t < air {
                return (
                    Point::new(x, rest.y - (speed * t - 0.5 * GRAVITY * t * t)),
                    angle,
                );
            }
            t -= air;
        }
        (rest, rotation)
    }
}

/// Seconds since the shake ended.
fn fall_seconds(started: Instant, now: Instant) -> f32 {
    now.duration_since(started)
        .saturating_sub(SHAKE)
        .as_secs_f32()
}

fn shake(elapsed: Duration) -> Vector {
    let t = elapsed.as_secs_f32();
    let decay = 1.0 - progress_of(elapsed, SHAKE);
    Vector::new(
        SHAKE_AMPLITUDE * decay * (t * 95.0).sin(),
        SHAKE_AMPLITUDE * 0.6 * decay * (t * 130.0).cos(),
    )
}

fn progress(started: Instant, now: Instant, duration: Duration) -> f32 {
    progress_of(now.duration_since(started), duration)
}

fn progress_of(elapsed: Duration, duration: Duration) -> f32 {
    (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

fn smooth(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// splitmix64, enough for picking spins and scatter without a crate.
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder() -> image::Handle {
        image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255])
    }

    const AREA: Area = Area {
        floor: 700.0,
        width: 900.0,
        columns: 3,
    };

    #[test]
    fn ten_clicks_inside_three_seconds_complete_a_burst() {
        let start = Instant::now();
        let mut clicks = Clicks::default();
        for index in 0..9 {
            assert!(!clicks.record(start + Duration::from_millis(index * 200)));
        }
        assert!(clicks.record(start + Duration::from_millis(1800)));
        // The burst starts over rather than firing on every later click.
        assert!(!clicks.record(start + Duration::from_millis(1900)));
    }

    #[test]
    fn slow_clicks_never_complete_a_burst() {
        let start = Instant::now();
        let mut clicks = Clicks::default();
        for index in 0..30 {
            assert!(!clicks.record(start + Duration::from_millis(index * 350)));
        }
    }

    #[test]
    fn cards_fall_to_rest_above_the_floor_and_go_quiet() {
        let start = Instant::now();
        let mut collapse = Collapse::default();
        collapse.fall(start, 6, 0..4, AREA, placeholder(), 7);
        let generation = collapse.generation();
        assert!(collapse.animating());
        let slot = Point::new(16.0, 16.0);
        assert!(matches!(collapse.look(0, slot), Look::Normal(_)));
        collapse.tick(generation, start + Duration::from_millis(400));
        assert!(matches!(collapse.look(0, slot), Look::Loose { .. }));
        assert_eq!(collapse.look(5, slot), Look::Hidden);
        assert_eq!(collapse.caption_alpha(), 0.0);
        collapse.tick(generation, start + Duration::from_secs(4));
        assert!(collapse.resting());
        assert!(!collapse.animating());
        for index in 0..4 {
            let Look::Loose { position, .. } = collapse.look(index, slot) else {
                panic!("card {index} should lie in the heap");
            };
            let bottom = position.y + FRAME + 2.0 * HALF_POSTER.y;
            assert!(bottom > AREA.floor - 2.0 * HALF_POSTER.y - 60.0);
            assert!(position.y + FRAME + HALF_POSTER.y < AREA.floor);
            assert!(position.x >= 0.0 && position.x + 160.0 <= AREA.width);
        }
    }

    #[test]
    fn rebuild_flies_back_to_the_slots_and_ends_idle() {
        let start = Instant::now();
        let mut collapse = Collapse::default();
        collapse.fall(start, 3, 0..3, AREA, placeholder(), 1);
        let generation = collapse.generation();
        collapse.tick(generation, start + Duration::from_secs(4));
        collapse.restore(start + Duration::from_secs(4));
        assert!(collapse.animating());
        let slot = Point::new(192.0, 16.0);
        collapse.tick(generation, start + Duration::from_millis(4299));
        let Look::Loose { position, .. } = collapse.look(1, slot) else {
            panic!("a rebuilding card keeps its poster look");
        };
        assert!((position.x - slot.x).abs() < 1.0 && (position.y - slot.y).abs() < 1.0);
        collapse.tick(generation, start + Duration::from_millis(4400));
        assert!(collapse.idle());
        assert_eq!(collapse.look(1, slot), Look::Normal(Vector::ZERO));
    }

    #[test]
    fn faded_cards_return_when_restored() {
        let start = Instant::now();
        let mut collapse = Collapse::default();
        collapse.fade(start);
        let generation = collapse.generation();
        collapse.tick(generation, start + Duration::from_secs(1));
        assert!(collapse.alpha() > 0.0 && collapse.alpha() < 1.0);
        collapse.tick(generation, start + FADE_OUT);
        assert!(collapse.gone());
        assert!(!collapse.animating());
        assert_eq!(collapse.look(0, Point::ORIGIN), Look::Hidden);
        collapse.restore(start + Duration::from_secs(5));
        assert!(collapse.animating());
        collapse.tick(generation, start + Duration::from_secs(6));
        assert!(collapse.idle());
        assert_eq!(collapse.alpha(), 1.0);
    }

    #[test]
    fn stale_ticks_are_ignored() {
        let start = Instant::now();
        let mut collapse = Collapse::default();
        collapse.fade(start);
        let stale = collapse.generation();
        collapse.reset();
        collapse.fade(start);
        collapse.tick(stale, start + FADE_OUT);
        assert!(!collapse.gone());
    }

    mod app {
        use std::time::{Duration, Instant};

        use iced::Size;
        use iced::keyboard::Modifiers;
        use wineo_core::runtime::State;

        use crate::sidebar::Section;
        use crate::tests::{app, card};
        use crate::{App, Message};

        fn library(count: usize) -> App {
            let mut app = app();
            app.cards = (1..=count)
                .map(|index| card(&format!("game-{index}"), State::Inactive, 0))
                .collect();
            app.grid_motion.snap(false, count);
            app
        }

        fn click_library(app: &mut App, times: usize) {
            for _ in 0..times {
                let _ = app.update(Message::SectionClicked(Section::Library));
            }
        }

        fn tick(app: &mut App, after: Duration) {
            let generation = app.collapse.generation();
            let _ = app.update(Message::CollapseTick(generation, Instant::now() + after));
        }

        #[test]
        fn nine_clicks_change_nothing_and_ten_collapse() {
            let mut app = library(6);
            click_library(&mut app, 9);
            assert!(app.collapse.idle());
            click_library(&mut app, 1);
            assert!(!app.collapse.idle());
            assert!(app.collapse.animating());
            assert_eq!(app.section, Section::Library);
            assert!(!app.panel_open);
        }

        #[test]
        fn keyboard_shortcuts_never_count() {
            let mut app = library(6);
            for _ in 0..12 {
                crate::shortcuts::tests::press(&mut app, "1", Modifiers::CTRL);
            }
            assert_eq!(app.section, Section::Library);
            assert!(app.collapse.idle());
        }

        #[test]
        fn fallen_cards_rest_quietly_and_a_click_rebuilds_without_opening() {
            let mut app = library(6);
            click_library(&mut app, 10);
            tick(&mut app, Duration::from_secs(4));
            assert!(app.collapse.resting());
            // The subscription only asks for frames while this is true.
            assert!(!app.collapse.animating());
            let _ = app.update(Message::Select("game-2".into()));
            let _ = app.update(Message::DoubleClick("game-2".into()));
            assert!(!app.panel_open);
            assert!(app.selected.is_none());
            assert!(app.operation.is_none());
            let _ = app.update(Message::Rebuild);
            assert!(app.collapse.animating());
            tick(&mut app, Duration::from_secs(5));
            assert!(app.collapse.idle());
            assert!(!app.panel_open);
            let _ = app.update(Message::Select("game-2".into()));
            assert!(app.panel_open);
        }

        #[test]
        fn the_heap_restores_itself_and_ignores_stale_timers() {
            let mut app = library(6);
            click_library(&mut app, 10);
            let generation = app.collapse.generation();
            tick(&mut app, Duration::from_secs(4));
            let _ = app.update(Message::CollapseTimeout(generation.wrapping_sub(1)));
            assert!(app.collapse.resting());
            let _ = app.update(Message::CollapseTimeout(generation));
            assert!(app.collapse.animating());
            tick(&mut app, Duration::from_secs(5));
            assert!(app.collapse.idle());
        }

        #[test]
        fn leaving_the_library_or_filtering_restores_at_once() {
            let mut app = library(6);
            click_library(&mut app, 10);
            let _ = app.update(Message::Section(Section::Trash));
            assert!(app.collapse.idle());
            let _ = app.update(Message::Section(Section::Library));
            click_library(&mut app, 10);
            assert!(!app.collapse.idle());
            let _ = app.update(Message::SearchChanged("game-1".into()));
            assert!(app.collapse.idle());
        }

        #[test]
        fn an_open_panel_closes_before_the_drop() {
            let mut app = library(6);
            let _ = app.update(Message::Select("game-1".into()));
            assert!(app.panel_open);
            click_library(&mut app, 10);
            assert!(!app.panel_open);
            assert!(!app.panel_motion.active());
            assert!(!app.grid_motion.active());
            assert!(!app.collapse.idle());
            assert_eq!(app.selected.as_deref(), Some("game-1"));
        }

        #[test]
        fn reduced_motion_fades_out_and_the_empty_library_brings_it_back() {
            let mut app = library(6);
            app.reduced_motion = true;
            click_library(&mut app, 10);
            tick(&mut app, Duration::from_secs(1));
            assert!(app.collapse.alpha() > 0.0 && app.collapse.alpha() < 1.0);
            tick(&mut app, Duration::from_secs(4));
            assert!(app.collapse.gone());
            assert!(!app.collapse.animating());
            assert!(app.collapse.faded());
            // The library area's own click handler sends this.
            let _ = app.update(Message::Rebuild);
            assert!(app.collapse.animating());
            tick(&mut app, Duration::from_secs(6));
            assert!(app.collapse.idle());
            assert_eq!(app.collapse.alpha(), 1.0);
        }

        #[test]
        fn collapse_moves_only_drawn_cards_never_the_grid() {
            use crate::layout_probe::layout;
            let mut app = library(6);
            let size = Size::new(app.viewport_width, 10_000.0);
            let idle = layout(&mut app.grid(), size);
            click_library(&mut app, 10);
            tick(&mut app, Duration::from_secs(4));
            let fallen = layout(&mut app.grid(), size);
            assert_eq!(fallen.size(), idle.size());
            let floor = app.scroll_offset + app.viewport_height;
            for (before, after) in idle.children().iter().zip(fallen.children()) {
                let poster_center = after.bounds().y + 2.0 + 118.0;
                assert!(after.bounds().y > before.bounds().y);
                assert!(poster_center < floor && poster_center > floor - 300.0);
            }
            assert_eq!(app.scroll_offset, 0.0);
        }
    }
}
