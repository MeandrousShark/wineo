use std::collections::BTreeSet;
use std::process::ExitCode;

use serde_json::{Value, json};
use wineo_core::gog::api::OwnedProduct;
use wineo_core::gog::folder;
use wineo_core::gog::owned;
use wineo_core::gog::update::{self, Batch, Status};
use wineo_core::heroic_import;
use wineo_core::paths::Paths;

use super::Args;
use crate::commands::gog_support::{
    Account, dlc_json, error_json, error_subject, reason_json, update_label, warn,
};
use crate::commands::table::render_columns;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    NotInstalled,
    Heroic,
    Imported,
    /// Installed by wineo rather than migrated.
    Installed,
}

impl State {
    fn json(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Heroic => "heroic",
            Self::Imported => "imported",
            Self::Installed => "installed",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "not installed",
            Self::Heroic => "in Heroic",
            Self::Imported => "imported",
            Self::Installed => "installed",
        }
    }
}

struct Row<'a> {
    game: &'a OwnedProduct,
    state: State,
    /// The environment that records the game, whether or not it could be
    /// checked.
    environment: Option<String>,
    status: Option<&'a Status>,
}

pub(super) fn run(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    let account = Account::new();
    let mut session = account.session(paths)?;
    let owned = owned::load(&mut session, args.refresh)?;
    let mut warnings: Vec<String> = owned.warning.iter().cloned().collect();
    // GOG games live in the host library, so a portable folder has none to
    // check.
    let batch = if paths.portable_root().is_none() {
        update::check_all(&mut session, args.refresh)?
    } else {
        Batch::default()
    };
    for status in &batch.statuses {
        warnings.extend(status.warnings.iter().cloned());
    }
    let in_heroic = heroic_ids(paths, &mut warnings);
    // Migrated is decided by what the environments record, so a game whose
    // update check failed is still listed as imported.
    let recorded = update::recorded_games(paths);

    let mut games: Vec<&OwnedProduct> = owned.games.iter().collect();
    games.sort_by_cached_key(|game| (game.title.to_lowercase(), game.id));
    let rows: Vec<Row> = games
        .into_iter()
        .map(|game| {
            let status = batch
                .statuses
                .iter()
                .find(|status| status.gog_id == game.id);
            let environment = status
                .map(|status| status.environment.clone())
                .or_else(|| recorded.get(&game.id).cloned());
            let state = if let Some(name) = &environment {
                if installed_by_wineo(paths, name, game.id) {
                    State::Installed
                } else {
                    State::Imported
                }
            } else if in_heroic.contains(&game.id.to_string()) {
                State::Heroic
            } else {
                State::NotInstalled
            };
            Row {
                game,
                state,
                environment,
                status,
            }
        })
        .collect();

    // On stderr in both modes, and in the JSON for scripts.
    for warning in &warnings {
        warn(warning);
    }
    if args.json {
        crate::commands::json::print(&json!({
            "games": rows.iter().map(game_json).collect::<Vec<_>>(),
            "errors": batch.errors.iter().map(error_json).collect::<Vec<_>>(),
            "warnings": warnings,
        }))?;
    } else {
        print_table(&rows);
        for error in &batch.errors {
            eprintln!("Failed {}: {}", error_subject(error), error.message);
        }
    }
    Ok(if batch.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Whether `name`'s folder for game `id` records that wineo installed it.
/// A state that can't be read doesn't say, so the game reads as imported.
fn installed_by_wineo(paths: &Paths, name: &str, id: u64) -> bool {
    folder::find(paths, name, &id.to_string())
        .ok()
        .flatten()
        .is_some_and(|state| state.installed_by_wineo())
}

/// The ids of the GOG games Heroic installed. A Heroic that can't be read
/// only means none are marked.
fn heroic_ids(paths: &Paths, warnings: &mut Vec<String>) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let sources = match heroic_import::sources(paths, None) {
        Ok(sources) => sources,
        Err(error) => {
            warnings.push(format!("Couldn't read Heroic's games: {error:#}"));
            return ids;
        }
    };
    for source in sources {
        match source.gog_installed() {
            Ok(listing) => ids.extend(listing.games.into_iter().map(|game| game.id)),
            Err(error) => warnings.push(format!("Couldn't read Heroic's GOG games: {error:#}")),
        }
    }
    ids
}

fn game_json(row: &Row) -> Value {
    let platforms = &row.game.works_on;
    let platforms: Vec<&str> = [
        (platforms.windows, "windows"),
        (platforms.mac, "mac"),
        (platforms.linux, "linux"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let status = row.status;
    json!({
        "id": row.game.id.to_string(),
        "title": row.game.title,
        "platforms": platforms,
        "state": row.state.json(),
        "environment": row.environment,
        "track": status.map(|status| status.track.as_str()),
        "version": status.and_then(|status| status.installed.as_deref()),
        "update": status.map(|status| status.state.as_str()),
        "applicable": status.is_some_and(|status| status.applicable),
        "reason": status.map_or(Value::Null, |status| reason_json(&status.reason)),
        "unfinished": status.is_some_and(|status| status.unfinished),
        "dlc": status.map(|status| dlc_json(&status.dlc)).unwrap_or_default(),
    })
}

fn print_table(rows: &[Row]) {
    if rows.is_empty() {
        println!("You don't own any GOG games.");
        return;
    }
    let mut table = vec![
        [
            "ID",
            "TITLE",
            "STATE",
            "ENVIRONMENT",
            "TRACK",
            "VERSION",
            "UPDATE",
        ]
        .map(str::to_owned),
    ];
    for row in rows {
        let status = row.status;
        table.push([
            row.game.id.to_string(),
            row.game.title.clone(),
            row.state.label().to_owned(),
            row.environment.clone().unwrap_or_default(),
            status
                .map(|status| status.track.as_str().to_owned())
                .unwrap_or_default(),
            status
                .and_then(|status| status.installed.clone())
                .unwrap_or_default(),
            status
                .map(|status| update_label(status.state).to_owned())
                .unwrap_or_default(),
        ]);
    }
    for line in render_columns(&table) {
        println!("{line}");
    }
}
