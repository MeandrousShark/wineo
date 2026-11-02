//! One read of what a library refresh needs, so a frontend that rebuilds its
//! library every few seconds reads each config once per refresh.
//!
//! Run state, locks, and processes stay out on purpose: they're read per
//! environment as an overview goes, because the overview's own lost-run
//! cleanup changes them.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::environment::{self, Environment};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::steam;
use crate::trash::PendingScan;

/// One view of the disk for one refresh. Like `PendingScan`, keep it only for
/// a batch of reads, never across refreshes.
pub struct RefreshSnapshot<'a> {
    paths: &'a Paths,
    pending: PendingScan,
    environments: BTreeMap<String, LoadedEnvironment>,
    steam: SteamFiles<'a>,
}

struct LoadedEnvironment {
    loaded: Result<Environment>,
    /// False when a check refused the load before it opened the config.
    config_read: bool,
    /// The config of a refused load, read if someone asks for it.
    refused_config: OnceCell<Option<EnvironmentConfig>>,
}

impl<'a> RefreshSnapshot<'a> {
    pub fn read(paths: &'a Paths) -> Result<Self> {
        Self::read_with(paths, None)
    }

    /// `read`, with Steam's `appinfo.vdf` going through `appinfo` when given,
    /// so a caller that refreshes often parses it only when it changes.
    pub fn read_with(paths: &'a Paths, appinfo: Option<&'a steam::AppInfoCache>) -> Result<Self> {
        // Nothing a refresh does trashes or restores, so one scan serves it.
        let pending = PendingScan::new(paths);
        let environments = environment::names(paths)?
            .into_iter()
            .map(|name| {
                let (loaded, config_read) = Environment::load_noting_read(paths, &name, &pending);
                let environment = LoadedEnvironment {
                    loaded,
                    config_read,
                    refused_config: OnceCell::new(),
                };
                (name, environment)
            })
            .collect();
        Ok(Self {
            paths,
            pending,
            environments,
            steam: SteamFiles::new(paths).with_appinfo(appinfo),
        })
    }

    pub fn paths(&self) -> &'a Paths {
        self.paths
    }

    pub fn pending(&self) -> &PendingScan {
        &self.pending
    }

    /// `name`'s load, or None when it wasn't an environment at this read.
    pub fn environment(&self, name: &str) -> Option<Result<&Environment, &Error>> {
        self.environments
            .get(name)
            .map(|environment| environment.loaded.as_ref())
    }

    /// Every environment by name, in the order `environment::names` gives.
    pub(crate) fn environments(
        &self,
    ) -> impl Iterator<Item = (&str, Result<&Environment, &Error>)> {
        self.environments
            .iter()
            .map(|(name, environment)| (name.as_str(), environment.loaded.as_ref()))
    }

    /// What stats goes by: the config as far as it parses, even when the
    /// load was refused for a reason that has nothing to do with the file.
    pub(crate) fn lenient_config(&self, name: &str) -> Option<&EnvironmentConfig> {
        let environment = self.environments.get(name)?;
        match &environment.loaded {
            Ok(loaded) => Some(&loaded.config),
            // The load read the file and failed, and so would a second read.
            Err(_) if environment.config_read => None,
            Err(_) => environment
                .refused_config
                .get_or_init(|| crate::stats::lenient_config(self.paths, name))
                .as_ref(),
        }
    }

    pub(crate) fn steam(&self) -> &SteamFiles<'a> {
        &self.steam
    }

    /// What Steam's `appinfo.vdf` says, read on the first ask and then kept
    /// for this refresh. None when Steam isn't found, and an error when the
    /// file can't be read as a whole.
    pub fn appinfo(
        &self,
    ) -> Option<&std::result::Result<steam::appinfo::AppInfo, steam::appinfo::ReadError>> {
        self.steam.appinfo()
    }
}

/// Steam's files for one batch of reads. Each is read at most once, on the
/// first thing that asks, so a library with no Steam game reads none of them.
pub(crate) struct SteamFiles<'a> {
    paths: &'a Paths,
    /// Read into `read_global` when no caller gave it.
    given_global: Option<&'a GlobalConfig>,
    read_global: OnceCell<Result<GlobalConfig>>,
    install: OnceCell<Option<steam::Install>>,
    /// The reason nothing can be read, or the parsed playtime file.
    playtimes: OnceCell<std::result::Result<steam::Playtimes, String>>,
    /// Where `appinfo` is read through, when the caller keeps one.
    appinfo_cache: Option<&'a steam::AppInfoCache>,
    appinfo: OnceCell<Option<AppInfoRead>>,
}

type AppInfoRead = Arc<std::result::Result<steam::appinfo::AppInfo, steam::appinfo::ReadError>>;

impl<'a> SteamFiles<'a> {
    pub(crate) fn new(paths: &'a Paths) -> Self {
        Self::with(paths, None)
    }

    /// For a caller that already has the global config.
    pub(crate) fn with_global(paths: &'a Paths, global: &'a GlobalConfig) -> Self {
        Self::with(paths, Some(global))
    }

    fn with(paths: &'a Paths, given_global: Option<&'a GlobalConfig>) -> Self {
        Self {
            paths,
            given_global,
            read_global: OnceCell::new(),
            install: OnceCell::new(),
            playtimes: OnceCell::new(),
            appinfo_cache: None,
            appinfo: OnceCell::new(),
        }
    }

    fn with_appinfo(mut self, cache: Option<&'a steam::AppInfoCache>) -> Self {
        self.appinfo_cache = cache;
        self
    }

    /// Overviews give a bad global config two meanings, so its error is kept
    /// for each to read its own: no install, or an unknown playtime with why.
    pub(crate) fn global(&self) -> Result<&GlobalConfig, &Error> {
        match self.given_global {
            Some(global) => Ok(global),
            None => self
                .read_global
                .get_or_init(|| GlobalConfig::load(self.paths))
                .as_ref(),
        }
    }

    /// None when the global config can't be read or Steam isn't installed.
    pub(crate) fn install(&self) -> Option<&steam::Install> {
        self.install
            .get_or_init(|| {
                self.global()
                    .ok()
                    .and_then(|global| steam::active_for(self.paths, global))
            })
            .as_ref()
    }

    /// None when Steam isn't installed.
    pub(crate) fn appinfo(
        &self,
    ) -> Option<&std::result::Result<steam::appinfo::AppInfo, steam::appinfo::ReadError>> {
        self.appinfo
            .get_or_init(|| {
                let install = self.install()?;
                Some(match self.appinfo_cache {
                    Some(cache) => cache.read(install),
                    None => Arc::new(steam::appinfo::read(install)),
                })
            })
            .as_deref()
    }

    pub(crate) fn playtime(&self, app_id: &str) -> steam::Playtime {
        let playtimes = self.playtimes.get_or_init(|| match self.global() {
            Ok(_) => self
                .install()
                .map(steam::Playtimes::load)
                .ok_or_else(|| "Steam was not found".to_owned()),
            Err(error) => Err(format!("global config can't be read: {error}")),
        });
        match playtimes {
            Ok(playtimes) => playtimes.get(app_id),
            Err(reason) => steam::Playtime::Unknown(reason.clone()),
        }
    }
}

#[cfg(test)]
mod tests;
