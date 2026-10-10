//! Requests to Steam's Web API. Steam takes the key as a query parameter, so
//! the URL is a secret here. Nothing in this module puts it, or the text of
//! any error that could carry it, into an `ApiError`: every failure is
//! classified and given a fixed message instead of being scrubbed afterward.

use std::io::{self, Read};
use std::time::Duration;

use reqwest::Url;
use serde_json::Value;

use crate::download::http_client_following;

pub const BASE: &str = "https://api.steampowered.com";

/// A large library is a few hundred kilobytes.
const MAX_BODY: u64 = 32 * 1024 * 1024;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("can't reach Steam")]
    Connect,
    #[error("Steam didn't answer in time")]
    Timeout,
    #[error("the request to Steam failed")]
    Request,
    /// 401 or 403, which Steam answers for a key it doesn't accept.
    #[error("Steam rejected the Web API key")]
    Rejected,
    #[error("Steam answered with status {0}")]
    Status(u16),
    #[error("Steam's answer couldn't be read")]
    Body,
    #[error("Steam's answer is too large")]
    TooLarge,
    #[error("Steam's answer isn't valid JSON")]
    Json,
}

pub struct SteamApi {
    client: reqwest::blocking::Client,
    base: Url,
    max_body: u64,
}

impl SteamApi {
    /// The real API, over HTTPS, following no redirects.
    pub fn new() -> Result<Self, ApiError> {
        Self::build(BASE, true, TIMEOUT)
    }

