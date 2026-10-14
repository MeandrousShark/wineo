//! The SteamGridDB API key: where it is kept, how it is found, and checking
//! it against the API.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use serde_json::Value;

use crate::art::{Format, Kind, MAX_BYTES};
use crate::cancel::Token;
use crate::download::Transport;
use crate::error::{Error, Result};
use crate::key_store::{self, KeySpec};
use crate::paths::Paths;

pub use crate::key_store::{Key, KeySource, Keyring, KeyringError, NoKeyring, Removed, Saved};

pub const API: &str = "https://www.steamgriddb.com/api/v2";

/// Overrides every saved key, for scripts and CI.
pub const KEY_VAR: &str = "WINEO_STEAMGRIDDB_KEY";

/// SteamGridDB's key: where it is kept and how it is named.
pub const KEY_SPEC: KeySpec = KeySpec {
    env_var: Some(KEY_VAR),
    account: "steamgriddb",
    file_name: "steamgriddb.key",
    label: "wineo steamgriddb key",
};

/// The plain text key file, always on this machine, never in a portable folder.
pub fn key_file(paths: &Paths) -> PathBuf {
    key_store::key_file(&KEY_SPEC, paths)
}

/// Looks for the key in `WINEO_STEAMGRIDDB_KEY`, then the keyring, then the
/// key file. An empty value counts as unset.
pub fn find_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Option<Key>> {
    key_store::find_key(&KEY_SPEC, paths, lookup, keyring)
}

/// Like `find_key`, but a missing key is an error that says how to add one.
pub fn require_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Key> {
    find_key(paths, lookup, keyring)?.ok_or(Error::NoSteamGridDbKey)
}

/// Saves the key to the keyring, or to the `0600` key file when there is no
/// keyring. A key file left from before is removed once the keyring has the
/// key, so no stale copy stays in plain text.
pub fn save_key(paths: &Paths, keyring: &dyn Keyring, key: &str) -> Result<Saved> {
    key_store::save_key(&KEY_SPEC, paths, keyring, key)
}

/// Deletes the key from the keyring and the key file.
pub fn remove_key(paths: &Paths, keyring: &dyn Keyring) -> Result<Removed> {
    key_store::remove_key(&KEY_SPEC, paths, keyring)
}

/// Whether SteamGridDB accepts `key`, from one authenticated request.
pub fn check_key(transport: &dyn Transport, key: &str) -> anyhow::Result<bool> {
    let response = transport.request(&format!("{API}/search/autocomplete/wineo"), Some(key))?;
    match response.status {
        200 => Ok(true),
        401 | 403 => Ok(false),
        status => bail!("SteamGridDB answered with status {status}"),
    }
}

/// A game from a search, with only what is guaranteed plus the year when the
/// result has a release date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub id: u64,
    pub name: String,
    pub year: Option<i16>,
}

/// An image from a game's list. Only `id` and `url` are guaranteed; anything
/// else missing is unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Image {
    pub id: u64,
    pub url: String,
    pub thumb: Option<String>,
    pub author: Option<String>,
    pub style: Option<String>,
    pub nsfw: bool,
    pub humor: bool,
    pub animated: bool,
    pub score: Option<i64>,
    pub official: bool,
    pub language: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// The format `mime` or the URL suggests, for ranking only. What is kept
    /// is decided by the downloaded bytes.
    pub format: Option<Format>,
}

/// Which filters an image list request carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filters {
    /// Static only, no NSFW or humor, and portrait grids: what ranking picks from.
    Ranked,
    /// Nothing filtered out, for an image the user named.
    None,
}

/// The most result pages fetch walks for one kind.
pub const MAX_PAGES: u32 = 10;

/// Portrait posters, the only grids fetch asks for.
const GRID_DIMENSIONS: &str = "600x900,342x482,660x930";

/// SteamGridDB, reached through `transport` with `key`.
pub struct Api<'a> {
    transport: &'a dyn Transport,
    key: &'a str,
}

impl<'a> Api<'a> {
    pub fn new(transport: &'a dyn Transport, key: &'a str) -> Self {
        Self { transport, key }
    }

