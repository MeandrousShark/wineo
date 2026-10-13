use super::*;

#[test]
fn the_default_maps_to_english() {
    assert_eq!(installer_code(DEFAULT).unwrap(), "en");
}

#[test]
fn every_table_entry_maps_to_its_code() {
    for (tag, code) in [
        ("zh", "cn"),
        ("zh-CN", "cn"),
        ("zh-Hans", "cn"),
        ("zh-Hant-TW", "cn"),
        ("ja", "jp"),
        ("ja-JP", "jp"),
        ("cs", "cz"),
        ("cs-CZ", "cz"),
        ("el", "gk"),
        ("el-GR", "gk"),
        ("pt-BR", "br"),
        ("es-MX", "es_mx"),
        ("es-419", "es_mx"),
    ] {
        assert_eq!(installer_code(tag).unwrap(), code, "{tag}");
    }
}

#[test]
fn other_regions_fall_back_to_the_language() {
    for (tag, code) in [
        ("en", "en"),
        ("en-US", "en"),
        ("en-GB", "en"),
        ("de-AT", "de"),
        ("fr-FR", "fr"),
        ("pt", "pt"),
        ("pt-PT", "pt"),
        ("es", "es"),
        ("es-ES", "es"),
        ("es-AR", "es"),
        ("fil", "fil"),
    ] {
        assert_eq!(installer_code(tag).unwrap(), code, "{tag}");
    }
}

#[test]
fn tags_match_case_insensitively() {
    assert_eq!(installer_code("PT-br").unwrap(), "br");
    assert_eq!(installer_code("ES-mx").unwrap(), "es_mx");
    assert_eq!(installer_code("ZH-hans").unwrap(), "cn");
    assert_eq!(installer_code("DE-at").unwrap(), "de");
}

#[test]
fn malformed_tags_are_refused() {
    const MESSAGE: &str = "Invalid gog.language: use a locale tag like en-US.";
    for bad in [
        "",
        "e",
        "english",
        "en_US",
        "en-",
        "-US",
        "en-U",
        "en-USA",
        "en-US-x",
        "e1",
        " en",
        "en-Latn-US-1",
        "en-Hans-Hant",
        "zh-4",
        "pt-BR ",
    ] {
        assert_eq!(validate(bad).unwrap_err(), MESSAGE, "{bad:?}");
        assert_eq!(installer_code(bad).unwrap_err(), MESSAGE, "{bad:?}");
    }
}

#[test]
fn well_formed_tags_outside_the_table_are_accepted() {
    for good in [
        "en", "en-US", "fr-FR", "pt-BR", "zh-Hans", "zh-CN", "es-419", "xx", "tlh",
    ] {
        validate(good).unwrap_or_else(|error| panic!("{good}: {error}"));
    }
}
