//! An account's public profile from `users.gog.com/users/<id>`.
//!
//! Only the display name is read. The answer also holds the join date,
//! avatar, and more, none of which wineo needs.

use serde_json::Value;

use crate::error::{Endpoint, Error, ErrorKind};

/// Far longer than any real name, so a runaway value is refused instead of
/// being saved and shown.
const MAX_NAME: usize = 128;

pub(crate) fn parse_username(json: &Value) -> Result<String, Error> {
    let malformed = || Error::new(Endpoint::Profile, ErrorKind::Malformed);
    let name = json
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(malformed)?;
    // The name is printed to a terminal, where control characters could
    // rewrite what's on screen.
    if name.len() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(malformed());
    }
    Ok(name.to_owned())
}
