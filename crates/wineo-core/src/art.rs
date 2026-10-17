//! The per-environment artwork store.
//!
//! Each kind is one file named for its kind and real format, like `icon.png`
//! or `grid.jpg`, in the environment's art dir. A target can override any kind
//! in a subdirectory named for it.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::cancel::Token;
use crate::config;
use crate::environment::{Environment, EnvironmentIdentity};
use crate::error::{Error, IoContext, Result};
use crate::files::{self, write_atomic};
use crate::name;
use crate::operation::{ConfigGuard, Locks};
use crate::paths::Paths;

/// The largest art file wineo accepts.
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Icon,
    Grid,
    Hero,
    Logo,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Icon, Kind::Grid, Kind::Hero, Kind::Logo];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Icon => "icon",
            Kind::Grid => "grid",
            Kind::Hero => "hero",
            Kind::Logo => "logo",
        }
    }

    pub fn accepts(self, format: Format) -> bool {
        match format {
            Format::Png | Format::Svg | Format::Jpeg => true,
            Format::Ico => self == Kind::Icon,
            Format::Webp => self != Kind::Icon,
        }
    }

    /// The file this kind is stored as in `format`.
    pub fn file_name(self, format: Format) -> String {
        format!("{}.{}", self.as_str(), format.extension())
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Kind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| format!("expected icon, grid, hero, or logo, got `{s}`"))
    }
}

/// Declared in the order an unrecorded duplicate is picked in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    Png,
    Svg,
    Ico,
    Jpeg,
    Webp,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Png,
        Format::Svg,
        Format::Ico,
        Format::Jpeg,
        Format::Webp,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Svg => "svg",
            Format::Ico => "ico",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Png => "PNG",
            Format::Svg => "SVG",
            Format::Ico => "ICO",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
        })
    }
}

/// The format of an image from its contents. None when it is none of the
/// formats art can be stored in.
pub fn detect(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(Format::Jpeg)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else if is_ico(bytes) {
        Some(Format::Ico)
    } else if is_svg(bytes) {
        Some(Format::Svg)
    } else {
        None
    }
}

/// ICO's magic is just `00 00 01 00`, which plenty of other files start with,
/// so the directory and every image it lists have to fit in the file too.
fn is_ico(bytes: &[u8]) -> bool {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if bytes.len() < 6 || u16_at(0) != 0 || u16_at(2) != 1 {
        return false;
    }
    let count = usize::from(u16_at(4));
    let directory_end = 6 + 16 * count;
    if count == 0 || directory_end > bytes.len() {
        return false;
    }
    (0..count).all(|index| {
        let entry = 6 + 16 * index;
        let size = u32_at(entry + 8) as usize;
        let offset = u32_at(entry + 12) as usize;
        size > 0
            && offset >= directory_end
            && offset
                .checked_add(size)
                .is_some_and(|end| end <= bytes.len())
    })
}

/// SVG has no magic bytes. This skips what may come before the root element
/// and checks the root is `svg`, without parsing the XML.
fn is_svg(bytes: &[u8]) -> bool {
    let mut rest = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    loop {
        rest = rest.trim_ascii_start();
        let skipped = if rest.starts_with(b"<?") {
            after(rest, b"?>")
        } else if rest.starts_with(b"<!--") {
            after(&rest[4..], b"-->")
        } else if rest.starts_with(b"<!DOCTYPE") {
            after_doctype(rest)
        } else {
            break;
        };
        match skipped {
            Some(next) => rest = next,
            None => return false,
        }
    }
    rest.strip_prefix(b"<svg")
        .and_then(|after| after.first())
        .is_some_and(|next| next.is_ascii_whitespace() || matches!(next, b'>' | b'/'))
}

/// What follows the first `end` in `bytes`.
fn after<'a>(bytes: &'a [u8], end: &[u8]) -> Option<&'a [u8]> {
    bytes
        .windows(end.len())
        .position(|window| window == end)
        .map(|at| &bytes[at + end.len()..])
}

/// What follows a doctype, whose internal subset and quoted ids may hold `>`.
fn after_doctype(bytes: &[u8]) -> Option<&[u8]> {
    let mut quote = None;
    let mut depth = 0usize;
    for (at, &byte) in bytes.iter().enumerate() {
        match (quote, byte) {
            (Some(open), _) if byte == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'[') => depth += 1,
            (None, b']') => depth = depth.saturating_sub(1),
            (None, b'>') if depth == 0 => return Some(&bytes[at + 1..]),
            _ => {}
        }
    }
    None
}

/// The file a kind resolves to in one art dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub format: Format,
    /// Other files for the same kind, which lose to this one.
    pub duplicates: Vec<PathBuf>,
}

/// Finds `kind` in `dir`. The `recorded` format wins when its file exists.
/// Otherwise the first format in `Format::ALL` order does. Only formats the
/// kind accepts are looked for.
pub fn find(dir: &Path, kind: Kind, recorded: Option<Format>) -> Result<Option<Found>> {
    let mut present = Vec::new();
    for format in Format::ALL.into_iter().filter(|f| kind.accepts(*f)) {
        let path = dir.join(kind.file_name(format));
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => present.push((format, path)),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        }
    }
    if present.is_empty() {
        return Ok(None);
    }
    let winner = recorded
        .and_then(|format| present.iter().position(|(f, _)| *f == format))
        .unwrap_or(0);
    let (format, path) = present.remove(winner);
    Ok(Some(Found {
        path,
        format,
        duplicates: present.into_iter().map(|(_, path)| path).collect(),
    }))
}

