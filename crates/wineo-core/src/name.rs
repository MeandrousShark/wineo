/// Names end up in file names and dotted config keys, so they are limited to
/// ASCII letters, digits, `-`, and `_`.
pub fn is_valid(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Describes characters that cannot appear in an environment name or tag.
pub fn problem(name: &str) -> Option<String> {
    if is_valid(name) {
        return None;
    }
    if name.is_empty() {
        return Some("can't be empty".to_owned());
    }
    let mut found: Vec<String> = Vec::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            continue;
        }
        let shown = if character == ' ' {
            "spaces".to_owned()
        } else {
            format!("`{character}`")
        };
        if !found.contains(&shown) {
            found.push(shown);
        }
    }
    Some(format!("{} not allowed", found.join(" and ")))
}

/// Runner names are directory names and config values, never dotted config
/// keys, so unlike environment names they may contain dots. A leading dot is
/// not allowed, which rules out `.`, `..`, and half-finished extractions.
pub fn is_valid_runner(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_letters_digits_dash_underscore() {
        for name in ["notepad", "Photoshop-2026", "my_app", "7zip"] {
            assert!(is_valid(name), "{name}");
        }
    }

    #[test]
    fn runner_names_allow_dots_but_never_lead_with_one() {
        for name in ["wine-10.0", "GE-Proton9-20", "wine_tkg", "7z"] {
            assert!(is_valid_runner(name), "{name}");
        }
        for name in ["", ".", "..", ".hidden", "has space", "a/b", "caf\u{e9}"] {
            assert!(!is_valid_runner(name), "{name}");
        }
        // Environment names stay stricter, since those become dotted keys.
        assert!(!is_valid("wine-10.0"));
    }

    #[test]
    fn rejects_everything_else() {
        for name in [
            "",
            "has space",
            "dotted.name",
            "slash/name",
            "caf\u{e9}",
            "a=b",
        ] {
            assert!(!is_valid(name), "{name}");
        }
    }
}
