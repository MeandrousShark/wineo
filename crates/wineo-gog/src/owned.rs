//! The account's owned games, from `embed.gog.com/account/getFilteredProducts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Endpoint, Error, ErrorKind};
use crate::json;

/// More would be over 50,000 games at GOG's 100 a page, so a larger count
/// is taken as a broken answer rather than followed.
pub(crate) const MAX_PAGES: u32 = 500;

/// Platform flags as GOG states them, which are sometimes wrong. Product
/// details' installers are the better source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platforms {
    pub windows: bool,
    pub mac: bool,
    pub linux: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedProduct {
    pub id: u64,
    pub title: String,
    /// An HTTPS base URL without a size suffix or extension, as GOG gives it.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub works_on: Platforms,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub dlc_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPage {
    pub page: u32,
    pub total_pages: u32,
    pub products: Vec<OwnedProduct>,
}

pub(crate) fn parse_page(json: &Value) -> Result<OwnedPage, Error> {
    let malformed = || Error::new(Endpoint::Owned, ErrorKind::Malformed);
    let number = |name: &str| {
        json.get(name)
            .and_then(json::id)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(malformed)
    };
    let page = number("page")?;
    let total_pages = number("totalPages")?;
    if total_pages > MAX_PAGES {
        return Err(malformed());
    }
    let products = json
        .get("products")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?
        .iter()
        .map(|product| parse_product(product).ok_or_else(malformed))
        .collect::<Result<_, _>>()?;
    Ok(OwnedPage {
        page,
        total_pages,
        products,
    })
}

fn parse_product(product: &Value) -> Option<OwnedProduct> {
    let id = json::id(product.get("id")?)?;
    let works_on = product
        .get("worksOn")
        .map_or_else(Platforms::default, |flags| Platforms {
            windows: json::flag(flags, "Windows"),
            mac: json::flag(flags, "Mac"),
            linux: json::flag(flags, "Linux"),
        });
    Some(OwnedProduct {
        id,
        title: json::text(product, "title").unwrap_or_default(),
        image: product.get("image").and_then(json::https_url),
        works_on,
        slug: json::text(product, "slug"),
        dlc_count: product
            .get("dlcCount")
            .and_then(json::id)
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or(0),
    })
}