/// Where a target's overrides live inside its environment's art dir.
pub fn target_dir(art_dir: &Path, target: &str) -> PathBuf {
    art_dir.join(target)
}

/// Which art dir a resolved file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Environment,
    Target,
}

/// The art a kind resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub origin: Origin,
    pub found: Found,
    /// None for a file `art.toml` does not record, which counts as manual
    /// with an unknown origin.
    pub source: Option<Source>,
}

/// The art `kind` resolves to for `target`, falling back to the environment's
/// when the target does not override it.
pub fn resolve(
    art_dir: &Path,
    config: &ArtConfig,
    target: Option<&str>,
    kind: Kind,
) -> Result<Option<Resolved>> {
    let lookup = |origin, dir: &Path, target| -> Result<Option<Resolved>> {
        let record = config.record(target, kind);
        Ok(
            find(dir, kind, record.map(|r| r.format))?.map(|found| Resolved {
                origin,
                source: record
                    .filter(|r| r.format == found.format)
                    .map(|r| r.source.clone()),
                found,
            }),
        )
    };
    if let Some(target) = target {
        let dir = target_dir(art_dir, target);
        if let Some(resolved) = lookup(Origin::Target, &dir, Some(target))? {
            return Ok(Some(resolved));
        }
    }
    lookup(Origin::Environment, art_dir, None)
}

/// The dir art for `target`, or the environment for None, is stored in.
pub fn spot_dir(art_dir: &Path, target: Option<&str>) -> PathBuf {
    match target {
        Some(target) => target_dir(art_dir, target),
        None => art_dir.to_owned(),
    }
}

/// How messages name an environment or one of its targets.
fn spot_name(env: &str, target: Option<&str>) -> String {
    match target {
        Some(target) => format!("{env}.{target}"),
        None => env.to_owned(),
    }
}

/// What `set_manual` stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub path: PathBuf,
    pub format: Format,
    /// Files the kind had in other formats, now removed.
    pub removed: Vec<PathBuf>,
}

