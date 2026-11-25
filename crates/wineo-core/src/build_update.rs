//! Newer upstream builds of the families wineo already downloaded: checking
//! for them, then fetching them into their own folders beside the old ones.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::build_removal;
use crate::cancel::Token;
use crate::components::{self, Builds, Component};
use crate::config::GlobalConfig;
use crate::download::{self, Client, Progress, Provider, RECEIPT, RateLimited, Release, Transport};
use crate::error::Error;
use crate::external_runners;
use crate::paths::Paths;
use crate::runners::Runners;

/// What a check found, one entry per family checked in the providers' fixed
/// order. A family with nothing installed has no entry unless it was named.
#[derive(Debug)]
pub struct Report {
    pub families: Vec<Family>,
}

#[derive(Debug)]
pub struct Family {
    pub provider: Provider,
    /// The newest installed build's folder name. None only when looking for
    /// it failed, and then `status` says why.
    pub installed: Option<String>,
    pub status: Status,
}

#[derive(Debug)]
pub enum Status {
    UpToDate,
    Available(Release),
    Failed(anyhow::Error),
}

impl Report {
    /// The releases to download, in order.
    pub fn updates(&self) -> impl Iterator<Item = &Release> {
        self.families
            .iter()
            .filter_map(|family| match &family.status {
                Status::Available(release) => Some(release),
                _ => None,
            })
    }

    pub fn failed(&self) -> bool {
        self.families
            .iter()
            .any(|family| matches!(family.status, Status::Failed(_)))
    }
}

/// A family that was asked for by name has nothing installed. The CLI adds
/// how to get one, since it knows the spelling of the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{} isn't installed", .0.label())]
pub struct NotInstalled(pub Provider);

/// Asks upstream for the newest release of each family in `wanted`, or of
/// every installed family when it is empty, in the providers' fixed order
/// and each once. A named family with nothing installed is its error. One family's
/// failure never stops the others, except that once GitHub says the rate
/// limit is used up, the rest get that answer without asking again.
/// `Client::select` takes no token, so a cancel is honored between two
/// families, and the ones left fail as canceled.
pub fn check<T: Transport>(
    client: &Client<T>,
    paths: &Paths,
    global: &GlobalConfig,
    wanted: &[Provider],
    token: &Token,
) -> Report {
    let mut families = Vec::new();
    let mut limited: Option<RateLimited> = None;
    let named = !wanted.is_empty();
    for provider in Provider::ALL {
        if named && !wanted.contains(&provider) {
            continue;
        }
        let installed = match newest_installed(paths, global, provider) {
            Ok(Some(installed)) => installed,
            Ok(None) if named => {
                families.push(Family {
                    provider,
                    installed: None,
                    status: Status::Failed(NotInstalled(provider).into()),
                });
                continue;
            }
            Ok(None) => continue,
            Err(error) => {
                families.push(Family {
                    provider,
                    installed: None,
                    status: Status::Failed(error),
                });
                continue;
            }
        };
        let status = if token.is_canceled() {
            Status::Failed(Error::Canceled("update check").into())
        } else if let Some(limit) = &limited {
            Status::Failed(limit.to_error())
        } else {
            match compare(client, provider, &installed) {
                Ok(status) => status,
                Err(error) => {
                    limited = limited.or_else(|| download::rate_limited(&error));
                    Status::Failed(error)
                }
            }
        };
        families.push(Family {
            provider,
            installed: Some(installed.name),
            status,
        });
    }
    Report { families }
}

/// One download's progress, with where it sits in the batch.
#[derive(Debug)]
pub struct Step<'a> {
    pub index: usize,
    pub count: usize,
    pub release: &'a Release,
    pub progress: Progress,
}

#[derive(Debug)]
pub enum Outcome {
    Installed(PathBuf),
    /// A finished download of the same build was already there, likely from
    /// another run.
    AlreadyThere(PathBuf),
    Failed(anyhow::Error),
}

/// What `apply` did, one entry per release in the report's order.
#[derive(Debug)]
pub struct Applied {
    pub families: Vec<(Provider, Outcome)>,
}

impl Applied {
    pub fn failed(&self) -> bool {
        self.families
            .iter()
            .any(|(_, outcome)| matches!(outcome, Outcome::Failed(_)))
    }
}

/// Downloads every release `report` offers, one after another, continuing
/// past a family that fails. One token covers the batch: `install` looks at
/// it before anything else, so after a cancel the rest fail as canceled
/// without a request, and finished builds stay.
pub fn apply<T: Transport>(
    client: &Client<T>,
    paths: &Paths,
    global: &GlobalConfig,
    report: &Report,
    token: &Token,
    mut progress: impl FnMut(Step<'_>),
) -> Applied {
    let releases: Vec<&Release> = report.updates().collect();
    let count = releases.len();
    let mut limited: Option<RateLimited> = None;
    let mut families = Vec::new();
    for (index, release) in releases.into_iter().enumerate() {
        let outcome = if let Some(limit) = &limited {
            Outcome::Failed(limit.to_error())
        } else {
            let mut already = false;
            let installed = client.install(paths, global, release, token, |step| {
                already |= matches!(step, Progress::AlreadyInstalled(_));
                progress(Step {
                    index,
                    count,
                    release,
                    progress: step,
                });
            });
            match installed {
                Ok(path) if already => Outcome::AlreadyThere(path),
                Ok(path) => Outcome::Installed(path),
                Err(error) => {
                    limited = limited.or_else(|| download::rate_limited(&error));
                    Outcome::Failed(error)
                }
            }
        };
        families.push((release.provider, outcome));
    }
    Applied { families }
}

struct Installed {
    name: String,
    version: Vec<u64>,
}

/// The alias whose rule decides a runner family. None for the components.
fn alias(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::GeProton => Some("GE-Proton-Latest"),
        Provider::ProtonCachyos => Some("Proton-CachyOS-Latest"),
        Provider::Dxvk | Provider::Vkd3d => None,
    }
}

