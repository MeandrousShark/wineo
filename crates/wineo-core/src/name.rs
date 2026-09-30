/// Names end up in file names and dotted config keys, so they are limited to
/// ASCII letters, digits, `-`, and `_`.
pub fn is_valid(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Derives an environment name from a human-readable game title. Letters with
/// common Latin accents are folded to ASCII so accented titles stay readable,
/// and everything else that is not allowed in a name becomes a single `-`.
/// The result is empty when the title has no ASCII-foldable letters or digits.
pub fn from_title(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for character in title.chars() {
        // Apostrophes and trademark marks vanish instead of splitting a word,
        // and combining accents ride along with the letter before them.
        if matches!(
            character,
            '\'' | '\u{2019}' | '\u{ae}' | '\u{2122}' | '\u{a9}' | '\u{300}'..='\u{36f}'
        ) {
            continue;
        }
        let mut buffer = [0; 4];
        let piece = fold(character).unwrap_or_else(|| character.encode_utf8(&mut buffer));
        if !piece
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push('-');
        }
        gap = false;
        out.push_str(&piece.to_ascii_lowercase());
    }
    out
}

fn fold(character: char) -> Option<&'static str> {
    Some(match character {
        '\u{e0}'..='\u{e5}' | '\u{c0}'..='\u{c5}' => "a",
        '\u{e6}' | '\u{c6}' => "ae",
        '\u{e7}' | '\u{c7}' => "c",
        '\u{e8}'..='\u{eb}' | '\u{c8}'..='\u{cb}' => "e",
        '\u{ec}'..='\u{ef}' | '\u{cc}'..='\u{cf}' => "i",
        '\u{f1}' | '\u{d1}' => "n",
        '\u{f2}'..='\u{f6}' | '\u{d2}'..='\u{d6}' | '\u{f8}' | '\u{d8}' => "o",
        '\u{153}' | '\u{152}' => "oe",
        '\u{df}' => "ss",
        '\u{f9}'..='\u{fc}' | '\u{d9}'..='\u{dc}' => "u",
        '\u{fd}' | '\u{ff}' | '\u{dd}' => "y",
        _ => return None,
    })
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
    fn titles_become_environment_names() {
        for (title, expected) in [
            ("The Sims 4", "the-sims-4"),
            ("Assassin's Creed: Origins", "assassins-creed-origins"),
            ("Hogwarts Legacy\u{ae}", "hogwarts-legacy"),
            ("S.T.A.L.K.E.R. 2", "s-t-a-l-k-e-r-2"),
            ("Pok\u{e9}mon Snap", "pokemon-snap"),
            ("  --Hades--  ", "hades"),
            ("Half-Life 2", "half-life-2"),
            ("under_score", "under_score"),
            ("\u{30cf}\u{30fc}\u{30c7}\u{30b9}", ""),
            ("Assassin\u{2019}s Creed\u{2122}", "assassins-creed"),
            (
                "\u{c5}ngstr\u{f6}m \u{df} \u{e6}\u{153}",
                "angstrom-ss-aeoe",
            ),
            ("Cafe\u{301}", "cafe"),
        ] {
            let derived = from_title(title);
            assert_eq!(derived, expected, "{title}");
            assert!(derived.is_empty() || is_valid(&derived), "{title}");
        }
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
