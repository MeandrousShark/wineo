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

/// Builds the whole owned list from its pages, asking `fetch` for each one
/// in turn, so a caller can check whatever it needs between requests. Any
/// page failing fails the whole list, so a partial list is never mistaken
/// for the library.
pub fn collect_pages<E: From<Error>>(
    mut fetch: impl FnMut(u32) -> Result<OwnedPage, E>,
) -> Result<Vec<OwnedProduct>, E> {
    let malformed = || E::from(Error::new(Endpoint::Owned, ErrorKind::Malformed));
    let first = fetch(1)?;
    if first.page != 1 {
        return Err(malformed());
    }
    let total = first.total_pages;
    let mut products = first.products;
    for page in 2..=total.min(MAX_PAGES) {
        let next = fetch(page)?;
        // A changed total means the library moved under the fetch, so the
        // pages may not add up to it.
        if next.page != page || next.total_pages != total {
            return Err(malformed());
        }
        products.extend(next.products);
    }
    // A purchase between pages can shift a game onto the next one.
    let mut seen = std::collections::HashSet::new();
    products.retain(|product| seen.insert(product.id));
    Ok(products)
}

/// `{"owned": [ids...]}`, every product the account owns. The shape comes
/// from other GOG tools and hasn't been checked against a live account.
/// Anything in the list that isn't an id is skipped.
pub(crate) fn parse_ids(json: &Value) -> Result<Vec<u64>, Error> {
    let owned = json
        .get("owned")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new(Endpoint::OwnedIds, ErrorKind::Malformed))?;
    let mut seen = std::collections::HashSet::new();
    Ok(owned
        .iter()
        .filter_map(json::id)
        .filter(|id| seen.insert(*id))
        .collect())
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
