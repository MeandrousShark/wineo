//! A native sideloaded Heroic game, imported as a native environment. There
//! is no prefix or runner, so this skips everything about them and ignores the
//! Wine settings Heroic keeps for native games too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{GlobalConfig, Target};
use crate::error::Error;
use crate::gamescope::GamescopeSetting;
use crate::heroic::{HeroicIssueKind, HeroicNativeImport, HeroicSource};
use crate::import::ImportedSettings;
use crate::launch;
use crate::name;
use crate::paths::Paths;

use super::{DesktopEntry, PlanError, Warning, default_name};

/// The target a native import gets.
const TARGET: &str = "game";

/// A native Heroic game and what would be imported of it.
#[derive(Debug, Clone)]
pub struct NativePlan {
    original: HeroicNativeImport,
    imported: HeroicNativeImport,
    name: String,
    desktop: DesktopEntry,
    cwd: Option<PathBuf>,
    /// Set only when migrating a GOG game.
    gog_id: Option<String>,
}

/// Inspects the native game `id` and plans it, named `name` or else a name
/// suggested from its title.
pub fn plan_native(
    source: &HeroicSource,
    id: &str,
    name: Option<String>,
) -> crate::Result<NativePlan> {
    let imported = source.inspect_native(id)?;
    let name = name.unwrap_or_else(|| default_name(&imported.title, Some(&imported.exe)));
    Ok(NativePlan::new(imported, name))
}

impl NativePlan {
    pub(crate) fn new(imported: HeroicNativeImport, name: String) -> Self {
        let desktop = DesktopEntry {
            enabled: false,
            name: imported.title.clone(),
        };
        Self {
            original: imported.clone(),
            imported,
            name,
            desktop,
            cwd: None,
            gog_id: None,
        }
    }

    /// The game as Heroic reported it.
    pub fn heroic(&self) -> &HeroicNativeImport {
        &self.original
    }

    /// The game as it will be imported, with every edit made to the plan.
    pub fn resolved(&self) -> &HeroicNativeImport {
        &self.imported
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn desktop(&self) -> &DesktopEntry {
        &self.desktop
    }

    /// The GOG game the environment is created for, when migrating one.
    pub fn gog_id(&self) -> Option<&str> {
        self.gog_id.as_deref()
    }

    pub(crate) fn set_gog_id(&mut self, gog_id: &str) {
        self.gog_id = Some(gog_id.to_owned());
    }

    /// Refuses a game whose stored settings can't be parsed, like its launcher
    /// arguments, rather than guessing at them.
    pub fn reject_invalid(&self) -> Result<(), PlanError> {
        match self
            .imported
            .issues
            .iter()
            .find(|issue| issue.kind == HeroicIssueKind::Invalid)
        {
            Some(issue) => Err(PlanError::Blocked(issue.message.clone())),
            None => Ok(()),
        }
    }

    /// Checks that the exe can be run, the way a launch would resolve it.
    pub fn check_exe(&self, paths: &Paths, global: &GlobalConfig) -> Result<(), PlanError> {
        match exe_problem(
            paths,
            global,
            &self.name,
            &self.imported.env,
            &self.imported.exe,
        ) {
            Some(problem) => Err(PlanError::Blocked(problem)),
            None => Ok(()),
        }
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), PlanError> {
        if let Some(problem) = name::problem(name) {
            return Err(PlanError::InvalidName(problem));
        }
        self.name = name.to_owned();
        Ok(())
    }

    pub fn set_gamemode(&mut self, enabled: bool) {
        self.imported.gamemode = Some(enabled);
    }

    pub fn set_mangohud(&mut self, enabled: bool) {
        self.imported.mangohud = Some(enabled);
    }

    pub fn set_gamescope(&mut self, setting: Option<GamescopeSetting>) {
        self.imported.gamescope = setting;
        self.imported.issues.retain(|issue| {
            !issue
                .field
                .as_deref()
                .is_some_and(|field| field.starts_with("gamescope."))
        });
    }

    pub fn set_environment<S: AsRef<str>>(
        &mut self,
        entries: impl IntoIterator<Item = S>,
    ) -> Result<(), PlanError> {
        let mut env = BTreeMap::new();
        for entry in entries {
            let Some((key, value)) = entry.as_ref().split_once('=') else {
                return Err(PlanError::EnvironmentFormat);
            };
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(PlanError::EnvironmentName(key.to_owned()));
            }
            if crate::heroic::MANAGED_ENV.contains(&key) {
                return Err(PlanError::ManagedEnvironment(key.to_owned()));
            }
            env.insert(key.to_owned(), value.to_owned());
        }
        self.imported.env = env;
        self.imported.issues.retain(|issue| {
            !issue.field.as_deref().is_some_and(|field| {
                field.starts_with("enviromentOptions")
                    || crate::heroic::MANAGED_ENV.contains(&field)
            })
        });
        Ok(())
    }

    pub fn set_executable(&mut self, exe: PathBuf) -> Result<(), PlanError> {
        if exe.as_os_str().is_empty() {
            return Err(PlanError::EmptyExecutable);
        }
        self.imported.exe = exe;
        Ok(())
    }

    pub fn set_working_directory(&mut self, cwd: Option<PathBuf>) {
        self.cwd = cwd;
    }

    pub fn working_directory(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    pub fn set_launcher_args(&mut self, value: &str) -> Result<(), PlanError> {
        let args =
            crate::heroic::parse_launcher_args(value).map_err(PlanError::InvalidArguments)?;
        self.imported.args = args;
        self.imported.issues.retain(|issue| {
            !(issue.kind == HeroicIssueKind::Invalid
                && issue.field.as_deref() == Some("launcherArgs"))
        });
        Ok(())
    }

    pub fn set_desktop_enabled(&mut self, enabled: bool) {
        self.desktop.enabled = enabled;
    }

    pub fn set_desktop_name(&mut self, name: &str) -> Result<(), PlanError> {
        if name.is_empty() {
            return Err(PlanError::EmptyDesktopName);
        }
        self.desktop.name = name.to_owned();
        Ok(())
    }

    /// The settings the environment is registered with.
    pub fn settings(&self) -> ImportedSettings {
        let imported = &self.imported;
        let target = Target {
            exe: imported.exe.to_string_lossy().into_owned(),
            args: imported.args.clone(),
            cwd: self
                .cwd
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            default: true,
            gamemode: None,
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            desktop: self.desktop.enabled,
            name: self.desktop.enabled.then(|| self.desktop.name.clone()),
        };
        ImportedSettings {
            targets: BTreeMap::from([(TARGET.to_owned(), target)]),
            env: imported.env.clone(),
            gamemode: imported.gamemode,
            mangohud: imported.mangohud,
            gamescope: imported.gamescope.clone(),
            hooks: imported.hooks.clone(),
            heroic_id: Some(imported.id.clone()),
            gog_id: self.gog_id.clone(),
            ..ImportedSettings::default()
        }
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

#[cfg(test)]
mod tests;
