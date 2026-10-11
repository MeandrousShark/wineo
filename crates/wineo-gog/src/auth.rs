//! GOG's OAuth flow, as the Galaxy desktop client does it. GOG has no public
//! client registration, so every Linux GOG tool (heroic-gogdl, Minigalaxy,
//! lgogdownloader) uses Galaxy's public client credentials.

use std::fmt;
use std::str::FromStr;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Endpoint, Error, ErrorKind};
use crate::json;

pub const CLIENT_ID: &str = "46899977096215655";
// Public, but still kept out of URLs that get shown and out of errors.
pub(crate) const CLIENT_SECRET: &str =
    "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
pub const REDIRECT_URI: &str = "https://embed.gog.com/on_login_success?origin=client";

const AUTH_BASE: &str = "https://auth.gog.com";

/// GOG's login page. After logging in, the browser lands on `REDIRECT_URI`
/// with a `code` added, and the user pastes that address back.
pub fn login_url() -> String {
    login_url_at(&Url::parse(AUTH_BASE).expect("the auth base is a URL"))
}

pub(crate) fn login_url_at(auth: &Url) -> String {
    let mut url = auth.clone();
    url.path_segments_mut()
        .expect("the auth base can have a path")
        .pop_if_empty()
        .push("auth");
    url.query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("response_type", "code")
        .append_pair("layout", "client2");
    url.into()
}

/// A string that must never be shown: `Debug` is redacted and there is no
/// `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// The one-time code from a login, traded for tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthCode(Secret);

impl AuthCode {
    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl fmt::Debug for AuthCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthCode(<redacted>)")
    }
}

/// Why pasted text isn't a login. Never carries the text itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasteError {
    #[error("Nothing was pasted.")]
    Empty,
    #[error("That isn't the address GOG's login ended on, or a login code.")]
    NotALogin,
    #[error("That address has no login code. Log in again and paste the address you land on.")]
    NoCode,
}

const CODE_LEN: std::ops::RangeInclusive<usize> = 8..=4096;

/// Reads what the user pasted after logging in: the whole address of GOG's
/// success page, or just its `code`.
pub fn parse_paste(text: &str) -> Result<AuthCode, PasteError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(PasteError::Empty);
    }
    if text.contains("://") {
        let url = Url::parse(text).map_err(|_| PasteError::NotALogin)?;
        let on_success = matches!(url.scheme(), "https" | "http")
            && url.host_str() == Some("embed.gog.com")
            && url.path() == "/on_login_success";
        if !on_success {
            return Err(PasteError::NotALogin);
        }
        let code = url
            .query_pairs()
            .find(|(name, _)| name == "code")
            .map(|(_, code)| code.into_owned())
            .ok_or(PasteError::NoCode)?;
        return if is_code(&code) {
            Ok(AuthCode(Secret(code)))
        } else {
            Err(PasteError::NoCode)
        };
    }
    if is_code(text) {
        Ok(AuthCode(Secret(text.to_owned())))
    } else {
        Err(PasteError::NotALogin)
    }
}

/// GOG's codes are long runs of URL-safe characters.
fn is_code(text: &str) -> bool {
    CODE_LEN.contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte))
}

/// A GOG account's numeric id, which names its cached files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(u64);

impl UserId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for UserId {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}

// A string, as GOG sends it, since 17 digits are past what JSON numbers
// reliably hold in other readers.
impl Serialize for UserId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for UserId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        json::id(&value)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("a GOG user id is a number"))
    }
}

/// What the token endpoint hands back. The access token lasts `expires_in`
/// seconds; the refresh token may change on every renewal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: Secret,
    pub refresh_token: Secret,
    pub expires_in: u64,
    pub user_id: UserId,
}

pub(crate) fn parse_tokens(json: &Value) -> Result<Tokens, Error> {
    let malformed = || Error::new(Endpoint::Token, ErrorKind::Malformed);
    let text = |name: &str| {
        json.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(Secret::new)
            .ok_or_else(malformed)
    };
    Ok(Tokens {
        access_token: text("access_token")?,
        refresh_token: text("refresh_token")?,
        expires_in: json
            .get("expires_in")
            .and_then(json::id)
            .ok_or_else(malformed)?,
        user_id: json
            .get("user_id")
            .and_then(json::id)
            .map(UserId)
            .ok_or_else(malformed)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "Abc123-_.~SECRETCODE";

    #[test]
    fn the_login_url_is_galaxy_s() {
        assert_eq!(
            login_url(),
            "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=client2"
        );
        assert!(!login_url().contains(CLIENT_SECRET));
    }

    #[test]
    fn a_pasted_address_or_bare_code_gives_the_code() {
        for pasted in [
            format!("https://embed.gog.com/on_login_success?origin=client&code={CODE}"),
            format!("  https://embed.gog.com/on_login_success?code={CODE}&origin=client\n"),
            format!("http://embed.gog.com/on_login_success?origin=client&code={CODE}"),
            CODE.to_owned(),
            format!("\t{CODE} "),
        ] {
            assert_eq!(parse_paste(&pasted).unwrap().expose(), CODE, "{pasted}");
        }
    }

    #[test]
    fn junk_is_refused_without_echoing_it() {
        let cases = [
            ("", PasteError::Empty),
            ("   ", PasteError::Empty),
            ("short", PasteError::NotALogin),
            ("has spaces SECRETCODE in it", PasteError::NotALogin),
            (
                "https://evil.example/on_login_success?code=SECRETCODE1",
                PasteError::NotALogin,
            ),
            (
                "https://embed.gog.com/elsewhere?code=SECRETCODE1",
                PasteError::NotALogin,
            ),
            (
                "ftp://embed.gog.com/on_login_success?code=SECRETCODE1",
                PasteError::NotALogin,
            ),
            (
                "https://embed.gog.com/on_login_success?origin=client",
                PasteError::NoCode,
            ),
            (
                "https://embed.gog.com/on_login_success?code=SE%20CRETCODE1",
                PasteError::NoCode,
            ),
            ("https://[broken SECRETCODE", PasteError::NotALogin),
            (
                "https://auth.gog.com/auth?client_id=46899977096215655&SECRETCODE",
                PasteError::NotALogin,
            ),
        ];
        for (pasted, expected) in cases {
            let error = parse_paste(pasted).unwrap_err();
            assert_eq!(error, expected, "{pasted}");
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains("SECRETCODE"), "{shown}");
            assert!(!shown.contains("evil"), "{shown}");
        }
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let code = parse_paste(CODE).unwrap();
        assert!(!format!("{code:?}").contains("SECRET"));
        let tokens = Tokens {
            access_token: Secret::new("ACCESSSECRET"),
            refresh_token: Secret::new("REFRESHSECRET"),
            expires_in: 3600,
            user_id: UserId(5),
        };
        let shown = format!("{tokens:?} {tokens:#?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("3600"));
    }

    #[test]
    fn user_ids_round_trip_as_strings_and_read_numbers() {
        let id = UserId(48_628_349_957_132_247);
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"48628349957132247\"");
        assert_eq!(
            serde_json::from_str::<UserId>("\"48628349957132247\"").unwrap(),
            id
        );
        assert_eq!(serde_json::from_str::<UserId>("7").unwrap(), UserId(7));
        for bad in ["\"../x\"", "\"\"", "-1", "\"1 2\"", "null"] {
            assert!(serde_json::from_str::<UserId>(bad).is_err(), "{bad}");
        }
    }
}
