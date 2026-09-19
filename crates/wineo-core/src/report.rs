use serde::Serialize;

use crate::components::{Builds, Component, LATEST};
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::Result;
use crate::install::{self, Wanted};
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::runners::Runners;
use crate::runtime::{Runtime, Status};

/// Everything `wineo status <name>` shows, gathered here so `--json` stays
/// cheap to add later.
#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentReport {
    pub name: String,
    pub status: Status,
    pub runner: String,
    pub runner_version: Option<String>,
    /// The prefix as stored in the config, e.g. `~/.local/share/wineo/prefixes/notepad`.
    pub prefix: String,
    /// The exe of the run's target in the current config. None if there's no
    /// run, or its target was removed from the config since.
    pub target_exe: Option<String>,
    /// An operation holding the environment right now, like a winetricks install.
    pub busy: Option<Operation>,
    /// Whether the last run's target, or the environment itself, asks for gamemode.
    pub gamemode: bool,
    /// What umu is told to look protonfixes up by, when this environment asked
    /// for them.
    pub gameid: Option<String>,
    pub store: Option<String>,
    /// The Direct3D builds this environment asks for, when it asks for any.
    pub dxvk: Option<ComponentReport>,
    pub vkd3d: Option<ComponentReport>,
}

/// What a config asks for against what its prefix actually has.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentReport {
    /// The config value, which may be `latest`.
    pub wanted: String,
    /// What `wanted` names right now. None when nothing is installed to pick.
    pub resolved: Option<String>,
    /// What the prefix's manifest records.
    pub installed: Option<String>,
    /// Whether the prefix already has what the config asks for.
    pub current: bool,
}

impl EnvironmentReport {
    pub fn load(paths: &Paths, global: &GlobalConfig, environment: &Environment) -> Result<Self> {
        let status = Runtime::new(paths, &environment.name).status()?;
        // An unresolvable runner is worth showing the rest of the status for.
        let runner_version = Runners::new(paths, global)
            .and_then(|runners| runners.resolve(&environment.config.runner))
            .ok()
            .and_then(|runner| runner.version());
        let target = status
            .run
            .as_ref()
            .and_then(|run| environment.config.targets.get(&run.target));
        let target_exe = target.map(|target| target.exe.clone());
        let components = components(paths, global, environment);
        Ok(Self {
            dxvk: components.0,
            vkd3d: components.1,
            busy: Locks::new(paths).busy_with(&environment.name)?,
            gamemode: crate::launch::gamemode(global, environment, target),
            name: environment.name.clone(),
            runner: environment.config.runner.clone(),
            runner_version,
            prefix: environment.config.prefix.clone(),
            gameid: environment.config.gameid.clone(),
            store: environment.config.store.clone(),
            target_exe,
            status,
        })
    }
}

/// Reading the manifest needs the runner and the prefix, and neither is
/// worth failing a whole status over, so an unreadable one reports nothing.
fn components(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> (Option<ComponentReport>, Option<ComponentReport>) {
    let wanted = Wanted::from_config(&environment.config);
    let resolved = Runners::new(paths, global)
        .and_then(|runners| runners.resolve(&environment.config.runner))
        .ok()
        .zip(environment.prefix(paths).ok());

    let report = |component: Component| {
        let asked = wanted.get(component)?.to_owned();
        let build = Builds::new(paths, global, component)
            .and_then(|builds| builds.resolve(&asked))
            .ok()
            .map(|build| build.name().to_owned());
        let installed = resolved
            .as_ref()
            .and_then(|(runner, prefix)| install::installed(runner, prefix, component).ok())
            .flatten();
        // A pinned build whose directory was removed still counts as current,
        // since its DLLs are in the prefix either way.
        let current = installed.as_deref() == build.as_deref()
            || installed.as_deref() == Some(asked.as_str());
        Some(ComponentReport {
            wanted: asked,
            resolved: build,
            installed,
            current,
        })
    };
    (report(Component::Dxvk), report(Component::Vkd3d))
}

/// The spelling `status` uses for a component line.
pub fn component_value(report: &ComponentReport) -> String {
    let mut value = if report.wanted == LATEST {
        match &report.resolved {
            Some(resolved) => format!("{LATEST} ({resolved})"),
            None => format!("{LATEST} (nothing installed)"),
        }
    } else {
        report.wanted.clone()
    };
    if !report.current {
        value.push_str(", will be installed on the next launch");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::*;
    use crate::config::EnvironmentConfig;
    use crate::runtime::{RunMode, RunState};

    fn test_paths(root: &Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(root.join("home"))),
            _ => None,
        })
        .unwrap()
    }

    fn environment(text: &str) -> Environment {
        let text =
            format!("prefix = \"~/prefix\"\nrunner = \"proton-ge\"\narch = \"win64\"\n{text}");
        Environment {
            name: "notepad".to_owned(),
            path: Path::new("/notepad.toml").to_owned(),
            config: EnvironmentConfig::parse(Path::new("/notepad.toml"), &text).unwrap(),
        }
    }

    // An unsupported runner name resolves deterministically to no version,
    // without needing wine installed to run this test.
    #[test]
    fn runner_version_is_none_when_the_runner_cant_be_resolved() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let env = environment("");
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.runner, "proton-ge");
        assert_eq!(report.runner_version, None);
        assert_eq!(report.prefix, "~/prefix");
    }

    #[test]
    fn target_exe_is_looked_up_from_the_current_config() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let env = environment("[targets.notepad]\nexe = \"notepad\"\n");
        let runtime = Runtime::new(&paths, &env.name);

        // No run yet: no target to report.
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.target_exe, None);

        let state = RunState::running(
            RunMode::Detached,
            "notepad".to_owned(),
            1,
            Some(1),
            &Default::default(),
        );
        runtime.write_state(&state).unwrap();
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.target_exe.as_deref(), Some("notepad"));

        let mut state = state;
        state.target = "gone".to_owned();
        runtime.write_state(&state).unwrap();
        let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
        assert_eq!(report.target_exe, None);
    }
}
