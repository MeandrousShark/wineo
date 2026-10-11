//! Bundled Noto Sans, so every machine renders the same text.
//!
//! Arch's noto-fonts ships no SemiBold, which made iced fall back to a
//! monospace font for every semibold title. The app only asks for Regular and
//! SemiBold, so those are the two weights bundled.

use std::borrow::Cow;

use iced::Font;
use iced::advanced::graphics::text::cosmic_text::fontdb::Database;
use iced::advanced::graphics::text::font_system;

pub(crate) const REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
pub(crate) const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-SemiBold.ttf");

pub(crate) const DEFAULT: Font = Font::with_name("Noto Sans");

const FAMILY: &str = "Noto Sans";

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

/// Clears the way for the bundled faces and points sans-serif at them.
///
/// Every installed "Noto Sans" face goes, whatever its weight or file kind,
/// so a system copy can never win over the bundle or be picked for a weight
/// the bundle lacks. Other families, like "Noto Sans CJK", stay.
///
/// Widgets build their fonts from `Font::DEFAULT`, which is the generic
/// sans-serif family. Without pointing that at Noto Sans, `Font { weight:
/// Semibold, ..Font::DEFAULT }` would skip the bundled font, since only
/// `Settings::default_font` names it.
fn prepare(db: &mut Database) {
    let stale: Vec<_> = db
        .faces()
        .filter(|face| face.families.iter().any(|(name, _)| name == FAMILY))
        .map(|face| face.id)
        .collect();
    for id in stale {
        db.remove_face(id);
    }
    db.set_sans_serif_family(FAMILY);
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::graphics::text::cosmic_text::fontdb::{
        Family, Query, Source, Stretch, Style, Weight,
    };
    use std::sync::Arc;

    fn be16(data: &[u8], at: usize) -> usize {
        usize::from(u16::from_be_bytes([data[at], data[at + 1]]))
    }

    fn table_record(data: &[u8], tag: &[u8; 4]) -> usize {
        (0..be16(data, 4))
            .map(|n| 12 + n * 16)
            .find(|start| &data[*start..*start + 4] == tag)
            .expect("table")
    }

    /// A copy of a bundled font standing in for a system face.
    fn fake(font: &[u8], family: &str, weight: u16) -> Vec<u8> {
        let mut data = font.to_vec();

        let os2 = table_record(&data, b"OS/2");
        let offset = u32::from_be_bytes(data[os2 + 8..os2 + 12].try_into().unwrap()) as usize;
        data[offset + 4..offset + 6].copy_from_slice(&weight.to_be_bytes());

        // A new name table at the end, with the family as ids 1 and 16 and
        // the PostScript name fontdb insists on as id 6.
        let utf16 =
            |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
        let names = [
            (1u16, utf16(family)),
            (6, utf16("Fake")),
            (16, utf16(family)),
        ];
        let mut name = Vec::new();
        for value in [0u16, names.len() as u16, 6 + 12 * names.len() as u16] {
            name.extend_from_slice(&value.to_be_bytes());
        }
        let mut strings = Vec::new();
        for (id, text) in &names {
            let at = strings.len() as u16;
            for value in [3u16, 1, 0x409, *id, text.len() as u16, at] {
                name.extend_from_slice(&value.to_be_bytes());
            }
            strings.extend_from_slice(text);
        }
        name.extend_from_slice(&strings);
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let record = table_record(&data, b"name");
        let start = u32::try_from(data.len()).unwrap();
        data[record + 8..record + 12].copy_from_slice(&start.to_be_bytes());
        data[record + 12..record + 16].copy_from_slice(&(name.len() as u32).to_be_bytes());
        data.extend_from_slice(&name);
        data
    }

    /// Makes a face look like a variable font by tagging an unused table `fvar`.
    fn variable(mut data: Vec<u8>) -> Vec<u8> {
        let record = table_record(&data, b"gasp");
        data[record..record + 4].copy_from_slice(b"fvar");
        data
    }

    fn pointer(source: &Source) -> *const u8 {
        match source {
            Source::Binary(data) => (**data).as_ref().as_ptr(),
            _ => panic!("expected a binary source"),
        }
    }

    fn resolve(db: &Database, weight: Weight) -> *const u8 {
        let id = db
            .query(&Query {
                families: &[Family::SansSerif],
                weight,
                stretch: Stretch::Normal,
                style: Style::Normal,
            })
            .expect("a face");
        pointer(&db.face(id).expect("face").source)
    }

    fn named(db: &Database, family: &str) -> usize {
        db.faces()
            .filter(|face| face.families.iter().any(|(name, _)| name == family))
            .count()
    }

    #[test]
    fn prepare_leaves_only_the_bundled_noto_sans_and_other_families() {
        let mut db = Database::new();
        for (weight, font) in [(400, REGULAR), (600, SEMIBOLD), (700, REGULAR)] {
            db.load_font_source(Source::Binary(Arc::new(fake(font, FAMILY, weight))));
        }
        db.load_font_source(Source::Binary(Arc::new(variable(fake(
            REGULAR, FAMILY, 400,
        )))));
        db.load_font_source(Source::Binary(Arc::new(fake(
            REGULAR,
            "Noto Sans CJK",
            400,
        ))));
        assert_eq!(named(&db, FAMILY), 4, "the fakes load as system faces");
        assert_eq!(named(&db, "Noto Sans CJK"), 1);

        prepare(&mut db);
        db.load_font_source(Source::Binary(Arc::new(REGULAR)));
        db.load_font_source(Source::Binary(Arc::new(SEMIBOLD)));

        assert_eq!(named(&db, FAMILY), 2);
        assert_eq!(named(&db, "Noto Sans CJK"), 1);
        assert_eq!(resolve(&db, Weight::NORMAL), REGULAR.as_ptr());
        assert_eq!(resolve(&db, Weight::SEMIBOLD), SEMIBOLD.as_ptr());
        // A weight the bundle lacks lands on a bundled face, not a system one.
        assert_eq!(resolve(&db, Weight::BOLD), SEMIBOLD.as_ptr());
    }
}
