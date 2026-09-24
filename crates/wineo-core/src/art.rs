//! The per-environment artwork store.
//!
//! Each kind is one file named for its kind and real format, like `icon.png`
//! or `grid.jpg`, in the environment's art dir. A target can override any kind
//! in a subdirectory named for it.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::config;
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::name;
use crate::operation::Locks;
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
) -> Result<Stored> {
    check_spot(paths, env, target)?;
    let bytes = read_art(file)?;
    let format = detect(&bytes)
        .filter(|format| kind.accepts(*format))
        .ok_or_else(|| Error::ArtFormat {
            path: file.to_owned(),
            accepted: accepted(kind),
        })?;
    let original = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let art_dir = paths.art_dir(env);
    let _guard = Locks::new(paths).config(&art_dir.join(ART_FILE))?;
    // Checked again under the lock, so art never outlives a concurrent `rm`.
    Environment::ensure_exists(paths, env)?;
    let mut art = ArtFile::open(&art_dir)?;
    let dir = spot_dir(&art_dir, target);
    fs::create_dir_all(&dir).context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(kind.file_name(format));
    let tmp = dir.join(format!(".{}.tmp", kind.file_name(format)));
    fs::write(&tmp, &bytes).context(|| format!("failed to write {}", path.display()))?;
    fs::rename(&tmp, &path).context(|| format!("failed to write {}", path.display()))?;
    art.set(
        target,
        kind,
        Record {
            format,
            source: Source::Manual(original),
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

/// Something `art list` warns about. None of them change the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// Files that lose to the one a kind resolves to.
    Duplicates {
        target: Option<String>,
        kind: Kind,
        used: PathBuf,
        ignored: Vec<PathBuf>,
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
    pub targets: Vec<(String, Vec<Resolved>)>,
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
    for target in environment.config.targets.keys() {
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
            overrides.extend(resolved);
        }
        if !overrides.is_empty() {
            targets.push((target.clone(), overrides));
        }
    }
    for target in config.targets.keys() {
        if !environment.config.targets.contains_key(target) {
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
        });
    }
}

/// Refuses art for an environment or target that doesn't exist.
fn check_spot(paths: &Paths, env: &str, target: Option<&str>) -> Result<()> {
    match target {
        None => Environment::ensure_exists(paths, env),
        Some(target) => {
            let environment = Environment::load(paths, env)?;
            if environment.config.targets.contains_key(target) {
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
fn accepted(kind: Kind) -> String {
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

/// The file in an environment's art dir that records where its art came from.
pub const ART_FILE: &str = "art.toml";

/// Where a piece of art came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A SteamGridDB asset ID.
    SteamGridDb(u64),
    /// Set by hand from a file with this name.
    Manual(String),
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
}

impl ArtConfig {
    /// Loads an environment's `art.toml`, treating a missing file as one
    /// that records nothing.
    pub fn load(art_dir: &Path) -> Result<Self> {
        let path = art_dir.join(ART_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&path, &text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).context(|| format!("failed to read {}", path.display())),
        }
    }

    /// `path` is only used for error messages.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        config::check_schema(path, text)?;
        let raw: RawArt = config::parse_toml(path, text)?;
        raw.validate()
            .map_err(|message| config::invalid(path, message))
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
        Ok(Self {
            original: doc.to_string(),
            path,
            doc,
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
        for key in ["steamgriddb", "manual"] {
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

    /// Writes the file if anything changed, marking it as the current version.
    /// Returns whether it wrote.
    pub fn save(&mut self) -> Result<bool> {
        if self.doc.to_string() == self.original {
            return Ok(false);
        }
        config::stamp_schema(&mut self.doc);
        let text = self.doc.to_string();
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
            let source = match (raw.steamgriddb, raw.manual) {
                (Some(asset), None) => Source::SteamGridDb(asset),
                (None, Some(original)) => Source::Manual(original),
                _ => {
                    return Err(format!(
                        "`{key}` needs exactly one of `steamgriddb` and `manual`"
                    ));
                }
            };
            records.insert(kind, Record { format, source });
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF\0";
    const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";

    /// One 16x16 image whose 4 data bytes follow the directory.
    fn ico() -> Vec<u8> {
        let mut bytes = vec![0, 0, 1, 0, 1, 0];
        bytes.extend([16, 16, 0, 0, 1, 0, 32, 0]);
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(22u32.to_le_bytes());
        bytes.extend([1, 2, 3, 4]);
        bytes
    }

    #[test]
    fn magic_bytes_detect_raster_formats() {
        assert_eq!(detect(PNG), Some(Format::Png));
        assert_eq!(detect(JPEG), Some(Format::Jpeg));
        assert_eq!(detect(WEBP), Some(Format::Webp));
        assert_eq!(detect(&ico()), Some(Format::Ico));
        assert_eq!(detect(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(detect(b"RIFF"), None);
        assert_eq!(detect(b""), None);
    }

    #[test]
    fn a_renamed_text_file_is_refused() {
        assert_eq!(detect(b"just some notes\n"), None);
        assert_eq!(detect(b"<html><body></body></html>"), None);
    }

    #[test]
    fn a_weak_or_truncated_ico_is_refused() {
        let full = ico();
        assert_eq!(detect(&full[..full.len() - 1]), None);
        assert_eq!(detect(&full[..10]), None);
        assert_eq!(detect(&full[..6]), None);
        let mut empty = full.clone();
        empty[4] = 0;
        assert_eq!(detect(&empty), None);
        let mut cursor = full.clone();
        cursor[2] = 2;
        assert_eq!(detect(&cursor), None);
        let mut many = full.clone();
        many[4] = 200;
        assert_eq!(detect(&many), None);
        let mut inside = full;
        inside[18..22].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(detect(&inside), None);
    }

    #[test]
    fn svg_is_sniffed_past_its_prolog() {
        for text in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            "<svg>",
            "\u{feff}<svg/>",
            "\n\t <?xml version=\"1.0\"?>\n<svg width=\"1\">",
            "<?xml version=\"1.0\"?><!-- made by <hand> --><svg\n>",
            "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x>y.dtd\">\n<svg>",
            "<!DOCTYPE svg [ <!ENTITY a \"<b>\"> ]><!-- c --><svg>",
        ] {
            assert_eq!(detect(text.as_bytes()), Some(Format::Svg), "{text}");
        }
    }

    #[test]
    fn xml_without_an_svg_root_is_refused() {
        for text in [
            "<?xml version=\"1.0\"?><html><svg/></html>",
            "<svgx>",
            "<svg",
            "<!-- <svg> -->",
            "<!-- never closed <svg>",
            "<?xml never closed <svg>",
            "<!DOCTYPE svg [ <svg>",
            "text <svg>",
        ] {
            assert_eq!(detect(text.as_bytes()), None, "{text}");
        }
    }

    #[test]
    fn each_kind_accepts_its_formats() {
        for kind in Kind::ALL {
            for format in [Format::Png, Format::Svg, Format::Jpeg] {
                assert!(kind.accepts(format), "{kind} {format}");
            }
            assert_eq!(kind.accepts(Format::Ico), kind == Kind::Icon, "{kind}");
            assert_eq!(kind.accepts(Format::Webp), kind != Kind::Icon, "{kind}");
        }
    }

    #[test]
    fn kinds_parse_and_name_their_files() {
        assert_eq!("hero".parse::<Kind>(), Ok(Kind::Hero));
        assert!("banner".parse::<Kind>().is_err());
        assert_eq!(Kind::Grid.file_name(Format::Jpeg), "grid.jpg");
        assert_eq!(Kind::Icon.file_name(Format::Ico), "icon.ico");
        assert_eq!(Kind::Logo.file_name(Format::Webp), "logo.webp");
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn missing_art_finds_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(find(root.path(), Kind::Icon, None).unwrap(), None);
        let absent = root.path().join("absent");
        assert_eq!(find(&absent, Kind::Icon, Some(Format::Png)).unwrap(), None);
    }

    #[test]
    fn unrecorded_duplicates_resolve_in_order() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let webp = touch(dir, "icon.webp");
        let jpg = touch(dir, "icon.jpg");
        let ico = touch(dir, "icon.ico");
        let svg = touch(dir, "icon.svg");
        let png = touch(dir, "icon.png");
        let found = find(dir, Kind::Icon, None).unwrap().unwrap();
        // WebP is not an icon format, so it is not art at all.
        assert_eq!(
            found,
            Found {
                path: png.clone(),
                format: Format::Png,
                duplicates: vec![svg.clone(), ico.clone(), jpg.clone()],
            }
        );
        fs::remove_file(&png).unwrap();
        fs::remove_file(&svg).unwrap();
        let found = find(dir, Kind::Icon, None).unwrap().unwrap();
        assert_eq!((found.format, found.duplicates), (Format::Ico, vec![jpg]));
        assert!(webp.exists());
    }

    #[test]
    fn the_recorded_format_wins_when_its_file_exists() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let png = touch(dir, "grid.png");
        let webp = touch(dir, "grid.webp");
        let found = find(dir, Kind::Grid, Some(Format::Webp)).unwrap().unwrap();
        assert_eq!(
            (found.path, found.duplicates),
            (webp.clone(), vec![png.clone()])
        );
        let found = find(dir, Kind::Grid, Some(Format::Jpeg)).unwrap().unwrap();
        assert_eq!((found.path, found.duplicates), (png, vec![webp]));
    }

    #[test]
    fn directories_named_like_art_are_not_art() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("logo.png")).unwrap();
        assert_eq!(find(root.path(), Kind::Logo, None).unwrap(), None);
    }

    #[test]
    fn a_target_falls_back_to_its_environment() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        let env_icon = touch(art, "icon.png");
        let env_hero = touch(art, "hero.jpg");
        let target_icon = touch(&target_dir(art, "editor"), "icon.svg");
        let config = ArtConfig::default();
        let at = |target, kind| {
            resolve(art, &config, target, kind)
                .unwrap()
                .map(|r| (r.origin, r.found.path))
        };

        assert_eq!(
            at(Some("editor"), Kind::Icon),
            Some((Origin::Target, target_icon))
        );
        assert_eq!(
            at(Some("editor"), Kind::Hero),
            Some((Origin::Environment, env_hero))
        );
        assert_eq!(
            at(None, Kind::Icon),
            Some((Origin::Environment, env_icon.clone()))
        );
        assert_eq!(
            at(Some("other"), Kind::Icon),
            Some((Origin::Environment, env_icon))
        );
        assert_eq!(at(Some("editor"), Kind::Logo), None);
    }

    const EXAMPLE: &str = "schema_version = 1
game = 5247

[icon]
file = \"icon.png\"
steamgriddb = 36952

[grid]
file = \"grid.jpg\"
manual = \"poster final.jpg\"

[targets.server.icon]
file = \"icon.svg\"
manual = \"server.svg\"
";

    fn example() -> ArtConfig {
        ArtConfig::parse(Path::new("art.toml"), EXAMPLE).unwrap()
    }

    fn manual(format: Format, original: &str) -> Record {
        Record {
            format,
            source: Source::Manual(original.to_owned()),
        }
    }

    #[test]
    fn the_plan_example_parses() {
        let config = example();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.game, Some(5247));
        assert_eq!(
            config.record(None, Kind::Icon),
            Some(&Record {
                format: Format::Png,
                source: Source::SteamGridDb(36952),
            })
        );
        assert_eq!(
            config.record(None, Kind::Grid),
            Some(&manual(Format::Jpeg, "poster final.jpg"))
        );
        assert_eq!(config.record(None, Kind::Hero), None);
        assert_eq!(
            config.record(Some("server"), Kind::Icon),
            Some(&manual(Format::Svg, "server.svg"))
        );
        assert_eq!(config.record(Some("server"), Kind::Grid), None);
        assert_eq!(config.record(Some("other"), Kind::Icon), None);
    }

    fn read(art: &Path) -> String {
        fs::read_to_string(art.join(ART_FILE)).unwrap()
    }

    #[test]
    fn a_new_file_matches_the_plan_example() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path().join("hades");
        let mut file = ArtFile::open(&art).unwrap();
        assert!(!file.save().unwrap());
        assert!(!art.exists());

        file.set_game(Some(5247)).unwrap();
        let icon = Record {
            format: Format::Png,
            source: Source::SteamGridDb(36952),
        };
        file.set(None, Kind::Icon, icon).unwrap();
        file.set(None, Kind::Grid, manual(Format::Jpeg, "poster final.jpg"))
            .unwrap();
        file.set(
            Some("server"),
            Kind::Icon,
            manual(Format::Svg, "server.svg"),
        )
        .unwrap();
        assert_eq!(
            file.config(),
            &ArtConfig {
                schema_version: 0,
                ..example()
            }
        );
        assert!(file.save().unwrap());
        assert_eq!(read(&art), EXAMPLE);
        assert_eq!(ArtConfig::load(&art).unwrap(), example());
        assert!(!ArtFile::open(&art).unwrap().save().unwrap());
    }

    #[test]
    fn comments_survive_a_set_and_a_forget() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        let original = "# Art for hades
schema_version = 1

[icon] # the official one
file = \"icon.png\" # keep this
steamgriddb = 36952

[targets.server.logo]
file = \"logo.png\"
manual = \"logo.png\"
";
        fs::write(art.join(ART_FILE), original).unwrap();
        let mut file = ArtFile::open(art).unwrap();
        file.set(None, Kind::Icon, manual(Format::Png, "mine.png"))
            .unwrap();
        file.set(None, Kind::Hero, manual(Format::Jpeg, "wide.jpg"))
            .unwrap();
        assert!(file.remove(Some("server"), Kind::Logo));
        assert!(!file.remove(Some("server"), Kind::Logo));
        assert!(!file.remove(None, Kind::Grid));
        assert!(file.save().unwrap());
        let text = read(art);
        assert!(text.starts_with("# Art for hades\n"), "{text}");
        assert!(text.contains("[icon] # the official one\n"), "{text}");
        assert!(text.contains("file = \"icon.png\" # keep this\n"), "{text}");
        assert!(text.contains("schema_version = 1\n"), "{text}");
        assert!(!text.contains("steamgriddb"), "{text}");
        assert!(!text.contains("targets"), "{text}");

        let config = ArtConfig::load(art).unwrap();
        assert_eq!(
            config.record(None, Kind::Icon),
            Some(&manual(Format::Png, "mine.png"))
        );
        assert_eq!(
            config.record(None, Kind::Hero),
            Some(&manual(Format::Jpeg, "wide.jpg"))
        );
        assert!(config.targets.is_empty());

        let mut file = ArtFile::open(art).unwrap();
        assert!(file.remove(None, Kind::Hero));
        assert!(file.save().unwrap());
        let text = read(art);
        assert!(text.starts_with("# Art for hades\n"), "{text}");
        assert!(text.contains("# keep this"), "{text}");
        assert!(!text.contains("hero"), "{text}");
    }

    #[test]
    fn inline_tables_can_be_edited() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        fs::write(
            art.join(ART_FILE),
            "schema_version = 1\nicon = { file = \"icon.png\", manual = \"a.png\" }\n\
             targets = { server = { grid = { file = \"grid.png\", manual = \"b.png\" } } }\n",
        )
        .unwrap();
        let mut file = ArtFile::open(art).unwrap();
        let asset = |format| Record {
            format,
            source: Source::SteamGridDb(7),
        };
        file.set(None, Kind::Icon, asset(Format::Ico)).unwrap();
        file.set(Some("server"), Kind::Logo, asset(Format::Webp))
            .unwrap();
        file.save().unwrap();
        let config = ArtConfig::load(art).unwrap();
        assert_eq!(config.record(None, Kind::Icon), Some(&asset(Format::Ico)));
        assert_eq!(
            config.record(Some("server"), Kind::Logo),
            Some(&asset(Format::Webp))
        );
        assert!(config.record(Some("server"), Kind::Grid).is_some());
    }

    #[test]
    fn an_edited_legacy_file_gets_the_version() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        fs::write(art.join(ART_FILE), "game = 1\n").unwrap();
        let mut file = ArtFile::open(art).unwrap();
        assert!(!file.save().unwrap());
        assert_eq!(read(art), "game = 1\n");
        file.set_game(Some(2)).unwrap();
        file.save().unwrap();
        assert_eq!(read(art), "game = 2\nschema_version = 1\n");
        assert_eq!(file.config().schema_version, 1);
    }

    #[test]
    fn an_absent_art_toml_records_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(ArtConfig::load(root.path()).unwrap(), ArtConfig::default());
        assert_eq!(
            ArtConfig::load(&root.path().join("absent")).unwrap(),
            ArtConfig::default()
        );
    }

    #[test]
    fn a_missing_schema_version_is_legacy() {
        let text = "[logo]\nfile = \"logo.png\"\nsteamgriddb = 1\n";
        let config = ArtConfig::parse(Path::new("art.toml"), text).unwrap();
        assert_eq!(config.schema_version, 0);
        assert!(config.record(None, Kind::Logo).is_some());
    }

    #[test]
    fn unsupported_schema_versions_are_refused() {
        for version in ["0", "2", "\"1\"", "1.0"] {
            let text = format!("schema_version = {version}\n");
            let err = ArtConfig::parse(Path::new("art.toml"), &text).unwrap_err();
            assert!(
                matches!(err, Error::SchemaVersion { .. }),
                "{version}: {err}"
            );
        }
    }

    #[test]
    fn unknown_or_missing_keys_are_refused() {
        for text in [
            "schema_version = 1\ncolor = 1\n",
            "[icon]\nfile = \"icon.png\"\nmanual = \"a\"\nscore = 3\n",
            "[banner]\nfile = \"banner.png\"\nmanual = \"a\"\n",
            "[targets.server.banner]\nfile = \"banner.png\"\nmanual = \"a\"\n",
            "[icon]\nmanual = \"a\"\n",
            "game = -1\n",
        ] {
            let err = ArtConfig::parse(Path::new("art.toml"), text).unwrap_err();
            assert!(matches!(err, Error::ConfigSyntax { .. }), "{text}: {err}");
        }
    }

    #[test]
    fn a_kind_needs_exactly_one_source_and_a_valid_file() {
        for text in [
            "[icon]\nfile = \"icon.png\"\n",
            "[icon]\nfile = \"icon.png\"\nsteamgriddb = 1\nmanual = \"a\"\n",
            "[targets.server.hero]\nfile = \"hero.png\"\n",
            "[icon]\nfile = \"icon.webp\"\nmanual = \"a\"\n",
            "[icon]\nfile = \"grid.png\"\nmanual = \"a\"\n",
            "[icon]\nfile = \"../icon.png\"\nmanual = \"a\"\n",
            "[grid]\nfile = \"grid.jpeg\"\nmanual = \"a\"\n",
            "[targets.\"bad.name\".icon]\nfile = \"icon.png\"\nmanual = \"a\"\n",
        ] {
            let err = ArtConfig::parse(Path::new("art.toml"), text).unwrap_err();
            assert!(matches!(err, Error::ConfigInvalid { .. }), "{text}: {err}");
        }
    }

    #[test]
    fn recorded_files_win_and_others_are_unrecorded() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        let png = touch(art, "grid.png");
        let jpg = touch(art, "grid.jpg");
        let config = example();

        let grid = resolve(art, &config, None, Kind::Grid).unwrap().unwrap();
        assert_eq!(grid.found.path, jpg);
        assert_eq!(grid.found.duplicates, vec![png.clone()]);
        assert_eq!(grid.source, Some(Source::Manual("poster final.jpg".into())));

        // The recorded file is gone, so what is left is unrecorded.
        fs::remove_file(&jpg).unwrap();
        let grid = resolve(art, &config, None, Kind::Grid).unwrap().unwrap();
        assert_eq!((grid.found.path, grid.source), (png, None));

        let hero = touch(art, "hero.webp");
        let resolved = resolve(art, &config, None, Kind::Hero).unwrap().unwrap();
        assert_eq!((resolved.found.path, resolved.source), (hero, None));
        let resolved = resolve(art, &ArtConfig::default(), None, Kind::Grid)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, None);
    }

    #[test]
    fn each_dir_uses_its_own_record() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        let env_png = touch(art, "icon.png");
        let target = target_dir(art, "server");
        touch(&target, "icon.png");
        let svg = touch(&target, "icon.svg");
        let config = example();

        let icon = resolve(art, &config, Some("server"), Kind::Icon)
            .unwrap()
            .unwrap();
        assert_eq!(icon.origin, Origin::Target);
        assert_eq!(icon.found.path, svg);
        assert_eq!(icon.source, Some(Source::Manual("server.svg".into())));

        let icon = resolve(art, &config, None, Kind::Icon).unwrap().unwrap();
        assert_eq!(icon.found.path, env_png);
        assert_eq!(icon.source, Some(Source::SteamGridDb(36952)));

        // The target records its SVG, so its leftover PNG is unrecorded.
        fs::remove_file(&svg).unwrap();
        let icon = resolve(art, &config, Some("server"), Kind::Icon)
            .unwrap()
            .unwrap();
        assert_eq!((icon.origin, icon.source), (Origin::Target, None));
    }

    fn store() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| std::ffi::OsString::from(home.as_os_str()))
        })
        .unwrap();
        let config = paths.environment_config("hades");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n\
             [targets.server]\nexe = \"server.exe\"\n",
        )
        .unwrap();
        (root, paths)
    }

    fn source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join("from").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn set_keeps_the_bytes_and_records_the_original_name() {
        let (root, paths) = store();
        let file = source(root.path(), "My Poster.webp", WEBP);
        let stored = set_manual(&paths, "hades", None, Kind::Grid, &file).unwrap();
        let art = paths.art_dir("hades");
        assert_eq!(stored.path, art.join("grid.webp"));
        assert_eq!(stored.format, Format::Webp);
        assert_eq!(fs::read(&stored.path).unwrap(), WEBP);
        let text = read(&art);
        assert!(text.starts_with("schema_version = 1\n"), "{text}");
        assert_eq!(
            ArtConfig::load(&art).unwrap().record(None, Kind::Grid),
            Some(&manual(Format::Webp, "My Poster.webp"))
        );
        let listing = list(&paths, "hades").unwrap();
        let (_, grid) = &listing.kinds[1];
        assert_eq!(
            grid.as_ref().unwrap().source,
            Some(Source::Manual("My Poster.webp".into()))
        );
    }

    #[test]
    fn set_refuses_by_content_kind_and_size() {
        let (root, paths) = store();
        let text = source(root.path(), "icon.png", b"not an image");
        let icon = ico();
        let truncated = source(root.path(), "short.ico", &icon[..icon.len() - 1]);
        let svg = source(root.path(), "page.svg", b"<?xml version=\"1.0\"?><html/>");
        let webp = source(root.path(), "icon.webp", WEBP);
        for (file, kind) in [
            (&text, Kind::Icon),
            (&truncated, Kind::Icon),
            (&svg, Kind::Logo),
            (&webp, Kind::Icon),
        ] {
            let err = set_manual(&paths, "hades", None, kind, file).unwrap_err();
            assert!(matches!(err, Error::ArtFormat { .. }), "{err}");
        }
        let err = set_manual(
            &paths,
            "hades",
            None,
            Kind::Grid,
            &source(root.path(), "x.ico", &icon),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{} is not a PNG, SVG, JPEG, or WebP image",
                root.path().join("from/x.ico").display()
            )
        );

        let mut big = PNG.to_vec();
        big.resize(MAX_BYTES as usize + 1, 0);
        let big = source(root.path(), "big.png", &big);
        let err = set_manual(&paths, "hades", None, Kind::Hero, &big).unwrap_err();
        assert!(matches!(err, Error::ArtTooLarge(_)), "{err}");
        let mut limit = PNG.to_vec();
        limit.resize(MAX_BYTES as usize, 0);
        let limit = source(root.path(), "limit.png", &limit);
        set_manual(&paths, "hades", None, Kind::Hero, &limit).unwrap();

        let ok = source(root.path(), "ok.png", PNG);
        let err = set_manual(&paths, "hades", Some("nope"), Kind::Icon, &ok).unwrap_err();
        assert!(matches!(err, Error::UnknownTarget { .. }), "{err}");
        let err = set_manual(&paths, "nope", None, Kind::Icon, &ok).unwrap_err();
        assert!(matches!(err, Error::UnknownEnvironment(_)), "{err}");
        let art = paths.art_dir("hades");
        assert!(!art.join("icon.png").exists());
        assert!(!art.join("grid.ico").exists());
    }

    #[test]
    fn set_replaces_a_kind_that_changes_format() {
        let (root, paths) = store();
        let art = paths.art_dir("hades");
        let unrecorded = touch(&art, "icon.svg");
        let first = source(root.path(), "a.png", PNG);
        let stored = set_manual(&paths, "hades", None, Kind::Icon, &first).unwrap();
        assert_eq!(stored.removed, vec![unrecorded.clone()]);
        let second = source(root.path(), "b.ico", &ico());
        let stored = set_manual(&paths, "hades", None, Kind::Icon, &second).unwrap();
        assert_eq!(stored.path, art.join("icon.ico"));
        assert_eq!(stored.removed, vec![art.join("icon.png")]);
        let names: Vec<_> = fs::read_dir(&art)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"icon.ico".to_owned()));
    }

    #[test]
    fn a_target_override_is_set_and_removed_on_its_own() {
        let (root, paths) = store();
        let art = paths.art_dir("hades");
        let env_icon = source(root.path(), "env.png", PNG);
        set_manual(&paths, "hades", None, Kind::Icon, &env_icon).unwrap();
        let svg = source(root.path(), "server.svg", b"<svg/>");
        let stored = set_manual(&paths, "hades", Some("server"), Kind::Icon, &svg).unwrap();
        assert_eq!(stored.path, art.join("server/icon.svg"));
        let listing = list(&paths, "hades").unwrap();
        assert_eq!(listing.targets.len(), 1);
        assert_eq!(listing.targets[0].1[0].found.path, stored.path);

        let removed = remove(&paths, "hades", Some("server"), Kind::Icon).unwrap();
        assert_eq!(removed.files, vec![stored.path]);
        assert!(removed.record);
        assert!(!art.join("server").exists());
        assert!(art.join("icon.png").exists());
        let config = ArtConfig::load(&art).unwrap();
        assert!(config.targets.is_empty());
        assert!(config.record(None, Kind::Icon).is_some());

        let err = remove(&paths, "hades", Some("server"), Kind::Icon).unwrap_err();
        assert_eq!(err.to_string(), "`hades.server` has no icon art");
        let err = remove(&paths, "hades", None, Kind::Logo).unwrap_err();
        assert_eq!(err.to_string(), "`hades` has no logo art");
    }

    #[test]
    fn remove_takes_unrecorded_files_and_stale_records() {
        let (_root, paths) = store();
        let art = paths.art_dir("hades");
        let png = touch(&art, "hero.png");
        let jpg = touch(&art, "hero.jpg");
        let removed = remove(&paths, "hades", None, Kind::Hero).unwrap();
        assert_eq!((removed.files, removed.record), (vec![png, jpg], false));
        assert!(!art.join(ART_FILE).exists());

        fs::write(
            art.join(ART_FILE),
            "schema_version = 1\n[logo]\nfile = \"logo.png\"\nsteamgriddb = 3\n",
        )
        .unwrap();
        let removed = remove(&paths, "hades", None, Kind::Logo).unwrap();
        assert_eq!((removed.files.len(), removed.record), (0, true));
        assert_eq!(read(&art), "schema_version = 1\n");
    }

    #[test]
    fn list_warns_without_changing_anything() {
        let (_root, paths) = store();
        let art = paths.art_dir("hades");
        let text = "schema_version = 1
[icon]
file = \"icon.svg\"
manual = \"a.svg\"

[targets.gone.logo]
file = \"logo.png\"
manual = \"b.png\"
";
        fs::create_dir_all(&art).unwrap();
        fs::write(art.join(ART_FILE), text).unwrap();
        let png = touch(&art, "grid.png");
        let jpg = touch(&art, "grid.jpg");
        touch(&target_dir(&art, "gone"), "logo.png");

        let listing = list(&paths, "hades").unwrap();
        assert_eq!(
            listing.warnings,
            vec![
                Warning::MissingFile {
                    target: None,
                    kind: Kind::Icon,
                    path: art.join("icon.svg"),
                },
                Warning::Duplicates {
                    target: None,
                    kind: Kind::Grid,
                    used: png,
                    ignored: vec![jpg],
                },
                Warning::UnknownTarget("gone".into()),
            ]
        );
        assert_eq!(listing.kinds[0], (Kind::Icon, None));
        assert!(listing.targets.is_empty());
        assert_eq!(read(&art), text);
    }

    #[test]
    fn remove_all_takes_the_whole_dir() {
        let (root, paths) = store();
        assert!(!remove_all(&paths, "hades").unwrap());
        let file = source(root.path(), "a.png", PNG);
        set_manual(&paths, "hades", Some("server"), Kind::Icon, &file).unwrap();
        assert!(remove_all(&paths, "hades").unwrap());
        assert!(!paths.art_dir("hades").exists());
    }
}
