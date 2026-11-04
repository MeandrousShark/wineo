//! Adding or removing one tag, which `wineo tag` and the app's bulk tagging
//! run across many environments at once.

use std::path::Path;

use toml_edit::{Array, DocumentMut, Value};

use super::config_path;
use super::{ConfigFile, file_kind};
use crate::config::{self, EnvironmentConfig};
use crate::deprecated;
use crate::edit::EditError;
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::files;
use crate::operation::Locks;
use crate::paths::Paths;

/// One tag to add to or remove from an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagChange<'a> {
    Add(&'a str),
    Remove(&'a str),
}

impl<'a> TagChange<'a> {
    pub fn tag(self) -> &'a str {
        match self {
            Self::Add(tag) | Self::Remove(tag) => tag,
        }
    }
}

/// Whether `change_tags` wrote the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tagged {
    Changed,
    /// It already had the tag, or already didn't, so nothing was written.
    Unchanged,
}

/// Refuses a tag `set` would refuse, so a caller tagging many environments
/// can check once before writing any of them.
pub fn check_tag(tag: &str) -> Result<()> {
    config::validate_tags(&[tag.to_owned()]).map_err(|reason| EditError::InvalidList(reason).into())
}

/// Adds or removes one tag on one environment. The list is read and changed
/// under the config lock, so a tag another writer added in the meantime is
/// kept. An unchanged list writes nothing. Desktop entries are left alone,
/// since no tag reaches one.
pub fn change_tags(paths: &Paths, name: &str, change: TagChange) -> Result<Tagged> {
    change_tags_with_target(paths, name, change, |path, target, contents| {
        files::write_config(path, target, contents)
    })
}

/// `write` gets the config's path, the target its lock was keyed on, and the
/// new contents, like `update_with_target`'s.
fn change_tags_with_target(
    paths: &Paths,
    name: &str,
    change: TagChange,
    write: impl FnOnce(&Path, &Path, &str) -> Result<()>,
) -> Result<Tagged> {
    let tag = change.tag();
    check_tag(tag)?;
    Environment::ensure_exists(paths, name)?;
    let file = ConfigFile::Environment(name);
    let (_, path) = config_path(paths, file)?;
    let guard = Locks::new(paths).config(&path)?;
    let text = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?
        .ok_or_else(|| Error::UnknownEnvironment(name.to_owned()))?;
    config::check_schema(&path, &text)?;
    // A config that can't be read is an error even when the tag is already
    // where it was asked to be, the same as `set` with an unchanged value.
    let current = EnvironmentConfig::parse(&path, &text)?;
    let has = current.tags.iter().any(|existing| existing == tag);
    if has == matches!(change, TagChange::Add(_)) {
        return Ok(Tagged::Unchanged);
    }

    let mut doc: DocumentMut = text.parse().map_err(|source| Error::ConfigParse {
        path: path.clone(),
        source: Box::new(source),
    })?;
    let renamed = deprecated::rename_document(file_kind(file), &path, &mut doc)?;
    match change {
        TagChange::Add(tag) => match doc.get_mut("tags").and_then(|item| item.as_array_mut()) {
            Some(tags) => push_tag(tags, tag),
            None => {
                let mut tags = Array::new();
                tags.push(tag);
                doc["tags"] = toml_edit::value(tags);
            }
        },
        TagChange::Remove(tag) => {
            if let Some(tags) = doc.get_mut("tags").and_then(|item| item.as_array_mut()) {
                remove_tag(tags, tag);
            }
        }
    }
    deprecated::restore_document(&mut doc, &renamed);
    config::stamp_schema(&mut doc);
    let updated = doc.to_string();
    EnvironmentConfig::parse(&path, &updated)?;
    write(&path, guard.config_target(), &updated)?;
    Ok(Tagged::Changed)
}

/// Appends `tag` laid out like the tag before it: on its own line in a
/// multi-line list, in the same quotes, without copying a comment.
fn push_tag(tags: &mut Array, tag: &str) {
    let Some(last) = tags.iter().last() else {
        tags.push(tag);
        return;
    };
    let mut value: Value = if raw(last).starts_with('\'') {
        format!("'{tag}'")
    } else {
        format!("\"{tag}\"")
    }
    .parse()
    .expect("a valid tag is a valid TOML string");
    let prefix = layout(last.decor().prefix().and_then(|raw| raw.as_str()), " ");
    value.decor_mut().set_prefix(prefix);
    // With no trailing comma, the last tag's suffix holds the line break before
    // `]`, which belongs after the new last tag instead.
    let last_suffix = last
        .decor()
        .suffix()
        .and_then(|raw| raw.as_str())
        .unwrap_or("")
        .to_owned();
    let moves = !tags.trailing_comma() && last_suffix.contains('\n');
    if moves {
        let at = tags.len() - 1;
        if let Some(last) = tags.get_mut(at) {
            last.decor_mut().set_suffix("");
        }
        value.decor_mut().set_suffix(last_suffix);
    } else {
        value.decor_mut().set_suffix("");
    }
    tags.push_formatted(value);
}

/// Removes `tag`, which a valid config holds once at most, leaving its
/// neighbors laid out as they were.
fn remove_tag(tags: &mut Array, tag: &str) {
    let found = tags.iter().position(|value| value.as_str() == Some(tag));
    if let Some(at) = found {
        let removed = tags.remove(at);
        if tags.is_empty() {
            return;
        }
        // The tag that moves up to the opening spot takes the removed one's
        // spacing, unless it starts its own line, which it keeps.
        if at == 0
            && let Some(first) = tags.get_mut(0)
            && !first
                .decor()
                .prefix()
                .and_then(|raw| raw.as_str())
                .unwrap_or("")
                .contains('\n')
        {
            let prefix = layout(removed.decor().prefix().and_then(|raw| raw.as_str()), "");
            first.decor_mut().set_prefix(prefix);
        }
        // A removed last tag with no trailing comma held the line break
        // before `]`.
        let removed_suffix = removed.decor().suffix().and_then(|raw| raw.as_str());
        if at == tags.len()
            && !tags.trailing_comma()
            && let Some(suffix) = removed_suffix.filter(|suffix| suffix.contains('\n'))
        {
            let suffix = layout(Some(suffix), "");
            if let Some(last) = tags.get_mut(at - 1) {
                last.decor_mut().set_suffix(suffix);
            }
        }
    }
}

/// The whitespace a decor puts around a value, without any comment in it.
fn layout(decor: Option<&str>, default: &str) -> String {
    match decor {
        Some(decor) => match decor.rsplit_once('\n') {
            Some((_, indent)) => format!("\n{indent}"),
            None if decor.trim().is_empty() => decor.to_owned(),
            None => default.to_owned(),
        },
        None => default.to_owned(),
    }
}

fn raw(value: &Value) -> &str {
    match value {
        Value::String(string) => string
            .as_repr()
            .and_then(|repr| repr.as_raw().as_str())
            .unwrap_or(""),
        _ => "",
    }
}

#[cfg(test)]
mod tests;
