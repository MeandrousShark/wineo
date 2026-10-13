use std::collections::BTreeSet;
use std::thread::{self, JoinHandle};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::exfiltrator::raw::WithRawSiginfo;
use signal_hook::iterator::{Handle, SignalsInfo};
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::import::{ExternalDependency, ImportProgress};
use wineo_core::paths::Paths;

/// Waits for wine to exit in `name`'s prefix before it is copied, replaced,
/// or deleted. A config that can't be loaded is left for the command to report.
pub fn wait_idle(paths: &Paths, global: &GlobalConfig, name: &str) -> anyhow::Result<()> {
    let Ok(environment) = Environment::load(paths, name) else {
        return Ok(());
    };
    environment.wait_idle(paths, global, || {
        eprintln!("Waiting for wine to exit in {name}");
    })?;
    Ok(())
}

pub fn copy_progress() -> impl FnMut(ImportProgress) {
    let mut copied = 0_u64;
    move |event| {
        if let ImportProgress::Copied { .. } = event {
            copied += 1;
            if copied.is_multiple_of(500) {
                eprintln!("Copied {copied} entries");
            }
        }
    }
}

pub fn print_external_links(label: &str, links: &[ExternalDependency], verbose: bool, note: &str) {
    for line in external_link_lines(label, links, verbose, note) {
        println!("{line}");
    }
}

/// Review lines for links that point outside a prefix. Most are Wine's
/// `dosdevices` drive mappings, so only the count shows unless `verbose`.
pub fn external_link_lines(
    label: &str,
    links: &[ExternalDependency],
    verbose: bool,
    note: &str,
) -> Vec<String> {
    if links.is_empty() {
        return Vec::new();
    }
    let count = links.len();
    if !verbose {
        return vec![format!(
            "  {label} links: {count}, {note} (-v to list them)"
        )];
    }
    let mut lines = vec![format!("  {label} links: {count}, {note}")];
    lines.extend(links.iter().map(|link| {
        format!(
            "  {label}: {} -> {}",
            link.source.display(),
            link.target.display()
        )
    }));
    lines
}

pub struct Cancellation {
    pub token: Token,
    signals: Handle,
    thread: Option<JoinHandle<()>>,
}

impl Cancellation {
    pub fn new() -> anyhow::Result<Self> {
        Self::listen(Token::new(), Mode::Normal)
    }

    /// Listens during an after hook. `announce` prints the waiting line up
    /// front, for when Ctrl+C was already pressed and the user is waiting to
    /// leave, so the next Ctrl+C kills the hook.
    pub fn after_hook(announce: bool) -> anyhow::Result<Self> {
        Self::listen(Token::new(), Mode::After { announce })
    }

    pub fn with_token(token: &Token) -> anyhow::Result<Self> {
        Self::listen(token.clone(), Mode::Normal)
    }

    pub fn after_hook_with(token: &Token, announce: bool) -> anyhow::Result<Self> {
        Self::listen(token.clone(), Mode::After { announce })
    }

    fn listen(token: Token, mode: Mode) -> anyhow::Result<Self> {
        let after = matches!(mode, Mode::After { .. });
        let announce = matches!(mode, Mode::After { announce: true });
        // Keep repeated Ctrl+C signals queued so a quick second one forces Stop.
        let mut signals = SignalsInfo::<WithRawSiginfo>::new([SIGINT, SIGTERM, SIGHUP, SIGUSR1])?;
        // Only once the handler is in, so a quick Ctrl+C after the line counts.
        if announce {
            say_waiting_for_after();
        }
        let handle = signals.handle();
        let listener = token.clone();
        let thread = thread::spawn(move || {
            let mut waiting = announce;
            loop {
                // Only SIGINT counts every delivery; other signals keep their
                // original once-per-batch behavior.
                let mut seen = BTreeSet::new();
                for received in signals.wait() {
                    let signal = received.si_signo;
                    if signal != SIGINT && !seen.insert(signal) {
                        continue;
                    }
                    if after && signal != SIGUSR1 && !waiting {
                        waiting = true;
                        say_waiting_for_after();
                    } else if after {
                        // Drop joins this thread, so the line is out before exit.
                        if !listener.is_forced() {
                            say("The after hook was killed.");
                        }
                        listener.force();
                    } else if signal == SIGUSR1 || listener.is_canceled() {
                        listener.force();
                    } else {
                        listener.cancel();
                    }
                }
                // Drop closes the handle, which wakes `wait` with an empty batch.
                if signals.is_closed() {
                    return;
                }
            }
        });
        Ok(Self {
            token,
            signals: handle,
            thread: Some(thread),
        })
    }

    pub fn result<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        if self.token.is_canceled() {
            Err(crate::prompt::Canceled.into())
        } else {
            result
        }
    }

    pub fn published_result<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => self.result(Err(error)),
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    After { announce: bool },
}

fn say_waiting_for_after() {
    say("Waiting for the after hook, press Ctrl+C again to kill it.");
}

/// The terminal may be gone after a SIGHUP, and a failed write must not
/// abort the shutdown.
fn say(message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{message}");
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.signals.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_cancel_does_not_override_published_success() {
        let cancellation = Cancellation::new().unwrap();
        cancellation.token.cancel();
        assert_eq!(
            cancellation.published_result(Ok("installed")).unwrap(),
            "installed"
        );
        assert!(
            cancellation
                .published_result::<()>(Err(anyhow::anyhow!("failed")))
                .unwrap_err()
                .is::<crate::prompt::Canceled>()
        );
    }
}
