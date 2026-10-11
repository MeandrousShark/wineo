//! Bundled Noto Sans, so every machine renders the same text.
//!
//! Arch's noto-fonts ships no SemiBold, which made iced fall back to a
//! monospace font for every semibold title.

use iced::Font;
use iced::advanced::graphics::text::font_system;

pub(crate) const REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
pub(crate) const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-SemiBold.ttf");

pub(crate) const DEFAULT: Font = Font::with_name("Noto Sans");

/// Points the generic sans-serif family at Noto Sans.
///
/// Widgets build their fonts from `Font::DEFAULT`, which is the generic
/// sans-serif family. Without this, `Font { weight: Semibold, ..Font::DEFAULT }`
/// would skip the bundled font, since only `Settings::default_font` names it.
pub(crate) fn use_as_sans_serif() {
    let mut system = font_system().write().expect("font system lock");
    system.raw().db_mut().set_sans_serif_family("Noto Sans");
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::graphics::text::cosmic_text::fontdb::{
        Database, Family, Query, Source, Stretch, Style, Weight,
    };
    use std::sync::Arc;

    // Uses its own database so the shared font system, and the other tests
    // measuring text with it, stay untouched.
    #[test]
    fn semibold_sans_serif_resolves_to_bundled_noto_sans() {
        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(REGULAR)));
        db.load_font_source(Source::Binary(Arc::new(SEMIBOLD)));
        db.set_sans_serif_family("Noto Sans");

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
}
