//! What GOG adds to a migrated game once it is an environment: its total
//! playtime and its artwork.
//!
//! The migration has already succeeded when this runs, so nothing here fails
//! the call because GOG or the network does: each problem becomes a note in
//! the report. Only a request that can't be about this environment (a
//! portable root, an environment that doesn't run the game) is an error, and
//! that is decided before anything is fetched.
//!
//! The network is used with no lock held. The environment's config lock is
//! taken only to raise the playtime baseline, and `art::publish_source` takes
//! the art file's own lock, so the two never nest. Config locks come last in
//! the order `folder.rs` describes, and this takes no lock before them.

use anyhow::Result;
use wineo_gog::{Client, GameArt};

use super::account::Session;
use super::migrate::host_only;
use crate::art::{self, Kind, Source};
use crate::art_fetch::{self, Fetched, Mode};
use crate::cancel::Token;
use crate::download::Transport;
use crate::environment::{Environment, EnvironmentIdentity};
use crate::operation::Locks;
use crate::paths::Paths;
use crate::stats::{self, Imported};
use crate::steamgriddb::{Api, download_image};

#[derive(Debug, thiserror::Error)]
pub enum ExtrasError {
    #[error("{name} doesn't run GOG game {gog_id}.")]
    OtherGame { name: String, gog_id: u64 },
}

/// What to import, and what to use for it.
pub struct Request<'a> {
    pub name: &'a str,
    pub gog_id: u64,
    /// The game's title, to find it on SteamGridDB for kinds GOG lacks.
    pub title: &'a str,
    /// The kinds to fill, like `GlobalConfig::art.fetch.kinds()`.
    pub kinds: &'a [Kind],
    /// Downloads GOG's images and talks to SteamGridDB.
    pub transport: &'a dyn Transport,
    /// SteamGridDB's key, when there is one. Without it a kind GOG lacks
    /// stays empty.
    pub steamgriddb_key: Option<&'a str>,
    /// For ranking SteamGridDB images; see `steamgriddb::language`.
    pub language: &'a str,
}

/// The imported playtime baseline before and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Playtime {
    /// GOG's total.
    pub gog_seconds: i64,
    pub before_seconds: i64,
    pub after_seconds: i64,
}

impl Playtime {
    /// Whether the baseline went up.
    pub fn raised(&self) -> bool {
        self.after_seconds > self.before_seconds
    }
}

/// A kind that now has art, and where it came from: `Source::Gog`, or
/// `Source::SteamGridDb` for a kind GOG lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filled {
    pub kind: Kind,
    pub source: Source,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// None when playtime was skipped, GOG has none, or it failed; a note
    /// says which.
    pub playtime: Option<Playtime>,
    pub art: Vec<Filled>,
    /// Anything skipped or failed, ready to show as warnings.
    pub notes: Vec<String>,
}

/// Imports GOG's playtime (with a login) and artwork (without one) for the
/// environment that runs `request.gog_id`. Read-only toward GOG.
pub fn import(
    paths: &Paths,
    session: Option<&mut Session<'_>>,
    request: &Request<'_>,
) -> Result<Report> {
    run(paths, session, request, super::client_from_env())
}

fn run(
    paths: &Paths,
    session: Option<&mut Session<'_>>,
    request: &Request<'_>,
    client: Result<Client, wineo_gog::Error>,
) -> Result<Report> {
    host_only(paths)?;
    let name = request.name;
    // One snapshot under the config lock, so the game that is checked and the
    // identity that later writes are held to are the same environment.
    let identity = {
        let _lock = Locks::new(paths).config(&paths.environment_config(name))?;
        let environment = Environment::load(paths, name)?;
        if environment.config.gog_id.as_deref() != Some(request.gog_id.to_string().as_str()) {
            return Err(ExtrasError::OtherGame {
                name: name.to_owned(),
                gog_id: request.gog_id,
            }
            .into());
        }
        // Writes below only land on this config, not one that replaced it
        // while GOG was answering.
        EnvironmentIdentity::read(paths, name)
            .ok_or_else(|| crate::Error::UnknownEnvironment(name.to_owned()))?
    };

    let mut report = Report::default();
    match session {
        Some(session) => playtime(paths, request, session, identity, &mut report),
        None => report
            .notes
            .push("Skipped GOG playtime: not logged in to GOG.".to_owned()),
    }
    match client {
        Ok(client) => artwork(paths, request, &client, identity, &mut report),
        Err(error) => report.notes.push(format!("Skipped GOG artwork: {error}.")),
    }
    Ok(report)
}