/// Copies `file` into the store as `kind` for `target`, or the environment for
/// None, and records it as manual. Its format comes from its contents, and its
/// bytes are kept as they are.
pub fn set_manual(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    file: &Path,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Stored> {
    let original = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    check_spot(paths, env, target)?;
    let bytes = read_art(file)?;
    store_manual(
        paths,
        env,
        target,
        kind,
        &bytes,
        &original,
        file,
        expected_identity,
    )
}

/// Like `set_manual`, but records `original` as the source's name instead of
/// the file's own.
pub fn set_manual_as(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    file: &Path,
    original: &str,
) -> Result<Stored> {
    check_spot(paths, env, target)?;
    let bytes = read_art(file)?;
    store_manual(paths, env, target, kind, &bytes, original, file, None)
}

/// Like `set_manual`, for an image already in memory. `original` is the file
/// name recorded as its source.
pub fn set_manual_bytes(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    bytes: &[u8],
    original: &str,
) -> Result<Stored> {
    check_spot(paths, env, target)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::ArtTooLarge(PathBuf::from(original)));
    }
    store_manual(
        paths,
        env,
        target,
        kind,
        bytes,
        original,
        Path::new(original),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn store_manual(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    bytes: &[u8],
    original: &str,
    file: &Path,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Stored> {
    let format = detect(bytes)
        .filter(|format| kind.accepts(*format))
        .ok_or_else(|| Error::ArtFormat {
            path: file.to_owned(),
            accepted: accepted(kind),
        })?;

    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    check_environment_identity(paths, env, expected_identity)?;
    // Checked again under the lock, so art never outlives a concurrent `rm`.
    Environment::ensure_exists(paths, env)?;
    let mut art = ArtFile::open(&art_dir)?;
    let dir = spot_dir(&art_dir, target);
    fs::create_dir_all(&dir).context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(kind.file_name(format));
    let tmp = dir.join(format!(".{}.tmp", kind.file_name(format)));
    fs::write(&tmp, bytes).context(|| format!("failed to write {}", path.display()))?;
    fs::rename(&tmp, &path).context(|| format!("failed to write {}", path.display()))?;
    art.set(
        target,
        kind,
        Record {
            format,
            source: Source::Manual(original.to_owned()),
        },
    )?;
    art.save()?;
    // Only once the new file is recorded, so a failure leaves a duplicate the
    // record already outranks rather than no art at all.
    let mut removed = Vec::new();
    if let Some(found) = find(&dir, kind, Some(format))? {
        for old in found.duplicates {
            fs::remove_file(&old).context(|| format!("failed to remove {}", old.display()))?;
            removed.push(old);
        }
    }
    Ok(Stored {
        path,
        format,
        removed,
    })
}

fn check_environment_identity(
    paths: &Paths,
    env: &str,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<()> {
    if expected_identity
        .is_some_and(|expected| EnvironmentIdentity::read(paths, env) != Some(expected))
    {
        return Err(Error::ArtEnvironmentChanged(env.to_owned()));
    }
    Ok(())
}

/// A SteamGridDB image, downloaded and identified.
#[derive(Debug)]
pub struct Downloaded {
    pub asset: u64,
    pub bytes: Vec<u8>,
    pub format: Format,
}

/// Publishes a downloaded image as `kind` for `target`, or the environment
/// for None, recorded as SteamGridDB `asset`. Without `replace`, art that
/// appeared since the fetch began is kept and None is returned. The file is
/// staged beside its final name and published without replacing anything
/// but the kind's own files this decided to replace.
pub(crate) fn publish_fetched(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    image: &Downloaded,
    replace: bool,
) -> Result<Option<Stored>> {
    publish_fetched_with_cancel(paths, env, target, kind, image, replace, None, None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_fetched_with_cancel(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    image: &Downloaded,
    replace: bool,
    cancel: Option<&Token>,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Option<Stored>> {
    publish_source(
        paths,
        env,
        target,
        kind,
        &image.bytes,
        image.format,
        Source::SteamGridDb(image.asset),
        replace,
        cancel,
        expected_identity,
    )
}

/// Publishes `bytes`, already identified as `format`, as `kind` recorded
/// with `source`. Otherwise as `publish_fetched`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_source(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    bytes: &[u8],
    format: Format,
    source: Source,
    replace: bool,
    cancel: Option<&Token>,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Option<Stored>> {
    let art_dir = paths.art_dir(env);
    #[cfg(test)]
    test_hook("art.lock");
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    if cancel.is_some_and(Token::is_canceled) {
        return Err(Error::Canceled("download"));
    }
    check_environment_identity(paths, env, expected_identity)?;
    check_spot(paths, env, target)?;
    let mut art = ArtFile::open(&art_dir)?;
    let dir = spot_dir(&art_dir, target);
    let current = find(
        &dir,
        kind,
        art.config().record(target, kind).map(|r| r.format),
    )?;
    if current.is_some() && !replace {
        return Ok(None);
    }
    let old: Vec<PathBuf> = current
        .map(|found| {
            std::iter::once(found.path)
                .chain(found.duplicates)
                .collect()
        })
        .unwrap_or_default();
    fs::create_dir_all(&dir).context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(kind.file_name(format));
    let context = || format!("failed to write {}", path.display());
    let mut staged = tempfile::Builder::new()
        .prefix(".wineo-fetch-")
        .tempfile_in(&dir)
        .context(context)?;
    staged.write_all(bytes).context(context)?;
    staged.as_file().sync_all().context(context)?;
    fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o644)).context(context)?;
    #[cfg(test)]
    test_hook("art.publish");
    // The last check, right before publishing: once the file is in place,
    // its record is saved too, so the two never disagree.
    if cancel.is_some_and(Token::is_canceled) {
        return Err(Error::Canceled("download"));
    }
    let published = if old.contains(&path) {
        staged.persist(&path).map(drop)
    } else {
        staged.persist_noclobber(&path).map(drop)
    };
    published
        .map_err(|err| err.error)
        .context(|| format!("cannot publish without replacing {}", path.display()))?;
    art.set(target, kind, Record { format, source })?;
    art.save()?;
    let mut removed = Vec::new();
    for file in old.into_iter().filter(|file| *file != path) {
        fs::remove_file(&file).context(|| format!("failed to remove {}", file.display()))?;
        removed.push(file);
    }
    Ok(Some(Stored {
        path,
        format,
        removed,
    }))
}

/// The SteamGridDB game an environment's art is recorded for.
pub fn saved_game(paths: &Paths, env: &str) -> Result<Option<u64>> {
    Ok(ArtConfig::load(&paths.art_dir(env))?.game)
}

/// Records `game` as the environment's SteamGridDB game.
pub fn save_game(paths: &Paths, env: &str, game: u64) -> Result<()> {
    save_game_for(paths, env, game, None, None)
}

/// Like `save_game`, but refused unless the environment is still the one
/// `expected_identity` was read from, or when `cancel` was canceled by the
/// time the art lock is held.
pub(crate) fn save_game_for(
    paths: &Paths,
    env: &str,
    game: u64,
    expected_identity: Option<EnvironmentIdentity>,
    cancel: Option<&Token>,
) -> Result<()> {
    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    if cancel.is_some_and(Token::is_canceled) {
        return Err(Error::Canceled("SteamGridDB game"));
    }
    Environment::ensure_exists(paths, env)?;
    check_environment_identity(paths, env, expected_identity)?;
    let mut art = ArtFile::open(&art_dir)?;
    #[cfg(test)]
    test_hook("art.save_game");
    if cancel.is_some_and(Token::is_canceled) {
        return Err(Error::Canceled("SteamGridDB game"));
    }
    art.set_game(Some(game))?;
    art.save()?;
    Ok(())
}

/// Runs the test hook set for `at` on this thread, once. Tests use it to
/// cancel in the gap just before a write.
#[cfg(test)]
pub(crate) fn test_hook(at: &str) {
    let hook = TEST_HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        let index = hooks.iter().position(|(point, _)| *point == at)?;
        Some(hooks.remove(index).1)
    });
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
type TestHooks = std::cell::RefCell<Vec<(&'static str, Box<dyn FnOnce()>)>>;

#[cfg(test)]
thread_local! {
    static TEST_HOOKS: TestHooks = const { std::cell::RefCell::new(Vec::new()) };
}

/// Sets a test hook for `at` on this thread.
#[cfg(test)]
pub(crate) fn on_test_hook(at: &'static str, hook: impl FnOnce() + 'static) {
    TEST_HOOKS.with(|hooks| hooks.borrow_mut().push((at, Box::new(hook))));
}

/// What `remove` took away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub files: Vec<PathBuf>,
    /// Whether `art.toml` had a record, even one whose file was gone.
    pub record: bool,
}

/// Removes `kind` for `target`, or the environment for None: every file it
/// has and its record. A target's removal never touches the environment's art
/// it falls back to. The target itself need not exist, so art left behind by
/// one that was removed can still be cleaned up.
pub fn remove(paths: &Paths, env: &str, target: Option<&str>, kind: Kind) -> Result<Removed> {
    if let Some(target) = target
        && !name::is_valid(target)
    {
        return Err(Error::InvalidName(target.to_owned()));
    }
    Environment::ensure_exists(paths, env)?;
    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    Environment::ensure_exists(paths, env)?;
    let mut art = ArtFile::open(&art_dir)?;
    let dir = spot_dir(&art_dir, target);
    let mut files = Vec::new();
    if let Some(found) = find(&dir, kind, None)? {
        files.push(found.path);
        files.extend(found.duplicates);
    }
    let record = art.remove(target, kind);
    if files.is_empty() && !record {
        return Err(Error::NoArt {
            spot: spot_name(env, target),
            kind,
        });
    }
    // The record goes first, so a failure leaves unrecorded files rather than
    // a record claiming a file that is gone.
    art.save()?;
    for file in &files {
        fs::remove_file(file).context(|| format!("failed to remove {}", file.display()))?;
    }
    if target.is_some() {
        // Only an empty dir goes, so anything a user put there stays.
        let _ = fs::remove_dir(&dir);
    }
    Ok(Removed { files, record })
}

/// Drops the record of `kind` for `target`, or the environment for None, if
/// its file is gone. Files are never touched. Returns whether it dropped one.
pub fn drop_missing_record(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
) -> Result<bool> {
    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    Environment::ensure_exists(paths, env)?;
    let mut art = ArtFile::open(&art_dir)?;
    let Some(record) = art.config().record(target, kind) else {
        return Ok(false);
    };
    let path = spot_dir(&art_dir, target).join(kind.file_name(record.format));
    if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
        return Ok(false);
    }
    art.remove(target, kind);
    art.save()
}

/// Drops the records for a target the environment no longer has. Its files
/// stay. Returns whether it dropped any.
pub fn drop_unknown_target(paths: &Paths, env: &str, target: &str) -> Result<bool> {
    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    let environment = Environment::load(paths, env)?;
    if has_target(&environment, target) {
        return Ok(false);
    }
    let mut art = ArtFile::open(&art_dir)?;
    art.remove_target(target);
    art.save()
}

/// Removes an environment's whole art dir, for `rm`. Returns whether there was one.
pub fn remove_all(paths: &Paths, env: &str) -> Result<bool> {
    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    match fs::symlink_metadata(&art_dir) {
        Ok(_) => {
            fs::remove_dir_all(&art_dir)
                .context(|| format!("failed to remove {}", art_dir.display()))?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).context(|| format!("failed to read {}", art_dir.display())),
    }
}

/// Removes a removed target's art dir and records, so a later target with the
/// same name starts clean. Returns whether it had any.
pub fn remove_target(paths: &Paths, env: &str, target: &str) -> Result<bool> {
    TargetCleanup::prepare(paths, env, [target.to_owned()])?.finish()
}

/// The art cleanup for targets a config write removes. Prepared before the
/// write, so an invalid `art.toml` stops the command before anything changes,
/// and the art lock is held from then until it is finished.
pub(crate) struct TargetCleanup {
    art_dir: PathBuf,
    art: ArtFile,
    targets: Vec<String>,
    _guard: ConfigGuard,
}

impl TargetCleanup {
    pub(crate) fn prepare(
        paths: &Paths,
        env: &str,
        targets: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let targets: Vec<String> = targets.into_iter().collect();
        if let Some(bad) = targets.iter().find(|target| !name::is_valid(target)) {
            return Err(Error::InvalidName(bad.clone()));
        }
        let art_dir = paths.art_dir(env);
        let guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
        let art = ArtFile::open(&art_dir)?;
        Ok(Self {
            art_dir,
            art,
            targets,
            _guard: guard,
        })
    }

    /// Returns whether any of the targets had art.
    pub(crate) fn finish(mut self) -> Result<bool> {
        let mut had = false;
        for target in &self.targets {
            had |= self.art.remove_target(target);
        }
        self.art.save()?;
        for target in &self.targets {
            let dir = target_dir(&self.art_dir, target);
            match fs::symlink_metadata(&dir) {
                Ok(_) => {
                    fs::remove_dir_all(&dir)
                        .context(|| format!("failed to remove {}", dir.display()))?;
                    had = true;
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).context(|| format!("failed to read {}", dir.display()));
                }
            }
        }
        Ok(had)
    }
}

/// Something `art list` warns about. None of them change the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// Files that lose to the one a kind resolves to.
    Duplicates {
        target: Option<String>,
        kind: Kind,
        used: PathBuf,
        ignored: Vec<PathBuf>,
        /// Whether `used` wins because `art.toml` records it, rather than by
        /// format order.
        recorded: bool,
    },
    /// A record whose file is gone.
    MissingFile {
        target: Option<String>,
        kind: Kind,
        path: PathBuf,
    },
    /// A `[targets.<target>]` table for a target the environment doesn't have.
    UnknownTarget(String),
}

