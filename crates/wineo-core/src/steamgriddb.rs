//! The SteamGridDB API key: where it is kept, how it is found, and checking
//! it against the API.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use serde_json::Value;

use crate::art::{Format, Kind, MAX_BYTES};
use crate::download::Transport;
use crate::error::{Error, IoContext, Result};
use crate::paths::Paths;

pub const API: &str = "https://www.steamgriddb.com/api/v2";

/// Overrides every saved key, for scripts and CI.
pub const KEY_VAR: &str = "WINEO_STEAMGRIDDB_KEY";

const KEY_FILE: &str = "steamgriddb.key";

/// Why the keyring could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringError {
    /// There is no keyring service to ask, so the key file stands in.
    Unavailable,
    /// The keyring is there but failed, which is reported rather than
    /// quietly falling back to plain text.
    Failed(String),
}

/// Where the key is kept when a keyring service is running. Behind a trait so
/// tests never touch a real keyring.
pub trait Keyring {
    fn get(&self) -> Result<Option<String>, KeyringError>;
    fn set(&self, key: &str) -> Result<(), KeyringError>;
    /// Returns whether there was a key to delete.
    fn delete(&self) -> Result<bool, KeyringError>;
}

/// A keyring that is never available, so the key file is always used.
pub struct NoKeyring;

impl Keyring for NoKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        Err(KeyringError::Unavailable)
    }

    fn set(&self, _key: &str) -> Result<(), KeyringError> {
        Err(KeyringError::Unavailable)
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        Err(KeyringError::Unavailable)
    }
}

/// Where a found key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Variable,
    Keyring,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub value: String,
    pub source: KeySource,
}

/// The plain text key file, always on this machine, never in a portable folder.
pub fn key_file(paths: &Paths) -> PathBuf {
    paths.host_data().join(KEY_FILE)
}

/// Looks for the key in `WINEO_STEAMGRIDDB_KEY`, then the keyring, then the
/// key file. An empty value counts as unset.
pub fn find_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Option<Key>> {
    if let Some(value) = lookup(KEY_VAR)
        .and_then(|value| value.into_string().ok())
        .and_then(clean)
    {
        return Ok(Some(Key {
            value,
            source: KeySource::Variable,
        }));
    }
    match keyring.get() {
        Ok(Some(value)) => {
            if let Some(value) = clean(value) {
                return Ok(Some(Key {
                    value,
                    source: KeySource::Keyring,
                }));
            }
        }
        Ok(None) | Err(KeyringError::Unavailable) => {}
        Err(KeyringError::Failed(reason)) => return Err(Error::Keyring(reason)),
    }
    let path = key_file(paths);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(clean(text).map(|value| Key {
            value,
            source: KeySource::File,
        })),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context(|| format!("failed to read {}", path.display())),
    }
}

/// Like `find_key`, but a missing key is an error that says how to add one.
pub fn require_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Key> {
    find_key(paths, lookup, keyring)?.ok_or(Error::NoSteamGridDbKey)
}

fn clean(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Where `save_key` put the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Keyring,
    /// No keyring was available, so it is in this plain text file.
    File(PathBuf),
}

/// Saves the key to the keyring, or to the `0600` key file when there is no
/// keyring. A key file left from before is removed once the keyring has the
/// key, so no stale copy stays in plain text.
pub fn save_key(paths: &Paths, keyring: &dyn Keyring, key: &str) -> Result<Saved> {
    let path = key_file(paths);
    match keyring.set(key) {
        Ok(()) => {
            remove_file(&path)?;
            Ok(Saved::Keyring)
        }
        Err(KeyringError::Unavailable) => {
            write_private(&path, key)?;
            Ok(Saved::File(path))
        }
        Err(KeyringError::Failed(reason)) => Err(Error::Keyring(reason)),
    }
}

/// What `remove_key` deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    pub keyring: bool,
    pub file: bool,
}

/// Deletes the key from the keyring and the key file.
pub fn remove_key(paths: &Paths, keyring: &dyn Keyring) -> Result<Removed> {
    let from_keyring = match keyring.delete() {
        Ok(deleted) => deleted,
        Err(KeyringError::Unavailable) => false,
        Err(KeyringError::Failed(reason)) => return Err(Error::Keyring(reason)),
    };
    Ok(Removed {
        keyring: from_keyring,
        file: remove_file(&key_file(paths))?,
    })
}