fn playtime(
    paths: &Paths,
    request: &Request<'_>,
    session: &mut Session<'_>,
    identity: EnvironmentIdentity,
    report: &mut Report,
) {
    let (user, gog_id) = (session.user_id(), request.gog_id);
    let minutes = match session.call(|client, token| client.playtime(token, user, gog_id)) {
        Ok(Some(minutes)) => minutes,
        Ok(None) => {
            report
                .notes
                .push(format!("GOG has no playtime for {}.", request.title));
            return;
        }
        Err(error) => {
            report
                .notes
                .push(format!("Couldn't import GOG playtime: {error}"));
            return;
        }
    };
    let Some(gog_seconds) = minutes
        .checked_mul(60)
        .and_then(|seconds| i64::try_from(seconds).ok())
    else {
        report
            .notes
            .push("Couldn't import GOG playtime: the total is out of range.".to_owned());
        return;
    };
    match raise_baseline(paths, request.name, gog_seconds, identity) {
        Ok(Some(playtime)) => report.playtime = Some(playtime),
        Ok(None) => report.notes.push(format!(
            "Skipped GOG playtime: {} changed while it was being fetched.",
            request.name
        )),
        Err(error) => report
            .notes
            .push(format!("Couldn't save GOG playtime: {error}")),
    }
}

/// Makes the imported baseline the larger of what it is and `gog_seconds`.
/// The log is append-only, so a total that isn't larger appends nothing,
/// and running this again changes nothing. None means the environment was
/// replaced meanwhile.
fn raise_baseline(
    paths: &Paths,
    name: &str,
    gog_seconds: i64,
    identity: EnvironmentIdentity,
) -> crate::Result<Option<Playtime>> {
    // Held across the read and the append, so two runs can't each read a
    // small baseline and the smaller total be the one that lands last.
    let _lock = Locks::new(paths).config(&paths.environment_config(name))?;
    if EnvironmentIdentity::read(paths, name) != Some(identity) {
        return Ok(None);
    }
    let existing = stats::read(&paths.stats_log(name))?.imported;
    let before_seconds = existing.map_or(0, |imported| imported.seconds);
    let after_seconds = before_seconds.max(gog_seconds);
    if after_seconds > before_seconds {
        stats::set_imported(
            paths,
            name,
            Imported {
                seconds: after_seconds,
                last_played: existing.and_then(|imported| imported.last_played),
            },
        )?;
    }
    Ok(Some(Playtime {
        gog_seconds,
        before_seconds,
        after_seconds,
    }))
}

fn artwork(
    paths: &Paths,
    request: &Request<'_>,
    client: &Client,
    identity: EnvironmentIdentity,
    report: &mut Report,
) {
    let name = request.name;
    let mut missing = Vec::new();
    for kind in Kind::ALL
        .into_iter()
        .filter(|kind| request.kinds.contains(kind))
    {
        match art_fetch::existing(paths, name, None, kind) {
            Ok(None) => missing.push(kind),
            Ok(Some(_)) => {}
            Err(error) => report
                .notes
                .push(format!("Skipped {kind} artwork: {error}")),
        }
    }
    if missing.is_empty() {
        return;
    }
    // A GOG failure isn't GOG lacking the art, so it doesn't fall back.
    let links = match client.game_art(request.gog_id) {
        Ok(links) => links,
        Err(error) => {
            report
                .notes
                .push(format!("Couldn't get GOG artwork: {error}"));
            return;
        }
    };
    let mut lacking = Vec::new();
    for kind in missing {
        let Some(url) = link(&links, kind) else {
            lacking.push(kind);
            continue;
        };
        match store(paths, request, kind, url, identity) {
            Ok(true) => report.art.push(Filled {
                kind,
                source: Source::Gog(request.gog_id),
            }),
            // Art appeared since the check, and it is kept.
            Ok(false) => {}
            Err(error) => report
                .notes
                .push(format!("Couldn't fetch GOG {kind} artwork: {error}")),
        }
    }
    steamgriddb(paths, request, &lacking, identity, report);
}

