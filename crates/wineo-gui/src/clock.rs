//! The time things happen at: the real clock, or in tests one that only
//! moves when told to, so timing never depends on how fast a loaded machine
//! runs them.

use std::time::Instant;

pub(crate) fn now() -> Instant {
    #[cfg(test)]
    if let Some(now) = FAKE.with(std::cell::Cell::get) {
        return now;
    }
    Instant::now()
}

#[cfg(test)]
thread_local! {
    static FAKE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// A clock for this thread that only moves when told to, back to the real
/// one when dropped.
#[cfg(test)]
pub(crate) struct Fake(Instant);

#[cfg(test)]
impl Fake {
    pub(crate) fn start() -> Self {
        let at = Instant::now();
        FAKE.with(|fake| fake.set(Some(at)));
        Self(at)
    }

    pub(crate) fn advance(&mut self, by: std::time::Duration) {
        self.0 += by;
        let at = self.0;
        FAKE.with(|fake| fake.set(Some(at)));
    }

    pub(crate) fn now(&self) -> Instant {
        self.0
    }
}

#[cfg(test)]
impl Drop for Fake {
    fn drop(&mut self) {
        FAKE.with(|fake| fake.set(None));
    }
}
