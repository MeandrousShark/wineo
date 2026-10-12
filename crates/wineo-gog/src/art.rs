//! A game's artwork links from `api.gog.com/v2/games/<id>`.
//!
//! Product details' `images.logo` is a store tile, not a transparent logo, so
//! the v2 answer's `_links` are used instead. Each link is optional, and only
//! HTTPS ones are kept: the images are fetched as given, so a plain HTTP or
//! protocol-relative address is dropped rather than upgraded.

use serde_json::Value;

use crate::error::{Endpoint, Error, ErrorKind};
use crate::json;

/// HTTPS image URLs for one game. Each is optional, since games vary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameArt {
    /// `boxArtImage`, the vertical cover (342x482).
    pub box_art: Option<String>,
    /// `backgroundImage` (2560x655).
    pub background: Option<String>,
    /// `galaxyBackgroundImage` (2560x670).
    pub galaxy_background: Option<String>,
    /// `logo`, a transparent PNG (1600x740).
    pub logo: Option<String>,
    /// `icon` (128x128).
    pub icon: Option<String>,
    /// `iconSquare` (128x128).
    pub icon_square: Option<String>,
}

impl GameArt {
    /// Whether GOG offered no usable link at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

pub(crate) fn parse_game_art(json: &Value, id: u64) -> Result<GameArt, Error> {
    let malformed = || Error::new(Endpoint::GameArt, ErrorKind::Malformed);
    // Links for another game would be filed under the wrong one.
    let product = json
        .get("_embedded")
        .and_then(|embedded| embedded.get("product"))
        .and_then(|product| product.get("id"))
        .and_then(json::id)
        .ok_or_else(malformed)?;
    if product != id {
        return Err(malformed());
    }
    let links = json.get("_links").ok_or_else(malformed)?;
    let url = |name: &str| {
        links
            .get(name)
            .and_then(|link| link.get("href"))
            .and_then(json::strict_https_url)
    };
    Ok(GameArt {
        box_art: url("boxArtImage"),
        background: url("backgroundImage"),
        galaxy_background: url("galaxyBackgroundImage"),
        logo: url("logo"),
        icon: url("icon"),
        icon_square: url("iconSquare"),
    })
}
