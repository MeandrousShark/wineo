//! Bundled Noto Sans, so every machine renders the same text.
//!
//! Arch's noto-fonts ships no SemiBold, which made iced fall back to a
//! monospace font for every semibold title.

use std::borrow::Cow;

use iced::Font;
use iced::advanced::graphics::text::cosmic_text::fontdb::{Database, Style, Weight};
use iced::advanced::graphics::text::font_system;

pub(crate) const REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
pub(crate) const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-SemiBold.ttf");

pub(crate) const DEFAULT: Font = Font::with_name("Noto Sans");

const FAMILY: &str = "Noto Sans";

/// Makes the bundled Noto Sans the one every font request resolves to.
///
/// Widgets build their fonts from `Font::DEFAULT`, which is the generic
/// sans-serif family. Without pointing that at Noto Sans, `Font { weight:
/// Semibold, ..Font::DEFAULT }` would skip the bundled font, since only
/// `Settings::default_font` names it.
///
/// The bundled faces load here, ahead of the settings' font list, because
/// fontdb picks the first of several equal matches and system fonts are
/// already in the database. Iced skips the later load of the same bytes.
pub(crate) fn install() {
    let mut system = font_system().write().expect("font system lock");
    drop_system_faces(system.raw().db_mut());
    system.load_font(Cow::Borrowed(REGULAR));
    system.load_font(Cow::Borrowed(SEMIBOLD));
    system.raw().db_mut().set_sans_serif_family(FAMILY);
}

/// Removes installed static Noto Sans faces at the weights the bundle covers.
///
/// Other weights, like the system Bold, stay. Variable fonts stay too, since
/// one file carries every weight and removing it would take Bold with it.
fn drop_system_faces(db: &mut Database) {
    let stale: Vec<_> = db
        .faces()
        .filter(|face| {
            face.families.iter().any(|(name, _)| name == FAMILY)
                && face.style == Style::Normal
                && (face.weight == Weight::NORMAL || face.weight == Weight::SEMIBOLD)
        })
        .map(|face| face.id)
        .collect();
    for id in stale {
        let variable = db
            .with_face_data(id, |data, _| is_variable(data))
            .unwrap_or(true);
        if !variable {
            db.remove_face(id);
        }
    }
}

/// Whether an sfnt file has an `fvar` table. Collections count as variable so
/// they are left alone.
fn is_variable(data: &[u8]) -> bool {
    let Some(count) = data.get(4..6) else {
        return true;
    };
    if data.starts_with(b"ttcf") {
        return true;
    }
    let count = usize::from(u16::from_be_bytes([count[0], count[1]]));
    (0..count).any(|n| {
        let start = 12 + n * 16;
        data.get(start..start + 4) == Some(b"fvar".as_slice())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::graphics::text::cosmic_text::fontdb::{Family, Query, Source, Stretch};
    use std::sync::Arc;

    fn bundled_db() -> Database {
        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(REGULAR)));
        db.load_font_source(Source::Binary(Arc::new(SEMIBOLD)));
        db.set_sans_serif_family(FAMILY);
        db
    }

    fn face_count(db: &Database, weight: Weight) -> usize {
        db.faces().filter(|face| face.weight == weight).count()
    }

    // Uses its own database so the shared font system, and the other tests
    // measuring text with it, stay untouched.
    #[test]
    fn semibold_sans_serif_resolves_to_bundled_noto_sans() {
        let db = bundled_db();

        let weight = |weight| {
            let id = db
                .query(&Query {
                    families: &[Family::SansSerif],
                    weight,
                    stretch: Stretch::Normal,
                    style: Style::Normal,
                })
                .expect("bundled face");
            db.face(id).expect("face").weight
        };
        assert_eq!(weight(Weight::NORMAL), Weight::NORMAL);
        assert_eq!(weight(Weight::SEMIBOLD), Weight::SEMIBOLD);
    }

    #[test]
    fn installed_static_faces_at_bundled_weights_are_dropped() {
        // Owned copies stand in for faces loaded from the system.
        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(REGULAR.to_vec())));
        db.load_font_source(Source::Binary(Arc::new(SEMIBOLD.to_vec())));

        drop_system_faces(&mut db);

        assert_eq!(face_count(&db, Weight::NORMAL), 0);
        assert_eq!(face_count(&db, Weight::SEMIBOLD), 0);
    }

    #[test]
    fn the_bundled_faces_survive_when_loaded_after_the_drop() {
        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(REGULAR.to_vec())));
        drop_system_faces(&mut db);
        db.load_font_source(Source::Binary(Arc::new(REGULAR)));

        assert_eq!(face_count(&db, Weight::NORMAL), 1);
    }

    #[test]
    fn other_weights_are_kept() {
        // A Regular copy with its OS/2 weight class patched to 700 stands in
        // for a system Bold.
        let mut bold = REGULAR.to_vec();
        let count = usize::from(u16::from_be_bytes([bold[4], bold[5]]));
        let record = (0..count)
            .map(|n| 12 + n * 16)
            .find(|start| &bold[*start..*start + 4] == b"OS/2")
            .expect("OS/2 table");
        let offset = u32::from_be_bytes(bold[record + 8..record + 12].try_into().unwrap());
        let weight_class = offset as usize + 4;
        bold[weight_class..weight_class + 2].copy_from_slice(&700u16.to_be_bytes());

        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(bold)));
        drop_system_faces(&mut db);

        assert_eq!(face_count(&db, Weight::BOLD), 1);
    }

    #[test]
    fn variable_fonts_are_recognised() {
        assert!(!is_variable(REGULAR));
        assert!(!is_variable(SEMIBOLD));

        let mut variable = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        variable.extend_from_slice(b"fvar");
        variable.extend_from_slice(&[0; 12]);
        assert!(is_variable(&variable));
        assert!(is_variable(b"ttcf\0\0\0\0\0\0\0\0"));
        assert!(is_variable(b""));
    }
}
