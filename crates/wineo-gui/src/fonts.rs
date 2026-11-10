//! Bundled Noto Sans, so every machine renders the same text, and the
//! Appearance tab's font choice on top of it.
//!
//! Arch's noto-fonts ships no SemiBold, which made iced fall back to a
//! monospace font for every semibold title. The app only asks for Regular and
//! SemiBold, so those are the two weights bundled.
//!
//! Widgets draw with `Font::DEFAULT`, the generic sans-serif family, so a font
//! change is one remap of that family rather than a font on every widget.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};

use iced::Font;
use iced::advanced::graphics::text::cosmic_text::fontdb::{self, Database};
use iced::advanced::graphics::text::cosmic_text::skrifa::{FontRef, GlyphId, MetadataProvider};
use iced::advanced::graphics::text::font_system;
use iced::font::Weight;

pub(crate) const REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
pub(crate) const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-SemiBold.ttf");

/// The bundled family's name.
pub(crate) const BUILT_IN: &str = "Noto Sans";

/// Counts remaps, so measurements kept from an older family are dropped.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The weight semibold text asks for in the family in use.
static HEAVY: AtomicU16 = AtomicU16::new(SEMIBOLD_WEIGHT);
/// The family sans-serif points at.
static CURRENT: Mutex<&str> = Mutex::new(BUILT_IN);

/// Makes the bundled Noto Sans the one every font request resolves to.
///
/// The bundled faces load here, ahead of the settings' font list, because
/// fontdb picks the first of several equal matches and system fonts are
/// already in the database. Iced skips the later load of the same bytes.
pub(crate) fn install() {
    let mut system = font_system().write().expect("font system lock");
    prepare(system.raw().db_mut());
    system.load_font(Cow::Borrowed(REGULAR));
    system.load_font(Cow::Borrowed(SEMIBOLD));
}

/// `install`, once per test process, before any test measures text. The
/// app installs the bundle at startup, so tests measure with it too rather
/// than with whatever fonts the machine has.
#[cfg(test)]
pub(crate) fn install_once() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(install);
}

/// Clears the way for the bundled faces and points sans-serif at them.
///
/// Every installed "Noto Sans" face goes, whatever its weight or file kind,
/// so a system copy can never win over the bundle or be picked for a weight
/// the bundle lacks. Other families, like "Noto Sans CJK", stay.
fn prepare(db: &mut Database) {
    let stale: Vec<_> = db
        .faces()
        .filter(|face| face.families.iter().any(|(name, _)| name == BUILT_IN))
        .map(|face| face.id)
        .collect();
    for id in stale {
        db.remove_face(id);
    }
    db.set_sans_serif_family(BUILT_IN);
}

/// Bumped by every remap.
pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The family text draws in now.
pub(crate) fn current() -> &'static str {
    *CURRENT.lock().expect("current font lock")
}

/// The family `wanted` names when it's installed, otherwise the bundle.
pub(crate) fn resolve(wanted: Option<&str>) -> &'static str {
    #[cfg(test)]
    install_once();
    let Some(wanted) = wanted.filter(|name| *name != BUILT_IN) else {
        return BUILT_IN;
    };
    let mut system = font_system().write().expect("font system lock");
    if has_family(system.raw().db(), wanted) {
        intern(wanted)
    } else {
        BUILT_IN
    }
}