/// Everything `art list` shows for an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub game: Option<u64>,
    /// Every kind, in `Kind::ALL` order, with the environment's art for it.
    pub kinds: Vec<(Kind, Option<Resolved>)>,
    /// Each of the environment's targets that overrides at least one kind,
    /// with just its overrides.
    pub targets: Vec<(String, Vec<(Kind, Resolved)>)>,
    pub warnings: Vec<Warning>,
}

pub fn list(paths: &Paths, env: &str) -> Result<Listing> {
    let environment = Environment::load(paths, env)?;
    let art_dir = paths.art_dir(env);
    let config = ArtConfig::load(&art_dir)?;
    let mut warnings = Vec::new();
    let mut kinds = Vec::new();
    for kind in Kind::ALL {
        let resolved = resolve(&art_dir, &config, None, kind)?;
        check(
            &art_dir,
            &config,
            None,
            kind,
            resolved.as_ref(),
            &mut warnings,
        );
        kinds.push((kind, resolved));
    }
    let mut targets = Vec::new();
    let mut known_targets: Vec<String> = environment.config.targets.keys().cloned().collect();
    if let config::Kind::Proxied(proxied) = &environment.config.kind {
        known_targets.push(proxied.launcher.as_str().to_owned());
    }
    known_targets.sort();
    known_targets.dedup();
    for target in &known_targets {
        let mut overrides = Vec::new();
        for kind in Kind::ALL {
            let dir = target_dir(&art_dir, target);
            let record = config.record(Some(target), kind);
            let resolved = find(&dir, kind, record.map(|r| r.format))?.map(|found| Resolved {
                origin: Origin::Target,
                source: record
                    .filter(|r| r.format == found.format)
                    .map(|r| r.source.clone()),
                found,
            });
            check(
                &art_dir,
                &config,
                Some(target),
                kind,
                resolved.as_ref(),
                &mut warnings,
            );
            overrides.extend(resolved.map(|resolved| (kind, resolved)));
        }
        if !overrides.is_empty() {
            targets.push((target.clone(), overrides));
        }
    }
    for target in config.targets.keys() {
        if !has_target(&environment, target) {
            warnings.push(Warning::UnknownTarget(target.clone()));
        }
    }
    Ok(Listing {
        game: config.game,
        kinds,
        targets,
        warnings,
    })
}

