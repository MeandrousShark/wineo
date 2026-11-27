use std::time::Duration;

use iced::Task;
use iced::futures::channel::mpsc::unbounded;
use wineo_core::build_lock::Kind;
use wineo_core::build_removal::{self, Removed};
use wineo_core::build_update::{self, Family, Outcome, Report, Status, Step};
use wineo_core::cancel::Token;
use wineo_core::components::{self, Builds, Component};
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release, Transport};
use wineo_core::paths::Paths;
use wineo_core::report::OverviewError;
use wineo_core::runners::{self, Runners};

use crate::download_progress::{Throttle, download_line};
use crate::downloads_page::{Applied, Build, Checked, Event, Offer, ReleasePage, Snapshot};
use crate::task;
use crate::{Message, runner_label};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn warnings(paths: &Paths, errors: &[OverviewError]) -> Vec<String> {
    errors
        .iter()
        .map(|error| {
            let path = build_removal::config_path(paths, error);
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
        move || remove(&paths, &build).map(|removed| removed.bytes),
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
    mut progress: impl FnMut(String),
) -> Result<Option<String>, String> {
    let global = GlobalConfig::load(paths).map_err(|error| format!("{error:#}"))?;
    let mut throttle = Throttle::new(PROGRESS_INTERVAL);
    let mut already = false;
    client
        .install(paths, &global, release, token, |step| {
            already |= matches!(step, Progress::AlreadyInstalled(_));
            if throttle.pass(&step) {
                progress(download_line(release.provider, step));
            }
        })
        .map(|path| {
            let verb = if already {
                "Already installed in"
            } else {
                "Installed in"
            };
            Some(paths.contract_text(&format!("{verb} {}", path.display())))
        })
        .or_else(|error| {
            if canceled(&error) {
                Ok(None)
            } else {
                Err(paths.contract_text(&format!("{error:#}")))
            }
        })
}

fn canceled(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<wineo_core::Error>(),
        Some(wineo_core::Error::Canceled(_))
    )
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

/// Asks upstream about every installed family. A family the cancel skipped
/// isn't an error to show, since the page says the check was canceled.
pub(crate) fn check_builds<T: Transport>(
    client: &Client<T>,
    paths: &Paths,
    token: &Token,
) -> Checked {
    let global = match GlobalConfig::load(paths) {
        Ok(global) => global,
        Err(error) => return Checked::failed(paths.contract_text(&format!("{error:#}"))),
    };
    let report = build_update::check(client, paths, &global, &[], token);
    let mut checked = Checked {
        families: report.families.len(),
        canceled: token.is_canceled(),
        ..Checked::default()
    };
    for family in report.families {
        match family.status {
            Status::UpToDate => {}
            Status::Available(release) => checked.offers.push(Offer {
                installed: family.installed.unwrap_or_default(),
                release,
            }),
            Status::Failed(error) if checked.canceled && canceled(&error) => {}
            Status::Failed(error) => checked.errors.push(paths.contract_text(&format!(
                "Couldn't check {}: {error:#}",
                family.provider.label()
            ))),
        }
    }
    checked
}

pub(crate) fn check(generation: u64, paths: Paths, token: Token) -> Task<Message> {
    task::blocking(
        move || {
            let client = Client::new().map_err(|error| format!("{error:#}"))?;
            Ok(check_builds(&client, &paths, &token))
        },
        move |result| {
            Message::Downloads(Event::Checked(
                generation,
                result.unwrap_or_else(Checked::failed),
            ))
        },
    )
}

/// The status line for one step of the batch, led by which download of how
/// many it is.
pub(crate) fn step_line(step: Step<'_>) -> String {
    let line = match step.progress {
        Progress::Downloading { received, total } => format!(
            "Downloaded {} of {}",
            crate::size::format_bytes(received),
            crate::size::format_bytes(total)
        ),
        progress => download_line(step.release.provider, progress),
    };
    format!("{} of {}: {line}", step.index + 1, step.count)
}

/// Downloads each offer in turn, going on past one that fails. `progress`
/// gets the build being downloaded and its status line.
pub(crate) fn update_builds<T: Transport>(
    client: &Client<T>,
    paths: &Paths,
    offers: &[Offer],
    token: &Token,
    mut progress: impl FnMut(String, String),
) -> Applied {
    let global = match GlobalConfig::load(paths) {
        Ok(global) => global,
        Err(error) => return Applied::failed(paths.contract_text(&format!("{error:#}"))),
    };
    let report = Report {
        families: offers
            .iter()
            .map(|offer| Family {
                provider: offer.release.provider,
                installed: Some(offer.installed.clone()),
                status: Status::Available(offer.release.clone()),
            })
            .collect(),
    };
    let mut throttle = Throttle::new(PROGRESS_INTERVAL);
    let result = build_update::apply(client, paths, &global, &report, token, |step| {
        if throttle.pass(&step.progress) {
            let name = step.release.installed_name.clone();
            progress(name, paths.contract_text(&step_line(step)));
        }
    });
    let mut applied = Applied {
        canceled: token.is_canceled(),
        ..Applied::default()
    };
    for (offer, (_, outcome)) in offers.iter().zip(result.families) {
        let name = offer.release.installed_name.clone();
        match outcome {
            Outcome::Installed(_) => applied.installed.push(name),
            Outcome::AlreadyThere(_) => applied.already.push(name),
            // A stopped batch says so once, not once per build it skipped.
            Outcome::Failed(error) if applied.canceled && canceled(&error) => {}
            Outcome::Failed(error) => applied
                .errors
                .push(paths.contract_text(&format!("Couldn't download {name}: {error:#}"))),
        }
    }
    applied
}

pub(crate) fn update(
    generation: u64,
    paths: Paths,
    offers: Vec<Offer>,
    token: Token,
) -> Task<Message> {
    let (sender, receiver) = unbounded();
    let task = task::blocking(
        move || {
            let client = Client::new().map_err(|error| format!("{error:#}"))?;
            Ok(update_builds(
                &client,
                &paths,
                &offers,
                &token,
                |name, line| {
                    let _ = sender
                        .unbounded_send(Message::Downloads(Event::Stepped(generation, name, line)));
                },
            ))
        },
        move |result| {
            Message::Downloads(Event::Updated(
                generation,
                result.unwrap_or_else(Applied::failed),
            ))
        },
    );
    Task::batch([Task::run(receiver, |message| message), task])
}

fn work<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl Fn(Result<T, String>) -> Event + Send + 'static,
) -> Task<Message> {
    task::blocking(run, move |result| Message::Downloads(done(result)))
}
