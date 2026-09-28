//! A native sideloaded Heroic game, imported as a native environment. There
//! is no prefix or runner, so this skips everything about them and ignores the
//! Wine settings Heroic keeps for native games too.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::GlobalConfig;
use crate::error::Error;
use crate::heroic::{HeroicIssueKind, HeroicNativeImport};
use crate::launch;
use crate::paths::Paths;

use super::Warning;

/// A native Heroic game and what would be imported of it.
#[derive(Debug, Clone)]
pub struct NativePlan {
    imported: HeroicNativeImport,
    name: String,
}

impl NativePlan {
    pub(crate) fn new(imported: HeroicNativeImport, name: String) -> Self {
        Self { imported, name }
    }

    /// The game as Heroic reported it.
    pub fn heroic(&self) -> &HeroicNativeImport {
        &self.imported
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// What wasn't imported, if anything.
    pub fn warnings(&self) -> Vec<Warning> {
        let omitted = self
            .imported
            .issues
            .iter()
            .filter(|issue| issue.kind == HeroicIssueKind::Unsupported)
            .count();
        let mut warnings = Vec::new();
        if omitted > 0 {
            warnings.push(Warning::Omitted(omitted));
        }
        let attention: Vec<_> = self
            .imported
            .issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.kind,
                    HeroicIssueKind::NeedsAttention | HeroicIssueKind::Conflict
                )
            })
            .cloned()
            .collect();
        if !attention.is_empty() {
            warnings.push(Warning::NeedsAttention(attention));
        }
        warnings
    }
}

/// Why a native game's exe can't be run, checked the way a launch resolves it.
pub(super) fn exe_problem(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    env: &BTreeMap<String, String>,
    exe: &Path,
) -> Option<String> {
    match launch::check_native_exe(paths, global, name, env, &exe.to_string_lossy()) {
        Ok(_) => None,
        Err(Error::ExeNotFound(_) | Error::NotOnPath(_)) => {
            Some(format!("executable not found: {}", exe.display()))
        }
        Err(err) => Some(err.to_string()),
    }
}
