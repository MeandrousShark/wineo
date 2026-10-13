//! Cancellation shared by callers and long-running core operations.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    /// 0 none, 1 canceled, 2 forced. Only written under `wakers`, so a
    /// waiter holding that lock never misses a change.
    level: AtomicU8,
    wakers: Mutex<Wakers>,
    raised: Condvar,
}

#[derive(Default)]
struct Wakers {
    next: u64,
    list: Vec<(u64, Waker)>,
}

impl State {
    fn lock(&self) -> MutexGuard<'_, Wakers> {
        self.wakers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn raise(&self, to: u8) {
        let woken: Vec<Waker> = {
            let guard = self.lock();
            if self.level.load(Ordering::Relaxed) >= to {
                return;
            }
            self.level.store(to, Ordering::Relaxed);
            self.raised.notify_all();
            guard.list.iter().map(|(_, waker)| waker.clone()).collect()
        };
        // Called unlocked so a waker may use the token again.
        for waker in woken {
            waker();
        }
    }
}

/// A stop request that can be shared across threads.
#[derive(Clone, Default)]
pub struct Token(Arc<State>);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Token")
            .field(&self.0.level.load(Ordering::Relaxed))
            .finish()
    }
}

/// Keeps a waker registered with a token until it is dropped.
#[must_use = "the waker is removed as soon as the subscription is dropped"]
pub struct Subscription {
    state: Arc<State>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let removed = {
            let mut wakers = self.state.lock();
            wakers
                .list
                .iter()
                .position(|(id, _)| *id == self.id)
                .map(|at| wakers.list.remove(at))
        };
        // A waker can own another subscription, whose drop locks again.
        drop(removed);
    }
}

impl Token {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks an operation to stop cleanly.
    pub fn cancel(&self) {
        self.0.raise(1);
    }

    /// Asks an operation to kill its children immediately.
    pub fn force(&self) {
        self.0.raise(2);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.level.load(Ordering::Relaxed) >= 1
    }

    pub fn is_forced(&self) -> bool {
        self.0.level.load(Ordering::Relaxed) >= 2
    }

    /// Calls `wake` whenever the level rises, so a waiter can block rather
    /// than poll. A change that lands before this returns is not announced,
    /// so check the level after subscribing.
    pub fn subscribe(&self, wake: impl Fn() + Send + Sync + 'static) -> Subscription {
        let mut wakers = self.0.lock();
        let id = wakers.next;
        wakers.next += 1;
        wakers.list.push((id, Arc::new(wake)));
        Subscription {
            state: self.0.clone(),
            id,
        }
    }

    /// Waits briefly between checks of a child process, returning early when
    /// the level rises during the wait.
    pub fn wait_timeout(&self, duration: Duration) {
        let guard = self.0.lock();
        let seen = self.0.level.load(Ordering::Relaxed);
        let _ = self
            .0
            .raised
            .wait_timeout_while(guard, duration, |_| {
                self.0.level.load(Ordering::Relaxed) == seen
            })
            .unwrap_or_else(PoisonError::into_inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::thread;
    use std::time::Instant;

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

    fn counter() -> (Arc<AtomicUsize>, impl Fn() + Send + Sync + 'static) {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        (count, move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[test]
    fn subscriber_runs_when_the_level_rises() {
        let token = Token::new();
        let (count, wake) = counter();
        let _subscription = token.subscribe(wake);
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        token.force();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        token.force();
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn dropped_subscription_is_not_called() {
        let token = Token::new();
        let (count, wake) = counter();
        drop(token.subscribe(wake));
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dropping_a_waker_that_owns_a_subscription_does_not_deadlock() {
        let token = Token::new();
        let first = token.subscribe(|| {});
        let second = token.subscribe(move || {
            let _ = &first;
        });
        drop(second);
        token.cancel();
    }

    #[test]
    fn subscriber_may_use_the_token() {
        let token = Token::new();
        let inner = token.clone();
        let _subscription = token.subscribe(move || {
            assert!(inner.is_canceled());
            drop(inner.subscribe(|| {}));
        });
        token.cancel();
    }

    #[test]
    fn wait_timeout_returns_early_on_cancel() {
        let token = Token::new();
        let remote = token.clone();
        let canceler = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            remote.cancel();
        });
        let start = Instant::now();
        token.wait_timeout(Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_secs(5));
        canceler.join().unwrap();
    }

    #[test]
    fn wait_timeout_still_waits_once_canceled() {
        let token = Token::new();
        token.cancel();
        let start = Instant::now();
        token.wait_timeout(Duration::from_millis(60));
        assert!(start.elapsed() >= Duration::from_millis(50));
    }
}
