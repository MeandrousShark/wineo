//! Product details from `api.gog.com/products/<id>`, installer link
//! resolution, and Galaxy build lists from `content-system.gog.com`.
//!
//! Parsing is lenient below the top level: a list entry missing what makes
//! it usable is skipped, so one odd installer can't hide the rest, while a
//! product with no id is a malformed answer.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use crate::error::{Endpoint, Error, ErrorKind};
use crate::json;

/// `content_system_compatibility`, as GOG states it. Sometimes wrong, so
/// `Product::installer_platforms` gives the installers' view to check it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Compatibility {
    pub windows: bool,
    pub mac: bool,
    pub linux: bool,
}

/// HTTPS image URLs. Each is optional, since products vary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Images {
    pub background: Option<String>,
    pub logo: Option<String>,
    pub logo2x: Option<String>,
    pub icon: Option<String>,
    pub sidebar_icon: Option<String>,
    pub sidebar_icon2x: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadFile {
    /// GOG's file id, like `en1installer0`.
    pub id: String,
    pub size: u64,
    /// An API link that `Client::resolve_link` turns into the real download.
    pub downlink: String,
}

/// One installer or patch, for one OS and language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub id: String,
    pub name: Option<String>,
    /// GOG's own OS name, kept raw: `windows`, `linux`, or `mac`.
    pub os: String,
    /// A language code, like `en`.
    pub language: String,
    pub language_full: Option<String>,
    pub version: Option<String>,
    pub total_size: u64,
    pub files: Vec<DownloadFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: u64,
    pub title: String,
    pub slug: Option<String>,
    /// `game`, `dlc`, `pack`, and so on.
    pub game_type: Option<String>,
    pub compatibility: Compatibility,
    /// Language codes to their English names.
    pub languages: BTreeMap<String, String>,
    pub images: Images,
    pub installers: Vec<Download>,
    pub patches: Vec<Download>,
    /// The ids in `dlcs.products`, owned or not.
    pub dlc_ids: Vec<u64>,
    /// Full details of each DLC, from `expanded_dlcs`.
    pub dlcs: Vec<Product>,
}

impl Product {
    /// The platforms GOG offers installers for, to cross-check
    /// `compatibility`.
    pub fn installer_platforms(&self) -> Compatibility {
        let has = |os: &str| self.installers.iter().any(|installer| installer.os == os);
        Compatibility {
            windows: has("windows"),
            mac: has("mac") || has("osx"),
            linux: has("linux"),
        }
    }
}

pub(crate) fn parse_product(json: &Value) -> Result<Product, Error> {
    parse_product_inner(json, true)
        .ok_or_else(|| Error::new(Endpoint::Product, ErrorKind::Malformed))
}

fn parse_product_inner(json: &Value, with_dlcs: bool) -> Option<Product> {
    let id = json::id(json.get("id")?)?;
    let compatibility =
        json.get("content_system_compatibility")
            .map_or_else(Compatibility::default, |flags| Compatibility {
                windows: json::flag(flags, "windows"),
                mac: json::flag(flags, "osx"),
                linux: json::flag(flags, "linux"),
            });
    let languages = json
        .get("languages")
        .and_then(Value::as_object)
        .map(|languages| {
            languages
                .iter()
                .filter_map(|(code, name)| Some((code.clone(), name.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let images = json.get("images").map_or_else(Images::default, |images| {
        let url = |name: &str| images.get(name).and_then(json::https_url);
        Images {
            background: url("background"),
            logo: url("logo"),
            logo2x: url("logo2x"),
            icon: url("icon"),
            sidebar_icon: url("sidebarIcon"),
            sidebar_icon2x: url("sidebarIcon2x"),
        }
    });
    let downloads = |kind: &str| {
        json.get("downloads")
            .and_then(|downloads| downloads.get(kind))
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(parse_download).collect())
            .unwrap_or_default()
    };
    let dlc_ids = json
        .get("dlcs")
        .and_then(|dlcs| dlcs.get("products"))
        .and_then(Value::as_array)
        .map(|products| {
            products
                .iter()
                .filter_map(|product| json::id(product.get("id")?))
                .collect()
        })
        .unwrap_or_default();
    let dlcs = if with_dlcs {
        json.get("expanded_dlcs")
            .and_then(Value::as_array)
            .map(|dlcs| {
                dlcs.iter()
                    .filter_map(|dlc| parse_product_inner(dlc, false))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(Product {
        id,
        title: json::text(json, "title").unwrap_or_default(),
        slug: json::text(json, "slug"),
        game_type: json::text(json, "game_type"),
        compatibility,
        languages,
        images,
        installers: downloads("installers"),
        patches: downloads("patches"),
        dlc_ids,
        dlcs,
    })
}

fn parse_download(json: &Value) -> Option<Download> {
    let files: Vec<DownloadFile> = json
        .get("files")?
        .as_array()?
        .iter()
        .filter_map(|file| {
            Some(DownloadFile {
                id: file.get("id").and_then(id_text)?,
                size: file.get("size").and_then(json::id).unwrap_or(0),
                downlink: json::text(file, "downlink")?,
            })
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    Some(Download {
        id: json.get("id").and_then(id_text)?,
        name: json::text(json, "name"),
        os: json::text(json, "os")?,
        language: json::text(json, "language")?,
        language_full: json::text(json, "language_full"),
        version: json::text(json, "version"),
        total_size: json.get("total_size").and_then(json::id).unwrap_or(0),
        files,
    })
}

/// File and installer ids are usually strings, sometimes numbers.
fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => number.as_u64().map(|id| id.to_string()),
        _ => None,
    }
}

/// Where an installer file can be downloaded from, and where GOG publishes
/// its checksum. The link is signed and short-lived, so `Debug` hides both.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedLink {
    pub downlink: String,
    /// A URL to GOG's checksum XML, when GOG publishes one.
    pub checksum: Option<String>,
}

impl fmt::Debug for ResolvedLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedLink")
            .field("downlink", &"<redacted>")
            .field("checksum", &self.checksum.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

pub(crate) fn parse_link(json: &Value) -> Result<ResolvedLink, Error> {
    Ok(ResolvedLink {
        downlink: json::text(json, "downlink")
            .ok_or_else(|| Error::new(Endpoint::Link, ErrorKind::Malformed))?,
        checksum: json::text(json, "checksum"),
    })
}

/// One Galaxy build of a product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub build_id: String,
    pub version_name: Option<String>,
    /// None for the default branch.
    pub branch: Option<String>,
    pub date_published: Option<String>,
    pub generation: Option<u64>,
    /// The build's manifest, which 0051's downloader reads.
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builds {
    pub items: Vec<Build>,
    pub has_private_branches: bool,
}

pub(crate) fn parse_builds(json: &Value) -> Result<Builds, Error> {
    let items = json
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new(Endpoint::Builds, ErrorKind::Malformed))?
        .iter()
        .filter_map(|item| {
            Some(Build {
                build_id: item.get("build_id").and_then(id_text)?,
                version_name: json::text(item, "version_name"),
                branch: json::text(item, "branch"),
                date_published: json::text(item, "date_published"),
                generation: item.get("generation").and_then(json::id),
                link: json::text(item, "link"),
            })
        })
        .collect();
    Ok(Builds {
        items,
        has_private_branches: json::flag(json, "has_private_branches"),
    })
}