    /// Games whose names match `term`.
    pub fn search(&self, term: &str) -> anyhow::Result<Vec<Game>> {
        let mut url = reqwest::Url::parse(API)?;
        url.path_segments_mut()
            .map_err(|()| anyhow!("bad API URL"))?
            .extend(["search", "autocomplete", term]);
        Ok(self.data(url.as_str())?.iter().filter_map(game).collect())
    }

    /// Looks up one game by its SteamGridDB ID.
    pub fn game(&self, id: u64) -> anyhow::Result<Option<Game>> {
        let url = format!("{API}/games/id/{id}");
        let response = self.transport.request(&url, Some(self.key))?;
        match response.status {
            200 => Ok(Self::parse_answer(response.body)?
                .get("data")
                .and_then(game)),
            404 => Ok(None),
            401 | 403 => bail!("SteamGridDB rejected the key"),
            status => bail!("SteamGridDB answered with status {status}"),
        }
    }

    /// Looks up one game by its Steam app ID.
    pub fn steam_game(&self, app_id: u64) -> anyhow::Result<Option<Game>> {
        let url = format!("{API}/games/steam/{app_id}");
        let response = self.transport.request(&url, Some(self.key))?;
        match response.status {
            200 => Ok(Self::parse_answer(response.body)?
                .get("data")
                .and_then(game)),
            404 => Ok(None),
            401 | 403 => bail!("SteamGridDB rejected the key"),
            status => bail!("SteamGridDB answered with status {status}"),
        }
    }

    /// The URL of one page of a game's images of `kind`, with or without the
    /// hard filters.
    pub fn images_url(game: u64, kind: Kind, page: u32, filters: Filters) -> String {
        let path = match kind {
            Kind::Icon => "icons",
            Kind::Grid => "grids",
            Kind::Hero => "heroes",
            Kind::Logo => "logos",
        };
        let query = match (filters, kind) {
            (Filters::Ranked, Kind::Grid) => {
                format!("types=static&nsfw=false&humor=false&dimensions={GRID_DIMENSIONS}")
            }
            (Filters::Ranked, _) => "types=static&nsfw=false&humor=false".to_owned(),
            // Every value spelled out, so no API default can hide an image.
            (Filters::None, _) => {
                "types=static,animated&nsfw=any&humor=any&epilepsy=any".to_owned()
            }
        };
        format!("{API}/{path}/game/{game}?{query}&page={page}")
    }

    /// Every image of `kind` for `game`, walking pages until one is empty, up
    /// to `MAX_PAGES`. An image seen on an earlier page is not repeated.
    pub fn images(&self, game: u64, kind: Kind, filters: Filters) -> anyhow::Result<Vec<Image>> {
        self.images_with_cancel(game, kind, filters, &Token::new())
    }

    /// `images`, stopping between pages once `cancel` is canceled.
    pub fn images_with_cancel(
        &self,
        game: u64,
        kind: Kind,
        filters: Filters,
        cancel: &Token,
    ) -> anyhow::Result<Vec<Image>> {
        let mut images: Vec<Image> = Vec::new();
        // Pages start at 0, as SteamGridDB's own Decky plugin requests them.
        for page in 0..MAX_PAGES {
            check_download_cancel(cancel)?;
            let data = self.data(&Self::images_url(game, kind, page, filters))?;
            if data.is_empty() {
                break;
            }
            for found in data.iter().filter_map(image) {
                if !images.iter().any(|seen| seen.id == found.id) {
                    images.push(found);
                }
            }
        }
        Ok(images)
    }

    /// Downloads an image, abandoning it past the art size cap. The key is
    /// not sent, since images come from a CDN rather than the API.
    pub fn download(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        self.download_with_progress(url, |_, _| {}, &Token::new())
    }

    /// Keeps fetch callers simple while allowing the picker to show progress.
    pub fn download_with_progress(
        &self,
        url: &str,
        progress: impl FnMut(u64, Option<u64>),
        cancel: &Token,
    ) -> anyhow::Result<Vec<u8>> {
        download_image(self.transport, url, progress, cancel)
    }

    /// The `data` array of an API answer.
    fn data(&self, url: &str) -> anyhow::Result<Vec<Value>> {
        let response = self.transport.request(url, Some(self.key))?;
        match response.status {
            200 => {}
            401 | 403 => bail!("SteamGridDB rejected the key"),
            status => bail!("SteamGridDB answered with status {status}"),
        }
        let answer = Self::parse_answer(response.body)?;
        Ok(match answer.get("data") {
            Some(Value::Array(data)) => data.clone(),
            _ => Vec::new(),
        })
    }

