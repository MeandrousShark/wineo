//! Registering a planned Heroic import.
//!
//! Applying takes two calls, `apply` and then `Registered::finish`, because a
//! frontend fetches art between them: the desktop entry is written last so it
//! gets the art's icon from the start.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::art::{self, Kind};
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::desktop::{self, Change};
use crate::environment::Environment;
use crate::heroic::HeroicSource;
use crate::import::{self, ImportProgress};
use crate::paths::Paths;
use crate::runner::Runner;
use crate::stats;

use super::{GamePlan, NativePlan};

/// Why an apply was refused before it wrote anything.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("Heroic settings changed while they were being reviewed; inspect the game again")]
    SourceChanged,
}

/// What became of the playtime Heroic tracked for the game. Never fails an
/// import: a game worth playing is worth importing even if its history can't be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playtime {
    /// Heroic never played it, so there was nothing to carry over.
    None,
    Imported,
    /// Heroic's playtime couldn't be read. Holds why.
    Unreadable(String),
    /// The environment's play log couldn't be written. Holds why.
    NotRecorded(String),
}

/// An environment that's registered, with its desktop entries still to write.
#[derive(Debug)]
pub struct Registered {
    pub environment: Environment,
    pub playtime: Playtime,
    pub warnings: Vec<String>,
    runner: Option<Runner>,
    destination: Option<PathBuf>,
}

impl Registered {
    /// The runner the environment was imported with. None for a native game.
    pub fn runner(&self) -> Option<&Runner> {
        self.runner.as_ref()
    }

    /// Where the prefix lives. None for a native game.
    pub fn destination(&self) -> Option<&Path> {
        self.destination.as_deref()
    }

    /// Brings the environment's desktop entries in line with its config, and
    /// says what changed.
    pub fn finish(&self, paths: &Paths, global: &GlobalConfig) -> crate::Result<Vec<Change>> {
        match (&self.runner, &self.destination) {
            (Some(runner), Some(destination)) => {
                desktop::reconcile(paths, global, &self.environment, Some(runner), destination)
            }
            _ => desktop::reconcile_native(paths, global, &self.environment),
        }
    }
}

/// Registers `plan` as an environment, copying or adopting its prefix, and
/// carries over Heroic's playtime. Refuses when Heroic's settings changed since
/// the game was inspected.
pub fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    plan: &GamePlan,
    token: &Token,
    progress: impl FnMut(ImportProgress),
) -> anyhow::Result<Registered> {
    ensure_unchanged(source, &plan.heroic().id, &plan.heroic().fingerprint)?;
    let settings = plan.settings();
    let environment = import::execute_reviewed(
        paths,
        global,
        plan.request().clone(),
        Some(plan.inspection()),
        Some(&settings),
        token,
        progress,
    )?;
    let playtime = import_playtime(paths, source, &plan.heroic().id, &environment.name);
    Ok(Registered {
        environment,
        playtime,
        warnings: Vec::new(),
        runner: Some(plan.runner().clone()),
        destination: Some(plan.destination().to_owned()),
    })
}

/// Registers `plan` as a native environment, which has no prefix, and carries
/// over Heroic's playtime. Refuses when Heroic's settings changed since the game
/// was inspected.
pub fn apply_native(
    paths: &Paths,
    source: &HeroicSource,
    plan: &NativePlan,
) -> anyhow::Result<Registered> {
    ensure_unchanged(source, &plan.heroic().id, &plan.heroic().fingerprint)?;
    let mut warnings = Vec::new();
    let environment = import::register_native(paths, plan.name(), &plan.settings(), |warning| {
        warnings.push(warning)
    })?;
    let playtime = import_playtime(paths, source, &plan.heroic().id, &environment.name);
    Ok(Registered {
        environment,
        playtime,
        warnings,
        runner: None,
        destination: None,
    })
}

/// Stores Heroic's cached `poster` as `env`'s Grid art, recorded as a manual
/// pick named `heroic-<app_name>.<ext>` so its origin is visible. Returns None
/// without storing anything when the file is gone, too large, or not a format
/// Grid art can be. Heroic's file is only read.
///
/// The format is judged by the file's signature alone, since core has no
/// image decoder: a file with a valid header and a corrupt body is stored.
/// Callers must pass a poster they have already decoded successfully.
pub fn store_poster(
    paths: &Paths,
    env: &str,
    app_name: &str,
    poster: &Path,
) -> crate::Result<Option<art::Stored>> {
    let mut bytes = Vec::new();
    let read = fs::File::open(poster)
        .and_then(|file| file.take(art::MAX_BYTES + 1).read_to_end(&mut bytes));
    match read {
        Ok(_) => {}
        // Heroic may have pruned its cache since the game was listed.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(crate::error::Error::Io {
                context: format!("failed to read {}", poster.display()),
                source: error,
            });
        }
    }
    if bytes.len() as u64 > art::MAX_BYTES {
        return Ok(None);
    }
    let Some(format) = art::detect(&bytes).filter(|format| Kind::Grid.accepts(*format)) else {
        return Ok(None);
    };
    art::set_manual_bytes(
        paths,
        env,
        None,
        Kind::Grid,
        &bytes,
        &poster_name(app_name, format),
    )
    .map(Some)
}

/// The source name a Heroic poster is recorded under, so its provenance reads
/// as a manual pick from Heroic.
pub fn poster_name(app_name: &str, format: art::Format) -> String {
    format!("heroic-{app_name}.{}", format.extension())
}

pub(super) fn ensure_unchanged(
    source: &HeroicSource,
    id: &str,
    fingerprint: &crate::heroic::InputFingerprint,
) -> anyhow::Result<()> {
    if source.unchanged(id, fingerprint)? {
        Ok(())
    } else {
        Err(ApplyError::SourceChanged.into())
    }
}

/// Carries `game_id`'s Heroic playtime into `name`'s new play log, if Heroic
/// ever played it.
pub(super) fn import_playtime(
    paths: &Paths,
    source: &HeroicSource,
    game_id: &str,
    name: &str,
) -> Playtime {
    match source.playtime(game_id) {
        Ok(Some(playtime)) => {
            let imported = stats::Imported {
                seconds: playtime.seconds,
                last_played: playtime.last_played,
            };
            match stats::set_imported(paths, name, imported) {
                Ok(()) => Playtime::Imported,
                Err(error) => Playtime::NotRecorded(format!("{:#}", anyhow::Error::from(error))),
            }
        }
        Ok(None) => Playtime::None,
        Err(error) => Playtime::Unreadable(format!("{:#}", anyhow::Error::from(error))),
    }
}

/// Appends `body` to `name`'s import log as a timestamped section, and says
/// where the log is.
pub fn write_import_log(paths: &Paths, name: &str, body: &str) -> anyhow::Result<PathBuf> {
    let path = paths.import_log(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "=== {} ===", jiff::Timestamp::now())?;
    write!(file, "{body}")?;
    if !body.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file)?;
    Ok(path)
}

#[cfg(test)]
mod tests;