/// The link for a kind: the cover for the poster, the wider Galaxy
/// background for the hero, and the square icon for the icon.
fn link(links: &GameArt, kind: Kind) -> Option<&str> {
    match kind {
        Kind::Grid => links.box_art.as_deref(),
        Kind::Hero => links
            .galaxy_background
            .as_deref()
            .or(links.background.as_deref()),
        Kind::Logo => links.logo.as_deref(),
        Kind::Icon => links.icon_square.as_deref().or(links.icon.as_deref()),
    }
}

/// Downloads and publishes one image without replacing anything. False when
/// the kind got art in the meantime.
fn store(
    paths: &Paths,
    request: &Request<'_>,
    kind: Kind,
    url: &str,
    identity: EnvironmentIdentity,
) -> Result<bool> {
    let bytes = download_image(request.transport, url, |_, _| {}, &Token::new())?;
    let Some(format) = art::detect(&bytes).filter(|format| kind.accepts(*format)) else {
        anyhow::bail!("it isn't a {} image", art::accepted(kind));
    };
    let stored = art::publish_source(
        paths,
        request.name,
        None,
        kind,
        &bytes,
        format,
        Source::Gog(request.gog_id),
        false,
        None,
        Some(identity),
    )?;
    Ok(stored.is_some())
}

/// Fills kinds GOG has no image for from SteamGridDB, in the game the
/// environment already has saved, or the one titled exactly like this game.
fn steamgriddb(
    paths: &Paths,
    request: &Request<'_>,
    kinds: &[Kind],
    identity: EnvironmentIdentity,
    report: &mut Report,
) {
    let (Some(key), false) = (request.steamgriddb_key, kinds.is_empty()) else {
        return;
    };
    let name = request.name;
    let api = Api::new(request.transport, key);
    let mut note = |text: String| report.notes.push(text);
    let game = match art::saved_game(paths, name) {
        Ok(Some(game)) => game,
        Ok(None) => {
            let games = match api.search(request.title) {
                Ok(games) => games,
                Err(error) => {
                    note(format!("Couldn't search SteamGridDB: {error}"));
                    return;
                }
            };
            let title = request.title.trim();
            let Some(game) = games
                .iter()
                .find(|game| game.name.trim().eq_ignore_ascii_case(title))
            else {
                note(format!("SteamGridDB has no game titled {title}."));
                return;
            };
            if let Err(error) = art::save_game_for(paths, name, game.id, Some(identity)) {
                note(format!("Couldn't save the SteamGridDB game: {error}"));
                return;
            }
            game.id
        }
        Err(error) => {
            note(format!("Skipped SteamGridDB artwork: {error}"));
            return;
        }
    };
    for &kind in kinds {
        match art_fetch::fetch_kind_for(
            paths,
            name,
            None,
            kind,
            &api,
            game,
            request.language,
            Mode::Fill,
            Some(identity),
        ) {
            Ok(Fetched::Stored { asset, .. }) => report.art.push(Filled {
                kind,
                source: Source::SteamGridDb(asset),
            }),
            Ok(Fetched::NoneFound) => {
                note(format!(
                    "SteamGridDB has no {kind} artwork for {}.",
                    request.title
                ));
            }
            Ok(Fetched::Skipped { .. } | Fetched::AssetNotFound) => {}
            Err(error) => note(format!(
                "Couldn't fetch {kind} artwork from SteamGridDB: {error}"
            )),
        }
    }
}

#[cfg(test)]
mod tests;