    fn parse_answer(body: impl Read) -> anyhow::Result<Value> {
        let answer: Value = serde_json::from_reader(body)
            .map_err(|err| anyhow!("SteamGridDB sent an unreadable answer: {err}"))?;
        if answer.get("success").and_then(Value::as_bool) == Some(false) {
            let errors: Vec<&str> = answer
                .get("errors")
                .and_then(Value::as_array)
                .map(|errors| errors.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            bail!("SteamGridDB refused the request: {}", errors.join(", "));
        }
        Ok(answer)
    }
}

/// An image download the server answered with something but 200, kept typed
/// so callers can tell a failing server from a missing file.
#[derive(Debug, thiserror::Error)]
#[error("the download answered with status {0}")]
pub(crate) struct DownloadStatus(pub u16);

/// An image download whose body broke off partway.
#[derive(Debug, thiserror::Error)]
#[error("failed to download {url}: {error}")]
pub(crate) struct DownloadRead {
    url: String,
    // Not named `source`: thiserror would chain it, and `{:#}` would print
    // the reason twice.
    error: io::Error,
}

/// Downloads an image through `transport`, abandoning it past the art size
/// cap. No key is sent, since images come from a CDN rather than an API.
pub(crate) fn download_image(
    transport: &dyn Transport,
    url: &str,
    mut progress: impl FnMut(u64, Option<u64>),
    cancel: &Token,
) -> anyhow::Result<Vec<u8>> {
    check_download_cancel(cancel)?;
    let (response, total) = transport.request_with_length(url, None)?;
    if response.status != 200 {
        return Err(DownloadStatus(response.status).into());
    }
    if total.is_some_and(|length| length > MAX_BYTES) {
        bail!("{url} is larger than 32 MiB");
    }
    let mut body = response.body;
    let mut bytes = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        check_download_cancel(cancel)?;
        let remaining = MAX_BYTES + 1 - bytes.len() as u64;
        let count = loop {
            match body
                .by_ref()
                .take(remaining.min(buffer.len() as u64))
                .read(&mut buffer)
            {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                    check_download_cancel(cancel)?;
                }
                result => {
                    break result.map_err(|error| DownloadRead {
                        url: url.to_owned(),
                        error,
                    })?;
                }
            }
        };
        if count == 0 {
            check_download_cancel(cancel)?;
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() as u64 > MAX_BYTES {
            bail!("{url} is larger than 32 MiB");
        }
        progress(bytes.len() as u64, total);
    }
    Ok(bytes)
}

fn check_download_cancel(cancel: &Token) -> anyhow::Result<()> {
    if cancel.is_canceled() {
        return Err(crate::error::Error::Canceled("download").into());
    }
    Ok(())
}

fn game(value: &Value) -> Option<Game> {
    let year = value
        .get("release_date")
        .and_then(Value::as_i64)
        .and_then(|seconds| jiff::Timestamp::from_second(seconds).ok())
        .map(|time| time.to_zoned(jiff::tz::TimeZone::UTC).year());
    Some(Game {
        id: value.get("id")?.as_u64()?,
        name: value.get("name")?.as_str()?.to_owned(),
        year,
    })
}

