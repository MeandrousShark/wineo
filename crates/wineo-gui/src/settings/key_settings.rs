//! The state both API key rows share. Each provider keeps its own checking
//! and storage and says how its status reads; the settings, results, and
//! what a save or remove leaves behind are the same.

use std::fmt;
use std::path::Path;

use wineo_core::key_store::{Key, KeySource, Removed as CoreRemoved, Saved as CoreSaved};
use wineo_core::paths::Paths;

pub(super) use wineo_core::error::one_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    Saving,
    Removing,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// Where a provider's key comes from, and how that reads on its row.
pub(crate) trait KeyStatus: Sized {
    fn new(source: Option<KeySource>, override_note: bool) -> Self;

    fn source(&self) -> Option<KeySource>;

    fn override_note(&self) -> bool;

    fn label(&self, paths: &Paths) -> String;

    fn warning(&self, paths: &Paths) -> Option<String>;

    fn available(&self) -> bool {
        self.source().is_some()
    }
}

/// A provider's row, which `S` is the status of.
#[derive(Debug, Clone)]
pub(crate) struct Settings<S> {
    pub(crate) status: Option<S>,
    pub(crate) status_error: Option<String>,
    pub(crate) warning: Option<String>,
    pub(crate) input_open: bool,
    pub(crate) input: Secret,
    pub(crate) busy: Option<Busy>,
    pub(crate) error: Option<String>,
    pub(crate) result: Option<String>,
}

// Not derived, which would ask `S` for a default it doesn't have.
impl<S> Default for Settings<S> {
    fn default() -> Self {
        Self {
            status: None,
            status_error: None,
            warning: None,
            input_open: false,
            input: Secret::default(),
            busy: None,
            error: None,
            result: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Saved<S> {
    pub(crate) status: S,
    pub(crate) warning: Option<String>,
    pub(crate) result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Removed<S> {
    pub(crate) status: S,
    pub(crate) result: String,
}

pub(crate) fn plain_text_warning(path: &Path) -> String {
    format!(
        "warning: no keyring, so the key is stored in plain text in {}",
        path.display()
    )
}

/// The status of the key a provider found, if any.
pub(crate) fn status_of<S: KeyStatus>(found: Option<Key>) -> S {
    let source = found.map(|key| key.source);
    S::new(source, source == Some(KeySource::Variable))
}

/// Whether a variable's key is the one in use, which a saved key can't
/// replace.
pub(crate) fn overridden(found: Option<Key>) -> bool {
    found.is_some_and(|key| key.source == KeySource::Variable)
}

/// What a save leaves: the key shows from the variable when one overrides it,
/// else from where it went.
pub(crate) fn saved<S: KeyStatus>(
    core: CoreSaved,
    override_note: bool,
    keyring_message: &str,
) -> Saved<S> {
    let (saved_source, warning, result) = match core {
        CoreSaved::Keyring => (KeySource::Keyring, None, Some(keyring_message.to_owned())),
        CoreSaved::File(path) => (KeySource::File, Some(plain_text_warning(&path)), None),
    };
    Saved {
        status: S::new(
            Some(if override_note {
                KeySource::Variable
            } else {
                saved_source
            }),
            override_note,
        ),
        warning,
        result,
    }
}

/// What a remove leaves: only an overriding variable's key is still there.
pub(crate) fn removed<S: KeyStatus>(
    core: &CoreRemoved,
    override_note: bool,
    removed_message: &str,
    nothing_message: &str,
) -> Removed<S> {
    Removed {
        status: S::new(override_note.then_some(KeySource::Variable), override_note),
        result: if core.keyring || core.file {
            removed_message.to_owned()
        } else {
            nothing_message.to_owned()
        },
    }
}
