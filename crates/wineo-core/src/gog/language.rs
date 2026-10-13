//! The `gog.language` setting: a BCP 47 locale tag, and the mapping from it
//! to the installer language code GOG's product details use (`en`, `de`).
//! State files record GOG's code, never the tag.

/// What `gog.language` is when unset.
pub const DEFAULT: &str = "en-US";

/// A tag's language and region, normalized for comparing: the language in
/// lowercase and the region in uppercase. The rest of a tag (extlang,
/// script, variants, extensions, private use) is checked and ignored, since
/// GOG's codes don't depend on it.
struct Tag {
    language: String,
    region: Option<String>,
}

fn alpha(part: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphabetic())
}

fn alnum(part: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Parses a well-formed BCP 47 language tag (RFC 5646 section 2.1), which
/// is hyphen-separated and case-insensitive. The language is 2 or 3 letters,
/// narrower than the RFC's 4 to 8 letter forms, which are reserved or
/// unassigned and would only let a typo like `english` through. A private-use-only tag (`x-...`)
/// and the old grandfathered tags have no language to map, so they're refused.
fn parse(tag: &str) -> Option<Tag> {
    let parts: Vec<&str> = tag.split('-').collect();
    let mut rest = parts.iter().copied().peekable();
    let language = rest.next().filter(|part| alpha(part, 2..=3))?;
    for _ in 0..3 {
        rest.next_if(|part| alpha(part, 3..=3));
    }
    rest.next_if(|part| alpha(part, 4..=4));
    let region = rest.next_if(|part| {
        alpha(part, 2..=2) || part.bytes().all(|b| b.is_ascii_digit()) && part.len() == 3
    });
    while rest
        .next_if(|part| {
            alnum(part, 5..=8) || (alnum(part, 4..=4) && part.as_bytes()[0].is_ascii_digit())
        })
        .is_some()
    {}
    // An extension is a singleton and then one or more subtags of 2-8.
    while rest
        .next_if(|part| alnum(part, 1..=1) && !part.eq_ignore_ascii_case("x"))
        .is_some()
    {
        let mut any = false;
        while rest.next_if(|part| alnum(part, 2..=8)).is_some() {
            any = true;
        }
        if !any {
            return None;
        }
    }
    if rest
        .next_if(|part| part.eq_ignore_ascii_case("x"))
        .is_some()
    {
        let mut any = false;
        while rest.next_if(|part| alnum(part, 1..=8)).is_some() {
            any = true;
        }
        if !any {
            return None;
        }
    }
    if rest.next().is_some() {
        return None;
    }
    Some(Tag {
        language: language.to_ascii_lowercase(),
        region: region.map(str::to_ascii_uppercase),
    })
}

/// What a refused value is told, after `Invalid gog.language:`.
pub const FORM_HINT: &str = "use a locale tag like en-US";

/// Checks the form of a tag. Which languages exist is not checked, since GOG
/// decides what it offers.
pub fn validate(tag: &str) -> Result<(), String> {
    parse(tag)
        .map(drop)
        .ok_or_else(|| format!("Invalid gog.language: {FORM_HINT}."))
}

/// Tags where GOG's code isn't the lowercase language subtag, as
/// (language, region, GOG code). A row with a region is checked before the
/// same language's row without one. GOG documents none of its codes; the
/// fake GOG fixtures only use `en`, so every row is a recollection to confirm
/// against real product details:
/// - unconfirmed: `zh` -> `cn`, `ja` -> `jp`, `cs` -> `cz`, `el` -> `gk`,
///   `pt-BR` -> `br`, `es-MX` and `es-419` -> `es_mx`.
const GOG_CODES: &[(&str, Option<&str>, &str)] = &[
    ("pt", Some("BR"), "br"),
    ("es", Some("MX"), "es_mx"),
    ("es", Some("419"), "es_mx"),
    ("zh", None, "cn"),
    ("ja", None, "jp"),
    ("cs", None, "cz"),
    ("el", None, "gk"),
];

/// GOG's installer language code for a locale tag, case-insensitively.
pub fn installer_code(tag: &str) -> Result<String, String> {
    validate(tag)?;
    let tag = parse(tag).expect("validated above");
    let found = GOG_CODES.iter().find(|(language, region, _)| {
        *language == tag.language && (region.is_none() || *region == tag.region.as_deref())
    });
    Ok(found.map_or(tag.language, |(_, _, code)| (*code).to_owned()))
}

#[cfg(test)]
mod tests;
