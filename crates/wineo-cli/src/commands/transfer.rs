use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::{Handle, Signals};
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

pub fn print_external_links(links: &[ExternalDependency], verbose: bool, note: &str) {
    let count = links.len();
    if count > 0 {
        if !verbose && count > 12 {
            println!("  external links: {count} (showing first 12; use -v for all)");
        } else {
            println!("  external links: {count}");
        }
    }
    for link in links.iter().take(if verbose { count } else { 12 }) {
        println!(
            "  external: {} -> {} ({note})",
            link.source.display(),
            link.target.display()
        );
    }
}

pub struct Cancellation {
    pub token: Token,
    signals: Handle,
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn new() -> anyhow::Result<Self> {
        Self::listen(Token::new(), false)
    }

    pub fn after_hook() -> anyhow::Result<Self> {
        Self::listen(Token::new(), true)
    }

    pub fn with_token(token: &Token) -> anyhow::Result<Self> {
        Self::listen(token.clone(), false)
    }

    pub fn after_hook_with(token: &Token) -> anyhow::Result<Self> {
        Self::listen(token.clone(), true)
    }

    fn listen(token: Token, after: bool) -> anyhow::Result<Self> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP, SIGUSR1])?;
        let handle = signals.handle();
        let listener = token.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            let mut waiting = false;
            while !thread_stop.load(Ordering::Relaxed) {
                for signal in signals.pending() {
                    if after && signal != SIGUSR1 && !waiting {
                        waiting = true;
                        use std::io::Write;
                        let _ = writeln!(
                            std::io::stderr(),
                            "Waiting for the after hook, press Ctrl+C again to kill it."
                        );
                    } else if after || signal == SIGUSR1 || listener.is_canceled() {
                        listener.force();
                    } else {
                        listener.cancel();
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(Self {
            token,
            signals: handle,
            thread: Some(thread),
            stop,
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

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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
