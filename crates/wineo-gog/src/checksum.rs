//! GOG's checksum documents for installer files, like
//!
//! ```xml
//! <file name="setup_x.exe" available="1" md5="0123..." chunks="2" total_size="1234">
//!   <chunk id="0" from="0" to="999" method="md5">...</chunk>
//! </file>
//! ```
//!
//! Only the root element's `md5` and `total_size` are read. The parse is
//! strict about the root, so anything that isn't plainly this shape counts
//! as no checksum at all rather than a checksum to trust.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checksum {
    /// Lowercase hex.
    pub md5: String,
    pub total_size: u64,
}

pub(crate) fn parse(bytes: &[u8]) -> Option<Checksum> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text).trim();
    if let Some(declaration) = rest.strip_prefix("<?xml") {
        let end = declaration.find("?>")?;
        rest = declaration[end + 2..].trim_start();
    }
    let rest = rest.strip_prefix("<file")?;
    let (attributes, tail, closed) = attributes(rest)?;
    let tail = tail.trim_end();
    if closed {
        if !tail.trim_start().is_empty() {
            return None;
        }
    } else {
        tail.strip_suffix("</file>")?;
    }
    let md5 = attributes.get("md5")?;
    if md5.len() != 32 || !md5.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let size = attributes.get("total_size")?;
    if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Checksum {
        md5: md5.to_ascii_lowercase(),
        total_size: size.parse().ok()?,
    })
}

/// The attributes of a start tag whose name was just read, the text after
/// the tag, and whether the tag closed itself.
fn attributes(mut rest: &str) -> Option<(BTreeMap<&str, &str>, &str, bool)> {
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    let mut found = BTreeMap::new();
    loop {
        let trimmed = rest.trim_start_matches(is_space);
        if let Some(tail) = trimmed.strip_prefix("/>") {
            return Some((found, tail, true));
        }
        if let Some(tail) = trimmed.strip_prefix('>') {
            return Some((found, tail, false));
        }
        // `<filex` is another element, and attributes need space between.
        if trimmed.len() == rest.len() {
            return None;
        }
        let name_end = trimmed
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')))?;
        let (name, after) = trimmed.split_at(name_end);
        if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            return None;
        }
        let after = after
            .trim_start_matches(is_space)
            .strip_prefix('=')?
            .trim_start_matches(is_space);
        let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let after = &after[1..];
        let end = after.find(quote)?;
        let value = &after[..end];
        if value.contains('<') || found.insert(name, value).is_some() {
            return None;
        }
        rest = &after[end + 1..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD5: &str = "0123456789abcdef0123456789ABCDEF";

    #[test]
    fn gog_s_document_gives_its_md5_and_size() {
        let document = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<file name=\"setup_x_(64bit).exe\" available=\"1\" notavailablemsg=\"\" md5=\"{MD5}\" chunks=\"2\" timestamp=\"2024-01-02 03:04:05\" total_size=\"1234\">\n\t<chunk id=\"0\" from=\"0\" to=\"999\" method=\"md5\">aa</chunk>\n\t<chunk id=\"1\" from=\"1000\" to=\"1233\" method=\"md5\">bb</chunk>\n</file>\n"
        );
        assert_eq!(
            parse(document.as_bytes()),
            Some(Checksum {
                md5: MD5.to_ascii_lowercase(),
                total_size: 1234,
            })
        );
        let short = format!("<file md5='{MD5}' total_size='7'/>");
        assert_eq!(parse(short.as_bytes()).unwrap().total_size, 7);
    }

    #[test]
    fn anything_else_is_no_checksum() {
        for bad in [
            String::new(),
            "{\"md5\":\"x\"}".to_owned(),
            format!("<files md5=\"{MD5}\" total_size=\"1\"></files>"),
            format!("<file md5=\"{MD5}\" total_size=\"1\">"),
            format!("<file md5=\"{MD5}\" total_size=\"1\"/>trailing"),
            format!("<file md5=\"{MD5}\"></file>"),
            "<file md5=\"abc\" total_size=\"1\"></file>".to_owned(),
            format!("<file md5=\"{MD5}\" total_size=\"-1\"></file>"),
            format!("<file md5=\"{MD5}\" total_size=\"\"></file>"),
            format!("<file md5=\"{MD5}\" md5=\"{MD5}\" total_size=\"1\"></file>"),
            format!("<file md5=\"{MD5}\"total_size=\"1\"></file>"),
            format!("<file md5={MD5} total_size=\"1\"></file>"),
            format!("<file md5=\"{MD5}\" total_size=\"1\" x=\"<\"></file>"),
            format!("<?xml version=\"1.0\"<file md5=\"{MD5}\" total_size=\"1\"></file>"),
            format!("junk<file md5=\"{MD5}\" total_size=\"1\"></file>"),
        ] {
            assert_eq!(parse(bad.as_bytes()), None, "{bad}");
        }
        assert_eq!(parse(&[0xff, 0xfe, b'<']), None);
    }
}
