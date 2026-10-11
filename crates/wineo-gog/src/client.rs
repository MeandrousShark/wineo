use std::io::{self, Read};
use std::time::Duration;

use reqwest::Url;
use serde_json::Value;

use crate::auth::{self, AuthCode, CLIENT_ID, CLIENT_SECRET, REDIRECT_URI, Tokens};
use crate::error::{Endpoint, Error, ErrorKind};
use crate::owned::{self, MAX_PAGES, OwnedPage, OwnedProduct};
use crate::product::{self, Builds, Product, ResolvedLink};

/// Product details with every DLC expanded run to a few megabytes.
const MAX_BODY: u64 = 16 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Applies to each read of a blocking response, not the whole body.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The base URL of each GOG service the client talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub auth: Url,
    pub embed: Url,
    pub api: Url,
    pub content_system: Url,
}

impl Endpoints {
    pub fn production() -> Self {
        let parse = |url: &str| Url::parse(url).expect("production endpoints are URLs");
        Self {
            auth: parse("https://auth.gog.com"),
            embed: parse("https://embed.gog.com"),
            api: parse("https://api.gog.com"),
            content_system: parse("https://content-system.gog.com"),
        }
    }

    /// Every service under one base, the way a fake server serves them.
    pub fn all_at(base: &str) -> Option<Self> {
        let base = Url::parse(base).ok()?;
        (!base.cannot_be_a_base()).then(|| Self {
            auth: base.clone(),
            embed: base.clone(),
            api: base.clone(),
            content_system: base,
        })
    }
}

pub struct Client {
    http: reqwest::blocking::Client,
    endpoints: Endpoints,
    max_body: u64,
}

impl Client {
    /// GOG itself, over HTTPS only. `user_agent` is the caller's product and
    /// version, like `wineo/0.30.0`, and nothing personal.
    pub fn new(user_agent: &str) -> Result<Self, Error> {
        Self::build(
            user_agent,
            Endpoints::production(),
            true,
            CONNECT_TIMEOUT,
            READ_TIMEOUT,
        )
    }

    /// A server standing in for GOG, which may be plain HTTP. Release builds
    /// don't have it, so they can only ever talk to GOG over HTTPS.
    #[cfg(any(test, debug_assertions))]
    pub fn with_base(user_agent: &str, base: &str) -> Result<Self, Error> {
        let endpoints =
            Endpoints::all_at(base).ok_or(Error::new(Endpoint::Setup, ErrorKind::Request))?;
        Self::build(user_agent, endpoints, false, CONNECT_TIMEOUT, READ_TIMEOUT)
    }

