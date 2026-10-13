//! The `gog.language` setting: a BCP 47 locale tag, and the mapping from it
//! to the installer language code GOG's product details use (`en`, `de`).
//! State files record GOG's code, never the tag.

/// What `gog.language` is when unset.
pub const DEFAULT: &str = "en-US";

/// A tag's language and region, normalized for comparing: the language in
/// lowercase and the region in uppercase.
struct Tag {
    language: String,
    region: Option<String>,
}

fn parse(tag: &str) -> Option<Tag> {
    let mut parts = tag.split('-');
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let mut rest = parts.peekable();
    // A script is four letters; a region is two letters or three digits.
    // The script is checked and dropped, since the language and region pick
    // GOG's code (all Chinese is one code).
    let _script =
        rest.next_if(|part| part.len() == 4 && part.bytes().all(|b| b.is_ascii_alphabetic()));
    let region = rest.next_if(|part| {
        (part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()))
            || (part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit()))
    });
    if rest.next().is_some() {
        return None;
    }
    Some(Tag {
        language: language.to_ascii_lowercase(),
        region: region.map(str::to_ascii_uppercase),
    })
}

/// Checks the form of a tag. Which languages exist is not checked, since GOG
/// decides what it offers.
pub fn validate(tag: &str) -> Result<(), String> {
    parse(tag)
        .map(drop)
        .ok_or_else(|| "Invalid gog.language: use a locale tag like en-US.".to_owned())
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
