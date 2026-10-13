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
        "en-US-x",
        "en-US-u",
        "en-US-u-ca-",
        "x-private",
        "en--US",
        "en-US-",
        "enenenenen-US",
        "en-ünï",
        "en-US-x-",
        "sl-rozaj-rozaj",
        "sl-rozaj-ROZAJ",
        "en-u-ca-gregory-u-nu-latn",
        "en-a-foo-A-bar",
        "i-klingon",
        "en-GB-oed",
        "x-whatever",
        "de-CH-19011901x",
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
        "en",
        "en-US",
        "fr-FR",
        "pt-BR",
        "zh-Hans",
        "zh-CN",
        "es-419",
        "xx",
        "tlh",
        "de-CH-1901",
        "sl-rozaj",
        "sl-rozaj-biske",
        "en-US-u-ca-gregory",
        "en-US-x-private",
        "zh-cmn-Hans-CN",
        "en-a-bbb-ccc",
        "SL-ROZAJ",
        "qaa",
    ] {
        validate(good).unwrap_or_else(|error| panic!("{good}: {error}"));
    }
}

#[test]
fn full_tags_map_by_language_and_region() {
    assert_eq!(installer_code("de-CH-1901").unwrap(), "de");
    assert_eq!(installer_code("en-US-u-ca-gregory").unwrap(), "en");
    assert_eq!(installer_code("sl-rozaj").unwrap(), "sl");
    assert_eq!(installer_code("en-US-x-private").unwrap(), "en");
    assert_eq!(installer_code("zh-cmn-Hans-CN").unwrap(), "cn");
    assert_eq!(installer_code("pt-BR-u-nu-latn").unwrap(), "br");
    assert_eq!(installer_code("es-419-x-a").unwrap(), "es_mx");
    // The private-use part isn't a region.
    assert_eq!(installer_code("pt-x-br").unwrap(), "pt");
}

#[test]
fn distinct_variants_and_singletons_are_fine() {
    for good in [
        "sl-rozaj-biske",
        "en-a-foo-b-bar",
        "en-u-ca-gregory-t-es",
        "de-1901-1996",
    ] {
        validate(good).unwrap_or_else(|error| panic!("{good}: {error}"));
    }
}

#[test]
fn inno_names_and_locales_follow_the_code() {
    assert_eq!(inno_language("en"), Some("english"));
    assert_eq!(inno_language("br"), Some("brazilianportuguese"));
    assert_eq!(inno_language("jp"), Some("japanese"));
    assert_eq!(inno_language("xx"), None);
    for (tag, code, locale) in [
        ("fr-FR", "fr", Some("fr_FR.UTF-8")),
        ("fr-CA", "fr", Some("fr_CA.UTF-8")),
        ("fr", "fr", Some("fr_FR.UTF-8")),
        ("pt-BR", "br", Some("pt_BR.UTF-8")),
        ("zh-Hans", "cn", Some("zh_CN.UTF-8")),
        ("ja", "jp", Some("ja_JP.UTF-8")),
        ("es-419", "es_mx", Some("es_MX.UTF-8")),
        // An installer that fell back to English runs in English.
        ("fr-CA", "en", Some("en_US.UTF-8")),
        ("xx-YY", "xx", None),
    ] {
        assert_eq!(posix_locale(tag, code).as_deref(), locale, "{tag} {code}");
    }
}
