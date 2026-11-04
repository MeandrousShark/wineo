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

// The rule for comments in a tags array: removing a tag drops only its own
// comment, the one on the line where the tag ends. Every other comment line is
// kept, on a line of its own, and adding a tag keeps them all. Odd layouts may
// move a kept comment to another line or change blank lines; that's a known
// limit, not a loss.
//
// toml_edit splits the text between values into decor: a value's prefix, from
// the comma before it, and its suffix, up to the comma after it. The array's
// trailing text follows the last value, or its trailing comma. So the text
// after a value is its suffix plus the next prefix or the trailing text, and
// the first line of that, up to a line break, is the value's own comment.

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
    let indent = layout(prefix(last), " ");
    let at = tags.len() - 1;
    // What's on the line after the last tag, like its own comment, stays on
    // that line, after the comma the new tag brings. Its suffix's other lines
    // stay where they were.
    let after = if tags.trailing_comma() {
        tags.trailing().as_str().unwrap_or("").to_owned()
    } else {
        suffix(last).to_owned()
    };
    let (comment, rest) = first_line(&after).unwrap_or(("", after.as_str()));
    let indent = if comment.is_empty() || indent.contains('\n') {
        indent
    } else {
        format!("\n{indent}")
    };
    value.decor_mut().set_prefix(format!("{comment}{indent}"));
    if tags.trailing_comma() {
        value.decor_mut().set_suffix("");
        tags.set_trailing(rest);
    } else {
        if let Some(last) = tags.get_mut(at) {
            last.decor_mut().set_suffix("");
        }
        value.decor_mut().set_suffix(rest);
    }
    tags.push_formatted(value);
}

/// Removes `tag`, which a valid config holds once at most, dropping only its
/// own comment.
fn remove_tag(tags: &mut Array, tag: &str) {
    let Some(at) = tags.iter().position(|value| value.as_str() == Some(tag)) else {
        return;
    };
    let removed = tags.remove(at);
    let before = prefix(&removed);
    if let Some(next) = tags.get_mut(at) {
        let after = format!("{}{}", suffix(&removed), prefix(next));
        let joined = match first_line(&after) {
            Some((_, rest)) => joined(head(before), rest),
            // Only spacing follows, so the removed tag's spacing is kept.
            None => before.to_owned(),
        };
        next.decor_mut().set_prefix(joined);
        return;
    }
    // The removed tag was the last one, so what followed it closes the array.
    let after = format!(
        "{}{}",
        suffix(&removed),
        tags.trailing().as_str().unwrap_or("")
    );
    let rest = first_line(&after).map_or(after.as_str(), |(_, rest)| rest);
    let closing = joined(head(before), rest);
    match tags.len().checked_sub(1) {
        Some(last) if !tags.trailing_comma() => {
            tags.set_trailing("");
            if let Some(last) = tags.get_mut(last) {
                let suffix = format!("{}{closing}", suffix(last));
                last.decor_mut().set_suffix(suffix);
            }
        }
        _ => tags.set_trailing(closing),
    }
}

/// `kept` then `rest`, with a line break between them when `kept` could end
/// in a comment that would otherwise swallow what follows.
fn joined(kept: &str, rest: &str) -> String {
    if kept.is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n") {
        format!("{kept}{rest}")
    } else {
        format!("{kept}\n{rest}")
    }
}

/// Splits text at its first line break, `\n` or `\r\n`: what's on the line,
/// such as a comment, and the rest from the break on.
fn first_line(text: &str) -> Option<(&str, &str)> {
    let at = text.find('\n')?;
    let at = if text[..at].ends_with('\r') {
        at - 1
    } else {
        at
    };
    Some(text.split_at(at))
}

/// A prefix without its last line, the indent before the value. Whatever
/// comes before that, comments included, belongs to the lines above.
fn head(prefix: &str) -> &str {
    let Some(at) = prefix.rfind('\n') else {
        return "";
    };
    prefix[..at].strip_suffix('\r').unwrap_or(&prefix[..at])
}

fn prefix(value: &Value) -> &str {
    value
        .decor()
        .prefix()
        .and_then(|raw| raw.as_str())
        .unwrap_or("")
}

fn suffix(value: &Value) -> &str {
    value
        .decor()
        .suffix()
        .and_then(|raw| raw.as_str())
        .unwrap_or("")
}

/// The whitespace a decor puts around a value, without any comment in it.
fn layout(decor: &str, default: &str) -> String {
    match decor.rsplit_once('\n') {
        Some((line, indent)) if line.ends_with('\r') => format!("\r\n{indent}"),
        Some((_, indent)) => format!("\n{indent}"),
        None if decor.trim().is_empty() => decor.to_owned(),
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