fn check(
    art_dir: &Path,
    config: &ArtConfig,
    target: Option<&str>,
    kind: Kind,
    resolved: Option<&Resolved>,
    warnings: &mut Vec<Warning>,
) {
    let owned = target.map(str::to_owned);
    if let Some(record) = config.record(target, kind) {
        let path = spot_dir(art_dir, target).join(kind.file_name(record.format));
        if !fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
            warnings.push(Warning::MissingFile {
                target: owned.clone(),
                kind,
                path,
            });
        }
    }
    if let Some(resolved) = resolved
        && !resolved.found.duplicates.is_empty()
    {
        warnings.push(Warning::Duplicates {
            target: owned,
            kind,
            used: resolved.found.path.clone(),
            ignored: resolved.found.duplicates.clone(),
            recorded: resolved.source.is_some(),
        });
    }
}

/// Refuses art for an environment or target that doesn't exist.
pub fn check_spot(paths: &Paths, env: &str, target: Option<&str>) -> Result<()> {
    match target {
        None => Environment::ensure_exists(paths, env),
        Some(target) => {
            let environment = Environment::load(paths, env)?;
            if has_target(&environment, target) {
                Ok(())
            } else {
                Err(Error::UnknownTarget {
                    environment: env.to_owned(),
                    target: target.to_owned(),
                })
            }
        }
    }
}

fn has_target(environment: &Environment, target: &str) -> bool {
    environment.config.targets.contains_key(target)
        || matches!(
            &environment.config.kind,
            config::Kind::Proxied(proxied) if proxied.launcher.as_str() == target
        )
}

/// Reads an art file, refusing one over the cap without reading all of it.
fn read_art(file: &Path) -> Result<Vec<u8>> {
    let context = || format!("failed to read {}", file.display());
    let mut bytes = Vec::new();
    fs::File::open(file)
        .context(context)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context(context)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::ArtTooLarge(file.to_owned()));
    }
    Ok(bytes)
}

