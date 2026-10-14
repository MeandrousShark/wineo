//! A thin progress bar with its count in words, and the bookkeeping that
//! keeps it from flashing: work that finishes quickly never shows it.

use std::time::{Duration, Instant};

use iced::widget::{progress_bar, row, text};
use iced::{Center, Element, Length, Task, border};

use crate::theme::Palette;
use crate::wake::Wake;

/// How long work runs before its bar appears, so a warm open never flashes it.
pub(crate) const REVEAL_AFTER: Duration = Duration::from_millis(500);

const GIRTH: f32 = 4.0;
const LENGTH: f32 = 120.0;

/// A bar for `done` of `total` with `label` beside it. The label carries the
/// state in words, so the bar is never the only signal.
pub(crate) fn bar<'a, Message: 'a>(
    done: usize,
    total: usize,
    label: impl text::IntoFragment<'a>,
    palette: Palette,
) -> Element<'a, Message> {
    row![
        progress_bar(0.0..=total.max(1) as f32, done as f32)
            .length(Length::Fixed(LENGTH))
            .girth(GIRTH)
            .style(move |_| progress_bar::Style {
                background: palette.muted.scale_alpha(0.3).into(),
                bar: palette.primary.into(),
                border: border::rounded(GIRTH / 2.0),
            }),
        text(label).size(12).color(palette.muted),
    ]
    .spacing(8)
    .align_y(Center)
    .into()
}

/// Counts finished work against the work asked for, and says when the bar is
/// worth showing: only once work has run for `REVEAL_AFTER` and still
/// remains. The reveal is a single one-shot wakeup that exists only while
/// work is pending, never a periodic timer.
pub(crate) struct Delayed<Message> {
    done: usize,
    total: usize,
    revealed: bool,
    wake: Wake,
    message: fn(u64) -> Message,
}

