//! Installing and uninstalling Steam games by handing the request to Steam's
//! own dialogs, and following what the manifest says while Steam works.
//! Wineo never moves game files itself.

use std::io;
use std::time::Duration;

use anyhow::Context;

use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::launch::{Launch, SteamRequest};
use crate::operation::Locks;
use crate::paths::Paths;
use crate::runtime::{Runtime, State};
use crate::steam::{self, AppState, Install};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

impl Action {
    fn request(self) -> SteamRequest {
        match self {
            Action::Install => SteamRequest::Install,
            Action::Uninstall => SteamRequest::Uninstall,
        }
    }

    fn done(self) -> &'static str {
        match self {
            Action::Install => "installed",
            Action::Uninstall => "uninstalled",
        }
    }
}

/// What asking Steam came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Steam accepted the request and opened its dialog. Nothing has been
    /// installed or removed yet.
    Opened,
    AlreadyInstalled,
    /// Carries the environment's name.
    AlreadyInstalling(String),
    NotInstalled,
}

impl Outcome {
    /// The line to show when there was nothing to ask Steam.
    pub fn note(&self) -> Option<String> {
        match self {
            Outcome::Opened => None,
            Outcome::AlreadyInstalled => Some("Already installed.".to_owned()),
            Outcome::AlreadyInstalling(name) => {
                Some(format!("Steam is already installing {name}."))
            }
            Outcome::NotInstalled => Some("Not installed.".to_owned()),
        }
    }
}

pub fn install(paths: &Paths, global: &GlobalConfig, name: &str) -> anyhow::Result<Outcome> {
    request(paths, global, name, Action::Install)
}

pub fn uninstall(paths: &Paths, global: &GlobalConfig, name: &str) -> anyhow::Result<Outcome> {
    request(paths, global, name, Action::Uninstall)
}

/// Opens Steam's install or uninstall dialog for a Steam game, detached, and
/// returns once Steam accepts the request.
pub fn request(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    action: Action,
) -> anyhow::Result<Outcome> {
    request_with(
        paths,
        global,
        name,
        action,
        &mut |launch| crate::supervisor::request_steam(launch),
        &|install, app_id| Ok(!steam::matching_reapers(install, app_id)?.is_empty()),
    )
}

/// `request` with the handoff and the scan for a running game passed in, so
/// a test can record the request instead of running Steam.
pub(crate) fn request_with(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    action: Action,
    handoff: &mut dyn FnMut(&Launch) -> crate::Result<()>,
    running_in_steam: &dyn Fn(&Install, &str) -> io::Result<bool>,
) -> anyhow::Result<Outcome> {
    let environment = Environment::load(paths, name)?;
    let Some(proxied) = environment.config.proxied() else {
        anyhow::bail!("Only Steam games can be {}.", action.done());
    };
    let app_id = proxied.app_id.clone();
    // Held through the handoff, so the check and the request are one step
    // against other installs, uninstalls, and imports of this app.
    let _guard = Locks::new(paths).steam_app(&app_id)?;
    let install = steam::active_for(paths, global);
    let state = steam::app_state(install.as_ref(), &app_id);
    match (&state, action) {
        (AppState::Unavailable(path), _) => {
            anyhow::bail!("Steam's library at {} isn't available.", path.display())
        }
        (AppState::Unknown(reason), _) => anyhow::bail!("{reason}."),
        (AppState::Installed | AppState::Updating, Action::Install) => {
            return Ok(Outcome::AlreadyInstalled);
        }
        (AppState::Installing(_), Action::Install) => {
            return Ok(Outcome::AlreadyInstalling(name.to_owned()));
        }
        (AppState::Uninstalling, Action::Install) => {
            anyhow::bail!("Steam is uninstalling {name}.")
        }
        (AppState::NotInstalled, Action::Uninstall) => return Ok(Outcome::NotInstalled),
        // A queued install still opens the dialog, and so does a partial one
        // for uninstalling, since Steam's dialog cancels it.
        (AppState::NotInstalled | AppState::InstallQueued, Action::Install)
        | (
            AppState::Installed
            | AppState::Updating
            | AppState::Installing(_)
            | AppState::InstallQueued
            | AppState::Uninstalling,
            Action::Uninstall,
        ) => {}
    }
    let install = install.expect("a state other than unknown has an install");
    if action == Action::Uninstall {
        let status = Runtime::new(paths, name).status()?;
        let running = matches!(status.state, State::Starting | State::Running)
            || running_in_steam(&install, &app_id)
                .with_context(|| format!("failed to scan Steam processes for {name}"))?;
        if running {
            anyhow::bail!("Stop {name} before uninstalling it.");
        }
    }
    handoff(&Launch::steam_request(
        paths,
        install,
        &app_id,
        action.request(),
    ))?;
    Ok(Outcome::Opened)
}

/// One look at the manifest while following.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// Installing, and Steam hasn't written a manifest. Wineo can't tell an
    /// open dialog from a canceled one.
    Waiting,
    Queued,
    Installing {
        percent: Option<u8>,
        /// Bytes done and in all for the phase `percent` comes from, so the
        /// pair always agrees with it. `total` is 0 until Steam writes totals.
        done: u64,
        total: u64,
    },
    /// Uninstalling, and the game isn't gone yet.
    Uninstalling,
    /// The manifest couldn't be read this time. Steam rewrites it as it
    /// works, so following goes on.
    Unreadable(String),
    /// Installed, or for an uninstall, gone.
    Finished,
    /// The install's manifest disappeared, or the uninstall's came back fully
    /// installed.
    SteamCanceled,
}

