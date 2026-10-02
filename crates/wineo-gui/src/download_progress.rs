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
    let mut last_progress = Instant::now() - interval;
    client.install(paths, global, release, token, |progress| {
        let now = Instant::now();
        if matches!(progress, Progress::Downloading { .. })
            && now.duration_since(last_progress) < interval
        {
            return;
        }
        last_progress = now;
        progress_line(download_line(release.provider, progress));
    })
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
