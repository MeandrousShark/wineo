//! A blocking client for GOG's web APIs: login, the owned list, product
//! details, installer links, Galaxy builds with their manifests and chunks,
//! playtime, artwork links, account names, and installer downloads into a
//! cache. It knows tokens, URLs, and folders, never wineo's environments,
//! prefixes, configs, or runners.
//!
//! Tokens, authorization codes, the client secret, and pasted redirect
//! addresses are secrets. None of them can reach an `Error`: every failure is
//! classified into a fixed message that names the endpoint, never the URL
//! with its query string, and reqwest's own errors are only inspected, never
//! formatted. Types that hold a secret redact it in `Debug` and have no
//! `Display`.

mod art;
mod auth;
mod cache;
mod checksum;
mod chunks;
mod client;
mod download;
mod error;
mod json;
mod manifest;
mod owned;
mod product;
mod profile;

#[cfg(any(test, feature = "test-server"))]
pub mod fake;

pub use art::GameArt;
pub use auth::{
    AuthCode, CLIENT_ID, PasteError, REDIRECT_URI, Secret, Tokens, UserId, login_url, parse_paste,
};
pub use cache::{CachedFile, DownloadCache, Package, Pin};
pub use chunks::{ChunkLinks, LinkScope, SecureLink};
pub use client::{Client, Endpoints};
pub use download::{Downloaded, Verification};
pub use error::{Class, Endpoint, Error, ErrorKind};
pub use manifest::{
    Chunk, Dependency, DependencyExecutable, DependencyManifest, DependencyRepository, Depot,
    DepotDirectory, DepotFile, DepotItem, DepotLink, DepotManifest, Repository, RepositoryProduct,
};
pub use owned::{OwnedPage, OwnedProduct, Platforms, collect_pages};
pub use product::{
    Build, Builds, Compatibility, Download, DownloadFile, Images, Product, ResolvedLink,
};
