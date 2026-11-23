use crate::error::Error;
use crate::library::Id;
use crate::paths::Paths;

/// The reason a library write failed, without the refusal wrapper's own
/// sentence, which the warning around it would repeat.
pub(crate) fn cause(error: &Error) -> String {
    let text = match error {
        Error::LibraryUnreadable {
            newer_version: false,
            source,
        } => format!("it can't be read: {}", crate::error::chain(&**source)),
        Error::LibraryUnreadable {
            newer_version: true,
            ..
        } => error.to_string(),
        other => crate::error::chain(other),
    };
    text.trim_end_matches(|c: char| c.is_whitespace() || c == '.')
        .to_owned()
}

/// Stamps a new environment's library entry. The new id, or the warning to
/// show when the stamp failed.
pub(crate) fn fresh_warning(paths: &Paths, name: &str) -> Result<Id, String> {
    crate::library::fresh(paths, name).map_err(|error| {
        format!(
            "Can't update library.toml: {}. {name} may show an old star or hidden flag once it can be read.",
            cause(&error)
        )
    })
}
