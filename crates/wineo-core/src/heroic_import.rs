//! Importing Heroic sideloads: what a game would become, and what a frontend
//! can change before it is registered.
//!
//! A frontend inspects a game with `HeroicSource`, plans it against a wineo
//! runner with `GamePlan::new`, checks the plan's findings and warnings, edits
//! it through the plan's setters, and applies it. The plan never writes
//! anything and never prints, so a GUI can build and rebuild one freely.

mod plan;
mod runner;

pub use plan::{
    COVERED_BY_PROTON, DesktopEntry, Finding, GamePlan, InputKind, MissingBuild, Options,
    PlanError, Policy, Resolution, Warning, is_dropped_conflict, redact_env_value,
    runner_dependency_lines, shared_prefix_lines,
};
pub use runner::{RunnerCopy, RunnerSuggestion, inspect_copyable_runner, suggest_runner};

/// Why a game can't be imported, without any frontend's wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Heroic stored launcher arguments that can't be parsed, and nobody was
    /// there to correct them.
    LauncherArgs,
    /// Something in Heroic's settings can't be imported. Holds the message of
    /// the first setting that blocks it.
    Blocked(String),
}
