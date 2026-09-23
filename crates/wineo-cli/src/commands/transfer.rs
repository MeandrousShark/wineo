use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use signal_hook::SigId;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

pub struct Cancellation {
    pub flag: Arc<AtomicBool>,
    handlers: Vec<SigId>,
}

impl Cancellation {
    pub fn new() -> anyhow::Result<Self> {
        let mut this = Self {
            flag: Arc::new(AtomicBool::new(false)),
            handlers: Vec::new(),
        };
        for signal in [SIGINT, SIGTERM, SIGHUP] {
            this.handlers
                .push(signal_hook::flag::register(signal, this.flag.clone())?);
        }
        Ok(this)
    }

    pub fn result<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        if self.flag.load(Ordering::Relaxed) {
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
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_cancel_does_not_override_published_success() {
        let cancellation = Cancellation::new().unwrap();
        cancellation.flag.store(true, Ordering::Relaxed);
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
