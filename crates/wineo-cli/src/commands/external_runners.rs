//! The one-time question about builds Steam, Heroic, and Lutris installed.

use std::io::Write;

use wineo_core::config::GlobalConfig;
use wineo_core::external_runners::Source;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::settings;

use crate::prompt::{Input, Prompt, Reply};

const QUESTION: &str = "Use Proton and Wine builds from Steam, Heroic, and Lutris too?";

/// Whether the question is still open. A portable folder never uses other
/// launchers' builds, so there it isn't worth asking.
pub fn due(paths: &Paths, global: &GlobalConfig) -> bool {
    global.external_runners.is_none() && paths.portable_root().is_none()
}

/// `text` for a runner, naming the launcher when the build isn't wineo's own.
pub fn with_owner(paths: &Paths, runner: &Runner, text: String) -> String {
    match runner.owner(paths) {
        Source::Wineo => text,
        launcher => format!("{text} ({launcher})"),
    }
}

/// Asks the question and saves the answer, so it never comes up again.
/// Going back saves nothing.
pub fn offer<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
) -> anyhow::Result<Reply<bool>> {
    let reply = prompt.confirm(QUESTION, true)?;
    if let Reply::Value(on) = reply {
        settings::set_external_runners(paths, on)?;
    }
    Ok(reply)
}
