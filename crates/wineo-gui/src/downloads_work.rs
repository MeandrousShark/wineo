use std::time::Duration;

use iced::Task;
use iced::futures::channel::mpsc::unbounded;
use wineo_core::build_lock::Kind;
use wineo_core::build_removal::{self, Removed};
use wineo_core::cancel::Token;
use wineo_core::components::{self, Builds, Component};
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Provider, Release, Transport};
use wineo_core::paths::Paths;
use wineo_core::report::OverviewError;
use wineo_core::runners::{self, Runners};

use crate::downloads_page::{Build, Event, ReleasePage, Snapshot};
use crate::{Message, download_progress, runner_label};

pub(crate) fn warnings(paths: &Paths, errors: &[OverviewError]) -> Vec<String> {
    errors
        .iter()
        .map(|error| {
            let path = match error.name.strip_prefix("template ") {
                Some(name) => paths.templates_dir().join(format!("{name}.toml")),
                None => paths.environment_config(&error.name),
            };
            paths.contract_text(&format!(
                "Skipped {}: {}",
                path.display(),
                wineo_core::error::one_line(&error.message)
            ))
        })
        .collect()
}

pub(crate) fn read(paths: &Paths) -> Result<Snapshot, String> {
    let run = || -> anyhow::Result<Snapshot> {
        let global = GlobalConfig::load(paths)?;
        let runners = Runners::new(paths, &global)?;
        let mut snapshot = Snapshot::default();
        let _ = build_removal::sweep(paths, Kind::Runner, runners.root());
        let listing = runners.list()?;
        for runner in &listing.runners {
            let mut build = Build::new(Kind::Runner, runner.name());
            build.launcher = runner_label::launcher(paths, runner);
            build.different = listing.different_builds(runner.name());
            snapshot.builds.push(build);
        }
        for (name, reason) in listing.skipped {
            snapshot
                .skipped
                .push(format!("Runners / {name}: {}", reason.as_str()));
        }
        for skipped in listing.external_skipped {
            let folder = skipped.directory.file_name().map_or_else(
                || skipped.directory.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            snapshot.skipped.push(format!(
                "{} / {folder}: {}",
                skipped.source,
                skipped.reason.as_str()
            ));
        }
        for component in Component::ALL {
            let builds = Builds::new(paths, &global, component)?;
            let _ = build_removal::sweep(paths, component.into(), builds.root());
            let listing = builds.list()?;
            for build in listing.builds {
                snapshot
                    .builds
                    .push(Build::new(component.into(), build.name()));
            }
            for (name, reason) in listing.skipped {
                snapshot.skipped.push(format!(
                    "{} / {name}: {}",
                    component.label(),
                    reason.describe(component)
                ));
            }
        }
        let mut uses = build_removal::uses_all(paths);
        for build in &mut snapshot.builds {
            match &mut uses {
                Ok(uses) => {
                    let uses = uses
                        .remove(&(build.kind, build.name.clone()))
                        .unwrap_or_default();
                    build.users = uses.labels;
                    snapshot.warnings.extend(warnings(paths, &uses.errors));
                }
                Err(error) => build.error = Some(paths.contract_text(&format!("{error:#}"))),
            }
        }
        snapshot.warnings.sort();
        snapshot.warnings.dedup();
        Ok(snapshot)
    };
    run().map_err(|error| paths.contract_text(&format!("{error:#}")))
}

pub(crate) fn load(generation: u64, paths: Paths) -> Task<Message> {
    work(
        move || read(&paths),
        move |result| Event::Loaded(generation, result),
    )
}

/// Core refuses these too, but a row that slipped through must not even
/// start measuring or moving another launcher's files.
fn refuse_external(build: &Build) -> Result<(), String> {
    match build.launcher {
        Some(launcher) => Err(wineo_core::Error::ExternalBuild {
            name: build.name.clone(),
            launcher,
        }
        .to_string()),
        None => Ok(()),
    }
}

pub(crate) fn size(paths: &Paths, build: &Build) -> Result<u64, String> {
    refuse_external(build)?;
    let result = match build.kind {
        Kind::Runner => runners::size(paths, &build.name),
        Kind::Dxvk => components::size(paths, Component::Dxvk, &build.name),
        Kind::Vkd3d => components::size(paths, Component::Vkd3d, &build.name),
    };
    result.map_err(|error| paths.contract_text(&format!("{error:#}")))
}

pub(crate) fn measure(generation: u64, paths: Paths, build: Build) -> Task<Message> {
    work(
        move || size(&paths, &build),
        move |result| Event::Measured(generation, result),
    )
}

pub(crate) fn remove(paths: &Paths, build: &Build) -> Result<Removed, String> {
    refuse_external(build)?;
    let result = match build.kind {
        Kind::Runner => runners::remove(paths, &build.name),
        Kind::Dxvk => components::remove(paths, Component::Dxvk, &build.name),
        Kind::Vkd3d => components::remove(paths, Component::Vkd3d, &build.name),
    };
    result.map_err(|error| paths.contract_text(&format!("{error:#}")))
}

pub(crate) fn delete(generation: u64, paths: Paths, build: Build) -> Task<Message> {
    work(
        move || {
            remove(&paths, &build).map(|removed| (removed.bytes, warnings(&paths, &removed.errors)))
        },
        move |result| Event::Deleted(generation, result),
    )
}

pub(crate) fn list<T: Transport>(
    client: &Client<T>,
    provider: Provider,
    page: u32,
) -> Result<ReleasePage, String> {
    client
        .list(provider, page)
        .map(|catalog| ReleasePage {
            releases: catalog.releases,
            has_more: catalog.has_more,
        })
        .map_err(|error| format!("{error:#}"))
}

pub(crate) fn releases(generation: u64, provider: Provider, page: u32) -> Task<Message> {
    work(
        move || {
            list(
                &Client::new().map_err(|error| format!("{error:#}"))?,
                provider,
                page,
            )
        },
        move |result| Event::Listed(generation, page, result),
    )
}

pub(crate) fn install<T: Transport>(
    client: &Client<T>,
    paths: &Paths,
    release: &Release,
    token: &Token,
    progress: impl FnMut(String),
) -> Result<Option<String>, String> {
    let global = GlobalConfig::load(paths).map_err(|error| format!("{error:#}"))?;
    download_progress::install(
        client,
        paths,
        &global,
        release,
        token,
        Duration::from_millis(100),
        progress,
    )
    .map(|path| Some(paths.contract_text(&format!("Installed in {}", path.display()))))
    .or_else(|error| {
        if matches!(
            error.downcast_ref::<wineo_core::Error>(),
            Some(wineo_core::Error::Canceled("download"))
        ) {
            Ok(None)
        } else {
            Err(paths.contract_text(&format!("{error:#}")))
        }
    })
}

pub(crate) fn download(
    generation: u64,
    paths: Paths,
    release: Release,
    token: Token,
) -> Task<Message> {
    let (sender, receiver) = unbounded();
    let task = work(
        move || {
            install(
                &Client::new().map_err(|error| format!("{error:#}"))?,
                &paths,
                &release,
                &token,
                |line| {
                    let _ = sender
                        .unbounded_send(Message::Downloads(Event::Progress(generation, line)));
                },
            )
        },
        move |result| Event::Downloaded(generation, result),
    );
    Task::batch([Task::run(receiver, |message| message), task])
}

fn work<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl Fn(Result<T, String>) -> Event + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(run)
                .await
                .map_err(|error| error.to_string())?
        },
        move |result| Message::Downloads(done(result)),
    )
}
