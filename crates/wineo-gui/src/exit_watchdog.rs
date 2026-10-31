//! Ends the process when work that can't be interrupted holds it open after
//! the window closed.
//!
//! iced drops its Tokio runtime inside `run()`, and that drop waits for
//! blocking tasks with no limit, so a `process::exit` after `run()` is never
//! reached. The watchdog is a plain thread, since the runtime is what hangs.

use std::io::Write;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// How long work gets to finish after the window closes.
pub(crate) const EXIT_BOUND: Duration = Duration::from_secs(10);

type Expire = Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) struct Watchdog {
    bound: Duration,
    expire: Expire,
    /// The line the running watchdog will print, once started.
    line: Option<String>,
}

impl Watchdog {
    pub(crate) fn new(bound: Duration, expire: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            bound,
            expire: Arc::new(expire),
            line: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn started(&self) -> Option<&str> {
        self.line.as_deref()
    }

    /// Starts counting down to a stderr line naming `work`. Only the first
    /// start counts, so the line names what was stuck when the exit began. A
    /// start that couldn't get a thread leaves the watchdog free to try again.
    pub(crate) fn start(&mut self, work: &[String]) -> bool {
        if self.line.is_some() {
            return false;
        }
        let line = abandoned_line(self.bound, work);
        let (bound, expire, said) = (self.bound, self.expire.clone(), line.clone());
        let spawned = thread::Builder::new()
            .name("exit-watchdog".to_owned())
            .spawn(move || {
                thread::sleep(bound);
                expire(&said);
            });
        if spawned.is_err() {
            return false;
        }
        self.line = Some(line);
        true
    }
}

impl Default for Watchdog {
    fn default() -> Self {
        #[cfg(not(test))]
        {
            Self::new(EXIT_BOUND, exit_process)
        }
        // A test that exits the app must not take the test binary with it.
        #[cfg(test)]
        {
            Self::new(Duration::ZERO, |_| {})
        }
    }
}

/// Says what was given up on, then ends the process cleanly: the window
/// already closed, and a stuck write or hook is a limit, not a failure.
pub(crate) fn exit_process(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
    std::process::exit(0);
}

/// The one stderr line: what the exit abandoned, or that nothing tracked was
/// left, which is the untracked artwork prune or another background task.
pub(crate) fn abandoned_line(bound: Duration, work: &[String]) -> String {
    let seconds = bound.as_secs();
    let what = match work {
        [] => "background work that isn't tracked".to_owned(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };
    format!("wineo-gui: gave up after {seconds} s on {what}.")
}

#[cfg(test)]
pub(crate) mod tests;