/// Points sans-serif at `family`, which `resolve` gave, and makes all text
/// shape again.
///
/// Iced reshapes a paragraph only when the font system's version changes,
/// and only `load_font` bumps it. The chosen faces are already in fontdb from
/// the system scan, so this loads an empty buffer instead: fontdb adds no
/// face for bytes it can't parse, and the version still goes up.
pub(crate) fn remap(family: &'static str) {
    #[cfg(test)]
    if !live::on() {
        live::record(family);
        return;
    }
    let mut system = font_system().write().expect("font system lock");
    // `db_mut` also drops cosmic-text's cache of font matches.
    let db = system.raw().db_mut();
    db.set_sans_serif_family(family);
    HEAVY.store(heavy_weight(db, family), Ordering::Relaxed);
    system.load_font(Cow::Owned(Vec::new()));
    *CURRENT.lock().expect("current font lock") = family;
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Semibold text in the family in use.
///
/// cosmic-text only takes a family at the exact weight asked for and has no
/// synthetic bold, so semibold in a family without that face would come
/// from another family. This asks for bold instead, then regular, so heavy
/// text keeps the chosen typeface.
pub(crate) fn semibold() -> Font {
    Font {
        weight: match HEAVY.load(Ordering::Relaxed) {
            BOLD_WEIGHT => Weight::Bold,
            NORMAL_WEIGHT => Weight::Normal,
            _ => Weight::Semibold,
        },
        ..Font::DEFAULT
    }
}

const SEMIBOLD_WEIGHT: u16 = 600;
const BOLD_WEIGHT: u16 = 700;
const NORMAL_WEIGHT: u16 = 400;

/// Semibold, then bold, then regular: the first upright weight `family`
/// has. Variable fonts register at their default weight, so they land on
/// that one.
fn heavy_weight(db: &Database, family: &str) -> u16 {
    let weights: Vec<u16> = db
        .faces()
        .filter(|face| face.style == fontdb::Style::Normal)
        .filter(|face| face.families.iter().any(|(name, _)| name == family))
        .map(|face| face.weight.0)
        .collect();
    [SEMIBOLD_WEIGHT, BOLD_WEIGHT]
        .into_iter()
        .find(|weight| weights.contains(weight))
        .unwrap_or(NORMAL_WEIGHT)
}

/// The family in use by name, for widgets that draw through the renderer's
/// text cache, such as pick lists. That cache keys on the font and not the
/// font system's version, so the generic family would keep old glyphs.
pub(crate) fn named() -> Font {
    Font::with_name(current())
}

/// The installed families a user can pick, by name, without the bundle's
/// and without monospace ones, which stay the system's.
pub(crate) struct Families {
    pub(crate) names: Vec<&'static str>,
    /// Those among `names` whose own faces can't draw their names, such as
    /// CJK or Arabic fonts without Latin letters, so their rows show in the
    /// bundle instead of empty boxes.
    pub(crate) fallback: HashSet<&'static str>,
}

/// The families a user can pick, read from the font system now.
pub(crate) fn families() -> Families {
    #[cfg(test)]
    install_once();
    let mut system = font_system().write().expect("font system lock");
    let db = system.raw().db();
    let mut seen = HashSet::new();
    let mut names: Vec<&'static str> = db
        .faces()
        .filter(|face| !face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.as_str()))
        .filter(|name| *name != BUILT_IN && *name != ICONS && seen.insert(*name))
        .map(intern)
        .collect();
    names.sort_by_cached_key(|name| (name.to_lowercase(), *name));
    let fallback = names
        .iter()
        .copied()
        .filter(|name| !draws_own_name(db, name))
        .collect();
    Families { names, fallback }
}

/// `draws_own_name` against the font system. The bundle always can.
pub(crate) fn can_draw_own_name(family: &str) -> bool {
    if family == BUILT_IN {
        return true;
    }
    let mut system = font_system().write().expect("font system lock");
    draws_own_name(system.raw().db(), family)
}

/// Whether the face `family` draws regular upright text with has a glyph for
/// every character of its name but whitespace. A face that can't be found or
/// read counts as not drawing it, since the bundle always can.
fn draws_own_name(db: &Database, family: &str) -> bool {
    #[cfg(test)]
    checks::count();
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };
    let Some(id) = db.query(&query) else {
        return false;
    };
    db.with_face_data(id, |data, index| {
        let Ok(font) = FontRef::from_index(data, index) else {
            return false;
        };
        let charmap = font.charmap();
        family.chars().filter(|ch| !ch.is_whitespace()).all(|ch| {
            charmap
                .map(ch)
                .is_some_and(|glyph| glyph != GlyphId::NOTDEF)
        })
    })
    .unwrap_or(false)
}

/// Counts coverage checks on this thread, so tests can tell they run when
/// the list is built and not when it draws.
#[cfg(test)]
pub(crate) mod checks {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn count() {
        COUNT.set(COUNT.get() + 1);
    }

    pub(crate) fn so_far() -> usize {
        COUNT.get()
    }
}

/// Iced's own icon font, which has no letters.
const ICONS: &str = "Iced-Icons";

fn has_family(db: &Database, family: &str) -> bool {
    db.faces()
        .any(|face| face.families.iter().any(|(name, _)| name == family))
}

/// Family names live as long as the app, since `Font` holds a `&'static
/// str`. Each installed family is kept once, so this stays bounded.
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().expect("font names lock");
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(kept) = names.get(name) {
        return kept;
    }
    let kept: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(kept);
    kept
}

/// Tests share one font system, so a remap there would change what other
/// tests measure. Only a test process of its own turns remapping on; the
/// rest record what they would have remapped to.
#[cfg(test)]
pub(crate) mod live {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ON: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static RECORDED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn on() -> bool {
        ON.load(Ordering::Relaxed)
    }

    /// For a child process that has the font system to itself.
    pub(crate) fn turn_on() {
        ON.store(true, Ordering::Relaxed);
    }

    pub(super) fn record(family: &'static str) {
        RECORDED.with_borrow_mut(|recorded| recorded.push(family));
    }

    /// The remaps this thread asked for since the last call.
    pub(crate) fn take() -> Vec<&'static str> {
        RECORDED.with_borrow_mut(std::mem::take)
    }
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod switch_tests;
#[cfg(test)]
mod tests;
