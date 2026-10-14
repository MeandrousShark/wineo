//! Choosing, copying, and completing the runner a Heroic game needs.

use std::io::Write;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::heroic::{HeroicImport, HeroicRunnerKind};
use wineo_core::heroic_import::{
    GamePlan, MissingBuild, RunnerCopy, RunnerSuggestion, runner_dependency_lines, suggest_runner,
};
use wineo_core::import::{self, ExternalDependency};
use wineo_core::paths::Paths;

use crate::commands::components;
use crate::commands::import::choose_runner;
use crate::commands::transfer::Cancellation;
use crate::prompt::{Canceled, Input, Prompt, Reply, numbered};

/// The runner picked for a game, and what copying Heroic's runner left linked
/// outside wineo.
pub(crate) struct ChosenRunner {
    pub(crate) name: String,
    pub(crate) dependencies: Vec<ExternalDependency>,
}

impl ChosenRunner {
    pub(crate) fn named(name: String) -> Self {
        Self {
            name,
            dependencies: Vec::new(),
        }
    }
}

/// Asks about the game's runner, or takes the `selected` one when it's there.
pub(crate) fn pick_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    requested: Option<&str>,
    prompt: &mut Prompt<I, W>,
    verbose: bool,
) -> anyhow::Result<ChosenRunner> {
    match suggest_runner(paths, global, imported, requested)? {
        RunnerSuggestion::Use(name) => return Ok(ChosenRunner::named(name)),
        RunnerSuggestion::Copy(copy) => {
            let (heading, label, other) = match requested {
                None => (
                    "Heroic runner is not installed",
                    format!(
                        "Copy Heroic runner {} from {}",
                        copy.name,
                        copy.source.display()
                    ),
                    "Choose another installed or downloaded runner",
                ),
                Some(_) => (
                    "Selected runner is not installed",
                    format!("Copy Heroic runner from {}", copy.source.display()),
                    "Choose another runner",
                ),
            };
            let choices = numbered([(label, true), (other.to_owned(), false)]);
            match prompt.choose(heading, "Runner", &choices, Some("1"), true)? {
                Reply::Value(true) => {
                    return copy_heroic_runner(paths, global, imported, &copy, prompt);
                }
                Reply::Value(false) => {}
                Reply::Back => return Err(anyhow::anyhow!(Canceled)),
            }
        }
        RunnerSuggestion::Choose { copy_blocked, .. } => {
            if let Some(reason) = copy_blocked {
                prompt.say(format!(
                    "Heroic runner cannot be copied: {reason}. Choose a compatible host runner."
                ))?;
            }
        }
    }
    let proton = Some(imported.runner.kind == HeroicRunnerKind::Proton);
    match choose_runner(prompt, paths, global, requested, proton, verbose)? {
        Reply::Value(value) => Ok(ChosenRunner::named(value)),
        Reply::Back => Err(anyhow::anyhow!(Canceled)),
    }
}

fn copy_heroic_runner<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    copy: &RunnerCopy,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ChosenRunner> {
    let root = global.runner_root(paths)?;
    let name = if wineo_core::name::is_valid_runner(&imported.runner.name)
        && !wineo_core::runner_alias::is_alias(&imported.runner.name)
        && !root.join(&imported.runner.name).exists()
    {
        imported.runner.name.clone()
    } else {
        match prompt.text("Wineo runner name", Some(&imported.runner.name), |value| {
            if !wineo_core::name::is_valid_runner(value) {
                return Err(
                    "runner names may use letters, digits, dots, dashes, and underscores"
                        .to_owned(),
                );
            }
            if wineo_core::runner_alias::is_alias(value) {
                return Err(
                    "latest aliases are managed by wineo; choose a concrete runner name".to_owned(),
                );
            }
            if root.join(value).exists() {
                return Err(format!("runner `{value}` is already installed"));
            }
            Ok(value.to_owned())
        })? {
            Reply::Value(value) => value,
            Reply::Back => return Err(Canceled.into()),
        }
    };
    let cancellation = Cancellation::new()?;
    let _source_guard = copy.lock_source(paths, global)?;
    let receipt = cancellation.published_result(import::copy_runner_reviewed(
        &copy.inspection,
        root,
        &name,
        &cancellation.token,
        |_| {},
    ))?;
    eprintln!(
        "Copied Heroic runner {} to {}",
        receipt.name,
        receipt.path.display()
    );
    eprintln!("This runner copy is retained if the prefix import is canceled.");
    for line in runner_dependency_lines(&receipt.external_dependencies, false) {
        eprintln!("Runner dependency: {line}");
    }
    Ok(ChosenRunner {
        name: receipt.name,
        dependencies: receipt.external_dependencies,
    })
}

/// Offers to download a missing DXVK/vkd3d-proton build for guided Heroic
/// import, the same way guided `new` does, one component at a time until
/// everything resolves or a person declines. Declining leaves the usual
/// error, so this changes nothing about what ends up refused.
pub(crate) fn ensure_components_available<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    ensure_components_available_with(paths, global, plan, prompt, |component, version| {
        components::download_build(paths, global, component, version)
    })
}

pub(super) fn ensure_components_available_with<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &GamePlan,
    prompt: &mut Prompt<I, W>,
    mut download: impl FnMut(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    loop {
        let Some(missing) = plan.missing_build(paths, global)? else {
            return Ok(());
        };
        let MissingBuild {
            component,
            ref version,
            ..
        } = missing;
        let question = match version.as_deref() {
            Some(name) => format!(
                "{} build `{name}` is not installed. Download that exact version?",
                component.label()
            ),
            None => format!(
                "No matching {} build is installed. Download the latest release?",
                component.label()
            ),
        };
        if !components::offer_download_with(
            prompt,
            component,
            version.as_deref(),
            &question,
            &mut download,
        )? {
            return Err(missing.into_error().into());
        }
    }
}
