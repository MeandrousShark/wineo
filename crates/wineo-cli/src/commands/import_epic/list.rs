use std::process::ExitCode;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use wineo_core::config::GlobalConfig;
use wineo_core::epic::account::Status;
use wineo_core::epic::install::{self, Local, LocalState};
use wineo_core::epic::legendary::Missing;
use wineo_core::epic::owned::{self, Game, Owned};
use wineo_core::epic::{self, Error};
use wineo_core::paths::Paths;

use super::Args;
use super::setup::{self, Found};
use crate::commands::gog_support::warn;
use crate::commands::table::render_columns;
use crate::commands::transfer::Cancellation;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    NotInstalled,
    Unfinished,
    Downloaded,
    Installed,
    Unavailable,
}

impl State {
    fn json(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Unfinished => "unfinished",
            Self::Downloaded => "downloaded",
            Self::Installed => "installed",
            Self::Unavailable => "unavailable",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "not installed",
            other => other.json(),
        }
    }
}

/// Why a game can't be installed through wineo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// Ubisoft Connect or the EA app manages it.
    ThirdParty,
    /// Epic has no Windows build of it.
    NoWindowsBuild,
    /// It needs an account at another launcher.
    NeedsLinking,
}

impl Reason {
    fn json(self) -> &'static str {
        match self {
            Self::ThirdParty => "third_party",
            Self::NoWindowsBuild => "no_windows_build",
            Self::NeedsLinking => "needs_linking",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ThirdParty => "unavailable: another launcher manages it",
            Self::NoWindowsBuild => "unavailable: no Windows build",
            Self::NeedsLinking => "unavailable: needs an account at another launcher",
        }
    }
}

/// A game's state: what wineo has of it, else what the owned list says.
fn state(game: &Game, local: Option<&Local>) -> (State, Option<Reason>) {
    if let Some(local) = local {
        let state = match local.state {
            LocalState::Unfinished => State::Unfinished,
            LocalState::Downloaded => State::Downloaded,
            LocalState::Installed => State::Installed,
        };
        return (state, None);
    }
    let reason = if game.third_party.is_some() {
        Some(Reason::ThirdParty)
    } else if game.version.is_none() {
        Some(Reason::NoWindowsBuild)
    } else if game.partner_link.is_some() {
        Some(Reason::NeedsLinking)
    } else {
        None
    };
    match reason {
        Some(reason) => (State::Unavailable, Some(reason)),
        None => (State::NotInstalled, None),
    }
}