/// The formats `kind` accepts, for a message, like "PNG, SVG, or JPEG".
pub(crate) fn accepted(kind: Kind) -> String {
    let names: Vec<String> = Format::ALL
        .into_iter()
        .filter(|format| kind.accepts(*format))
        .map(|format| format.to_string())
        .collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{}, or {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

/// A copy of an environment's art, staged beside where it will be published,
/// for `clone` and `portable copy`. The source's art lock is held until it is
/// published or dropped, so the copy can't go stale in between.
pub(crate) struct StagedArt {
    staging: Option<(tempfile::TempDir, PathBuf)>,
    destination: PathBuf,
    _guard: ConfigGuard,
}

impl StagedArt {
    /// Stages a copy of `source` for `destination`. Nothing is staged when
    /// the source has no art.
    pub(crate) fn stage(locks: &Locks, source: &Path, destination: &Path) -> Result<Self> {
        let guard = locks.config(&source.join(ART_FILE))?;
        let staging = if fs::symlink_metadata(source).is_ok() {
            let parent = destination
                .parent()
                .expect("an art dir always has a parent");
            fs::create_dir_all(parent)
                .context(|| format!("failed to create {}", parent.display()))?;
            let dir = tempfile::Builder::new()
                .prefix(".wineo-art-")
                .tempdir_in(parent)
                .context(|| format!("failed to create a staging dir in {}", parent.display()))?;
            // A subdir rather than the temp dir itself, which is private.
            let staged = dir.path().join("art");
            copy_tree(source, &staged)?;
            Some((dir, staged))
        } else {
            None
        };
        Ok(Self {
            staging,
            destination: destination.to_owned(),
            _guard: guard,
        })
    }

    /// Stages art with cancellation and entry progress, preserving literal links.
    pub(crate) fn stage_with(
        locks: &Locks,
        source: &Path,
        destination: &Path,
        staging_root: &Path,
        cancel: &Token,
        progress: &mut impl FnMut(crate::import::ImportProgress),
    ) -> anyhow::Result<Self> {
        crate::import::check_cancelled(cancel)?;
        let guard = locks.config(&source.join(ART_FILE))?;
        let staging = if fs::symlink_metadata(source).is_ok() {
            let dir = tempfile::Builder::new()
                .prefix(".wineo-art-")
                .tempdir_in(staging_root)
                .context(|| {
                    format!(
                        "failed to create a staging dir in {}",
                        staging_root.display()
                    )
                })?;
            let staged = dir.path().join("art");
            let entries = art_entries(source, cancel)?;
            progress(crate::import::ImportProgress::Started { entries });
            copy_tree_with(source, &staged, cancel, progress)?;
            Some((dir, staged))
        } else {
            None
        };
        crate::import::check_cancelled(cancel)?;
        Ok(Self {
            staging,
            destination: destination.to_owned(),
            _guard: guard,
        })
    }

    /// Publishes the copy without replacing anything. Returns where it went,
    /// or None when there was no art to copy.
    pub(crate) fn publish(self) -> Result<Option<PathBuf>> {
        let Some((_dir, staged)) = &self.staging else {
            return Ok(None);
        };
        fs::create_dir_all(
            self.destination
                .parent()
                .expect("an art dir always has a parent"),
        )
        .context(|| {
            format!(
                "failed to create art directory for {}",
                self.destination.display()
            )
        })?;
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            staged,
            rustix::fs::CWD,
            &self.destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)
        .context(|| {
            format!(
                "cannot publish without replacing {}",
                self.destination.display()
            )
        })?;
        Ok(Some(self.destination.clone()))
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir(to).context(|| format!("failed to create {}", to.display()))?;
    let entries = fs::read_dir(from).context(|| format!("failed to read {}", from.display()))?;
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", from.display()))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .context(|| format!("failed to read {}", source.display()))?;
        if kind.is_dir() {
            copy_tree(&source, &destination)?;
        } else if kind.is_symlink() {
            let link = fs::read_link(&source)
                .context(|| format!("failed to read {}", source.display()))?;
            std::os::unix::fs::symlink(link, &destination)
                .context(|| format!("failed to create {}", destination.display()))?;
        } else {
            fs::copy(&source, &destination)
                .context(|| format!("failed to copy {}", source.display()))?;
        }
    }
    Ok(())
}

fn art_entries(path: &Path, cancel: &Token) -> anyhow::Result<u64> {
    crate::import::check_cancelled(cancel)?;
    let mut count = 1;
    if fs::symlink_metadata(path)?.is_dir() {
        for entry in fs::read_dir(path)? {
            count += art_entries(&entry?.path(), cancel)?;
        }
    }
    Ok(count)
}

fn copy_tree_with(
    from: &Path,
    to: &Path,
    cancel: &Token,
    progress: &mut impl FnMut(crate::import::ImportProgress),
) -> anyhow::Result<()> {
    crate::import::check_cancelled(cancel)?;
    fs::create_dir(to).context(|| format!("failed to create {}", to.display()))?;
    for entry in fs::read_dir(from).context(|| format!("failed to read {}", from.display()))? {
        crate::import::check_cancelled(cancel)?;
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree_with(&source, &destination, cancel, progress)?;
            continue;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&source)?, &destination)?;
        } else {
            let mut input = fs::File::open(&source)?;
            let mut output = fs::File::create(&destination)?;
            let mut buffer = [0; 128 * 1024];
            loop {
                crate::import::check_cancelled(cancel)?;
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
            }
            fs::set_permissions(&destination, input.metadata()?.permissions())?;
        }
        progress(crate::import::ImportProgress::Copied { path: source });
    }
    progress(crate::import::ImportProgress::Copied {
        path: from.to_owned(),
    });
    crate::import::check_cancelled(cancel)
}

/// The file in an environment's art dir that records where its art came from.
pub const ART_FILE: &str = "art.toml";

/// Where a piece of art came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A SteamGridDB asset ID.
    SteamGridDb(u64),
    /// Set by hand from a file with this name.
    Manual(String),
    /// GOG's own artwork for the game with this ID.
    Gog(u64),
}

