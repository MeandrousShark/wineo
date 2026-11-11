use super::fixtures::{fake, variable};
use super::*;
use iced::advanced::graphics::text::cosmic_text::fontdb::{
    Family, Query, Source, Stretch, Style, Weight,
};
use std::sync::Arc;

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
        db.load_font_source(Source::Binary(Arc::new(fake(font, BUILT_IN, weight))));
    }
    db.load_font_source(Source::Binary(Arc::new(variable(fake(
        REGULAR, BUILT_IN, 400,
    )))));
    db.load_font_source(Source::Binary(Arc::new(fake(
        REGULAR,
        "Noto Sans CJK",
        400,
    ))));
    assert_eq!(named(&db, BUILT_IN), 4, "the fakes load as system faces");
    assert_eq!(named(&db, "Noto Sans CJK"), 1);

    prepare(&mut db);
    db.load_font_source(Source::Binary(Arc::new(REGULAR)));
    db.load_font_source(Source::Binary(Arc::new(SEMIBOLD)));

    assert_eq!(named(&db, BUILT_IN), 2);
    assert_eq!(named(&db, "Noto Sans CJK"), 1);
    assert_eq!(resolve(&db, Weight::NORMAL), REGULAR.as_ptr());
    assert_eq!(resolve(&db, Weight::SEMIBOLD), SEMIBOLD.as_ptr());
    // A weight the bundle lacks lands on a bundled face, not a system one.
    assert_eq!(resolve(&db, Weight::BOLD), SEMIBOLD.as_ptr());
}

#[test]
fn semibold_asks_for_the_heaviest_weight_up_to_semibold_a_family_has() {
    let mut db = Database::new();
    for (family, weights) in [
        ("Regular Only", &[400][..]),
        ("With Bold", &[400, 700]),
        ("Full", &[300, 400, 600, 700]),
    ] {
        for weight in weights {
            db.load_font_source(Source::Binary(Arc::new(fake(REGULAR, family, *weight))));
        }
    }
    assert_eq!(heavy_weight(&db, "Regular Only"), 400);
    assert_eq!(heavy_weight(&db, "With Bold"), 700);
    assert_eq!(heavy_weight(&db, "Full"), 600);
    // A family the database lacks never gets here, but reads as regular.
    assert_eq!(heavy_weight(&db, "Missing"), 400);
    // The bundle has a semibold, so the app starts there.
    assert_eq!(semibold().weight, iced::font::Weight::Semibold);
}

#[test]
fn a_family_draws_its_own_name_only_when_its_regular_face_covers_it() {
    use super::fixtures::no_latin;
    let mut db = Database::new();
    db.load_font_source(Source::Binary(Arc::new(fake(
        REGULAR,
        "Covers Its Name",
        400,
    ))));
    db.load_font_source(Source::Binary(Arc::new(no_latin(
        REGULAR,
        "Lacks Latin",
        400,
    ))));
    // Only the bold face lacks the letters, and text asks for regular.
    db.load_font_source(Source::Binary(Arc::new(fake(REGULAR, "Bold Lacks", 400))));
    db.load_font_source(Source::Binary(Arc::new(no_latin(
        REGULAR,
        "Bold Lacks",
        700,
    ))));
    assert!(draws_own_name(&db, "Covers Its Name"));
    assert!(!draws_own_name(&db, "Lacks Latin"));
    assert!(draws_own_name(&db, "Bold Lacks"));
    // Noto Sans has no CJK, and no face at all can't draw anything.
    db.load_font_source(Source::Binary(Arc::new(fake(REGULAR, "思源黑体", 400))));
    assert!(!draws_own_name(&db, "思源黑体"));
    assert!(!draws_own_name(&db, "Missing"));
}

#[test]
fn whitespace_in_a_name_needs_no_glyph() {
    use super::fixtures::no_latin;
    let mut db = Database::new();
    db.load_font_source(Source::Binary(Arc::new(no_latin(
        REGULAR,
        " \u{3000}",
        400,
    ))));
    assert!(draws_own_name(&db, " \u{3000}"));
}