    /// The real API, except that debug builds send requests to the plain HTTP
    /// server named by `WINEO_TEST_STEAM_API`, so CLI tests can stand in for
    /// Steam. Release builds never look at the variable.
    pub fn from_env() -> Result<Self, ApiError> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var("WINEO_TEST_STEAM_API")
            .ok()
            .filter(|base| !base.is_empty())
        {
            return Self::build(&base, false, TIMEOUT);
        }
        Self::new()
    }

    /// A fixture server standing in for Steam, which may be plain HTTP.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_base(base: &str) -> Result<Self, ApiError> {
        Self::build(base, false, TIMEOUT)
    }

    fn build(base: &str, https_only: bool, timeout: Duration) -> Result<Self, ApiError> {
        let client = http_client_following(https_only, timeout, reqwest::redirect::Policy::none())
            .map_err(|_| ApiError::Request)?;
        Ok(Self {
            client,
            base: Url::parse(base).map_err(|_| ApiError::Request)?,
            max_body: MAX_BODY,
        })
    }

    /// GETs `path` (like `IPlayerService/GetOwnedGames/v1/`) with the key and
    /// `params`, and returns the JSON body of a 200 answer.
    pub fn get_json(
        &self,
        path: &str,
        params: &[(&str, &str)],
        key: &str,
    ) -> Result<Value, ApiError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| ApiError::Request)?
            .pop_if_empty()
            .extend(path.split('/').filter(|part| !part.is_empty()));
        // Steam wants the trailing slash on interface paths.
        if path.ends_with('/') {
            url.path_segments_mut()
                .map_err(|()| ApiError::Request)?
                .push("");
        }
        url.query_pairs_mut()
            .append_pair("key", key)
            .extend_pairs(params);

        let response = self.client.get(url).send().map_err(|error| {
            if error.is_timeout() {
                ApiError::Timeout
            } else if error.is_connect() {
                ApiError::Connect
            } else {
                ApiError::Request
            }
        })?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(ApiError::Rejected),
            status => return Err(ApiError::Status(status)),
        }
        let mut body = Vec::new();
        response
            .take(self.max_body + 1)
            .read_to_end(&mut body)
            .map_err(|error| {
                if timed_out(&error) {
                    ApiError::Timeout
                } else {
                    ApiError::Body
                }
            })?;
        if body.len() as u64 > self.max_body {
            return Err(ApiError::TooLarge);
        }
        serde_json::from_slice(&body).map_err(|_| ApiError::Json)
    }
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
pub(crate) mod fixture {
    //! A local HTTP server that answers each connection with the next canned
    //! reply and records the request lines it saw.

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub struct Server {
        pub base: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    /// A complete reply with a JSON body.
    pub fn reply(status: u16, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    impl Server {
        /// Serves `replies` in order, one per connection, then stops.
        pub fn start(replies: Vec<Vec<u8>>) -> Self {
            Self::start_stalling(replies, Duration::ZERO)
        }

        /// Like `start`, but holds each connection open for `stall` after
        /// writing the reply, so a short client timeout can run out.
        pub fn start_stalling(replies: Vec<Vec<u8>>, stall: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            std::thread::spawn(move || {
                for reply in replies {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut head = Vec::new();
                    let mut chunk = [0; 1024];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => head.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let line = String::from_utf8_lossy(&head);
                    seen.lock()
                        .unwrap()
                        .push(line.lines().next().unwrap_or_default().to_owned());
                    let _ = stream.write_all(&reply);
                    let _ = stream.flush();
                    std::thread::sleep(stall);
                }
            });
            Self { base, requests }
        }

        /// The request lines received so far.
        pub fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    /// A base URL nothing listens on.
    pub fn refused() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{Server, reply};
    use super::*;

    const KEY: &str = "SECRETKEY0123456789";
    const PATH: &str = "IPlayerService/GetOwnedGames/v1/";

    fn fetch(base: &str) -> Result<Value, ApiError> {
        SteamApi::with_base(base)
            .unwrap()
            .get_json(PATH, &[("steamid", "7656")], KEY)
    }

    /// Every layer's message, as a person would see it.
    fn shown(error: &ApiError) -> String {
        format!("{error:#} {error:?} {}", anyhow::Error::from(error.clone()))
    }

    fn assert_no_key(error: &ApiError) {
        let text = shown(error);
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains("key="), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
    }

    #[test]
    fn the_request_carries_the_key_and_parameters() {
        let server = Server::start(vec![reply(200, r#"{"response":{}}"#)]);
        assert_eq!(
            fetch(&server.base).unwrap(),
            serde_json::json!({"response": {}})
        );
        assert_eq!(
            server.requests(),
            [format!(
                "GET /IPlayerService/GetOwnedGames/v1/?key={KEY}&steamid=7656 HTTP/1.1"
            )]
        );
    }

    #[test]
    fn rejections_carry_no_key() {
        for status in [401, 403] {
            let server = Server::start(vec![reply(status, "Unauthorized")]);
            let error = fetch(&server.base).unwrap_err();
            assert_eq!(error, ApiError::Rejected);
            assert_no_key(&error);
        }
    }

    #[test]
    fn other_statuses_carry_no_key() {
        for status in [302, 404, 429, 500] {
            let server = Server::start(vec![reply(status, "no")]);
            let error = fetch(&server.base).unwrap_err();
            assert_eq!(error, ApiError::Status(status));
            assert_no_key(&error);
        }
    }

    #[test]
    fn a_refused_connection_carries_no_key() {
        let error = fetch(&super::fixture::refused()).unwrap_err();
        assert_eq!(error, ApiError::Connect);
        assert_no_key(&error);
    }

    #[test]
    fn malformed_json_carries_no_key() {
        let server = Server::start(vec![reply(200, &format!("{{\"echo\": \"{KEY}\" oops"))]);
        let error = fetch(&server.base).unwrap_err();
        assert_eq!(error, ApiError::Json);
        assert_no_key(&error);
    }

    #[test]
    fn a_body_cut_short_carries_no_key() {
        let cut =
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"res".to_vec();
        let server = Server::start(vec![cut]);
        let error = fetch(&server.base).unwrap_err();
        assert_eq!(error, ApiError::Body);
        assert_no_key(&error);
    }

    #[test]
    fn a_redirect_is_not_followed() {
        let target = Server::start(vec![reply(200, "{}")]);
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: {}/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            target.base
        );
        let server = Server::start(vec![redirect.into_bytes()]);
        let error = fetch(&server.base).unwrap_err();
        assert_eq!(error, ApiError::Status(302));
        assert!(target.requests().is_empty());
    }

    #[test]
    fn the_production_client_refuses_plain_http() {
        let api = SteamApi::build("http://127.0.0.1:9", true, TIMEOUT).unwrap();
        let error = api.get_json(PATH, &[], KEY).unwrap_err();
        assert_no_key(&error);
    }

    fn quick(base: &str) -> SteamApi {
        SteamApi::build(base, false, Duration::from_millis(150)).unwrap()
    }

    #[test]
    fn a_stalled_header_is_a_timeout() {
        // Nothing is ever written, and the connection stays open.
        let server = Server::start_stalling(vec![Vec::new()], Duration::from_secs(2));
        let error = quick(&server.base).get_json(PATH, &[], KEY).unwrap_err();
        assert_eq!(error, ApiError::Timeout);
        assert_no_key(&error);
    }

    #[test]
    fn a_stalled_body_is_a_timeout() {
        let cut = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"res".to_vec();
        let server = Server::start_stalling(vec![cut], Duration::from_secs(2));
        let error = quick(&server.base).get_json(PATH, &[], KEY).unwrap_err();
        assert_eq!(error, ApiError::Timeout);
        assert_no_key(&error);
    }

    #[test]
    fn an_oversized_answer_is_too_large() {
        let server = Server::start(vec![reply(
            200,
            &format!("{{\"pad\": \"{}\"}}", "x".repeat(64)),
        )]);
        let mut api = SteamApi::with_base(&server.base).unwrap();
        api.max_body = 32;
        let error = api.get_json(PATH, &[], KEY).unwrap_err();
        assert_eq!(error, ApiError::TooLarge);
        assert_no_key(&error);
    }
}
