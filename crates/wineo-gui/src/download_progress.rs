use std::time::{Duration, Instant};

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release, Transport};
use wineo_core::paths::Paths;

/// Throttles progress so a fast download doesn't flood the UI in the new, Heroic,
/// and checkup flows. Backdating lets the first Downloading event through immediately.
pub(crate) fn install<T: Transport>(
    client: &Client<T>,
    paths: &Paths,
    global: &GlobalConfig,
    release: &Release,
    token: &Token,
    interval: Duration,
    mut progress_line: impl FnMut(String),
) -> anyhow::Result<std::path::PathBuf> {
    let mut throttle = Throttle::new(interval);
    client.install(paths, global, release, token, |progress| {
        if throttle.pass(&progress) {
            progress_line(download_line(release.provider, progress));
        }
    })
}

/// Lets every step through but holds Downloading to one per interval.
pub(crate) struct Throttle {
    interval: Duration,
    last: Instant,
}

impl Throttle {
    /// Backdated, so the first Downloading goes through at once.
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Instant::now() - interval,
        }
    }

    pub(crate) fn pass(&mut self, progress: &Progress) -> bool {
        let now = Instant::now();
        if matches!(progress, Progress::Downloading { .. })
            && now.duration_since(self.last) < self.interval
        {
            return false;
        }
        self.last = now;
        true
    }
}

pub(crate) fn download_line(provider: Provider, progress: Progress) -> String {
    match progress {
        Progress::Downloading { received, total } => {
            format!(
                "Downloading {provider}: {} of {}",
                crate::size::format_bytes(received),
                crate::size::format_bytes(total)
            )
        }
        Progress::Verifying => format!("Verifying {provider}"),
        Progress::Extracting => format!("Extracting {provider}"),
        Progress::Installed(path) => format!("Installed in {}", path.display()),
        Progress::AlreadyInstalled(path) => format!("Already installed in {}", path.display()),
    }
}
