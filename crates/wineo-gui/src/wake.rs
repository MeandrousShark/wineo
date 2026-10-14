//! One-shot wakeups for deadlines the app waits on, such as the scrollbar
//! fade or a toast expiring. The view only needs to rerun at the deadline,
//! so nothing ticks in between.

use iced::Task;
use std::time::Instant;

use crate::Message;

#[derive(Default)]
pub(crate) struct Wake {
    generation: u64,
    due: Option<Instant>,
    handle: Option<iced::task::Handle>,
}

impl Wake {
    /// Makes sure a wakeup is pending for `want`, or none when it is `None`.
    /// Call it after every update. A pending wakeup that is already due
    /// earlier is kept, since firing re-arms for whatever deadline is then
    /// current; this keeps a burst of scroll events from spawning a sleeper
    /// each.
    pub(crate) fn arm(
        &mut self,
        want: Option<Instant>,
        message: fn(u64) -> Message,
    ) -> Task<Message> {
        match (self.due, want) {
            (None, None) => return Task::none(),
            (Some(due), Some(want)) if want >= due => return Task::none(),
            _ => {}
        }
        // The old sleeper is aborted so a paused deadline leaves no timer.
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        self.generation = self.generation.wrapping_add(1);
        self.due = want;
        let Some(at) = want else {
            return Task::none();
        };
        let generation = self.generation;
        let (task, handle) = Task::perform(
            async move {
                tokio::time::sleep_until(at.into()).await;
                message(generation)
            },
            |message| message,
        )
        .abortable();
        self.handle = Some(handle.abort_on_drop());
        task
    }

    /// Consumes the wakeup when `generation` is the live one. A stale one,
    /// from before the deadline moved, is ignored.
    pub(crate) fn fired(&mut self, generation: u64) -> bool {
        if self.due.is_none() || generation != self.generation {
            return false;
        }
        self.due = None;
        self.handle = None;
        true
    }

    #[cfg(test)]
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tick(generation: u64) -> Message {
        Message::ScrollIdle(generation)
    }

    #[test]
    fn arms_once_and_keeps_an_earlier_pending_wakeup() {
        let mut wake = Wake::default();
        let now = Instant::now();
        let _ = wake.arm(Some(now + Duration::from_secs(1)), tick);
        let generation = wake.generation();
        // A later deadline rides on the pending wakeup.
        let _ = wake.arm(Some(now + Duration::from_secs(2)), tick);
        assert_eq!(wake.generation(), generation);
        assert_eq!(wake.due(), Some(now + Duration::from_secs(1)));
        // An earlier one replaces it.
        let _ = wake.arm(Some(now + Duration::from_millis(500)), tick);
        assert_ne!(wake.generation(), generation);
        assert_eq!(wake.due(), Some(now + Duration::from_millis(500)));
    }

    #[test]
    fn a_stale_wakeup_is_ignored_and_the_live_one_fires_once() {
        let mut wake = Wake::default();
        let now = Instant::now();
        let _ = wake.arm(Some(now + Duration::from_secs(1)), tick);
        let stale = wake.generation();
        let _ = wake.arm(Some(now + Duration::from_millis(100)), tick);
        assert!(!wake.fired(stale));
        assert!(wake.due().is_some());
        assert!(wake.fired(wake.generation()));
        assert!(wake.due().is_none());
        assert!(!wake.fired(wake.generation()));
    }

    #[test]
    fn dropping_the_deadline_cancels_the_wakeup() {
        let mut wake = Wake::default();
        let now = Instant::now();
        let _ = wake.arm(Some(now + Duration::from_secs(1)), tick);
        let pending = wake.generation();
        let _ = wake.arm(None, tick);
        assert!(wake.due().is_none());
        assert!(!wake.fired(pending));
        // Nothing pending and nothing wanted stays quiet.
        let generation = wake.generation();
        let _ = wake.arm(None, tick);
        assert_eq!(wake.generation(), generation);
    }
}
