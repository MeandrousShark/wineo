//! The builds GOG offers for a Galaxy game, read only: what `--builds`
//! lists, and what a version picker offers.

use anyhow::{Result, anyhow};

use super::install::{self, GalaxyError};
use super::record::Follow;
use crate::environment::Environment;
use crate::gog::account::Session;
use crate::gog::folder::{Platform, Track};
use crate::gog::migrate::host_only;
use crate::gog::update::UpdateError;
use crate::gog::{names, product};
use crate::paths::Paths;

/// One generation 2 build GOG lists, and how it stands against the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// None for the default branch.
    pub branch: Option<String>,
    pub build: String,
    pub version: Option<String>,
    /// GOG's timestamp for it, as given.
    pub published: Option<String>,
    /// The game has this build installed.
    pub installed: bool,
    /// The first build GOG lists on its branch, which an update goes to.
    pub latest: bool,
    /// The game is pinned to it.
    pub pinned: bool,
    /// It is on a branch other than the default one that the game follows.
    pub followed: bool,
}

impl Listed {
    /// The day it was published, like `2026-06-16`.
    pub fn published_date(&self) -> Option<&str> {
        let date = self.published.as_deref()?.get(..10)?;
        let digits = |range: std::ops::Range<usize>| {
            date.get(range)
                .is_some_and(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
        };
        (digits(0..4)
            && date.get(4..5) == Some("-")
            && digits(5..7)
            && date.get(7..8) == Some("-")
            && digits(8..10))
        .then_some(date)
    }

    /// The marks that apply, in the order a listing shows them.
    pub fn marks(&self) -> Vec<&'static str> {
        [
            (self.installed, "installed"),
            (self.latest, "latest"),
            (self.pinned, "pinned"),
            (self.followed, "followed"),
        ]
        .into_iter()
        .filter_map(|(on, mark)| on.then_some(mark))
        .collect()
    }
}

/// The builds GOG lists for one game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub environment: String,
    pub gog_id: u64,
    pub title: String,
    /// What the game follows now.
    pub follow: Follow,
    /// The base game's installed build.
    pub installed: Option<String>,
    /// Whether GOG says the game has branches only a password lists.
    pub private_branches: bool,
    /// The default branch first, then each other branch in the order GOG
    /// lists it; newest first within each branch.
    pub builds: Vec<Listed>,
    pub warnings: Vec<String>,
}

/// Lists the generation 2 builds GOG offers for `name`'s Galaxy game.
/// Reads only, apart from GOG's product details cache (unless `read_only`)
/// and a renewed login. `password` is the seam for a private branch's
/// builds, which nothing sends yet.
///
/// An installer-tracked game has no builds to list: `UpdateError::NotGalaxy`.
pub fn list(
    paths: &Paths,
    session: &mut Session,
    name: &str,
    password: Option<&str>,
    refresh: bool,
    read_only: bool,
) -> Result<Listing> {
    host_only(paths)?;
    Environment::ensure_exists(paths, name)?;
    let mut state = crate::gog::update::game_of(paths, name)?;
    let committed = state
        .committed
        .clone()
        .ok_or_else(|| anyhow!("{name}'s GOG install hasn't started."))?;
    if committed.track != Track::Galaxy {
        return Err(UpdateError::NotGalaxy {
            name: name.to_owned(),
        }
        .into());
    }
    if committed.platform != Platform::Windows {
        return Err(GalaxyError::NotWindows.into());
    }
    // A build another tool installed counts, without being recorded.
    if state.journal.is_none()
        && state.owner.is_some()
        && let Some(seen) = crate::gog::update::seen_build(&state)
    {
        state.committed = Some(seen.committed);
    }
    let gog_id = committed.gog_id;
    let mut warnings = Vec::new();
    let title = match product::load_with(paths, session.client(), gog_id, refresh, !read_only) {
        Ok(details) => {
            warnings.extend(details.warning);
            Some(details.product.title).filter(|title| !title.is_empty())
        }
        Err(_) => None,
    }
    .or_else(|| names::product_names(&state.folder).get(&gog_id).cloned())
    .unwrap_or_else(|| format!("GOG game {gog_id}"));
    let builds = match install::builds_of(session, gog_id, password) {
        Ok(builds) => builds,
        // No content system for the game: nothing to list.
        Err(error)
            if error
                .downcast_ref::<wineo_gog::Error>()
                .is_some_and(|error| error.kind() == wineo_gog::ErrorKind::Status(404)) =>
        {
            wineo_gog::Builds {
                items: Vec::new(),
                has_private_branches: false,
            }
        }
        Err(error) => return Err(error),
    };
    let follow = super::follow_of(paths, &state.folder);
    let installed = state
        .committed
        .as_ref()
        .and_then(crate::gog::update::base_build)
        .map(str::to_owned);

    let mut branches: Vec<Option<&str>> = vec![None];
    for build in &builds.items {
        if build.generation == Some(2) && !branches.contains(&build.branch.as_deref()) {
            branches.push(build.branch.as_deref());
        }
    }
    let mut listed = Vec::new();
    for branch in branches {
        let mut first = true;
        for build in builds
            .items
            .iter()
            .filter(|build| build.generation == Some(2) && build.branch.as_deref() == branch)
        {
            listed.push(Listed {
                branch: build.branch.clone(),
                build: build.build_id.clone(),
                version: build.version_name.clone().filter(|name| !name.is_empty()),
                published: build.date_published.clone(),
                installed: installed.as_deref() == Some(build.build_id.as_str()),
                latest: std::mem::take(&mut first),
                pinned: follow.pinned.as_deref() == Some(build.build_id.as_str()),
                followed: branch.is_some() && follow.branch.as_deref() == branch,
            });
        }
    }
    Ok(Listing {
        environment: name.to_owned(),
        gog_id,
        title,
        follow,
        installed,
        private_branches: builds.has_private_branches,
        builds: listed,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(published: Option<&str>) -> Listed {
        Listed {
            branch: None,
            build: "1".to_owned(),
            version: None,
            published: published.map(str::to_owned),
            installed: true,
            latest: true,
            pinned: false,
            followed: false,
        }
    }

    #[test]
    fn the_published_day_is_the_date_part_of_gogs_timestamp() {
        assert_eq!(
            listed(Some("2026-06-16T10:00:00+00:00")).published_date(),
            Some("2026-06-16")
        );
        assert_eq!(
            listed(Some("2026-01-02T03:04:05+0000")).published_date(),
            Some("2026-01-02")
        );
        assert_eq!(listed(Some("sometime")).published_date(), None);
        assert_eq!(listed(None).published_date(), None);
    }

    #[test]
    fn marks_come_in_a_fixed_order() {
        let mut build = listed(None);
        assert_eq!(build.marks(), ["installed", "latest"]);
        build.pinned = true;
        build.followed = true;
        assert_eq!(build.marks(), ["installed", "latest", "pinned", "followed"]);
    }
}
