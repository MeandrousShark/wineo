//! Epic's own art for an installed game, from the addresses in the owned
//! list. The install has already succeeded when this runs, so nothing here
//! fails it: each problem becomes a note.

use anyhow::Result;

use super::owned::Image;
use crate::art::{self, Kind, Source};
use crate::art_fetch;
use crate::cancel::Token;
use crate::download::Transport;
use crate::environment::{Environment, EnvironmentIdentity};
use crate::operation::Locks;
use crate::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum ArtError {
    #[error("{name} doesn't run Epic game {app}.")]
    OtherGame { name: String, app: String },
}

/// What to import, and what to use for it.
pub struct Request<'a> {
    pub name: &'a str,
    pub app_name: &'a str,
    pub images: &'a [Image],
    /// The kinds to fill, like `GlobalConfig::art.fetch.kinds()`.
    pub kinds: &'a [Kind],
    pub transport: &'a dyn Transport,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The kinds that now have Epic's art.
    pub art: Vec<Kind>,
    /// Anything skipped or failed, ready to show as warnings.
    pub notes: Vec<String>,
}

impl Report {
    /// What happened, in a line, or none.
    pub fn line(&self) -> Option<String> {
        let kinds: Vec<&str> = [Kind::Grid, Kind::Hero, Kind::Logo, Kind::Icon]
            .into_iter()
            .filter(|kind| self.art.contains(kind))
            .map(|kind| match kind {
                Kind::Grid => "poster",
                other => other.as_str(),
            })
            .collect();
        (!kinds.is_empty()).then(|| format!("Added Epic art: {}.", kinds.join(", ")))
    }
}

/// Epic's image type for a kind: the tall box art for the poster, the wide
/// one for the hero, and the logo. Epic has no icon.
pub fn image_type(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Grid => Some("DieselGameBoxTall"),
        Kind::Hero => Some("DieselGameBox"),
        Kind::Logo => Some("DieselGameBoxLogo"),
        Kind::Icon => None,
    }
}

/// The address of Epic's image for `kind`, if the game has one.
pub fn link(images: &[Image], kind: Kind) -> Option<&str> {
    let wanted = image_type(kind)?;
    images
        .iter()
        .find(|image| image.kind == wanted)
        .map(|image| image.url.as_str())
}

/// Fills each kind the environment has no art for from Epic's images,
/// never replacing art it has. `token` is checked before each kind and
/// while an image downloads; a cancel keeps what was already written.
pub fn import(paths: &Paths, request: &Request<'_>, token: &Token) -> Result<Report> {
    let name = request.name;
    let identity = {
        let _lock = Locks::new(paths).config(&paths.environment_config(name))?;
        let environment = Environment::load(paths, name)?;
        if environment.config.epic_id.as_deref() != Some(request.app_name) {
            return Err(ArtError::OtherGame {
                name: name.to_owned(),
                app: request.app_name.to_owned(),
            }
            .into());
        }
        EnvironmentIdentity::acquire(paths, name)?
            .ok_or_else(|| crate::Error::UnknownEnvironment(name.to_owned()))?
    };
    let mut report = Report::default();
    for kind in Kind::ALL
        .into_iter()
        .filter(|kind| request.kinds.contains(kind))
    {
        let Some(url) = link(request.images, kind) else {
            continue;
        };
        if token.is_canceled() {
            report
                .notes
                .push(format!("Skipped {kind} artwork: canceled."));
            continue;
        }
        match art_fetch::existing(paths, name, None, kind) {
            Ok(None) => {}
            Ok(Some(_)) => continue,
            Err(error) => {
                report
                    .notes
                    .push(format!("Skipped {kind} artwork: {error}"));
                continue;
            }
        }
        match store(paths, request, kind, url, identity, token) {
            Err(_) if token.is_canceled() => {
                report
                    .notes
                    .push(format!("Skipped {kind} artwork: canceled."));
            }
            Ok(true) => report.art.push(kind),
            // Art appeared since the check, and it is kept.
            Ok(false) => {}
            Err(error) => report
                .notes
                .push(format!("Couldn't fetch Epic {kind} artwork: {error:#}")),
        }
    }
    Ok(report)
}

fn store(
    paths: &Paths,
    request: &Request<'_>,
    kind: Kind,
    url: &str,
    identity: EnvironmentIdentity,
    token: &Token,
) -> Result<bool> {
    let (bytes, format) = crate::gog::extras::fetch_image(request.transport, url, kind, token)?;
    let stored = art::publish_source(
        paths,
        request.name,
        None,
        kind,
        &bytes,
        format,
        Source::Epic(request.app_name.to_owned()),
        false,
        Some(token),
        identity,
    )?;
    Ok(stored.is_some())
}

#[cfg(test)]
mod tests;
