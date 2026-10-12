//! A blocking client for GOG's web APIs: login, the owned list, product
//! details, installer links, Galaxy build lists, and installer downloads
//! into a cache. It knows tokens, URLs, and folders, never wineo's
//! environments, prefixes, configs, or runners.
//!
//! Tokens, authorization codes, the client secret, and pasted redirect
//! addresses are secrets. None of them can reach an `Error`: every failure is
//! classified into a fixed message that names the endpoint, never the URL
//! with its query string, and reqwest's own errors are only inspected, never
//! formatted. Types that hold a secret redact it in `Debug` and have no
//! `Display`.

mod auth;
mod cache;
mod checksum;
mod client;
mod download;
mod error;
mod json;
mod owned;
mod product;

#[cfg(test)]
mod fake;

pub use auth::{
    AuthCode, CLIENT_ID, PasteError, REDIRECT_URI, Secret, Tokens, UserId, login_url, parse_paste,
};
pub use cache::{CachedFile, DownloadCache, Package, Pin};
pub use client::{Client, Endpoints};
pub use download::{Downloaded, Verification};
pub use error::{Class, Endpoint, Error, ErrorKind};
pub use owned::{OwnedPage, OwnedProduct, Platforms, collect_pages};
pub use product::{
    Build, Builds, Compatibility, Download, DownloadFile, Images, Product, ResolvedLink,
};
