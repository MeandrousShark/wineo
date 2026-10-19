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
const FADE_OUT: Duration = Duration::from_secs(3);
const FADE_IN: Duration = Duration::from_millis(800);
/// Captions return over the end of a rebuild, once the posters are nearly
/// home; a card turns back into its real self at this spring progress.
const CAPTION_DELAY: Duration = Duration::from_millis(150);
const CAPTION_FADE: Duration = Duration::from_millis(150);
const HOME: f32 = 0.98;
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
/// coordinates, how many columns the cards wrap at, and the farthest any
/// visible card's slot sits above the floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Area {
    pub(crate) floor: f32,
    pub(crate) width: f32,
    pub(crate) columns: usize,
    pub(crate) drop: f32,
    /// The poster's half size inside its 2 px selection frame, at the
    /// grid's zoom.
    pub(crate) half_poster: Vector,
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
        started: Instant,
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
                    started: now,
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
                    .map(|fall| fall.delay + fall.settle(area.drop, area.half_poster))
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
        match &self.phase {
            Phase::FadeOut { started } => {
                1.0 - smooth(progress(*started, self.now.unwrap_or(*started), FADE_OUT))
            }
            Phase::Gone => 0.0,
            Phase::FadeIn { started, from } => {
                from + (1.0 - from)
                    * smooth(progress(*started, self.now.unwrap_or(*started), FADE_IN))
            }
            _ => 1.0,
        }
    }

    /// Whether card `index` dropped, so it may be drawn as a lone poster.
    pub(crate) fn fell(&self, index: usize) -> bool {
        match &self.phase {
            Phase::Down { falls, .. } | Phase::Rebuild { falls, .. } => {
                falls.get(index).is_some_and(Option::is_some)
            }
            _ => false,
        }
    }

    /// The title and metadata's opacity. They fade during the shake, since
    /// only the poster can be drawn rotated.
    pub(crate) fn caption_alpha(&self) -> f32 {
        match &self.phase {
            Phase::Down { started, .. } => {
                1.0 - progress(*started, self.now.unwrap_or(*started), SHAKE)
            }
            Phase::Rebuild { started, .. } => {
                let now = self.now.unwrap_or(*started);
                progress(*started + CAPTION_DELAY, now, CAPTION_FADE)
            }
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
                    let position = Point::new(
                        bounded_axis(start.x, slot.x, progress),
                        bounded_axis(start.y, slot.y, progress),
                    );
                    // Nearly home the leftover tilt is under two degrees, so
                    // the real card takes over and its caption can fade in.
                    if progress >= HOME {
                        Look::Normal(position - slot)
                    } else {
                        Look::Loose {
                            position,
                            rotation: rotation * (1.0 - progress.clamp(0.0, 1.0)),
                        }
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
        let half = area.half_poster;
        let extent = half.x * rotation.sin().abs() + half.y * rotation.cos().abs();
        let center_y = area.floor - extent - self.stack;
        let max_x = (area.width - 2.0 * (half.x + FRAME)).max(0.0);
        let x = (slot.x + self.drift).clamp(0.0, max_x);
        (Point::new(x, center_y - half.y - FRAME), rotation)
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
    /// longest drop (`drop`, from the highest visible slot) and the longest
    /// climb (a card whose slot sits at the floor, below where it rests).
    fn settle(&self, drop: f32, half: Vector) -> f32 {
        let climb = -(2.0 * half.y + 2.0 * FRAME + half.x.hypot(half.y) + self.stack);
        [drop.max(0.0), climb]
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
mod tests;
