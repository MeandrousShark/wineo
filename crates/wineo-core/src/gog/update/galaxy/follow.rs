//! Which build a Galaxy game goes to: the branch it follows, a pin to one
//! build, and what a caller asks for (`--branch`, `--default-branch`,
//! `--build`, `--latest`). The state a choice leaves is a `Follow`, kept in
//! the record.

use wineo_gog::{Build, Builds};

use super::build;
use super::install::GalaxyError;
use super::record::Follow;

/// What a caller asks about the branch and build, besides keeping them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Choice {
    /// Follow what the record says.
    #[default]
    Keep,
    /// Follow this branch, unpinned.
    Branch(String),
    /// Follow the default branch, unpinned.
    DefaultBranch,
    /// Pin to this build, named by its id or its version name.
    Build(String),
    /// Unpin, and go to the latest build of the branch followed.
    Latest,
}

impl Choice {
    /// Whether this changes nothing about the branch or pin by itself.
    pub fn is_keep(&self) -> bool {
        *self == Self::Keep
    }
}

impl Follow {
    /// The default branch, unpinned.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Who the errors speak of.
pub(super) struct Who<'a> {
    pub(super) title: &'a str,
    /// The environment, for the way to its `--builds`. None before there is
    /// one.
    pub(super) name: Option<&'a str>,
}

/// The build a choice goes to, and the follow it leaves.
pub(super) struct Picked<'a> {
    pub(super) build: &'a Build,
    pub(super) to: Follow,
}

/// The latest generation 2 build on `branch`, or why the game has none
/// there.
pub(super) fn latest_on<'a>(
    builds: &'a Builds,
    branch: Option<&str>,
    who: &Who,
) -> Result<&'a Build, GalaxyError> {
    build::latest(builds, branch).ok_or_else(|| match branch {
        None => GalaxyError::NoBuild {
            title: who.title.to_owned(),
        },
        Some(branch) => GalaxyError::BranchEmpty {
            name: who.name.unwrap_or_default().to_owned(),
            branch: branch.to_owned(),
        },
    })
}

/// The generation 2 build `spec` names: an id, else a version name that
/// only one build has. One build listed on two branches counts once, on
/// `prefer` where it is.
fn named<'a>(
    builds: &'a Builds,
    spec: &str,
    prefer: Option<&str>,
    who: &Who,
) -> Result<&'a Build, GalaxyError> {
    let listed = || {
        builds
            .items
            .iter()
            .filter(|build| build.generation == Some(2) && build.link.is_some())
    };
    let mut found: Vec<&Build> = listed().filter(|build| build.build_id == spec).collect();
    if found.is_empty() {
        found = listed()
            .filter(|build| build.version_name.as_deref() == Some(spec))
            .collect();
        let mut builds: Vec<String> = Vec::new();
        for build in &found {
            if !builds.contains(&build.build_id) {
                builds.push(build.build_id.clone());
            }
        }
        if builds.len() > 1 {
            return Err(GalaxyError::AmbiguousBuild {
                version: spec.to_owned(),
                builds,
            });
        }
    }
    found
        .iter()
        .find(|build| build.branch.as_deref() == prefer)
        .or(found.first())
        .copied()
        .ok_or_else(|| GalaxyError::NoSuchBuild {
            build: spec.to_owned(),
            title: who.title.to_owned(),
            name: who.name.map(str::to_owned),
        })
}

