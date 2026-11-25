use std::fmt;
use std::io;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An environment's own identity, kept in its library entry. It survives
/// edits to the environment's config, and an environment made again under
/// the same name gets a new one. Unique, not secret.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id([u8; 16]);

impl Id {
    /// A new random id. Failing to get random bytes is an error, which the
    /// callers treat like a library that can't be written.
    pub(crate) fn mint() -> io::Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes)
            .map_err(|error| io::Error::other(format!("failed to get random bytes: {error}")))?;
        Ok(Self(bytes))
    }

    /// Exactly 32 lowercase hex characters; anything else is no id.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 {
            return None;
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
            *byte = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
        }
        Some(Self(bytes))
    }
}

fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({self})")
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// For `#[serde(deserialize_with)]`: a malformed id, or a value that isn't
/// text at all, reads as none, so it can't make the file around it
/// unreadable.
pub(super) fn lenient<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Id>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Text(String),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Raw::deserialize(deserializer)? {
        Raw::Text(text) => Id::parse(&text),
        Raw::Other(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_32_lowercase_hex_characters() {
        let text = "3f9c2a6d0b7e4c1895a2d4e6f8a1b3c5";
        let id = Id::parse(text).unwrap();
        assert_eq!(id.to_string(), text);
        for bad in [
            "",
            "3f9c2a6d0b7e4c1895a2d4e6f8a1b3c",
            "3f9c2a6d0b7e4c1895a2d4e6f8a1b3c50",
            "3F9C2A6D0B7E4C1895A2D4E6F8A1B3C5",
            "3f9c2a6d0b7e4c1895a2d4e6f8a1b3cg",
            "3f9c2a6d-b7e4c1895a2d4e6f8a1b3c5",
            "3f9c2a6d0b7e4c1895a2d4e6f8a1b3\u{e9}",
        ] {
            assert_eq!(Id::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn minted_ids_differ_and_round_trip() {
        let one = Id::mint().unwrap();
        let two = Id::mint().unwrap();
        assert_ne!(one, two);
        assert_eq!(Id::parse(&one.to_string()), Some(one));
    }
}
