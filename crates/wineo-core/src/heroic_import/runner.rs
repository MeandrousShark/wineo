//! Which wineo runner a Heroic game should use.

use std::fs;
use std::path::PathBuf;

use anyhow::Context;

use crate::config::GlobalConfig;
use crate::download;
use crate::heroic::{HeroicImport, HeroicRunnerKind};
use crate::import::{self, RunnerCopyInspection, RunnerProvenance};
use crate::paths::Paths;
use crate::runner::Runner;
use crate::runners::Runners;

/// Heroic's own runner tree, checked and ready to copy into wineo.
#[derive(Debug, Clone)]
pub struct RunnerCopy {
    pub inspection: RunnerCopyInspection,
    /// The name Heroic gave the runner.
    pub name: String,
    /// Where Heroic keeps it.
    pub source: PathBuf,
}

impl RunnerCopy {
    pub fn lock_source(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
    ) -> crate::Result<Option<crate::build_lock::Guard>> {
        let runners = Runners::new(paths, global)?;
        let source = crate::paths::resolve(&self.source);
        for runner in runners.list()?.runners {
            if crate::paths::resolve(&runners.root().join(runner.name())) == source {
                return crate::build_lock::runner(paths, &runner);
            }
        }
        Ok(None)
    }
}

/// What to do about a game's runner.
#[derive(Debug, Clone)]
pub enum RunnerSuggestion {
    /// The runner is installed in wineo, under this name.
    Use(String),
    /// The runner isn't installed, but Heroic's copy can be copied in.
    Copy(RunnerCopy),
    /// Choose from the installed or downloadable runners of a family. Holds
    /// why Heroic's own copy can't be used, when it has one.
    Choose {
        proton: bool,
        copy_blocked: Option<String>,
    },
}

/// Checks that Heroic's own copy of the game's runner can be copied, and is the
/// right kind of build for the game.
pub fn inspect_copyable_runner(imported: &HeroicImport) -> anyhow::Result<RunnerCopyInspection> {
    let source = imported
        .runner
        .source
        .as_deref()
        .context("Heroic did not provide a runner source")?;
    let reviewed = import::inspect_runner_copy(source)?;
    let compatible = match imported.runner.kind {
        HeroicRunnerKind::Proton => reviewed.supports_proton,
        HeroicRunnerKind::Wine | HeroicRunnerKind::System => reviewed.supports_wine,
        HeroicRunnerKind::Unknown => false,
    };
    anyhow::ensure!(
        compatible,
        "{} is not a compatible {} build",
        source.display(),
        if imported.runner.kind == HeroicRunnerKind::Proton {
            "Proton"
        } else {
            "Wine"
        }
    );
    Ok(reviewed)
}

/// Whether `runner` is the copy Heroic keeps, or a copy wineo made from it.
fn runner_matches_source(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    runner: &Runner,
) -> bool {
    let Some(source) = imported.runner.source.as_deref() else {
        return true;
    };
    let Ok(root) = global.runner_root(paths) else {
        return false;
    };
    let installed = root.join(runner.name());
    let source = crate::paths::resolve(source);
    if source == crate::paths::resolve(&installed) {
        return true;
    }
    // A copy lives under its own path, so only its receipt says where it came
    // from. A missing or unreadable receipt just means it isn't a known copy.
    fs::read(installed.join(download::RECEIPT))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RunnerProvenance>(&bytes).ok())
        .is_some_and(|receipt| {
            receipt.method == "copy" && crate::paths::resolve(&receipt.source) == source
        })
}

/// The name wineo knows Heroic's own build by, when external builds are on and
/// wineo can use it where it is. Heroic names a runner by its folder, which
/// wineo may have had to derive another name from. A same-named build in a
/// higher priority place, like wineo's own root, is what that name resolves
/// to, and it's what the game runs with.
fn in_place_name(runners: &Runners, imported: &HeroicImport) -> Option<String> {
    let source = imported.runner.source.as_deref()?;
    let proton = imported.runner.kind == HeroicRunnerKind::Proton;
    runners
        .external_names_at(source)
        .into_iter()
        .find_map(|name| {
            let runner = runners.resolve(&name).ok()?;
            (runner.is_proton() == proton).then(|| runner.selection().to_owned())
        })
}

/// Suggests what to do about the runner of `imported`. With `requested`, a
/// runner the person named, it only steps in when that runner isn't installed
/// or is Heroic's own name for a copy wineo doesn't have. With external
/// builds on, Heroic's build is used where it is and nothing is copied.
pub fn suggest_runner(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    requested: Option<&str>,
) -> crate::Result<RunnerSuggestion> {
    let proton = imported.runner.kind == HeroicRunnerKind::Proton;
    let runners = Runners::new(paths, global)?;
    let copy_offer = |copy_blocked: &mut Option<String>| {
        let source = imported.runner.source.as_deref()?;
        match inspect_copyable_runner(imported) {
            Ok(inspection) => Some(RunnerCopy {
                inspection,
                name: imported.runner.name.clone(),
                source: source.to_owned(),
            }),
            Err(error) => {
                *copy_blocked = Some(format!("{error:#}"));
                None
            }
        }
    };
    let mut copy_blocked = None;
    let in_place = in_place_name(&runners, imported);
    match requested {
        None => {
            if let Some(name) = in_place {
                return Ok(RunnerSuggestion::Use(name));
            }
            if let Ok(runner) = runners.resolve(&imported.runner.name)
                && runner_matches_source(paths, global, imported, &runner)
                && proton == runner.is_proton()
            {
                return Ok(RunnerSuggestion::Use(runner.selection().to_owned()));
            }
            if let Some(copy) = copy_offer(&mut copy_blocked) {
                return Ok(RunnerSuggestion::Copy(copy));
            }
        }
        Some(selected) => {
            if let Ok(runner) = runners.resolve(selected)
                && (selected != imported.runner.name
                    || runner_matches_source(paths, global, imported, &runner))
            {
                return Ok(RunnerSuggestion::Use(selected.to_owned()));
            }
            if selected == imported.runner.name
                && let Some(name) = in_place
            {
                return Ok(RunnerSuggestion::Use(name));
            }
            if selected == imported.runner.name
                && let Some(copy) = copy_offer(&mut copy_blocked)
            {
                return Ok(RunnerSuggestion::Copy(copy));
            }
        }
    }
    Ok(RunnerSuggestion::Choose {
        proton,
        copy_blocked,
    })
}

#[cfg(test)]
mod tests;