fn image(value: &Value) -> Option<Image> {
    let url = value.get("url")?.as_str()?.to_owned();
    let text = |key| value.get(key).and_then(Value::as_str);
    let style = text("style").map(str::to_owned);
    // Typed as a list by the official wrapper and sent as a string by the API.
    let official = match value.get("style") {
        Some(Value::String(style)) => style == "official",
        Some(Value::Array(styles)) => styles.iter().any(|style| style == "official"),
        _ => false,
    };
    let format = match text("mime") {
        Some("image/png") => Some(Format::Png),
        Some("image/jpeg") => Some(Format::Jpeg),
        Some("image/webp") => Some(Format::Webp),
        Some("image/vnd.microsoft.icon" | "image/x-icon") => Some(Format::Ico),
        _ => Format::ALL.into_iter().find(|format| {
            let path = url.split(['?', '#']).next().unwrap_or_default();
            path.to_ascii_lowercase()
                .ends_with(&format!(".{}", format.extension()))
        }),
    };
    Some(Image {
        id: value.get("id")?.as_u64()?,
        score: value.get("score").and_then(Value::as_i64),
        thumb: text("thumb").map(str::to_owned),
        author: value
            .get("author")
            .and_then(|author| author.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        style,
        nsfw: value.get("nsfw").and_then(Value::as_bool).unwrap_or(false),
        humor: value.get("humor").and_then(Value::as_bool).unwrap_or(false),
        animated: text("type").is_some_and(|kind| kind.eq_ignore_ascii_case("animated"))
            || text("mime").is_some_and(|mime| {
                mime.eq_ignore_ascii_case("image/gif") || mime.eq_ignore_ascii_case("image/apng")
            }),
        official,
        language: text("language").map(str::to_ascii_lowercase),
        width: value.get("width").and_then(Value::as_u64),
        height: value.get("height").and_then(Value::as_u64),
        format,
        url,
    })
}

/// The user's two-letter language from `LC_ALL`, `LC_MESSAGES`, then `LANG`,
/// the first that is set. `C` and `POSIX`, or nothing set at all, mean `en`.
pub fn language(lookup: impl Fn(&str) -> Option<OsString>) -> String {
    let value = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|var| lookup(var).and_then(|value| value.into_string().ok()))
        .find(|value| !value.is_empty());
    let Some(value) = value else {
        return "en".to_owned();
    };
    let code = value.split(['_', '.', '@']).next().unwrap_or_default();
    if code.eq_ignore_ascii_case("C") || code.eq_ignore_ascii_case("POSIX") || code.len() < 2 {
        return "en".to_owned();
    }
    code.to_ascii_lowercase()
}

/// Logo width from which a logo is sharp enough for its display size.
const LOGO_ENOUGH_WIDTH: u64 = 1200;

/// Hero width from which a hero is sharp enough for its display size.
const HERO_ENOUGH_WIDTH: u64 = 1920;

/// The long side beyond which a logo or hero is only used when nothing smaller
/// exists: decoding it costs seconds and hundreds of megabytes of RGBA.
const OVERSIZED_SIDE: u64 = 4096;

/// Sorts images best first by the plan's keys for `kind`. The lowest ID
/// breaks every tie, so the order is total.
///
/// Logos and heroes rank big enough over biggest, not the other way round:
/// decode time and memory grow with pixels while they are shown small, so
/// pixels past the threshold buy nothing. Oversized images go last so they
/// only win when nothing else exists.
pub fn rank(kind: Kind, images: &mut [Image], language: &str) {
    let language_rank = |image: &Image| match image.language.as_deref() {
        Some(found) if found == language => 0,
        Some("en") => 1,
        _ => 2,
    };
    // Highest first, with an unknown value after every known one.
    let high = |value: Option<i64>| (value.is_none(), std::cmp::Reverse(value));
    let area = |image: &Image| {
        high(image.width.zip(image.height).map(|(width, height)| {
            // The API's numbers are untrusted, and an overflow would panic in debug.
            i64::try_from(width.saturating_mul(height)).unwrap_or(i64::MAX)
        }))
    };
    let preferred = |image: &Image, over: Format| match image.format {
        Some(Format::Png) => 0,
        Some(format) if format == over => 1,
        _ => 2,
    };
    let big_enough = |image: &Image| {
        image
            .width
            .zip(image.height)
            .is_some_and(|(width, height)| width.min(height) >= 128)
    };
    // Oversized needs both sides known; big enough below needs only a known width.
    let oversized = |image: &Image| {
        image
            .width
            .zip(image.height)
            .is_some_and(|(width, height)| width.max(height) > OVERSIZED_SIDE)
    };
    let wide_enough = |image: &Image, threshold: u64| image.width.is_some_and(|w| w >= threshold);
    // Area for images below the threshold; every image at or above it ties.
    let capped_area = |image: &Image, threshold: u64| {
        if wide_enough(image, threshold) {
            high(Some(0))
        } else {
            area(image)
        }
    };
    images.sort_by(|a, b| {
        let common = |image: &Image| (high(image.score), area(image), image.id);
        match kind {
            Kind::Icon => (
                !big_enough(a),
                !a.official,
                language_rank(a),
                preferred(a, Format::Ico),
                common(a),
            )
                .cmp(&(
                    !big_enough(b),
                    !b.official,
                    language_rank(b),
                    preferred(b, Format::Ico),
                    common(b),
                )),
            Kind::Grid => (language_rank(a), common(a)).cmp(&(language_rank(b), common(b))),
            Kind::Hero => {
                let key = |image: &Image| {
                    (
                        oversized(image),
                        !wide_enough(image, HERO_ENOUGH_WIDTH),
                        high(image.score),
                        capped_area(image, HERO_ENOUGH_WIDTH),
                        image.id,
                    )
                };
                key(a).cmp(&key(b))
            }
            Kind::Logo => {
                let key = |image: &Image| {
                    (
                        oversized(image),
                        !image.official,
                        language_rank(image),
                        preferred(image, Format::Webp),
                        !wide_enough(image, LOGO_ENOUGH_WIDTH),
                        high(image.score),
                        capped_area(image, LOGO_ENOUGH_WIDTH),
                        image.id,
                    )
                };
                key(a).cmp(&key(b))
            }
        }
    });
}