    pub(crate) fn build(
        user_agent: &str,
        endpoints: Endpoints,
        https_only: bool,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, Error> {
        let http = reqwest::blocking::Client::builder()
            .https_only(https_only)
            .connect_timeout(connect_timeout)
            .timeout(read_timeout)
            .user_agent(user_agent)
            // Every endpoint answers directly. A redirect could carry the
            // bearer token or the client secret somewhere unexpected.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::new(Endpoint::Setup, ErrorKind::Request))?;
        Ok(Self {
            http,
            endpoints,
            max_body: MAX_BODY,
        })
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// The login page on this client's auth server.
    pub fn login_url(&self) -> String {
        auth::login_url_at(&self.endpoints.auth)
    }

    /// Trades a login's code for tokens.
    pub fn exchange_code(&self, code: &AuthCode) -> Result<Tokens, Error> {
        self.token(&[
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", REDIRECT_URI),
        ])
    }

    /// Renews the access token. GOG may hand back a new refresh token, which
    /// replaces the old one.
    pub fn refresh(&self, refresh_token: &str) -> Result<Tokens, Error> {
        self.token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
    }

    fn token(&self, params: &[(&str, &str)]) -> Result<Tokens, Error> {
        let mut url = join(&self.endpoints.auth, &["token"], Endpoint::Token)?;
        url.query_pairs_mut()
            .append_pair("client_id", CLIENT_ID)
            .append_pair("client_secret", CLIENT_SECRET)
            .extend_pairs(params);
        let json = self.get_json(Endpoint::Token, url, None)?;
        auth::parse_tokens(&json)
    }

    /// One page of owned games, counting from 1.
    pub fn owned_page(&self, access_token: &str, page: u32) -> Result<OwnedPage, Error> {
        let mut url = join(
            &self.endpoints.embed,
            &["account", "getFilteredProducts"],
            Endpoint::Owned,
        )?;
        url.query_pairs_mut()
            .append_pair("mediaType", "1")
            .append_pair("page", &page.to_string());
        let json = self.get_json(Endpoint::Owned, url, Some(access_token))?;
        owned::parse_page(&json)
    }

    /// Every owned game, from every page. Any page failing fails the whole
    /// list, so a partial list is never mistaken for the library.
    pub fn owned_games(&self, access_token: &str) -> Result<Vec<OwnedProduct>, Error> {
        let malformed = || Error::new(Endpoint::Owned, ErrorKind::Malformed);
        let first = self.owned_page(access_token, 1)?;
        if first.page != 1 {
            return Err(malformed());
        }
        let total = first.total_pages;
        let mut products = first.products;
        for page in 2..=total.min(MAX_PAGES) {
            let next = self.owned_page(access_token, page)?;
            // A changed total means the library moved under the fetch, so
            // the pages may not add up to it.
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

    /// A product's details with its installers, patches, and expanded DLC.
    /// The access token, when given, is sent as GOG's own clients send it.
    pub fn product(&self, id: u64, access_token: Option<&str>) -> Result<Product, Error> {
        let mut url = join(
            &self.endpoints.api,
            &["products", &id.to_string()],
            Endpoint::Product,
        )?;
        url.query_pairs_mut()
            .append_pair("expand", "downloads,expanded_dlcs")
            .append_pair("locale", "en-US");
        let json = self.get_json(Endpoint::Product, url, access_token)?;
        let product = product::parse_product(&json)?;
        // Details for another product would be filed under the wrong id.
        if product.id != id {
            return Err(Error::new(Endpoint::Product, ErrorKind::Malformed));
        }
        Ok(product)
    }

    /// Asks GOG where an installer file's `downlink` really downloads from.
    /// The link must be on this client's API server, since the access token
    /// goes with it.
    pub fn resolve_link(&self, access_token: &str, downlink: &str) -> Result<ResolvedLink, Error> {
        let untrusted = || Error::new(Endpoint::Link, ErrorKind::UntrustedLink);
        let url = Url::parse(downlink).map_err(|_| untrusted())?;
        if url.origin() != self.endpoints.api.origin() {
            return Err(untrusted());
        }
        let json = self.get_json(Endpoint::Link, url, Some(access_token))?;
        product::parse_link(&json)
    }

    /// The Windows builds Galaxy offers for a product.
    pub fn windows_builds(&self, id: u64, access_token: Option<&str>) -> Result<Builds, Error> {
        let mut url = join(
            &self.endpoints.content_system,
            &["products", &id.to_string(), "os", "windows", "builds"],
            Endpoint::Builds,
        )?;
        url.query_pairs_mut().append_pair("generation", "2");
        let json = self.get_json(Endpoint::Builds, url, access_token)?;
        product::parse_builds(&json)
    }

    fn get_json(&self, endpoint: Endpoint, url: Url, bearer: Option<&str>) -> Result<Value, Error> {
        let fail = |kind| Error::new(endpoint, kind);
        let mut request = self.http.get(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().map_err(|error| {
            fail(if error.is_timeout() {
                ErrorKind::Timeout
            } else if error.is_connect() {
                ErrorKind::Connect
            } else {
                ErrorKind::Request
            })
        })?;
        let status = response.status().as_u16();
        if status != 200 {
            // Only the token endpoint's 400 says anything useful in its body.
            let body = (endpoint == Endpoint::Token && status == 400)
                .then(|| self.read_body(response).ok())
                .flatten();
            return Err(fail(status_kind(endpoint, status, body.as_deref())));
        }
        let body = self.read_body(response).map_err(fail)?;
        serde_json::from_slice(&body).map_err(|_| fail(ErrorKind::Json))
    }

    fn read_body(&self, response: reqwest::blocking::Response) -> Result<Vec<u8>, ErrorKind> {
        let mut body = Vec::new();
        response
            .take(self.max_body + 1)
            .read_to_end(&mut body)
            .map_err(|error| {
                if timed_out(&error) {
                    ErrorKind::Timeout
                } else {
                    ErrorKind::Body
                }
            })?;
        if body.len() as u64 > self.max_body {
            return Err(ErrorKind::TooLarge);
        }
        Ok(body)
    }
}

/// Appends path segments to a base, which may itself have a path.
fn join(base: &Url, segments: &[&str], endpoint: Endpoint) -> Result<Url, Error> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|()| Error::new(endpoint, ErrorKind::Request))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

/// Classifies a status that isn't a success. Only the token endpoint's
/// `invalid_grant` is read from the body, and the body is never kept.
fn status_kind(endpoint: Endpoint, status: u16, body: Option<&[u8]>) -> ErrorKind {
    match status {
        400 if endpoint == Endpoint::Token && body.is_some_and(is_invalid_grant) => {
            ErrorKind::InvalidGrant
        }
        401 => ErrorKind::Unauthorized,
        429 => ErrorKind::RateLimited,
        500..=599 => ErrorKind::Server(status),
        _ => ErrorKind::Status(status),
    }
}

fn is_invalid_grant(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|json| {
            json.get("error")?
                .as_str()
                .map(|error| error == "invalid_grant")
        })
        .unwrap_or(false)
}

/// Whether a body read failed on a timeout. reqwest wraps its own timeout in
/// an `io::Error` of kind `Other`, so the chain is walked for it. The error
/// is only inspected, never formatted, since reqwest's text names the URL.
fn timed_out(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<reqwest::Error>()
            && error.is_timeout()
        {
            return true;
        }
        if let Some(error) = error.downcast_ref::<io::Error>() {
            if error.kind() == io::ErrorKind::TimedOut {
                return true;
            }
            // An io::Error's own source skips the error it wraps.
            if let Some(inner) = error.get_ref()
                && timed_out(inner)
            {
                return true;
            }
        }
        current = error.source();
    }
    false
}

#[cfg(test)]
mod tests;