/// The build `choice` goes to for a game that follows `from`, and the
/// follow it leaves. `reinstall` says a pinned game rebuilds the build it is
/// pinned to rather than leaving it alone.
pub(super) fn pick<'a>(
    builds: &'a Builds,
    choice: &Choice,
    from: &Follow,
    reinstall: bool,
    who: &Who,
) -> Result<Picked<'a>, GalaxyError> {
    match choice {
        Choice::Keep => {
            if let (true, Some(pinned)) = (reinstall, &from.pinned) {
                let build = named(builds, pinned, from.branch.as_deref(), who).map_err(|_| {
                    GalaxyError::PinGone {
                        title: who.title.to_owned(),
                        build: pinned.clone(),
                        name: who.name.unwrap_or_default().to_owned(),
                    }
                })?;
                return Ok(Picked {
                    build,
                    to: from.clone(),
                });
            }
            Ok(Picked {
                build: latest_on(builds, from.branch.as_deref(), who)?,
                to: from.clone(),
            })
        }
        Choice::Latest => Ok(Picked {
            build: latest_on(builds, from.branch.as_deref(), who)?,
            to: Follow {
                branch: from.branch.clone(),
                pinned: None,
            },
        }),
        Choice::DefaultBranch => Ok(Picked {
            build: latest_on(builds, None, who)?,
            to: Follow::default(),
        }),
        Choice::Branch(branch) => {
            let build =
                build::latest(builds, Some(branch)).ok_or_else(|| GalaxyError::NoBranch {
                    title: who.title.to_owned(),
                    branch: branch.clone(),
                    name: who.name.map(str::to_owned),
                })?;
            Ok(Picked {
                build,
                to: Follow {
                    branch: Some(branch.clone()),
                    pinned: None,
                },
            })
        }
        Choice::Build(spec) => {
            let build = named(builds, spec, from.branch.as_deref(), who)?;
            Ok(Picked {
                build,
                to: Follow {
                    branch: build.branch.clone(),
                    pinned: Some(build.build_id.clone()),
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(id: &str, version: &str, branch: Option<&str>, generation: u64) -> Build {
        Build {
            build_id: id.to_owned(),
            version_name: Some(version.to_owned()),
            branch: branch.map(str::to_owned),
            date_published: None,
            generation: Some(generation),
            link: Some(format!("link-{id}")),
        }
    }

    fn list() -> Builds {
        Builds {
            items: vec![
                build("3", "v3", None, 2),
                build("2", "same", None, 2),
                build("1", "same", None, 2),
                build("9", "v9", Some("beta"), 2),
                build("2", "same", Some("beta"), 2),
                build("0", "ancient", None, 1),
            ],
            has_private_branches: false,
        }
    }

    fn who() -> Who<'static> {
        Who {
            title: "Game",
            name: Some("game"),
        }
    }

    #[test]
    fn keeping_goes_to_the_latest_of_the_branch_followed() {
        let builds = list();
        let picked = pick(&builds, &Choice::Keep, &Follow::default(), false, &who()).unwrap();
        assert_eq!(picked.build.build_id, "3");
        let beta = Follow {
            branch: Some("beta".to_owned()),
            pinned: None,
        };
        let picked = pick(&builds, &Choice::Keep, &beta, false, &who()).unwrap();
        assert_eq!(picked.build.build_id, "9");
        assert_eq!(picked.to, beta);
    }

    #[test]
    fn a_branch_and_the_default_branch_unpin() {
        let builds = list();
        let pinned = Follow {
            branch: None,
            pinned: Some("1".to_owned()),
        };
        let picked = pick(
            &builds,
            &Choice::Branch("beta".to_owned()),
            &pinned,
            false,
            &who(),
        )
        .unwrap();
        assert_eq!(picked.build.build_id, "9");
        assert_eq!(picked.to.branch.as_deref(), Some("beta"));
        assert_eq!(picked.to.pinned, None);
        let picked = pick(&builds, &Choice::DefaultBranch, &pinned, false, &who()).unwrap();
        assert_eq!(picked.build.build_id, "3");
        assert!(picked.to.is_default());
        let picked = pick(&builds, &Choice::Latest, &pinned, false, &who()).unwrap();
        assert_eq!(picked.build.build_id, "3");
        assert!(picked.to.is_default());
    }

    #[test]
    fn an_unlisted_branch_is_refused_with_the_way_to_the_listing() {
        let error = pick(
            &list(),
            &Choice::Branch("nope".to_owned()),
            &Follow::default(),
            false,
            &who(),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.to_string(),
            "GOG lists no branch nope for Game. See: wineo update game --builds"
        );
    }

    #[test]
    fn a_build_is_named_by_id_or_by_a_version_only_one_build_has() {
        let builds = list();
        let by_id = pick(
            &builds,
            &Choice::Build("1".to_owned()),
            &Follow::default(),
            false,
            &who(),
        )
        .unwrap();
        assert_eq!(by_id.build.build_id, "1");
        assert_eq!(by_id.to.pinned.as_deref(), Some("1"));
        let by_name = pick(
            &builds,
            &Choice::Build("v3".to_owned()),
            &Follow::default(),
            false,
            &who(),
        )
        .unwrap();
        assert_eq!(by_name.build.build_id, "3");
        let ambiguous = pick(
            &builds,
            &Choice::Build("same".to_owned()),
            &Follow::default(),
            false,
            &who(),
        )
        .err()
        .unwrap();
        assert_eq!(
            ambiguous.to_string(),
            "same matches builds 2 and 1. Pick one with --build ID."
        );
        // Generation 1 builds are not listed.
        for unknown in ["0", "ancient", "42"] {
            let error = pick(
                &builds,
                &Choice::Build(unknown.to_owned()),
                &Follow::default(),
                false,
                &who(),
            )
            .err()
            .unwrap();
            assert_eq!(
                error.to_string(),
                format!(
                    "GOG doesn't list build {unknown} of Game. See: wineo update game --builds"
                )
            );
        }
    }

    #[test]
    fn a_build_on_two_branches_keeps_the_branch_followed_when_it_is_there() {
        let builds = list();
        let beta = Follow {
            branch: Some("beta".to_owned()),
            pinned: None,
        };
        let on_beta = pick(
            &builds,
            &Choice::Build("2".to_owned()),
            &beta,
            false,
            &who(),
        )
        .unwrap();
        assert_eq!(on_beta.to.branch.as_deref(), Some("beta"));
        let from_default = pick(
            &builds,
            &Choice::Build("2".to_owned()),
            &Follow::default(),
            false,
            &who(),
        )
        .unwrap();
        assert_eq!(from_default.to.branch, None);
        // Only on beta: the pin moves the game onto its branch.
        let moved = pick(
            &builds,
            &Choice::Build("9".to_owned()),
            &Follow::default(),
            false,
            &who(),
        )
        .unwrap();
        assert_eq!(moved.to.branch.as_deref(), Some("beta"));
    }

    #[test]
    fn a_reinstall_of_a_pinned_game_rebuilds_the_pin() {
        let builds = list();
        let pinned = Follow {
            branch: None,
            pinned: Some("1".to_owned()),
        };
        let picked = pick(&builds, &Choice::Keep, &pinned, true, &who()).unwrap();
        assert_eq!(picked.build.build_id, "1");
        assert_eq!(picked.to, pinned);
        let gone = Follow {
            branch: None,
            pinned: Some("77".to_owned()),
        };
        let error = pick(&builds, &Choice::Keep, &gone, true, &who())
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "GOG no longer lists build 77 of Game, which game is pinned to. Run wineo update game --latest to follow its branch again."
        );
    }

    #[test]
    fn a_branch_without_a_generation_2_build_has_its_own_note() {
        let builds = Builds {
            items: vec![
                build("0", "old", Some("beta"), 1),
                build("3", "v3", None, 2),
            ],
            has_private_branches: false,
        };
        let beta = Follow {
            branch: Some("beta".to_owned()),
            pinned: None,
        };
        let error = pick(&builds, &Choice::Keep, &beta, false, &who())
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "GOG has no Galaxy build of this game on branch beta that wineo can update. Return to the default branch with: wineo update game --default-branch"
        );
    }
}