/// A SteamGridDB stand-in for tests: canned answers by URL, and a record of
/// every request, so nothing ever reaches the network.
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{self, Read};
    use std::rc::Rc;

    use serde_json::{Value, json};

    use crate::download::{Response, Transport};

    type Body = Rc<dyn Fn() -> Box<dyn Read>>;
    type Answer = (u16, Option<u64>, Body);

    #[derive(Default)]
    pub struct FakeApi {
        answers: RefCell<HashMap<String, Answer>>,
        pub seen: RefCell<Vec<(String, Option<String>)>>,
    }

    impl FakeApi {
        pub fn answer(
            &self,
            url: impl Into<String>,
            status: u16,
            body: impl Fn() -> Box<dyn Read> + 'static,
        ) {
            self.answer_with_length(url, status, None, body);
        }

        pub fn answer_with_length(
            &self,
            url: impl Into<String>,
            status: u16,
            content_length: Option<u64>,
            body: impl Fn() -> Box<dyn Read> + 'static,
        ) {
            self.answers
                .borrow_mut()
                .insert(url.into(), (status, content_length, Rc::new(body)));
        }

        pub fn json(&self, url: impl Into<String>, data: Value) {
            let text = json!({ "success": true, "data": data }).to_string();
            self.answer(url, 200, move || {
                Box::new(io::Cursor::new(text.clone().into_bytes()))
            });
        }

        pub fn bytes(&self, url: impl Into<String>, bytes: &[u8]) {
            let bytes = bytes.to_vec();
            let length = bytes.len() as u64;
            self.answer_with_length(url, 200, Some(length), move || {
                Box::new(io::Cursor::new(bytes.clone()))
            });
        }

        pub fn bytes_with_length(&self, url: impl Into<String>, bytes: &[u8], length: u64) {
            let bytes = bytes.to_vec();
            self.answer_with_length(url, 200, Some(length), move || {
                Box::new(io::Cursor::new(bytes.clone()))
            });
        }

        pub fn urls(&self) -> Vec<String> {
            self.seen
                .borrow()
                .iter()
                .map(|(url, _)| url.clone())
                .collect()
        }
    }

    impl Transport for FakeApi {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
            anyhow::bail!("unexpected plain request to {url}")
        }

        fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
            self.answer_request(url, bearer)
                .map(|(response, _)| response)
        }

        fn request_with_length(
            &self,
            url: &str,
            bearer: Option<&str>,
        ) -> anyhow::Result<(Response, Option<u64>)> {
            self.answer_request(url, bearer)
        }
    }

    impl FakeApi {
        fn answer_request(
            &self,
            url: &str,
            bearer: Option<&str>,
        ) -> anyhow::Result<(Response, Option<u64>)> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), bearer.map(str::to_owned)));
            let answers = self.answers.borrow();
            let (status, content_length, body) = answers
                .get(url)
                .ok_or_else(|| anyhow::anyhow!("unexpected request to {url}"))?;
            Ok((
                Response {
                    status: *status,
                    body: body(),
                },
                *content_length,
            ))
        }
    }
}

#[cfg(test)]
mod tests;
