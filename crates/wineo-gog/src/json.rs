//! Small readers for GOG's JSON, which isn't consistent about types: ids
//! arrive as numbers in one endpoint and strings in another.

use serde_json::Value;

/// A non-negative integer, given as a number or a string of digits.
pub(crate) fn id(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
            text.parse().ok()
        }
        _ => None,
    }
}

/// A non-empty string field.
pub(crate) fn text(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

pub(crate) fn flag(value: &Value, name: &str) -> bool {
    value.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// GOG's image and link URLs are often protocol-relative (`//images...`).
/// Those, and plain HTTP ones, become HTTPS; anything else is dropped.
pub(crate) fn https_url(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .or_else(|| text.strip_prefix("//"))?;
    (!rest.is_empty() && !rest.starts_with('/')).then(|| format!("https://{rest}"))
}

/// A URL that is already HTTPS. Unlike `https_url`, plain HTTP and
/// protocol-relative addresses are refused rather than upgraded, for links
/// that are fetched as they are.
pub(crate) fn strict_https_url(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    // The URL parser reads `https:///host` as `https://host`, which isn't
    // what was written.
    if text.strip_prefix("https://")?.starts_with('/') {
        return None;
    }
    let url = reqwest::Url::parse(text).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some_and(|host| !host.is_empty())
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.into())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn ids_come_as_numbers_or_digit_strings() {
        assert_eq!(id(&json!(12)), Some(12));
        assert_eq!(id(&json!("12")), Some(12));
        for bad in [json!(-1), json!(1.5), json!(""), json!("1a"), json!(null)] {
            assert_eq!(id(&bad), None, "{bad}");
        }
    }

    #[test]
    fn strict_urls_must_already_be_https() {
        let ok = "https://images.gog-statics.com/a.jpg";
        assert_eq!(strict_https_url(&json!(ok)).as_deref(), Some(ok));
        for bad in [
            "http://images.gog-statics.com/a.jpg",
            "//images.gog-statics.com/a.jpg",
            "ftp://images.gog-statics.com/a.jpg",
            "https://user:pass@images.gog-statics.com/a.jpg",
            "https:///a.jpg",
            "javascript:alert(1)",
            "/relative",
            "",
        ] {
            assert_eq!(strict_https_url(&json!(bad)), None, "{bad}");
        }
        assert_eq!(strict_https_url(&json!(7)), None);
    }

    #[test]
    fn image_urls_become_https() {
        for (raw, expected) in [
            (
                "//images.gog.com/a.jpg",
                Some("https://images.gog.com/a.jpg"),
            ),
            ("http://images.gog.com/a", Some("https://images.gog.com/a")),
            ("https://images.gog.com/a", Some("https://images.gog.com/a")),
            ("/relative/path", None),
            ("///triple", None),
            ("javascript:alert(1)", None),
            ("", None),
        ] {
            assert_eq!(https_url(&json!(raw)).as_deref(), expected, "{raw}");
        }
    }
}
