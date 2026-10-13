use std::io::{self, Read};
use std::time::Duration;

use reqwest::Url;
use serde_json::Value;

use crate::art::{self, GameArt};
use crate::auth::{self, AuthCode, CLIENT_ID, CLIENT_SECRET, REDIRECT_URI, Tokens, UserId};
use crate::error::{Endpoint, Error, ErrorKind};
use crate::owned::{self, OwnedPage, OwnedProduct};
use crate::product::{self, Builds, Product, ResolvedLink};
use crate::profile;

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
    pub gameplay: Url,
    pub users: Url,
}

impl Endpoints {
    pub fn production() -> Self {
        let parse = |url: &str| Url::parse(url).expect("production endpoints are URLs");
        Self {
            auth: parse("https://auth.gog.com"),
            embed: parse("https://embed.gog.com"),
            api: parse("https://api.gog.com"),
            content_system: parse("https://content-system.gog.com"),
            gameplay: parse("https://gameplay.gog.com"),
            users: parse("https://users.gog.com"),
        }
    }

    /// Every service under one base, the way a fake server serves them.
    pub fn all_at(base: &str) -> Option<Self> {
        let base = Url::parse(base).ok()?;
        (!base.cannot_be_a_base()).then(|| Self {
            auth: base.clone(),
            embed: base.clone(),
            api: base.clone(),
            content_system: base.clone(),
            gameplay: base.clone(),
            users: base,
        })
    }
}

pub struct Client {
    http: reqwest::blocking::Client,
    /// For GOG's download servers. Their links are signed, so no token ever
    /// goes there, and redirects can be followed.
    pub(crate) cdn: reqwest::blocking::Client,
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
    /// don't have it, so they can only ever talk to GOG over HTTPS. The
    /// `test-server` feature brings it back for other crates' release tests.
    #[cfg(any(test, debug_assertions, feature = "test-server"))]
    pub fn with_base(user_agent: &str, base: &str) -> Result<Self, Error> {
        let endpoints =
            Endpoints::all_at(base).ok_or(Error::new(Endpoint::Setup, ErrorKind::Request))?;
        Self::build(user_agent, endpoints, false, CONNECT_TIMEOUT, READ_TIMEOUT)
    }

    /// `with_base` with each service placed separately, for a test that
    /// needs one of them somewhere else.
    #[cfg(any(test, debug_assertions, feature = "test-server"))]
    pub fn with_endpoints(user_agent: &str, endpoints: Endpoints) -> Result<Self, Error> {
        Self::build(user_agent, endpoints, false, CONNECT_TIMEOUT, READ_TIMEOUT)
    }