fn component(provider: Provider) -> Option<Component> {
    match provider {
        Provider::Dxvk => Some(Component::Dxvk),
        Provider::Vkd3d => Some(Component::Vkd3d),
        Provider::GeProton | Provider::ProtonCachyos => None,
    }
}

/// The newest build of `provider` in wineo's own root for it, by the
/// family's own rule, or None when the family isn't installed.
fn newest_installed(
    paths: &Paths,
    global: &GlobalConfig,
    provider: Provider,
) -> Result<Option<Installed>> {
    Ok(family_builds(paths, global, provider)?.pop())
}

/// Every build of `provider`'s family in wineo's own root, oldest first.
fn family_builds(
    paths: &Paths,
    global: &GlobalConfig,
    provider: Provider,
) -> Result<Vec<Installed>> {
    let root = provider.root(paths, global)?;
    if let Some(alias) = alias(provider) {
        return Ok(Runners::at(root)
            .family(alias)?
            .into_iter()
            .map(|(version, runner)| Installed {
                name: runner.name().to_owned(),
                version,
            })
            .collect());
    }
    let component = component(provider).expect("every provider is a runner or a component");
    let mut builds: Vec<_> = Builds::at(component, root.clone())
        .list()?
        .builds
        .into_iter()
        .filter(|build| upstream_name(provider, build.name()))
        .filter(|build| receipt_matches(provider, &root.join(build.name())))
        .map(|build| Installed {
            version: components::build_version(build.name()),
            name: build.name().to_owned(),
        })
        .collect();
    builds.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.name.cmp(&b.name)));
    Ok(builds)
}

/// The family's builds that are older than its newest, oldest first, leaving
/// out any that `wineo ... remove` would refuse whatever uses them: ones in a
/// root inside another launcher's folder, and ones that aren't plain folders.
/// What uses them is the removal scan's to say.
pub fn older_builds(
    paths: &Paths,
    global: &GlobalConfig,
    provider: Provider,
) -> Result<Vec<String>> {
    let root = provider.root(paths, global)?;
    if alias(provider).is_some() && external_runners::root_owner(paths, &root).is_some() {
        return Ok(Vec::new());
    }
    let mut builds = family_builds(paths, global, provider)?;
    builds.pop();
    Ok(builds
        .into_iter()
        .filter(|build| build_removal::managed(&root.join(&build.name), &build.name).is_ok())
        .map(|build| build.name)
        .collect())
}

/// Whether a component folder is named as a download names it, `dxvk-VER`
/// or `vkd3d-proton-VER` with VER dot-separated digits. Anything else, like
/// `dxvk-gplasync-2.8`, would compare as if it were upstream's.
fn upstream_name(provider: Provider, name: &str) -> bool {
    let prefix = match provider {
        Provider::Dxvk => "dxvk-",
        Provider::Vkd3d => "vkd3d-proton-",
        Provider::GeProton | Provider::ProtonCachyos => return false,
    };
    name.strip_prefix(prefix).is_some_and(|version| {
        version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

#[derive(Deserialize)]
struct Receipt {
    provider: String,
    architecture: String,
    stable: bool,
}

/// A build with no receipt goes by its name alone. One with a receipt must
/// say it is a stable x86_64 download of this provider, as a runner's must.
fn receipt_matches(provider: Provider, build: &Path) -> bool {
    match fs::read(build.join(RECEIPT)) {
        Ok(bytes) => serde_json::from_slice::<Receipt>(&bytes).is_ok_and(|receipt| {
            receipt.provider == provider.id() && receipt.architecture == "x86_64" && receipt.stable
        }),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// The family's newest release against its newest installed build, each
/// read by the family's own rule.
fn compare<T: Transport>(
    client: &Client<T>,
    provider: Provider,
    installed: &Installed,
) -> Result<Status> {
    let release = client.select(provider, None)?;
    let version = match alias(provider) {
        Some(alias) => crate::runner_alias::release_version(alias, &release.tag),
        None => upstream_name(provider, &release.installed_name)
            .then(|| components::build_version(&release.installed_name)),
    };
    let Some(version) = version else {
        bail!(
            "can't compare {}'s newest release, {}",
            provider.label(),
            release.tag
        );
    };
    if version <= installed.version {
        return Ok(Status::UpToDate);
    }
    if !release.verifiable() {
        bail!(
            "{} has no supported upstream checksum or GitHub SHA-256 digest; refusing unchecked installation",
            release.asset_name
        );
    }
    Ok(Status::Available(release))
}

#[cfg(test)]
mod tests;
