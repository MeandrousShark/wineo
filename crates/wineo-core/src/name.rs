/// Names end up in file names and dotted config keys, so they are limited to
/// ASCII letters, digits, `-`, and `_`.
pub fn is_valid(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
