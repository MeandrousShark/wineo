//! The per-environment artwork store.
//!
//! Each kind is one file named for its kind and real format, like `icon.png`
//! or `grid.jpg`, in the environment's art dir. A target can override any kind
//! in a subdirectory named for it.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::{IoContext, Result};

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

/// The art `kind` resolves to for `target`, falling back to the environment's
/// when the target does not override it. `recorded` gives each dir's recorded
/// format for the kind.
pub fn resolve(
    art_dir: &Path,
    target: Option<&str>,
    kind: Kind,
    recorded: impl Fn(Origin) -> Option<Format>,
) -> Result<Option<(Origin, Found)>> {
    if let Some(target) = target {
        let dir = target_dir(art_dir, target);
        if let Some(found) = find(&dir, kind, recorded(Origin::Target))? {
            return Ok(Some((Origin::Target, found)));
        }
    }
    Ok(find(art_dir, kind, recorded(Origin::Environment))?
        .map(|found| (Origin::Environment, found)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let none = |_| None;

        let (origin, found) = resolve(art, Some("editor"), Kind::Icon, none)
            .unwrap()
            .unwrap();
        assert_eq!((origin, found.path), (Origin::Target, target_icon));
        let (origin, found) = resolve(art, Some("editor"), Kind::Hero, none)
            .unwrap()
            .unwrap();
        assert_eq!((origin, found.path), (Origin::Environment, env_hero));
        let (origin, found) = resolve(art, None, Kind::Icon, none).unwrap().unwrap();
        assert_eq!(
            (origin, found.path),
            (Origin::Environment, env_icon.clone())
        );
        let (origin, found) = resolve(art, Some("other"), Kind::Icon, none)
            .unwrap()
            .unwrap();
        assert_eq!((origin, found.path), (Origin::Environment, env_icon));
        assert_eq!(
            resolve(art, Some("editor"), Kind::Logo, none).unwrap(),
            None
        );
    }

    #[test]
    fn each_dir_uses_its_own_record() {
        let root = tempfile::tempdir().unwrap();
        let art = root.path();
        let target = target_dir(art, "editor");
        touch(&target, "logo.png");
        let webp = touch(&target, "logo.webp");
        let recorded = |origin| (origin == Origin::Target).then_some(Format::Webp);
        let (_, found) = resolve(art, Some("editor"), Kind::Logo, recorded)
            .unwrap()
            .unwrap();
        assert_eq!(found.path, webp);
    }
}
