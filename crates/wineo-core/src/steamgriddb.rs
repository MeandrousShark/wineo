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
        let mut images: Vec<Image> = Vec::new();
        // Pages start at 0, as SteamGridDB's own Decky plugin requests them.
        for page in 0..MAX_PAGES {
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
        mut progress: impl FnMut(u64, Option<u64>),
        cancel: &Token,
    ) -> anyhow::Result<Vec<u8>> {
        Self::check_download_cancel(cancel)?;
        let (response, total) = self.transport.request_with_length(url, None)?;
        if response.status != 200 {
            bail!("the download answered with status {}", response.status);
        }
        if total.is_some_and(|length| length > MAX_BYTES) {
            bail!("{url} is larger than 32 MiB");
        }
        let mut body = response.body;
        let mut bytes = Vec::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            Self::check_download_cancel(cancel)?;
            let remaining = MAX_BYTES + 1 - bytes.len() as u64;
            let count = loop {
                match body
                    .by_ref()
                    .take(remaining.min(buffer.len() as u64))
                    .read(&mut buffer)
                {
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                        Self::check_download_cancel(cancel)?;
                    }
                    result => {
                        break result.map_err(|err| anyhow!("failed to download {url}: {err}"))?;
                    }
                }
            };
            if count == 0 {
                Self::check_download_cancel(cancel)?;
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
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::download::Response;

    /// A keyring in memory, or one that is unavailable or broken.
    struct FakeKeyring {
        key: RefCell<Option<String>>,
        error: Option<KeyringError>,
    }

    impl FakeKeyring {
        fn with(key: Option<&str>) -> Self {
            Self {
                key: RefCell::new(key.map(str::to_owned)),
                error: None,
            }
        }

        fn failing(error: KeyringError) -> Self {
            Self {
                key: RefCell::new(None),
                error: Some(error),
            }
        }

        fn check(&self) -> Result<(), KeyringError> {
            self.error.clone().map_or(Ok(()), Err)
        }
    }

    impl Keyring for FakeKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            self.check()?;
            Ok(self.key.borrow().clone())
        }

        fn set(&self, key: &str) -> Result<(), KeyringError> {
            self.check()?;
            *self.key.borrow_mut() = Some(key.to_owned());
            Ok(())
        }

        fn delete(&self) -> Result<bool, KeyringError> {
            self.check()?;
            Ok(self.key.borrow_mut().take().is_some())
        }
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&root.path().join("usb")),
        )
        .unwrap();
        (root, paths)
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), OsString::from(value)))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn found(paths: &Paths, env: &[(&str, &str)], keyring: &dyn Keyring) -> Option<Key> {
        find_key(paths, vars(env), keyring).unwrap()
    }

    #[test]
    fn key_debug_redacts_the_value() {
        let key = Key {
            value: "private-key-value".to_owned(),
            source: KeySource::Keyring,
        };

        let debug = format!("{key:?}");
        assert!(!debug.contains(&key.value));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("Keyring"));
    }

    #[test]
    fn the_key_file_is_on_the_host_even_in_a_portable_folder() {
        let (root, paths) = setup();
        assert!(paths.portable_root().is_some());
        assert_eq!(
            key_file(&paths),
            root.path().join(".local/share/wineo/steamgriddb.key")
        );
    }

    #[test]
    fn lookup_tries_the_variable_then_the_keyring_then_the_file() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "from-file\n").unwrap();
        let keyring = FakeKeyring::with(Some("from-keyring"));
        let key = |value: &str, source| {
            Some(Key {
                value: value.to_owned(),
                source,
            })
        };

        assert_eq!(
            found(&paths, &[(KEY_VAR, " from-var ")], &keyring),
            key("from-var", KeySource::Variable)
        );
        assert_eq!(
            found(&paths, &[(KEY_VAR, "")], &keyring),
            key("from-keyring", KeySource::Keyring)
        );
        assert_eq!(
            found(&paths, &[], &FakeKeyring::with(None)),
            key("from-file", KeySource::File)
        );
        assert_eq!(
            found(
                &paths,
                &[],
                &FakeKeyring::failing(KeyringError::Unavailable)
            ),
            key("from-file", KeySource::File)
        );
        let err = find_key(
            &paths,
            vars(&[]),
            &FakeKeyring::failing(KeyringError::Failed("locked".into())),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Keyring(_)), "{err}");
    }

    #[test]
    fn a_missing_key_says_how_to_add_one() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "\n").unwrap();
        assert_eq!(found(&paths, &[], &FakeKeyring::with(Some(" "))), None);
        let err = require_key(&paths, vars(&[]), &NoKeyring).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no SteamGridDB key, run `wineo art key` or set WINEO_STEAMGRIDDB_KEY"
        );
    }

    #[test]
    fn saving_prefers_the_keyring_and_drops_a_stale_file() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "old\n").unwrap();
        let keyring = FakeKeyring::with(None);
        assert_eq!(save_key(&paths, &keyring, "new").unwrap(), Saved::Keyring);
        assert_eq!(keyring.key.borrow().as_deref(), Some("new"));
        assert!(!file.exists());

        let err = save_key(
            &paths,
            &FakeKeyring::failing(KeyringError::Failed("denied".into())),
            "new",
        )
        .unwrap_err();
        assert!(matches!(err, Error::Keyring(_)), "{err}");
        assert!(!file.exists());
    }

    #[test]
    fn without_a_keyring_the_key_file_is_private() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "old\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            save_key(&paths, &NoKeyring, "abc123").unwrap(),
            Saved::File(file.clone())
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "abc123\n");
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(
            found(&paths, &[], &NoKeyring).unwrap().value,
            "abc123".to_owned()
        );
    }

    #[test]
    fn removal_deletes_from_both() {
        let (_root, paths) = setup();
        save_key(&paths, &NoKeyring, "abc").unwrap();
        let keyring = FakeKeyring::with(Some("abc"));
        assert_eq!(
            remove_key(&paths, &keyring).unwrap(),
            Removed {
                keyring: true,
                file: true
            }
        );
        assert_eq!(
            remove_key(&paths, &keyring).unwrap(),
            Removed {
                keyring: false,
                file: false
            }
        );
        assert_eq!(
            remove_key(&paths, &NoKeyring).unwrap(),
            Removed {
                keyring: false,
                file: false
            }
        );
    }

    struct Answer {
        status: u16,
        seen: RefCell<Vec<(String, String)>>,
    }

    impl Transport for Answer {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
            bail!("unexpected unauthenticated request to {url}")
        }

        fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), bearer.unwrap_or_default().to_owned()));
            Ok(Response {
                status: self.status,
                body: Box::new(io::empty()),
            })
        }
    }

    #[test]
    fn a_key_is_checked_with_one_authenticated_request() {
        let answer = |status| Answer {
            status,
            seen: RefCell::new(Vec::new()),
        };
        let ok = answer(200);
        assert!(check_key(&ok, "abc").unwrap());
        assert_eq!(
            ok.seen.into_inner(),
            [(format!("{API}/search/autocomplete/wineo"), "abc".to_owned())]
        );
        assert!(!check_key(&answer(401), "abc").unwrap());
        assert!(!check_key(&answer(403), "abc").unwrap());
        let err = check_key(&answer(500), "abc").unwrap_err();
        assert_eq!(err.to_string(), "SteamGridDB answered with status 500");
    }

    use super::fake::FakeApi;
    use serde_json::json;

    fn image_json(id: u64) -> serde_json::Value {
        json!({ "id": id, "url": format!("https://cdn.example/{id}.png") })
    }

    #[test]
    fn search_is_lenient_and_keeps_the_year_when_there_is_one() {
        let api = FakeApi::default();
        api.json(
            format!("{API}/search/autocomplete/Hades%20II"),
            json!([
                { "id": 1, "name": "Hades II", "release_date": 1_714_521_600, "verified": true },
                { "id": 2, "name": "Hades II Demo" },
                { "id": 3, "name": "No date", "release_date": null, "types": ["steam"] },
                { "name": "no id" },
                { "id": 4 },
            ]),
        );
        let games = Api::new(&api, "k").search("Hades II").unwrap();
        assert_eq!(
            games,
            [
                Game {
                    id: 1,
                    name: "Hades II".into(),
                    year: Some(2024)
                },
                Game {
                    id: 2,
                    name: "Hades II Demo".into(),
                    year: None
                },
                Game {
                    id: 3,
                    name: "No date".into(),
                    year: None
                },
            ]
        );
        assert_eq!(api.seen.borrow()[0].1.as_deref(), Some("k"));
    }

    #[test]
    fn game_looks_up_a_game_by_id() {
        let api = FakeApi::default();
        api.json(
            format!("{API}/games/id/1"),
            json!({
                "id": 1,
                "name": "Hades II",
                "release_date": 1_714_521_600,
                "verified": true
            }),
        );

        assert_eq!(
            Api::new(&api, "k").game(1).unwrap(),
            Some(Game {
                id: 1,
                name: "Hades II".into(),
                year: Some(2024),
            })
        );
        assert_eq!(api.urls(), [format!("{API}/games/id/1")]);
        assert_eq!(api.seen.borrow()[0].1.as_deref(), Some("k"));
    }

    #[test]
    fn game_returns_none_when_the_id_is_not_found() {
        let api = FakeApi::default();
        api.answer(format!("{API}/games/id/404"), 404, || Box::new(io::empty()));
        assert_eq!(Api::new(&api, "k").game(404).unwrap(), None);

        let api = FakeApi::default();
        api.json(format!("{API}/games/id/404"), Value::Null);
        assert_eq!(Api::new(&api, "k").game(404).unwrap(), None);
    }

    #[test]
    fn game_reports_a_rejected_key() {
        let api = FakeApi::default();
        api.answer(format!("{API}/games/id/1"), 401, || Box::new(io::empty()));

        let err = Api::new(&api, "k").game(1).unwrap_err();
        assert_eq!(err.to_string(), "SteamGridDB rejected the key");
    }

    #[test]
    fn steam_game_looks_up_a_game_by_steam_app_id() {
        let api = FakeApi::default();
        api.json(
            format!("{API}/games/steam/1086940"),
            json!({
                "id": 123,
                "name": "Baldur's Gate 3",
                "release_date": 1_660_780_800
            }),
        );

        assert_eq!(
            Api::new(&api, "k").steam_game(1086940).unwrap(),
            Some(Game {
                id: 123,
                name: "Baldur's Gate 3".into(),
                year: Some(2022),
            })
        );
        assert_eq!(api.urls(), [format!("{API}/games/steam/1086940")]);
        assert_eq!(api.seen.borrow()[0].1.as_deref(), Some("k"));
    }

    #[test]
    fn every_request_carries_the_hard_filters() {
        let grid = Api::images_url(9, Kind::Grid, 2, Filters::Ranked);
        assert_eq!(
            grid,
            format!(
                "{API}/grids/game/9?types=static&nsfw=false&humor=false&dimensions=600x900,342x482,660x930&page=2"
            )
        );
        for (kind, path) in [
            (Kind::Icon, "icons"),
            (Kind::Hero, "heroes"),
            (Kind::Logo, "logos"),
        ] {
            assert_eq!(
                Api::images_url(9, kind, 0, Filters::Ranked),
                format!("{API}/{path}/game/9?types=static&nsfw=false&humor=false&page=0")
            );
        }
    }

    #[test]
    fn an_unfiltered_request_leaves_nothing_out() {
        for (kind, path) in [
            (Kind::Icon, "icons"),
            (Kind::Grid, "grids"),
            (Kind::Hero, "heroes"),
            (Kind::Logo, "logos"),
        ] {
            assert_eq!(
                Api::images_url(9, kind, 3, Filters::None),
                format!(
                    "{API}/{path}/game/9?types=static,animated&nsfw=any&humor=any&epilepsy=any&page=3"
                )
            );
        }
    }

    #[test]
    fn pages_are_walked_until_one_is_empty() {
        let api = FakeApi::default();
        api.json(
            Api::images_url(9, Kind::Hero, 0, Filters::Ranked),
            json!([image_json(1), image_json(2)]),
        );
        api.json(
            Api::images_url(9, Kind::Hero, 1, Filters::Ranked),
            json!([image_json(2), image_json(3), { "id": 4 }]),
        );
        api.json(
            Api::images_url(9, Kind::Hero, 2, Filters::Ranked),
            json!([]),
        );
        let images = Api::new(&api, "k")
            .images(9, Kind::Hero, Filters::Ranked)
            .unwrap();
        let ids: Vec<u64> = images.iter().map(|image| image.id).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert_eq!(api.urls().len(), 3);
    }

    #[test]
    fn paging_stops_at_the_page_limit() {
        let api = FakeApi::default();
        for page in 0..MAX_PAGES + 2 {
            api.json(
                Api::images_url(9, Kind::Logo, page, Filters::Ranked),
                json!([image_json(u64::from(page))]),
            );
        }
        let images = Api::new(&api, "k")
            .images(9, Kind::Logo, Filters::Ranked)
            .unwrap();
        assert_eq!(images.len(), MAX_PAGES as usize);
        assert_eq!(api.urls().len(), MAX_PAGES as usize);
    }

    #[test]
    fn a_rejected_key_and_other_failures_are_errors() {
        let api = FakeApi::default();
        api.answer(
            Api::images_url(9, Kind::Icon, 0, Filters::Ranked),
            401,
            || Box::new(io::empty()),
        );
        let err = Api::new(&api, "k")
            .images(9, Kind::Icon, Filters::Ranked)
            .unwrap_err();
        assert_eq!(err.to_string(), "SteamGridDB rejected the key");
        api.json(format!("{API}/search/autocomplete/x"), json!(null));
        assert!(Api::new(&api, "k").search("x").unwrap().is_empty());
        let text = json!({ "success": false, "errors": ["Game not found"] }).to_string();
        api.answer(
            Api::images_url(8, Kind::Icon, 0, Filters::Ranked),
            200,
            move || Box::new(io::Cursor::new(text.clone().into_bytes())),
        );
        let err = Api::new(&api, "k")
            .images(8, Kind::Icon, Filters::Ranked)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "SteamGridDB refused the request: Game not found"
        );
    }

    #[test]
    fn downloads_send_no_key_and_are_capped() {
        let api = FakeApi::default();
        api.bytes("https://cdn.example/a.png", b"png");
        assert_eq!(
            Api::new(&api, "k")
                .download("https://cdn.example/a.png")
                .unwrap(),
            b"png"
        );
        assert_eq!(api.seen.borrow()[0].1, None);
        api.answer("https://cdn.example/big.png", 200, || {
            Box::new(io::repeat(0).take(MAX_BYTES + 1))
        });
        let err = Api::new(&api, "k")
            .download_with_progress("https://cdn.example/big.png", |_, _| {}, &Token::new())
            .unwrap_err();
        assert!(err.to_string().contains("larger than 32 MiB"), "{err}");
        api.answer(
            "https://cdn.example/gone.png",
            404,
            || Box::new(io::empty()),
        );
        assert!(
            Api::new(&api, "k")
                .download("https://cdn.example/gone.png")
                .is_err()
        );
    }

    #[test]
    fn downloads_enforce_declared_and_actual_size_limits() {
        let api = FakeApi::default();
        api.bytes_with_length(
            "https://cdn.example/declared-big.png",
            b"small body",
            MAX_BYTES + 1,
        );
        let err = Api::new(&api, "k")
            .download("https://cdn.example/declared-big.png")
            .unwrap_err();
        assert!(err.to_string().contains("larger than 32 MiB"), "{err}");

        api.answer_with_length("https://cdn.example/lying.png", 200, Some(1), || {
            Box::new(io::repeat(0).take(MAX_BYTES + 1))
        });
        let err = Api::new(&api, "k")
            .download("https://cdn.example/lying.png")
            .unwrap_err();
        assert!(err.to_string().contains("larger than 32 MiB"), "{err}");

        api.answer_with_length(
            "https://cdn.example/exact.png",
            200,
            Some(MAX_BYTES),
            || Box::new(io::repeat(0).take(MAX_BYTES)),
        );
        let bytes = Api::new(&api, "k")
            .download("https://cdn.example/exact.png")
            .unwrap();
        assert_eq!(bytes.len() as u64, MAX_BYTES);
    }

    struct ReadCounter(std::rc::Rc<std::cell::Cell<usize>>);

    impl Read for ReadCounter {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.set(self.0.get() + 1);
            Ok(0)
        }
    }

    #[test]
    fn canceled_before_the_first_chunk_does_not_read_the_body() {
        let api = FakeApi::default();
        let token = Token::new();
        let read_count = std::rc::Rc::new(std::cell::Cell::new(0));
        let answer_token = token.clone();
        let answer_count = read_count.clone();
        api.answer("https://cdn.example/early-cancel.png", 200, move || {
            answer_token.cancel();
            Box::new(ReadCounter(answer_count.clone()))
        });
        let err = Api::new(&api, "k")
            .download_with_progress("https://cdn.example/early-cancel.png", |_, _| {}, &token)
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::Canceled("download"))
        ));
        assert_eq!(read_count.get(), 0);
    }

    struct ShortReader {
        body: io::Cursor<Vec<u8>>,
        limit: usize,
    }

    impl Read for ShortReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.limit);
            self.body.read(&mut buffer[..count])
        }
    }

    struct InterruptedOnce {
        body: io::Cursor<Vec<u8>>,
        first_read: bool,
    }

    impl Read for InterruptedOnce {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.first_read {
                self.first_read = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.body.read(buffer)
        }
    }

    struct CancelAtEof {
        body: io::Cursor<Vec<u8>>,
        token: Token,
    }

    impl Read for CancelAtEof {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.body.position() == self.body.get_ref().len() as u64 {
                self.token.cancel();
                return Ok(0);
            }
            self.body.read(buffer)
        }
    }

    #[test]
    fn interrupted_reads_are_retried() {
        let api = FakeApi::default();
        api.answer("https://cdn.example/interrupted.png", 200, || {
            Box::new(InterruptedOnce {
                body: io::Cursor::new(b"after interrupt".to_vec()),
                first_read: true,
            })
        });
        assert_eq!(
            Api::new(&api, "k")
                .download("https://cdn.example/interrupted.png")
                .unwrap(),
            b"after interrupt"
        );
    }

    #[test]
    fn cancellation_during_the_eof_read_is_reported() {
        let api = FakeApi::default();
        let token = Token::new();
        let reader_token = token.clone();
        api.answer("https://cdn.example/eof-cancel.png", 200, move || {
            Box::new(CancelAtEof {
                body: io::Cursor::new(b"image".to_vec()),
                token: reader_token.clone(),
            })
        });
        let err = Api::new(&api, "k")
            .download_with_progress("https://cdn.example/eof-cancel.png", |_, _| {}, &token)
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::Canceled("download"))
        ));
    }

    #[test]
    fn downloads_report_progress_and_cancel_between_chunks() {
        let api = FakeApi::default();
        api.bytes("https://cdn.example/progress.png", b"image bytes");
        let mut progress = Vec::new();
        assert_eq!(
            Api::new(&api, "k")
                .download_with_progress(
                    "https://cdn.example/progress.png",
                    |received, total| progress.push((received, total)),
                    &Token::new(),
                )
                .unwrap(),
            b"image bytes"
        );
        assert_eq!(progress, [(11, Some(11))]);

        let token = Token::new();
        api.answer("https://cdn.example/cancel.png", 200, || {
            Box::new(ShortReader {
                body: io::Cursor::new(b"first chunk, then more".to_vec()),
                limit: 5,
            })
        });
        let mut canceled_progress = Vec::new();
        let error = Api::new(&api, "k")
            .download_with_progress(
                "https://cdn.example/cancel.png",
                |received, total| {
                    canceled_progress.push((received, total));
                    token.cancel();
                },
                &token,
            )
            .unwrap_err();
        assert_eq!(canceled_progress, [(5, None)]);
        assert!(matches!(
            error.downcast_ref::<crate::error::Error>(),
            Some(crate::error::Error::Canceled("download"))
        ));
    }

    #[test]
    fn image_fields_are_parsed_leniently() {
        let parsed = image(&json!({
            "id": 5, "url": "https://cdn.example/x.ICO?v=1", "style": ["official"],
            "thumb": "https://cdn.example/x-thumb.png",
            "author": {"name": "Ada"}, "nsfw": true, "humor": true, "type": "animated",
            "language": "DE", "width": 256, "height": 128, "score": -2, "mime": null,
        }))
        .unwrap();
        assert_eq!(
            parsed,
            Image {
                id: 5,
                url: "https://cdn.example/x.ICO?v=1".into(),
                thumb: Some("https://cdn.example/x-thumb.png".into()),
                author: Some("Ada".into()),
                style: None,
                nsfw: true,
                humor: true,
                animated: true,
                score: Some(-2),
                official: true,
                language: Some("de".into()),
                width: Some(256),
                height: Some(128),
                format: Some(Format::Ico),
            }
        );
        let bare = image(&json!({ "id": 6, "url": "https://cdn.example/y", "mime": "image/webp", "style": "custom" })).unwrap();
        assert_eq!(
            (
                bare.format,
                bare.official,
                bare.score,
                bare.thumb,
                bare.author,
                bare.style,
                bare.nsfw,
                bare.humor,
                bare.animated,
            ),
            (
                Some(Format::Webp),
                false,
                None,
                None,
                None,
                Some("custom".into()),
                false,
                false,
                false,
            )
        );
        let animated_mime = image(&json!({
            "id": 7, "url": "https://cdn.example/animated", "mime": "image/gif"
        }))
        .unwrap();
        assert!(animated_mime.animated);
        assert!(image(&json!({ "id": "7", "url": "u" })).is_none());
        assert!(image(&json!({ "id": 7 })).is_none());
    }

    #[test]
    fn language_comes_from_the_first_set_variable() {
        let lang = |pairs: &[(&str, &str)]| language(vars(pairs));
        assert_eq!(lang(&[("LANG", "de_DE.UTF-8")]), "de");
        assert_eq!(lang(&[("LC_MESSAGES", "fr_FR"), ("LANG", "de_DE")]), "fr");
        assert_eq!(
            lang(&[("LC_ALL", "ja_JP.UTF-8"), ("LC_MESSAGES", "fr_FR")]),
            "ja"
        );
        assert_eq!(lang(&[("LC_ALL", ""), ("LANG", "pt_BR")]), "pt");
        assert_eq!(lang(&[("LANG", "C.UTF-8")]), "en");
        assert_eq!(lang(&[("LC_ALL", "POSIX"), ("LANG", "de_DE")]), "en");
        assert_eq!(lang(&[]), "en");
    }

    fn candidate(id: u64) -> Image {
        Image {
            id,
            url: format!("https://cdn.example/{id}"),
            ..Image::default()
        }
    }

    fn ranked(kind: Kind, images: Vec<Image>, language: &str) -> Vec<u64> {
        let mut images = images;
        rank(kind, &mut images, language);
        images.iter().map(|image| image.id).collect()
    }

    fn sized(mut image: Image, width: u64, height: u64) -> Image {
        image.width = Some(width);
        image.height = Some(height);
        image
    }

    #[test]
    fn icons_rank_by_floor_style_language_format_score_size_then_id() {
        let official = |mut image: Image| {
            image.official = true;
            image
        };
        let lang = |mut image: Image, code: &str| {
            image.language = Some(code.into());
            image
        };
        let format = |mut image: Image, format| {
            image.format = Some(format);
            image
        };
        let score = |mut image: Image, score| {
            image.score = Some(score);
            image
        };
        let base = |id| sized(candidate(id), 256, 256);
        let images = vec![
            // Too small, even though it wins every later key.
            official(format(
                lang(score(sized(candidate(1), 512, 64), 99), "de"),
                Format::Png,
            )),
            base(2),
            official(base(3)),
            official(lang(base(4), "en")),
            official(lang(base(5), "de")),
            official(format(lang(base(6), "de"), Format::Ico)),
            official(format(lang(base(7), "de"), Format::Png)),
            official(score(format(lang(base(8), "de"), Format::Png), 5)),
            official(score(
                format(lang(sized(candidate(9), 512, 512), "de"), Format::Png),
                5,
            )),
            official(score(
                format(lang(sized(candidate(10), 512, 512), "de"), Format::Png),
                5,
            )),
        ];
        assert_eq!(
            ranked(Kind::Icon, images, "de"),
            [9, 10, 8, 7, 6, 5, 4, 3, 2, 1]
        );
    }

    #[test]
    fn grids_heroes_and_logos_rank_by_their_own_keys() {
        let with =
            |id, language: Option<&str>, score: Option<i64>, size: Option<(u64, u64)>| Image {
                language: language.map(str::to_owned),
                score,
                width: size.map(|(width, _)| width),
                height: size.map(|(_, height)| height),
                ..candidate(id)
            };
        let grids = vec![
            with(1, None, Some(50), Some((660, 930))),
            with(2, Some("en"), Some(1), None),
            with(3, Some("fr"), None, None),
            with(4, Some("fr"), Some(0), Some((600, 900))),
            with(5, Some("fr"), Some(0), Some((342, 482))),
            with(6, Some("fr"), Some(0), Some((342, 482))),
        ];
        assert_eq!(ranked(Kind::Grid, grids.clone(), "fr"), [4, 5, 6, 3, 2, 1]);
        // An English user ranks `en` first and everything else together.
        assert_eq!(ranked(Kind::Grid, grids.clone(), "en"), [2, 1, 4, 5, 6, 3]);
        // Heroes ignore language entirely.
        assert_eq!(ranked(Kind::Hero, grids, "fr"), [1, 2, 4, 5, 6, 3]);

        let logo = |id, official: bool, language: &str, format: Option<Format>, score: i64| Image {
            official,
            language: Some(language.to_owned()),
            format,
            score: Some(score),
            ..candidate(id)
        };
        let logos = vec![
            logo(1, false, "de", Some(Format::Png), 99),
            logo(2, true, "en", Some(Format::Png), 99),
            logo(3, true, "de", None, 99),
            logo(4, true, "de", Some(Format::Webp), 0),
            logo(5, true, "de", Some(Format::Png), 0),
            logo(6, true, "de", Some(Format::Png), 1),
        ];
        assert_eq!(ranked(Kind::Logo, logos, "de"), [6, 5, 4, 3, 2, 1]);
    }

    fn scored(mut image: Image, score: i64) -> Image {
        image.score = Some(score);
        image
    }

    #[test]
    fn oversized_logos_lose_to_any_normal_one() {
        let images = vec![
            scored(sized(candidate(1), 8192, 4482), 50),
            sized(candidate(2), 1600, 876),
        ];
        assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1]);
    }

    #[test]
    fn oversized_only_lists_still_yield_the_best_oversized() {
        let logos = vec![
            sized(candidate(1), 8192, 4482),
            scored(sized(candidate(2), 8192, 4482), 3),
        ];
        assert_eq!(ranked(Kind::Logo, logos, "en"), [2, 1]);
        let heroes = vec![
            sized(candidate(1), 5000, 1600),
            scored(sized(candidate(2), 6000, 2000), 3),
        ];
        assert_eq!(ranked(Kind::Hero, heroes, "en"), [2, 1]);
    }

    #[test]
    fn oversized_is_judged_on_the_long_side_only_when_both_sizes_are_known() {
        let heroes = vec![
            sized(candidate(1), 4097, 100),
            sized(candidate(2), 4096, 4096),
            Image {
                width: Some(9000),
                ..candidate(3)
            },
        ];
        // 4096 is not oversized, and a missing height means unknown.
        assert_eq!(ranked(Kind::Hero, heroes, "en"), [2, 3, 1]);
    }

    #[test]
    fn big_enough_logos_rank_by_score_not_size() {
        let images = vec![
            scored(sized(candidate(1), 3000, 1000), 1),
            scored(sized(candidate(2), 1200, 400), 5),
        ];
        assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1]);
    }

    #[test]
    fn big_enough_logos_beat_smaller_higher_scored_ones() {
        let images = vec![
            scored(sized(candidate(1), 1199, 600), 99),
            scored(sized(candidate(2), 1200, 100), 0),
        ];
        assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1]);
    }

    #[test]
    fn big_enough_logos_with_equal_scores_tie_on_id() {
        let images = vec![
            sized(candidate(3), 3000, 1000),
            sized(candidate(2), 1200, 400),
        ];
        assert_eq!(ranked(Kind::Logo, images, "en"), [2, 3]);
    }

    #[test]
    fn too_small_logos_still_prefer_more_pixels_after_score() {
        let images = vec![
            sized(candidate(1), 400, 100),
            sized(candidate(2), 800, 200),
            candidate(3),
        ];
        assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1, 3]);
    }

    #[test]
    fn heroes_rank_big_enough_by_score_over_bigger() {
        let images = vec![
            scored(sized(candidate(1), 3840, 1240), 2),
            scored(sized(candidate(2), 1920, 620), 5),
            scored(sized(candidate(3), 1919, 620), 99),
        ];
        assert_eq!(ranked(Kind::Hero, images, "en"), [2, 1, 3]);
    }

    #[test]
    fn unknown_dimensions_are_neither_oversized_nor_big_enough() {
        let images = vec![
            scored(sized(candidate(1), 8192, 4482), 99),
            scored(candidate(2), 99),
            scored(sized(candidate(3), 1600, 876), 1),
        ];
        // Known and big enough, then unknown, then oversized.
        assert_eq!(ranked(Kind::Logo, images.clone(), "en"), [3, 2, 1]);
        let heroes = vec![
            scored(sized(candidate(1), 8192, 4482), 99),
            scored(candidate(2), 99),
            scored(sized(candidate(3), 1920, 620), 1),
        ];
        assert_eq!(ranked(Kind::Hero, heroes, "en"), [3, 2, 1]);
    }

    #[test]
    fn a_known_width_is_big_enough_without_a_height() {
        let mut wide = scored(candidate(1), 1);
        wide.width = Some(1600);
        let images = vec![wide, scored(sized(candidate(2), 800, 300), 99)];
        assert_eq!(ranked(Kind::Logo, images, "en"), [1, 2]);
    }

    #[test]
    fn huge_reported_sizes_rank_without_overflowing() {
        let images = vec![
            sized(candidate(1), u64::MAX, u64::MAX),
            sized(candidate(2), 512, 512),
        ];
        assert_eq!(ranked(Kind::Icon, images, "en"), [1, 2]);
    }

    #[test]
    fn icons_and_grids_still_prefer_the_biggest_regardless_of_size() {
        let images = vec![
            sized(candidate(1), 512, 512),
            sized(candidate(2), 8192, 8192),
            sized(candidate(3), 1024, 1024),
        ];
        assert_eq!(ranked(Kind::Icon, images.clone(), "en"), [2, 3, 1]);
        assert_eq!(ranked(Kind::Grid, images, "en"), [2, 3, 1]);
    }
}
