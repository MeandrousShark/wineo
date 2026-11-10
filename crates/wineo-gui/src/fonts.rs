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
use std::sync::atomic::{AtomicU64, Ordering};

use iced::Font;
use iced::advanced::graphics::text::cosmic_text::fontdb::Database;
use iced::advanced::graphics::text::font_system;

pub(crate) const REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
pub(crate) const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-SemiBold.ttf");

/// The bundled family's name.
pub(crate) const BUILT_IN: &str = "Noto Sans";

/// Counts remaps, so measurements kept from an older family are dropped.
static GENERATION: AtomicU64 = AtomicU64::new(0);
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
    system.load_font(Cow::Owned(Vec::new()));
    *CURRENT.lock().expect("current font lock") = family;
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The family in use by name, for widgets that draw through the renderer's
/// text cache, such as pick lists. That cache keys on the font and not the
/// font system's version, so the generic family would keep old glyphs.
pub(crate) fn named() -> Font {
    Font::with_name(current())
}

/// The installed families a user can pick, by name, without the bundle's
/// and without monospace ones, which stay the system's.
pub(crate) fn families() -> Vec<&'static str> {
    #[cfg(test)]
    install_once();
    let mut system = font_system().write().expect("font system lock");
    let mut seen = HashSet::new();
    let mut names: Vec<&'static str> = system
        .raw()
        .db()
        .faces()
        .filter(|face| !face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.as_str()))
        .filter(|name| *name != BUILT_IN && *name != ICONS && seen.insert(*name))
        .map(intern)
        .collect();
    names.sort_by_cached_key(|name| (name.to_lowercase(), *name));
    names
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