pub(super) fn run(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let mut warnings = Vec::new();
    let owned = if args.dry_run {
        match dry_run(paths, &global, args, &mut warnings)? {
            Some(owned) => owned,
            None => return Ok(ExitCode::SUCCESS),
        }
    } else if args.refresh {
        let legendary = setup::obtain(paths, &global, args)?.expect("only a dry run stops");
        load(&legendary, true)?
    } else {
        // Listing what is saved never asks to download Legendary.
        match setup::find(paths, &global)? {
            Found::Usable(legendary) => load(&legendary, false)?,
            Found::Unusable { missing } => {
                let Some(owned) = owned::cached(paths) else {
                    return Err(not_listed(missing.as_ref()).into());
                };
                warnings.push(saved_list_warning(missing.as_ref()));
                owned
            }
        }
    };
    warnings.extend(owned.warning.iter().cloned());

    let mut games: Vec<&Game> = owned.games.iter().collect();
    games.sort_by_cached_key(|game| (game.title.to_lowercase(), game.app_name.clone()));
    // Epic games live in the host library, so a portable root has none.
    let (local, errors) = install::local(paths)?;

    // On stderr in both modes, and in the JSON for scripts.
    for warning in &warnings {
        warn(warning);
    }
    if args.json {
        crate::commands::json::print(&json!({
            "games": games
                .iter()
                .map(|game| game_json(game, local.get(&game.app_name)))
                .collect::<Vec<_>>(),
            // A record that can't be read names no game or environment.
            "errors": errors
                .iter()
                .map(|message| json!({"name": Value::Null, "id": Value::Null, "message": message}))
                .collect::<Vec<_>>(),
            "warnings": warnings,
        }))?;
    } else {
        print_table(&games, &local);
        for error in &errors {
            eprintln!("Failed: {error}");
        }
    }
    Ok(if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// What a listing would show, from the saved list alone: a dry run never
/// asks Epic or rewrites the saved list. Legendary is still looked up and
/// asked offline who is logged in, so a dry run fails where the real one
/// would. None when there is no saved list, after saying so.
fn dry_run(
    paths: &Paths,
    global: &GlobalConfig,
    args: &Args,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<Owned>> {
    let found = setup::find(paths, global)?;
    let account = match &found {
        Found::Usable(legendary) => {
            let cancellation = Cancellation::new()?;
            let status = epic::account::status(legendary, &cancellation.token);
            match cancellation.result(status.map_err(anyhow::Error::from))? {
                Status::LoggedIn { account } => Some(account),
                Status::LoggedOut => return Err(Error::LoggedOut.into()),
            }
        }
        Found::Unusable { .. } => None,
    };
    // A real refresh without a usable Legendary goes to get one, and fails
    // there without Python or on another machine.
    if args.refresh {
        match &found {
            Found::Unusable { missing: None } => return Err(Error::Python.into()),
            Found::Unusable { missing: Some(_) } => setup::check_can_download()?,
            Found::Usable(_) => {}
        }
    }
    // A list saved for another account is none, as for a real run.
    let saved = owned::cached(paths).filter(|owned| {
        account
            .as_ref()
            .is_none_or(|account| owned.account == *account)
    });
    let Some(owned) = saved else {
        match found {
            Found::Usable(_) => eprintln!("Dry run, so the owned Epic games weren't fetched."),
            Found::Unusable {
                missing: Some(missing),
            } if args.refresh => setup::say_dry_run(&missing),
            Found::Unusable { missing } => return Err(not_listed(missing.as_ref()).into()),
        }
        return Ok(None);
    };
    // Said only where a real run would have fetched the list again.
    let not_refreshed = "Dry run, so the list wasn't refreshed.".to_owned();
    match found {
        Found::Usable(_) if args.refresh || !owned::cache_is_fresh(paths) => {
            warnings.push(not_refreshed);
        }
        Found::Usable(_) => {}
        Found::Unusable { .. } if args.refresh => warnings.push(not_refreshed),
        Found::Unusable { missing } => warnings.push(saved_list_warning(missing.as_ref())),
    }
    Ok(Some(owned))
}

/// Why there is no list to show when no Legendary can be run.
fn not_listed(missing: Option<&Missing>) -> Error {
    match missing {
        Some(_) => Error::LoggedOut,
        None => Error::Python,
    }
}

/// The warning over a saved list shown because no Legendary can be run.
fn saved_list_warning(missing: Option<&Missing>) -> String {
    match missing {
        Some(_) => "Legendary isn't installed, so this is the saved list.".to_owned(),
        None => format!("{} This is the saved list.", Error::Python),
    }
}

fn load(legendary: &epic::legendary::Legendary, refresh: bool) -> anyhow::Result<Owned> {
    let cancellation = Cancellation::new()?;
    let owned = owned::load(legendary, refresh, &cancellation.token);
    cancellation.result(owned.map_err(anyhow::Error::from))
}

fn game_json(game: &Game, local: Option<&Local>) -> Value {
    let (state, reason) = state(game, local);
    // Only installed DLC is listed, as for GOG.
    let dlc: Vec<Value> = local
        .map(|local| {
            local
                .dlc
                .iter()
                .map(|id| {
                    let title = game
                        .dlc
                        .iter()
                        .find(|dlc| dlc.app_name == *id)
                        .map(|dlc| dlc.title.clone());
                    json!({"id": id, "title": title, "state": "installed"})
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": game.app_name,
        "title": game.title,
        "state": state.json(),
        "environment": local.and_then(|local| local.environment.as_deref()),
        "version": local.and_then(|local| local.version.as_deref()),
        // Filled once updates are checked.
        "update": Value::Null,
        "reason": reason.map(Reason::json),
        "dlc": dlc,
    })
}

fn print_table(games: &[&Game], local: &BTreeMap<String, Local>) {
    if games.is_empty() {
        println!("You don't own any Epic games.");
        return;
    }
    // The environment and version columns only once wineo has a game.
    let wide = games.iter().any(|game| local.contains_key(&game.app_name));
    let mut table = vec![["ID", "TITLE", "STATE", "ENVIRONMENT", "VERSION"].map(str::to_owned)];
    for game in games {
        let found = local.get(&game.app_name);
        let (state, reason) = state(game, found);
        table.push([
            game.app_name.clone(),
            game.title.clone(),
            match reason {
                Some(reason) => reason.label().to_owned(),
                None => state.label().to_owned(),
            },
            found
                .and_then(|local| local.environment.clone())
                .unwrap_or_default(),
            found
                .and_then(|local| local.version.clone())
                .unwrap_or_default(),
        ]);
    }
    let lines = if wide {
        render_columns(&table)
    } else {
        let narrow: Vec<[String; 3]> = table
            .into_iter()
            .map(|[id, title, state, _, _]| [id, title, state])
            .collect();
        render_columns(&narrow)
    };
    for line in lines {
        println!("{line}");
    }
}
