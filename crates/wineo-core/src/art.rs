//! The per-environment artwork store.
//!
//! Each kind is one file named for its kind and real format, like `icon.png`
//! or `grid.jpg`, in the environment's art dir. A target can override any kind
//! in a subdirectory named for it.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::config::{self, SCHEMA_VERSION};
use crate::error::{IoContext, Result};
use crate::files::write_atomic;
use crate::name;

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

    /// Writes `art.toml` into `art_dir`, as the current version.
    pub fn save(&self, art_dir: &Path) -> Result<()> {
        let text = toml::to_string(&RawArt::from(self)).expect("art records always serialize");
        write_atomic(&art_dir.join(ART_FILE), &text)
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

    /// Records `kind` for `target`, or forgets it when `record` is None.
    pub fn set(&mut self, target: Option<&str>, kind: Kind, record: Option<Record>) {
        let records = match target {
            Some(target) => self.targets.entry(target.to_owned()).or_default(),
            None => &mut self.kinds,
        };
        match record {
            Some(record) => records.insert(kind, record),
            None => records.remove(&kind),
        };
        self.targets.retain(|_, records| !records.is_empty());
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArt {
    #[serde(default)]
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    game: Option<u64>,
    // Spelled out rather than a flattened `RawKinds`, since serde can't
    // refuse unknown keys through `flatten`.
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<RawRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grid: Option<RawRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hero: Option<RawRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo: Option<RawRecord>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    targets: BTreeMap<String, RawKinds>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKinds {
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<RawRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grid: Option<RawRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hero: Option<RawRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo: Option<RawRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    steamgriddb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
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

impl From<&ArtConfig> for RawArt {
    fn from(config: &ArtConfig) -> Self {
        let kinds = RawKinds::from(&config.kinds);
        Self {
            schema_version: SCHEMA_VERSION,
            game: config.game,
            icon: kinds.icon,
            grid: kinds.grid,
            hero: kinds.hero,
            logo: kinds.logo,
            targets: config
                .targets
                .iter()
                .filter(|(_, records)| !records.is_empty())
                .map(|(name, records)| (name.clone(), RawKinds::from(records)))
                .collect(),
        }
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

impl From<&Records> for RawKinds {
    fn from(records: &Records) -> Self {
        let mut raw = Self::default();
        for (kind, slot) in raw.slots() {
            *slot = records.get(&kind).map(|record| {
                let (steamgriddb, manual) = match &record.source {
                    Source::SteamGridDb(asset) => (Some(*asset), None),
                    Source::Manual(original) => (None, Some(original.clone())),
                };
                RawRecord {
                    file: kind.file_name(record.format),
                    steamgriddb,
                    manual,
                }
            });
        }
        raw
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

    #[test]
    fn saving_writes_the_current_version_and_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path().join("hades");
        let mut config = example();
        config.save(&art).unwrap();
        let text = fs::read_to_string(art.join(ART_FILE)).unwrap();
        assert_eq!(text, EXAMPLE);
        assert_eq!(ArtConfig::load(&art).unwrap(), config);

        config.schema_version = 0;
        config.set(Some("server"), Kind::Icon, None);
        config.save(&art).unwrap();
        let text = fs::read_to_string(art.join(ART_FILE)).unwrap();
        assert!(text.starts_with("schema_version = 1\n"), "{text}");
        assert!(!text.contains("targets"), "{text}");
    }

    #[test]
    fn set_records_and_forgets_kinds() {
        let mut config = ArtConfig::default();
        config.set(
            Some("server"),
            Kind::Logo,
            Some(manual(Format::Webp, "l.webp")),
        );
        assert_eq!(
            config.record(Some("server"), Kind::Logo),
            Some(&manual(Format::Webp, "l.webp"))
        );
        config.set(Some("server"), Kind::Logo, None);
        assert!(config.targets.is_empty());
        config.set(None, Kind::Hero, Some(manual(Format::Png, "h.png")));
        assert_eq!(config.kinds.len(), 1);
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
}