/// What `art.toml` says about one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The stored file is the kind's file in this format.
    pub format: Format,
    pub source: Source,
}

pub type Records = BTreeMap<Kind, Record>;

/// `art/<env>/art.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtConfig {
    /// 0 for a legacy or absent file. Checked by `parse`.
    pub schema_version: u32,
    /// The SteamGridDB game, shared by the environment and its targets.
    pub game: Option<u64>,
    pub kinds: Records,
    pub targets: BTreeMap<String, Records>,
    /// The deprecated keys the file used, moved to their new names.
    pub deprecated: Vec<crate::deprecated::Moved>,
}

impl ArtConfig {
    /// Loads an environment's `art.toml`, treating a missing file as one
    /// that records nothing.
    pub fn load(art_dir: &Path) -> Result<Self> {
        let path = art_dir.join(ART_FILE);
        match files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?
        {
            Some(text) => Self::parse(&path, &text),
            None => Ok(Self::default()),
        }
    }

    /// `path` is only used for error messages.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        config::check_schema(path, text)?;
        let (raw, deprecated): (RawArt, _) =
            config::parse_config(path, text, crate::deprecated::FileKind::Art)?;
        let mut art = raw
            .validate()
            .map_err(|message| config::invalid(path, message))?;
        art.deprecated = deprecated;
        Ok(art)
    }

    /// The records for `target`, or the environment's for None.
    pub fn records(&self, target: Option<&str>) -> Option<&Records> {
        match target {
            Some(target) => self.targets.get(target),
            None => Some(&self.kinds),
        }
    }

    pub fn record(&self, target: Option<&str>, kind: Kind) -> Option<&Record> {
        self.records(target)?.get(&kind)
    }
}

/// An `art.toml` open for editing. Edits keep the comments and formatting of
/// everything they don't touch. Hold the file's config lock from `open`
/// through `save`, so a concurrent change is never overwritten.
#[derive(Debug)]
pub struct ArtFile {
    path: PathBuf,
    doc: DocumentMut,
    /// The text as read, or a new document's for a missing file.
    original: String,
    /// Deprecated keys `doc` has under their new names while it's edited.
    renamed: Vec<crate::deprecated::Renamed>,
    config: ArtConfig,
}