    pub(crate) fn build(
        user_agent: &str,
        endpoints: Endpoints,
        https_only: bool,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, Error> {
        let builder = || {
            reqwest::blocking::Client::builder()
                .https_only(https_only)
                .connect_timeout(connect_timeout)
                .timeout(read_timeout)
                .user_agent(user_agent)
        };
        let setup = |_| Error::new(Endpoint::Setup, ErrorKind::Request);
        let http = builder()
            // Every endpoint answers directly. A redirect could carry the
            // bearer token or the client secret somewhere unexpected.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(setup)?;
        let cdn = builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            // The signed link works like a password while it lasts.
            .referer(false)
            .build()
            .map_err(setup)?;
        Ok(Self {
            http,
            cdn,
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
        owned::collect_pages(|page| self.owned_page(access_token, page))
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

    /// How long an account has played a game, in minutes. None when GOG has
    /// no sessions for it, which it signals with a 404. This only reads: GOG's
    /// own clients post sessions to the same path, and wineo never does.
    pub fn playtime(
        &self,
        access_token: &str,
        user_id: UserId,
        game_id: u64,
    ) -> Result<Option<u64>, Error> {
        let url = join(
            &self.endpoints.gameplay,
            &[
                "games",
                &game_id.to_string(),
                "users",
                &user_id.to_string(),
                "sessions",
            ],
            Endpoint::Playtime,
        )?;
        // Not official documentation, so a missing or odd total is malformed
        // rather than guessed at.
        let Some(body) = self.fetch_json(Endpoint::Playtime, url, Some(access_token), true)? else {
            return Ok(None);
        };
        // Minutes are a JSON integer. Unlike ids, a string isn't accepted.
        body.get("time_sum")
            .and_then(Value::as_u64)
            .map(Some)
            .ok_or_else(|| Error::new(Endpoint::Playtime, ErrorKind::Malformed))
    }

    /// A game's artwork links. Public, so no token is sent.
    pub fn game_art(&self, id: u64) -> Result<GameArt, Error> {
        let mut url = join(
            &self.endpoints.api,
            &["v2", "games", &id.to_string()],
            Endpoint::GameArt,
        )?;
        url.query_pairs_mut().append_pair("locale", "en-US");
        let json = self.get_json(Endpoint::GameArt, url, None)?;
        art::parse_game_art(&json, id)
    }

    /// The public name on an account's profile. Public, so no token is sent.
    pub fn username(&self, user_id: UserId) -> Result<String, Error> {
        let url = join(
            &self.endpoints.users,
            &["users", &user_id.to_string()],
            Endpoint::Profile,
        )?;
        let json = self.get_json(Endpoint::Profile, url, None)?;
        profile::parse_username(&json)
    }

    fn get_json(&self, endpoint: Endpoint, url: Url, bearer: Option<&str>) -> Result<Value, Error> {
        self.fetch_json(endpoint, url, bearer, false)?
            .ok_or_else(|| Error::new(endpoint, ErrorKind::Status(404)))
    }

    /// Like `get_json`, except that a 404 is None when `missing_ok`.
    fn fetch_json(
        &self,
        endpoint: Endpoint,
        url: Url,
        bearer: Option<&str>,
        missing_ok: bool,
    ) -> Result<Option<Value>, Error> {
        let fail = |kind| Error::new(endpoint, kind);
        let mut request = self.http.get(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().map_err(|error| fail(send_error(&error)))?;
        let status = response.status().as_u16();
        if status == 404 && missing_ok {
            return Ok(None);
        }
        if status != 200 {
            // Only the token endpoint's 400 says anything useful in its body.
            let body = (endpoint == Endpoint::Token && status == 400)
                .then(|| self.read_body(response).ok())
                .flatten();
            return Err(fail(status_kind(endpoint, status, body.as_deref())));
        }
        let body = self.read_body(response).map_err(fail)?;
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|_| fail(ErrorKind::Json))
    }

    fn read_body(&self, response: reqwest::blocking::Response) -> Result<Vec<u8>, ErrorKind> {
        read_limited(response, self.max_body)
    }
}

/// Reads a whole body of at most `limit` bytes.
pub(crate) fn read_limited(
    response: reqwest::blocking::Response,
    limit: u64,
) -> Result<Vec<u8>, ErrorKind> {
    let mut body = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|error| read_error(&error))?;
    if body.len() as u64 > limit {
        return Err(ErrorKind::TooLarge);
    }
    Ok(body)
}

/// Classifies a request that got no answer. The error is only inspected.
pub(crate) fn send_error(error: &reqwest::Error) -> ErrorKind {
    if error.is_timeout() {
        ErrorKind::Timeout
    } else if error.is_connect() {
        ErrorKind::Connect
    } else {
        ErrorKind::Request
    }
}

/// Classifies a failed body read.
pub(crate) fn read_error(error: &io::Error) -> ErrorKind {
    if timed_out(error) {
        ErrorKind::Timeout
    } else {
        ErrorKind::Body
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
pub(crate) fn status_kind(endpoint: Endpoint, status: u16, body: Option<&[u8]>) -> ErrorKind {
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
