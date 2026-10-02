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

/// Suggests what to do about the runner of `imported`. With `requested`, a
/// runner the person named, it only steps in when that runner isn't installed
/// or is Heroic's own name for a copy wineo doesn't have.
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
    match requested {
        None => {
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
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;
    use crate::config::Defaults;
    use crate::heroic_import::plan::tests::{
        fake_proton_runner, fake_wine_runner, paths, reviewed_import,
    };

    fn global(root: &Path) -> GlobalConfig {
        GlobalConfig {
            defaults: Defaults {
                runner_root: Some(root.join("wineo-runners").display().to_string()),
                ..Defaults::default()
            },
            ..GlobalConfig::default()
        }
    }

    #[test]
    fn runner_copy_preflight_checks_access_and_kind() {
        let root = tempdir().unwrap();
        let mut imported = reviewed_import(root.path());
        imported.runner.source = Some(root.path().join("missing"));
        assert!(inspect_copyable_runner(&imported).is_err());
        let runner_root = root.path().join("runners");
        fake_wine_runner(&runner_root, "wine-tkg");
        imported.runner.source = Some(runner_root.join("wine-tkg"));
        imported.runner.kind = HeroicRunnerKind::Wine;
        assert!(inspect_copyable_runner(&imported).is_ok());
        imported.runner.kind = HeroicRunnerKind::Proton;
        assert!(inspect_copyable_runner(&imported).is_err());
    }

    #[test]
    fn an_installed_runner_of_the_right_kind_is_used() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        fake_wine_runner(&root.path().join("wineo-runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.runner.kind = HeroicRunnerKind::Wine;
        imported.runner.name = "wine-tkg".to_owned();
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "wine-tkg"));

        // An installed runner of another family isn't Heroic's choice.
        fake_proton_runner(&root.path().join("wineo-runners"), "GE-Proton9-1");
        imported.runner.kind = HeroicRunnerKind::Proton;
        imported.runner.name = "wine-tkg".to_owned();
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(
            suggestion,
            RunnerSuggestion::Choose {
                proton: true,
                copy_blocked: None
            }
        ));
    }

    #[test]
    fn heroics_own_copy_is_offered_when_wineo_lacks_it() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        let heroic_tools = root.path().join("heroic-tools");
        fake_wine_runner(&heroic_tools, "wine-heroic");
        let mut imported = reviewed_import(root.path());
        imported.runner.kind = HeroicRunnerKind::Wine;
        imported.runner.name = "wine-heroic".to_owned();
        imported.runner.source = Some(heroic_tools.join("wine-heroic"));
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        let RunnerSuggestion::Copy(copy) = suggestion else {
            panic!("expected a copy offer");
        };
        assert_eq!(copy.name, "wine-heroic");
        assert_eq!(copy.source, heroic_tools.join("wine-heroic"));

        // Naming another runner that isn't installed doesn't offer Heroic's.
        let suggestion = suggest_runner(&paths, &global, &imported, Some("nope")).unwrap();
        assert!(matches!(
            suggestion,
            RunnerSuggestion::Choose {
                proton: false,
                copy_blocked: None
            }
        ));
        // Naming Heroic's own runner does.
        let suggestion = suggest_runner(&paths, &global, &imported, Some("wine-heroic")).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
    }

    /// Heroic's runner, plus a same-named Proton runner in wineo's root with an
    /// optional receipt.
    fn copied_proton(root: &Path, receipt: Option<&str>) -> HeroicImport {
        let heroic_tools = root.join("heroic-tools");
        fake_proton_runner(&heroic_tools, "GE-Proton-latest");
        let installed = root.join("wineo-runners");
        fake_proton_runner(&installed, "GE-Proton-latest");
        if let Some(receipt) = receipt {
            fs::write(
                installed.join("GE-Proton-latest/.wineo-install.json"),
                receipt,
            )
            .unwrap();
        }
        let mut imported = reviewed_import(root);
        imported.runner.kind = HeroicRunnerKind::Proton;
        imported.runner.name = "GE-Proton-latest".to_owned();
        imported.runner.source = Some(heroic_tools.join("GE-Proton-latest"));
        imported
    }

    fn copy_receipt(source: &Path, method: &str) -> String {
        serde_json::json!({
            "architecture": "x86_64",
            "asset": "",
            "digest": "",
            "method": method,
            "provider": "ge-proton",
            "source": source,
            "stable": true,
            "variant": "baseline",
            "verified_digests": [],
            "version": "GE-Proton10-3",
        })
        .to_string()
    }

    #[test]
    fn a_runner_copied_from_heroic_is_used() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        let source = root.path().join("heroic-tools/GE-Proton-latest");
        let imported = copied_proton(root.path(), Some(&copy_receipt(&source, "copy")));
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
        let suggestion =
            suggest_runner(&paths, &global, &imported, Some("GE-Proton-latest")).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
    }

    #[test]
    fn a_same_named_runner_copied_from_elsewhere_is_not_heroics() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        let other = root.path().join("elsewhere/GE-Proton-latest");
        let imported = copied_proton(root.path(), Some(&copy_receipt(&other, "copy")));
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));

        // Only a copy receipt counts, not another install method.
        let source = root.path().join("heroic-tools/GE-Proton-latest");
        let imported = copied_proton(root.path(), Some(&copy_receipt(&source, "download")));
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
    }

    #[test]
    fn a_same_named_runner_without_a_usable_receipt_is_not_heroics() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        let imported = copied_proton(root.path(), None);
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));

        let imported = copied_proton(root.path(), Some("not json"));
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
    }

    #[test]
    fn a_copy_that_cannot_be_made_says_why() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        let mut imported = reviewed_import(root.path());
        imported.runner.kind = HeroicRunnerKind::Wine;
        imported.runner.name = "gone".to_owned();
        imported.runner.source = Some(root.path().join("missing"));
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        let RunnerSuggestion::Choose {
            proton,
            copy_blocked,
        } = suggestion
        else {
            panic!("expected a choice");
        };
        assert!(!proton);
        assert!(copy_blocked.is_some());
    }

    #[test]
    fn a_named_runner_that_is_installed_is_used_as_named() {
        let root = tempdir().unwrap();
        let global = global(root.path());
        let paths = paths(root.path());
        fake_wine_runner(&root.path().join("wineo-runners"), "wine-tkg");
        let imported = reviewed_import(root.path());
        let suggestion = suggest_runner(&paths, &global, &imported, Some("wine-tkg")).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "wine-tkg"));
    }
}
