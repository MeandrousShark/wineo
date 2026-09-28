//! Cancellation shared by callers and long-running core operations.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Debug, Default)]
struct State(AtomicU8);

/// A stop request that can be shared across threads.
#[derive(Debug, Clone, Default)]
pub struct Token(Arc<State>);

impl Token {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks an operation to stop cleanly.
    pub fn cancel(&self) {
        self.0.0.fetch_max(1, Ordering::Relaxed);
    }

    /// Asks an operation to kill its children immediately.
    pub fn force(&self) {
        self.0.0.store(2, Ordering::Relaxed);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.0.load(Ordering::Relaxed) >= 1
    }

    pub fn is_forced(&self) -> bool {
        self.0.0.load(Ordering::Relaxed) >= 2
    }

    /// Waits briefly between checks of a child process.
    pub fn wait_timeout(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_shared_by_clones() {
        let token = Token::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_canceled());
        assert!(!token.is_forced());
        token.force();
        assert!(clone.is_canceled());
        assert!(clone.is_forced());
    }
}
