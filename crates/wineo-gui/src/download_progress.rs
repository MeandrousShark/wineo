use std::time::{Duration, Instant};

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release};
use wineo_core::paths::Paths;

pub(crate) fn install(
    client: &Client,
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
            format!("Downloading {provider}: {received} / {total} bytes")
        }
        Progress::Verifying => format!("Verifying {provider}"),
        Progress::Extracting => format!("Extracting {provider}"),
        Progress::Installed(path) => format!("Installed in {}", path.display()),
        Progress::AlreadyInstalled(path) => format!("Already installed in {}", path.display()),
    }
}