impl ArtFile {
    pub fn open(art_dir: &Path) -> Result<Self> {
        let path = art_dir.join(ART_FILE);
        let (doc, config) = match fs::read_to_string(&path) {
            Ok(text) => {
                let config = ArtConfig::parse(&path, &text)?;
                let doc = text.parse().map_err(|source| Error::ConfigParse {
                    path: path.clone(),
                    source: Box::new(source),
                })?;
                (doc, config)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                (config::new_document(), ArtConfig::default())
            }
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        };
        let original = doc.to_string();
        let mut doc = doc;
        // Edits use the current names, and `save` puts the file's own back.
        let renamed =
            crate::deprecated::rename_document(crate::deprecated::FileKind::Art, &path, &mut doc)?;
        Ok(Self {
            original,
            path,
            doc,
            renamed,
            config,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The records as edited so far.
    pub fn config(&self) -> &ArtConfig {
        &self.config
    }

    pub fn set_game(&mut self, game: Option<u64>) -> Result<()> {
        match game {
            Some(game) => {
                let value = Value::from(self.integer("game", game)?);
                crate::edit::store_value(self.doc.as_table_mut(), "game", value);
            }
            None => {
                self.doc.remove("game");
            }
        }
        self.config.game = game;
        Ok(())
    }

    /// Records `kind` for `target`, or the environment for None.
    pub fn set(&mut self, target: Option<&str>, kind: Kind, record: Record) -> Result<()> {
        let source = match &record.source {
            Source::SteamGridDb(asset) => (
                "steamgriddb",
                Value::from(self.integer(kind.as_str(), *asset)?),
            ),
            Source::Manual(original) => ("manual", Value::from(original.as_str())),
            Source::Gog(game) => ("gog", Value::from(self.integer(kind.as_str(), *game)?)),
        };
        let mut parent = self.doc.as_table_mut();
        if let Some(target) = target {
            parent = child_table(child_table(parent, "targets"), target);
        }
        let table = parent
            .entry(kind.as_str())
            .or_insert_with(|| Item::Table(Table::new()));
        as_table(table);
        let table = table.as_table_mut().expect("just made a table");
        crate::edit::store_value(table, "file", Value::from(kind.file_name(record.format)));
        for key in ["steamgriddb", "manual", "gog"] {
            if key != source.0 {
                table.remove(key);
            }
        }
        crate::edit::store_value(table, source.0, source.1);
        let records = match target {
            Some(target) => self.config.targets.entry(target.to_owned()).or_default(),
            None => &mut self.config.kinds,
        };
        records.insert(kind, record);
        Ok(())
    }

    /// Forgets `kind` for `target`, dropping a target table left empty.
    /// Returns whether there was a record.
    pub fn remove(&mut self, target: Option<&str>, kind: Kind) -> bool {
        let root = self.doc.as_table_mut();
        let removed = match target {
            None => root.remove(kind.as_str()).is_some(),
            Some(target) => {
                let Some(targets) = root.get_mut("targets").and_then(Item::as_table_like_mut)
                else {
                    return false;
                };
                let Some(table) = targets.get_mut(target).and_then(Item::as_table_like_mut) else {
                    return false;
                };
                let removed = table.remove(kind.as_str()).is_some();
                if table.is_empty() {
                    targets.remove(target);
                }
                if targets.is_empty() {
                    root.remove("targets");
                }
                removed
            }
        };
        match target {
            None => {
                self.config.kinds.remove(&kind);
            }
            Some(target) => {
                if let Some(records) = self.config.targets.get_mut(target) {
                    records.remove(&kind);
                    if records.is_empty() {
                        self.config.targets.remove(target);
                    }
                }
            }
        }
        removed
    }

    /// Forgets everything recorded for `target`. Returns whether there was anything.
    pub fn remove_target(&mut self, target: &str) -> bool {
        let mut removed = false;
        for kind in Kind::ALL {
            removed |= self.remove(Some(target), kind);
        }
        removed
    }

    /// Writes the file if anything changed, marking it as the current version.
    /// Returns whether it wrote.
    pub fn save(&mut self) -> Result<bool> {
        // An older release only knows the old names, so the file keeps them.
        let mut written = self.doc.clone();
        crate::deprecated::restore_document(&mut written, &self.renamed);
        if written.to_string() == self.original {
            return Ok(false);
        }
        config::stamp_schema(&mut self.doc);
        config::stamp_schema(&mut written);
        let text = written.to_string();
        self.config = ArtConfig::parse(&self.path, &text)?;
        write_atomic(&self.path, &text)?;
        self.original = text;
        Ok(true)
    }

    fn integer(&self, key: &str, id: u64) -> Result<i64> {
        i64::try_from(id)
            .map_err(|_| config::invalid(&self.path, format!("`{key}` is out of range: {id}")))
    }
}

/// `key` in `parent` as a table, created implicit so it only shows up as part
/// of its children's headers.
fn child_table<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
    let item = parent.entry(key).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    as_table(item);
    item.as_table_mut()
        .expect("art.toml was validated, so this is a table")
}

/// Turns a hand-written inline table into a regular one, so it can be edited
/// like any other.
fn as_table(item: &mut Item) {
    if !item.is_table() {
        let taken = std::mem::take(item);
        *item = match taken.into_table() {
            Ok(table) => Item::Table(table),
            Err(other) => other,
        };
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArt {
    #[serde(default)]
    schema_version: u32,
    game: Option<u64>,
    // Spelled out rather than a flattened `RawKinds`, since serde can't
    // refuse unknown keys through `flatten`.
    icon: Option<RawRecord>,
    grid: Option<RawRecord>,
    hero: Option<RawRecord>,
    logo: Option<RawRecord>,
    #[serde(default)]
    targets: BTreeMap<String, RawKinds>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKinds {
    icon: Option<RawRecord>,
    grid: Option<RawRecord>,
    hero: Option<RawRecord>,
    logo: Option<RawRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    file: String,
    steamgriddb: Option<u64>,
    manual: Option<String>,
    gog: Option<u64>,
}

impl RawArt {
    fn validate(self) -> Result<ArtConfig, String> {
        let mut targets = BTreeMap::new();
        for (name, kinds) in self.targets {
            if !name::is_valid(&name) {
                return Err(format!(
                    "invalid target name `{name}`: use letters, digits, `-`, and `_`"
                ));
            }
            let records = kinds.validate(&format!("targets.{name}."))?;
            targets.insert(name, records);
        }
        let kinds = RawKinds {
            icon: self.icon,
            grid: self.grid,
            hero: self.hero,
            logo: self.logo,
        };
        Ok(ArtConfig {
            schema_version: self.schema_version,
            game: self.game,
            kinds: kinds.validate("")?,
            targets,
            deprecated: Vec::new(),
        })
    }
}

impl RawKinds {
    fn slots(&mut self) -> [(Kind, &mut Option<RawRecord>); 4] {
        [
            (Kind::Icon, &mut self.icon),
            (Kind::Grid, &mut self.grid),
            (Kind::Hero, &mut self.hero),
            (Kind::Logo, &mut self.logo),
        ]
    }

    /// `prefix` places the tables in the file, for error messages.
    fn validate(mut self, prefix: &str) -> Result<Records, String> {
        let mut records = Records::new();
        for (kind, slot) in self.slots() {
            let Some(raw) = slot.take() else { continue };
            let key = format!("{prefix}{kind}");
            let format = Format::ALL
                .into_iter()
                .filter(|format| kind.accepts(*format))
                .find(|format| kind.file_name(*format) == raw.file)
                .ok_or_else(|| {
                    let names: Vec<String> = Format::ALL
                        .into_iter()
                        .filter(|format| kind.accepts(*format))
                        .map(|format| kind.file_name(format))
                        .collect();
                    format!("`{key}.file` must be one of {}", names.join(", "))
                })?;
            let source = match (raw.steamgriddb, raw.manual, raw.gog) {
                (Some(asset), None, None) => Source::SteamGridDb(asset),
                (None, Some(original), None) => Source::Manual(original),
                (None, None, Some(game)) => Source::Gog(game),
                _ => {
                    return Err(format!(
                        "`{key}` needs exactly one of `steamgriddb`, `manual`, and `gog`"
                    ));
                }
            };
            records.insert(kind, Record { format, source });
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests;