impl<Message: Send + 'static> Delayed<Message> {
    /// `message` is what the wakeup sends, to be passed back to `reveal`.
    pub(crate) fn new(message: fn(u64) -> Message) -> Self {
        Self {
            done: 0,
            total: 0,
            revealed: false,
            wake: Wake::default(),
            message,
        }
    }

    /// Notes `count` more pieces of work started at `now`. Work added while
    /// some is running joins it and keeps its deadline.
    pub(crate) fn add(&mut self, count: usize, now: Instant) -> Task<Message> {
        if count == 0 {
            return Task::none();
        }
        if self.done >= self.total {
            self.done = 0;
            self.total = 0;
            self.revealed = false;
        }
        self.total += count;
        if self.revealed {
            return Task::none();
        }
        self.wake.arm(Some(now + REVEAL_AFTER), self.message)
    }

    /// One piece of work finished, whether it worked or not.
    pub(crate) fn finish_one(&mut self) {
        self.done = (self.done + 1).min(self.total);
        if self.done >= self.total {
            self.settle();
        }
    }

    /// All the work is over, though some pieces never reported.
    pub(crate) fn finish_all(&mut self) {
        self.done = self.total;
        self.settle();
    }

    /// Forgets the work, for a listing that was replaced.
    pub(crate) fn reset(&mut self) {
        self.done = 0;
        self.total = 0;
        self.settle();
    }

    fn settle(&mut self) {
        self.revealed = false;
        let _ = self.wake.arm(None, self.message);
    }

    /// The wakeup fired. A stale one, or one for work that is over, does
    /// nothing.
    pub(crate) fn reveal(&mut self, generation: u64) {
        if self.wake.fired(generation) && self.done < self.total {
            self.revealed = true;
        }
    }

    /// What to draw: pieces done and asked for, once worth showing.
    pub(crate) fn shown(&self) -> Option<(usize, usize)> {
        (self.revealed && self.done < self.total).then_some((self.done, self.total))
    }

    #[cfg(test)]
    pub(crate) fn pending_wakeup(&self) -> Option<Instant> {
        self.wake.due()
    }

    /// The generation the live wakeup would send.
    #[cfg(test)]
    pub(crate) fn wakeup_generation(&self) -> u64 {
        self.wake.generation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_probe::layout;

    fn tracker() -> Delayed<u64> {
        Delayed::new(|generation| generation)
    }

    /// Fires whatever wakeup is pending, as its timer would.
    fn fire(delayed: &mut Delayed<u64>) {
        let generation = delayed.wake.generation();
        delayed.reveal(generation);
    }

    #[test]
    fn appears_only_after_the_delay_and_while_work_remains() {
        let mut delayed = tracker();
        let start = Instant::now();
        let _ = delayed.add(3, start);
        assert_eq!(delayed.shown(), None);
        assert_eq!(delayed.pending_wakeup(), Some(start + REVEAL_AFTER));
        delayed.finish_one();
        assert_eq!(delayed.shown(), None);
        fire(&mut delayed);
        assert_eq!(delayed.shown(), Some((1, 3)));
        delayed.finish_one();
        assert_eq!(delayed.shown(), Some((2, 3)));
    }

    #[test]
    fn never_appears_when_the_work_ends_before_the_delay() {
        let mut delayed = tracker();
        let _ = delayed.add(2, Instant::now());
        let generation = delayed.wake.generation();
        delayed.finish_one();
        delayed.finish_one();
        assert_eq!(delayed.pending_wakeup(), None);
        // The wakeup was canceled, and one already in flight does nothing.
        delayed.reveal(generation);
        assert_eq!(delayed.shown(), None);
    }

    #[test]
    fn hides_at_completion_and_leaves_no_timer() {
        let mut delayed = tracker();
        assert_eq!(delayed.pending_wakeup(), None);
        let _ = delayed.add(2, Instant::now());
        fire(&mut delayed);
        delayed.finish_one();
        assert!(delayed.shown().is_some());
        delayed.finish_one();
        assert_eq!(delayed.shown(), None);
        assert_eq!(delayed.pending_wakeup(), None);
    }

    #[test]
    fn work_that_reports_nothing_is_ended_by_finish_all() {
        let mut delayed = tracker();
        let _ = delayed.add(5, Instant::now());
        fire(&mut delayed);
        delayed.finish_one();
        delayed.finish_all();
        assert_eq!(delayed.shown(), None);
        assert_eq!(delayed.pending_wakeup(), None);
        // A late report cannot push it past the total.
        delayed.finish_one();
        assert_eq!(delayed.shown(), None);
    }

    #[test]
    fn reset_forgets_the_work_and_the_timer() {
        let mut delayed = tracker();
        let _ = delayed.add(4, Instant::now());
        fire(&mut delayed);
        assert!(delayed.shown().is_some());
        delayed.reset();
        assert_eq!(delayed.shown(), None);
        assert_eq!(delayed.pending_wakeup(), None);
        // The next batch starts over with its own delay.
        let start = Instant::now();
        let _ = delayed.add(2, start);
        assert_eq!(delayed.shown(), None);
        assert_eq!(delayed.pending_wakeup(), Some(start + REVEAL_AFTER));
    }

    #[test]
    fn more_work_joins_the_pending_batch_without_moving_its_deadline() {
        let mut delayed = tracker();
        let start = Instant::now();
        let _ = delayed.add(2, start);
        let _ = delayed.add(3, start + Duration::from_millis(200));
        assert_eq!(delayed.pending_wakeup(), Some(start + REVEAL_AFTER));
        fire(&mut delayed);
        assert_eq!(delayed.shown(), Some((0, 5)));
        let _ = delayed.add(1, start + Duration::from_millis(900));
        assert_eq!(delayed.shown(), Some((0, 6)));
    }

    #[test]
    fn the_bar_draws_for_both_palettes_with_its_words() {
        for palette in [
            Palette::from_mode(iced::theme::Mode::Dark),
            Palette::from_mode(iced::theme::Mode::Light),
        ] {
            let mut view: Element<'_, ()> = bar(34, 300, "Preparing art... 34 of 300", palette);
            let node = layout(&mut view, iced::Size::new(400.0, 100.0));
            assert!(node.bounds().height <= 20.0);
            assert!(node.bounds().width > LENGTH);
        }
    }
}