impl Update {
    pub fn is_final(&self) -> bool {
        matches!(self, Update::Finished | Update::SteamCanceled)
    }
}

/// How a followed install or uninstall ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Finished,
    SteamCanceled,
    /// The token asked to stop following. Steam carries on.
    Stopped,
}

/// Once a second. Debug builds let tests shorten it with
/// `WINEO_TEST_STEAM_FOLLOW_INTERVAL_MS`.
fn poll_interval() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(millis) = std::env::var("WINEO_TEST_STEAM_FOLLOW_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        return Duration::from_millis(millis);
    }
    Duration::from_secs(1)
}

/// Follows one game's install or uninstall through its manifest. It takes no
/// lock, since Steam is the only thing changing the manifest.
pub struct Follow {
    action: Action,
    install: Option<Install>,
    app_id: String,
    title: String,
    /// An install has shown up in the manifest, or an uninstall has moved it
    /// off fully installed. Either makes a later reversal Steam's doing.
    moved: bool,
    /// How long `wait` sleeps between looks.
    pub interval: Duration,
}

impl Follow {
    pub fn new(
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        action: Action,
    ) -> anyhow::Result<Self> {
        let environment = Environment::load(paths, name)?;
        let Some(proxied) = environment.config.proxied() else {
            anyhow::bail!("Only Steam games can be {}.", action.done());
        };
        Ok(Self {
            action,
            install: steam::active_for(paths, global),
            app_id: proxied.app_id.clone(),
            title: proxied.title.clone().unwrap_or_else(|| name.to_owned()),
            moved: false,
            interval: poll_interval(),
        })
    }

    /// Starts an install follow as one already seen working, for when the
    /// request found Steam installing the game. Without it a manifest that
    /// vanishes before the first look reads as a dialog not yet answered, and
    /// the wait never ends.
    #[must_use]
    pub fn already_installing(mut self) -> Self {
        self.moved = true;
        self
    }

    /// The game's title: Steam's own once its manifest has been read, then the
    /// config's, then the environment's name.
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn poll(&mut self) -> Update {
        let (state, manifest) = steam::app_details(self.install.as_ref(), &self.app_id);
        if let Some(manifest) = &manifest
            && !manifest.name.is_empty()
        {
            self.title.clone_from(&manifest.name);
        }
        let installing = |manifest: Option<&steam::Manifest>| {
            let bytes = manifest.and_then(steam::Manifest::install_bytes);
            Update::Installing {
                percent: bytes.map(|(done, total)| steam::percent(done, total)),
                done: bytes.map_or(0, |(done, _)| done),
                total: bytes.map_or(0, |(_, total)| total),
            }
        };
        match (self.action, state) {
            (_, AppState::Unavailable(path)) => Update::Unreadable(format!(
                "Steam's library at {} isn't available",
                path.display()
            )),
            (_, AppState::Unknown(reason)) => Update::Unreadable(reason),
            (Action::Install, AppState::NotInstalled) if self.moved => Update::SteamCanceled,
            (Action::Install, AppState::NotInstalled) => Update::Waiting,
            (Action::Install, AppState::InstallQueued) => {
                self.moved = true;
                Update::Queued
            }
            (Action::Install, AppState::Installing(_)) => {
                self.moved = true;
                installing(manifest.as_ref())
            }
            // An update bit on an install that was seen working is the install
            // still finishing up.
            (Action::Install, AppState::Updating) if self.moved => installing(manifest.as_ref()),
            (Action::Install, AppState::Installed | AppState::Updating) => Update::Finished,
            (Action::Install, AppState::Uninstalling) => Update::SteamCanceled,
            (Action::Uninstall, AppState::NotInstalled) => Update::Finished,
            (Action::Uninstall, AppState::Installed | AppState::Updating) if self.moved => {
                Update::SteamCanceled
            }
            (Action::Uninstall, AppState::Installed | AppState::Updating) => Update::Uninstalling,
            (
                Action::Uninstall,
                AppState::Uninstalling | AppState::Installing(_) | AppState::InstallQueued,
            ) => {
                self.moved = true;
                Update::Uninstalling
            }
        }
    }

    /// Polls every `interval` until the install or uninstall ends or `cancel`
    /// asks to stop. `report` sees every look, including the last, with the
    /// title as that look left it.
    pub fn wait(&mut self, cancel: &Token, mut report: impl FnMut(&Update, &str)) -> End {
        loop {
            if cancel.is_canceled() {
                return End::Stopped;
            }
            let update = self.poll();
            report(&update, &self.title);
            match update {
                Update::Finished => return End::Finished,
                Update::SteamCanceled => return End::SteamCanceled,
                _ => {}
            }
            // Short sleeps keep Ctrl+C quick.
            let mut slept = Duration::ZERO;
            while slept < self.interval {
                if cancel.is_canceled() {
                    return End::Stopped;
                }
                let step = Duration::from_millis(50).min(self.interval - slept);
                std::thread::sleep(step);
                slept += step;
            }
        }
    }
}

#[cfg(test)]
mod tests;
