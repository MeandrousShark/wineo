use std::io::Write;
use std::path::Path;

use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::Provider;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::install::{self, Change, Wanted};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

use super::download;
use crate::prompt::{Input, Prompt, Reply};

/// Brings a prefix in line with what its config asks for, and says what it
/// changed. Every caller holds the environment's lock, since this writes into
/// the prefix. A prefix that already matches is untouched and silent.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    prefix: &Path,
    wanted: &Wanted,
) -> anyhow::Result<()> {
    for change in install::reconcile(paths, global, name, runner, prefix, wanted)? {
        eprintln!("{}", describe(&change, name));
    }
    Ok(())
}

/// The same, for the callers that want exactly what the config already says.
pub fn reconcile_config(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: &Runner,
    prefix: &Path,
) -> anyhow::Result<()> {
    let wanted = Wanted::from_config(&environment.config);
    reconcile(paths, global, &environment.name, runner, prefix, &wanted)
}

/// The component a `NoBuilds`/`UnknownBuild` error is about, the ones a
/// download can fix. `None` for every other problem, which needs a person.
pub fn missing_build_component(error: &Error) -> Option<Component> {
    let name = match error {
        Error::NoBuilds { component, .. } | Error::UnknownBuild { component, .. } => *component,
        _ => return None,
    };
    Component::ALL.into_iter().find(|c| c.as_str() == name)
}

/// Confirms `question`, and downloads the newest stable release for
/// `component` if the answer is yes. `Ok(true)` means it was downloaded.
pub fn offer_download<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    global: &GlobalConfig,
    component: Component,
    question: &str,
) -> anyhow::Result<bool> {
    if !matches!(prompt.confirm(question, true)?, Reply::Value(true)) {
        return Ok(false);
    }
    let provider = match component {
        Component::Dxvk => Provider::Dxvk,
        Component::Vkd3d => Provider::Vkd3d,
    };
    download::install_latest(paths, global, provider)?;
    Ok(true)
}

fn describe(change: &Change, name: &str) -> String {
    let label = |component: &Component| component.label();
    match change {
        Change::Installed { component, build } => {
            format!("Installed {} {build} into {name}", label(component))
        }
        Change::Replaced {
            component,
            from,
            to,
        } => format!("Replaced {} {from} with {to} in {name}", label(component)),
        Change::Removed { component, build } => {
            format!("Removed {} {build} from {name}", label(component))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::prompt::Script;

    fn paths(home: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home))).unwrap()
    }

    #[test]
    fn missing_build_component_reads_no_builds_and_unknown_build() {
        let no_builds = Error::NoBuilds {
            component: "dxvk",
            root: Path::new("/x").to_owned(),
        };
        assert_eq!(missing_build_component(&no_builds), Some(Component::Dxvk));

        let unknown = Error::UnknownBuild {
            component: "vkd3d",
            name: "nope".to_owned(),
            installed: Vec::new(),
            root: Path::new("/x").to_owned(),
        };
        assert_eq!(missing_build_component(&unknown), Some(Component::Vkd3d));
    }

    #[test]
    fn missing_build_component_ignores_other_errors() {
        assert_eq!(missing_build_component(&Error::NoHome), None);
    }

    #[test]
    fn offer_download_declines_without_downloading() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
        let downloaded = offer_download(
            &mut prompt,
            &paths,
            &global,
            Component::Dxvk,
            "Download the latest release?",
        )
        .unwrap();
        assert!(!downloaded);
    }
}