fn remove_file(path: &std::path::Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
    }
}

/// Writes through a temp file created `0600`, so the key is never readable by
/// anyone else, not even for a moment.
fn write_private(path: &std::path::Path, key: &str) -> Result<()> {
    let dir = path.parent().expect("the key file has a parent");
    fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    let tmp = dir.join(format!(".{KEY_FILE}.tmp"));
    remove_file(&tmp)?;
    let context = || format!("failed to write {}", path.display());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .context(context)?;
    file.write_all(format!("{key}\n").as_bytes())
        .and_then(|()| file.sync_all())
        .context(context)?;
    fs::rename(&tmp, path).context(context)
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
    pub score: Option<i64>,
    pub official: bool,
    pub language: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// The format `mime` or the URL suggests, for ranking only. What is kept
    /// is decided by the downloaded bytes.
    pub format: Option<Format>,
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

    /// The URL of one page of a game's images of `kind`, with the hard
    /// filters every request carries.
    pub fn images_url(game: u64, kind: Kind, page: u32) -> String {
        let path = match kind {
            Kind::Icon => "icons",
            Kind::Grid => "grids",
            Kind::Hero => "heroes",
            Kind::Logo => "logos",
        };
        let dimensions = match kind {
            Kind::Grid => format!("&dimensions={GRID_DIMENSIONS}"),
            _ => String::new(),
        };
        format!(
            "{API}/{path}/game/{game}?types=static&nsfw=false&humor=false{dimensions}&page={page}"
        )
    }

    /// Every image of `kind` for `game`, walking pages until one is empty, up
    /// to `MAX_PAGES`. An image seen on an earlier page is not repeated.
    pub fn images(&self, game: u64, kind: Kind) -> anyhow::Result<Vec<Image>> {
        let mut images: Vec<Image> = Vec::new();
        // Pages start at 0, as SteamGridDB's own Decky plugin requests them.
        for page in 0..MAX_PAGES {
            let data = self.data(&Self::images_url(game, kind, page))?;
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
        let response = self.transport.request(url, None)?;
        if response.status != 200 {
            bail!("the download answered with status {}", response.status);
        }
        let mut bytes = Vec::new();
        response
            .body
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| anyhow!("failed to download {url}: {err}"))?;
        if bytes.len() as u64 > MAX_BYTES {
            bail!("{url} is larger than 32 MiB");
        }
        Ok(bytes)
    }

    /// The `data` array of an API answer.
    fn data(&self, url: &str) -> anyhow::Result<Vec<Value>> {
        let response = self.transport.request(url, Some(self.key))?;
        match response.status {
            200 => {}
            401 | 403 => bail!("SteamGridDB rejected the key"),
            status => bail!("SteamGridDB answered with status {status}"),
        }
        let answer: Value = serde_json::from_reader(response.body)
            .map_err(|err| anyhow!("SteamGridDB sent an unreadable answer: {err}"))?;
        if answer.get("success").and_then(Value::as_bool) == Some(false) {
            let errors: Vec<&str> = answer
                .get("errors")
                .and_then(Value::as_array)
                .map(|errors| errors.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            bail!("SteamGridDB refused the request: {}", errors.join(", "));
        }
        Ok(match answer.get("data") {
            Some(Value::Array(data)) => data.clone(),
            _ => Vec::new(),
        })
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

/// Sorts images best first by the plan's keys for `kind`. The lowest ID
/// breaks every tie, so the order is total.
pub fn rank(kind: Kind, images: &mut [Image], language: &str) {
    let language_rank = |image: &Image| match image.language.as_deref() {
        Some(found) if found == language => 0,
        Some("en") => 1,
        _ => 2,
    };
    // Highest first, with an unknown value after every known one.
    let high = |value: Option<i64>| (value.is_none(), std::cmp::Reverse(value));
    let area = |image: &Image| {
        high(
            image
                .width
                .zip(image.height)
                .map(|(width, height)| (width * height) as i64),
        )
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
            Kind::Hero => common(a).cmp(&common(b)),
            Kind::Logo => (
                !a.official,
                language_rank(a),
                preferred(a, Format::Webp),
                common(a),
            )
                .cmp(&(
                    !b.official,
                    language_rank(b),
                    preferred(b, Format::Webp),
                    common(b),
                )),
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

    #[derive(Default)]
    pub struct FakeApi {
        answers: RefCell<HashMap<String, (u16, Body)>>,
        pub seen: RefCell<Vec<(String, Option<String>)>>,
    }

    impl FakeApi {
        pub fn answer(
            &self,
            url: impl Into<String>,
            status: u16,
            body: impl Fn() -> Box<dyn Read> + 'static,
        ) {
            self.answers
                .borrow_mut()
                .insert(url.into(), (status, Rc::new(body)));
        }

        pub fn json(&self, url: impl Into<String>, data: Value) {
            let text = json!({ "success": true, "data": data }).to_string();
            self.answer(url, 200, move || {
                Box::new(io::Cursor::new(text.clone().into_bytes()))
            });
        }

        pub fn bytes(&self, url: impl Into<String>, bytes: &[u8]) {
            let bytes = bytes.to_vec();
            self.answer(url, 200, move || Box::new(io::Cursor::new(bytes.clone())));
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
            self.seen
                .borrow_mut()
                .push((url.to_owned(), bearer.map(str::to_owned)));
            let answers = self.answers.borrow();
            let (status, body) = answers
                .get(url)
                .ok_or_else(|| anyhow::anyhow!("unexpected request to {url}"))?;
            Ok(Response {
                status: *status,
                body: body(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
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
    fn every_request_carries_the_hard_filters() {
        let grid = Api::images_url(9, Kind::Grid, 2);
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
                Api::images_url(9, kind, 0),
                format!("{API}/{path}/game/9?types=static&nsfw=false&humor=false&page=0")
            );
        }
    }

    #[test]
    fn pages_are_walked_until_one_is_empty() {
        let api = FakeApi::default();
        api.json(
            Api::images_url(9, Kind::Hero, 0),
            json!([image_json(1), image_json(2)]),
        );
        api.json(
            Api::images_url(9, Kind::Hero, 1),
            json!([image_json(2), image_json(3), { "id": 4 }]),
        );
        api.json(Api::images_url(9, Kind::Hero, 2), json!([]));
        let images = Api::new(&api, "k").images(9, Kind::Hero).unwrap();
        let ids: Vec<u64> = images.iter().map(|image| image.id).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert_eq!(api.urls().len(), 3);
    }

    #[test]
    fn paging_stops_at_the_page_limit() {
        let api = FakeApi::default();
        for page in 0..MAX_PAGES + 2 {
            api.json(
                Api::images_url(9, Kind::Logo, page),
                json!([image_json(u64::from(page))]),
            );
        }
        let images = Api::new(&api, "k").images(9, Kind::Logo).unwrap();
        assert_eq!(images.len(), MAX_PAGES as usize);
        assert_eq!(api.urls().len(), MAX_PAGES as usize);
    }

    #[test]
    fn a_rejected_key_and_other_failures_are_errors() {
        let api = FakeApi::default();
        api.answer(Api::images_url(9, Kind::Icon, 0), 401, || {
            Box::new(io::empty())
        });
        let err = Api::new(&api, "k").images(9, Kind::Icon).unwrap_err();
        assert_eq!(err.to_string(), "SteamGridDB rejected the key");
        api.json(format!("{API}/search/autocomplete/x"), json!(null));
        assert!(Api::new(&api, "k").search("x").unwrap().is_empty());
        let text = json!({ "success": false, "errors": ["Game not found"] }).to_string();
        api.answer(Api::images_url(8, Kind::Icon, 0), 200, move || {
            Box::new(io::Cursor::new(text.clone().into_bytes()))
        });
        let err = Api::new(&api, "k").images(8, Kind::Icon).unwrap_err();
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
            .download("https://cdn.example/big.png")
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
    fn image_fields_are_parsed_leniently() {
        let parsed = image(&json!({
            "id": 5, "url": "https://cdn.example/x.ICO?v=1", "style": ["official"],
            "language": "DE", "width": 256, "height": 128, "score": -2, "mime": null,
        }))
        .unwrap();
        assert_eq!(
            parsed,
            Image {
                id: 5,
                url: "https://cdn.example/x.ICO?v=1".into(),
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
            (bare.format, bare.official, bare.score),
            (Some(Format::Webp), false, None)
        );
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
}
