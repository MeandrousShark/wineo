use std::io::Write;

use wineo_core::components::{Builds, Component};
use wineo_core::config::GlobalConfig;
use wineo_core::error::Error;
use wineo_core::heroic_import::{Entry, Game, LibraryPlan};
use wineo_core::paths::Paths;
use wineo_core::steamgriddb::{Api, Game as ArtGame};

use super::{Access, ArtChoice};
use crate::style;
use crate::text;

#[cfg(test)]
use crate::snapshot;

pub(super) fn art(
    access: &Access,
    choice: &ArtChoice,
    env: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) {
    let Some(key) = choice.key.as_ref().filter(|_| choice.enabled) else {
        return;
    };
    let result = (|| -> anyhow::Result<()> {
        let transport = (access.transport)()?;
        let games = Api::new(&*transport, &key.value).search(&choice.term)?;
        let Some(top) = games.first() else {
            anyhow::bail!("no SteamGridDB games match `{}`", choice.term);
        };
        writeln!(
            out,
            "{env}: art from SteamGridDB game {}, {}",
            top.id,
            game_name(top)
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = writeln!(
            err,
            "{}",
            style::warning_stderr(format!("warning: art fetch failed: {error:#}"))
        );
    }
}

fn game_name(game: &ArtGame) -> String {
    match game.year {
        Some(year) => format!("{} ({year})", game.name),
        None => game.name.clone(),
    }
}

pub(super) fn library(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    library: &LibraryPlan,
    fetch_art: bool,
    yes: bool,
) -> anyhow::Result<bool> {
    let backfills = library.backfills();
    if !backfills.is_empty() {
        let names = backfills
            .iter()
            .map(|backfill| backfill.environment.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let noun = text::plural(backfills.len(), "environment", "environments");
        println!(
            "Would record the Heroic id for {} existing {noun}: {names}",
            backfills.len()
        );
    }
    for (component, versions) in if yes {
        Vec::new()
    } else {
        missing_builds(paths, global, library)?
    } {
        for (version, count) in versions {
            let build = version.map_or_else(
                || format!("the latest {}", component.label()),
                |version| format!("{} {version}", component.label()),
            );
            let noun = text::plural(count, "game", "games");
            let target = text::plural(count, "it", "them");
            println!(
                "Would offer to download {build} for {count} {noun}. Declining skips {target}."
            );
        }
    }
    let key = if fetch_art && !global.art.fetch.kinds().is_empty() {
        let check = (access.check_key)();
        check.print_skip();
        ArtChoice::checked(check, "").key
    } else {
        None
    };
    let mut failed = false;
    for entry in library.entries() {
        let (name, title, checked) = match entry {
            Entry::Ready(Game::Windows(plan)) => (
                plan.name(),
                plan.heroic().title.as_str(),
                super::super::import::check_review(
                    paths,
                    global,
                    plan.request(),
                    plan.inspection(),
                ),
            ),
            Entry::Pending(game) if !yes => {
                // Accepting the build offer would make this game ready to import.
                let plan = game.plan();
                (
                    plan.name(),
                    plan.heroic().title.as_str(),
                    super::super::import::check_review(
                        paths,
                        global,
                        plan.request(),
                        plan.inspection(),
                    ),
                )
            }
            Entry::Ready(Game::Native(plan)) => (
                plan.name(),
                plan.heroic().title.as_str(),
                super::super::import::check_name(paths, plan.name()),
            ),
            Entry::Pending(_) | Entry::Skipped(_) => continue,
        };
        if let Err(error) = checked {
            println!("{name}: would fail: {error:#}");
            failed = true;
            continue;
        }
        art(
            access,
            &ArtChoice {
                key: key.clone(),
                enabled: key.is_some(),
                term: title.to_owned(),
            },
            name,
            &mut crate::output::stdout(),
            &mut crate::output::stderr(),
        );
    }
    Ok(failed)
}

type BuildCounts = Vec<(Component, Vec<(Option<String>, usize)>)>;

fn missing_builds(
    paths: &Paths,
    global: &GlobalConfig,
    library: &LibraryPlan,
) -> anyhow::Result<BuildCounts> {
    let mut builds = Vec::new();
    for component in Component::ALL {
        let mut versions: Vec<(Option<String>, usize)> = Vec::new();
        for entry in library.entries() {
            let Entry::Pending(game) = entry else {
                continue;
            };
            let plan = game.plan();
            let wanted = plan.wanted();
            let Some(asked) = wanted.get(component) else {
                continue;
            };
            // Checking each component also finds builds hidden behind the first missing one.
            let version = match Builds::new(paths, global, component)?.resolve(asked) {
                Ok(_) => continue,
                Err(Error::NoBuilds { .. }) => None,
                Err(Error::UnknownBuild { name, .. }) => Some(name),
                Err(error) => return Err(error.into()),
            };
            match versions
                .iter_mut()
                .find(|(existing, _)| *existing == version)
            {
                Some((_, count)) => *count += 1,
                None => versions.push((version, 1)),
            }
        }
        if !versions.is_empty() {
            builds.push((component, versions));
        }
    }
    Ok(builds)
}

#[cfg(test)]
mod tests;
