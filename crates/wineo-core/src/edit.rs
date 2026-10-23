//! Dotted-key editing of TOML config documents, for `wineo set`.
//!
//! `set` and `unset` apply one edit at a time to a parsed [`DocumentMut`],
//! keeping the comments and formatting of everything they don't touch.

use toml_edit::{DocumentMut, Entry, Item, Table, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Environment,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("expected KEY=VALUE, got `{0}`")]
    MissingEquals(String),
    #[error("`{0}` is not a known setting")]
    UnknownKey(String),
    #[error("invalid value for `{key}`: {reason}")]
    InvalidValue { key: String, reason: String },
    #[error("`{key}` can't be changed: {reason}")]
    ReadOnly { key: String, reason: String },
    /// A key another command owns, with the whole refusal as its message.
    #[error("{0}")]
    Managed(String),
    #[error("{0}")]
    InvalidList(String),
    /// A refused value, with the whole refusal as its message.
    #[error("{0}")]
    Refused(String),
}

/// Splits `key=value` at the first `=`.
pub fn parse_assignment(s: &str) -> Result<(&str, &str), EditError> {
    s.split_once('=')
        .ok_or_else(|| EditError::MissingEquals(s.to_string()))
}

/// Sets `key` to `raw`, typed by the scope's schema.
pub fn set(doc: &mut DocumentMut, scope: Scope, key: &str, raw: &str) -> Result<(), EditError> {
    let root = doc.as_table_mut();
    match scope {
        Scope::Global => set_global(root, key, raw),
        Scope::Environment => set_environment(root, key, raw),
    }
}

/// Removes `key`. Returns Ok(false) if the key is valid but wasn't present.
pub fn unset(doc: &mut DocumentMut, scope: Scope, key: &str) -> Result<bool, EditError> {
    let root = doc.as_table_mut();
    match scope {
        Scope::Global => unset_global(root, key),
        Scope::Environment => unset_environment(root, key),
    }
}

/// Whether a parsed global file explicitly sets a key that also has a default.
pub fn global_has_key(text: &str, key: &str) -> bool {
    let Ok(mut doc) = text.parse::<DocumentMut>() else {
        return false;
    };
    // A key under a deprecated name is set all the same. Both names at once
    // is an error the read reports, and either one counts here.
    let _ = crate::deprecated::rename_document(
        crate::deprecated::FileKind::Global,
        std::path::Path::new(""),
        &mut doc,
    );
    doc.get(key).is_some()
}

fn set_environment(root: &mut Table, key: &str, raw: &str) -> Result<(), EditError> {
    match key {
        "prefix" => {
            let value = parse_string_value(key, raw)?;
            store_value(root, "prefix", value);
            Ok(())
        }
        "title" => {
            // A Steam game's title is the import's fallback, and `set` has
            // always written it as given.
            let value = if root.get("kind").and_then(Item::as_str) == Some("proxied") {
                parse_string_value(key, raw)?
            } else {
                Value::from(crate::config::normalize_title(raw).map_err(EditError::Refused)?)
            };
            store_value(root, key, value);
            Ok(())
        }
        "runner" | "gameid" | "store" | "dxvk" | "vkd3d" => {
            let value = parse_string_value(key, raw)?;
            store_value(root, key, value);
            Ok(())
        }
        "gamemode" | "wayland" | "mangohud" | "desktop" => {
            store_value(root, key, Value::from(parse_bool(key, raw)?));
            Ok(())
        }
        "gamescope" => set_gamescope(root, key, raw),
        "hooks" => set_hooks(root, key, raw),
        "tags" | "winetricks" => {
            let value = parse_args_value(key, raw)?;
            let values: Vec<String> = value
                .as_array()
                .expect("parse_args_value returns an array")
                .iter()
                .map(|item| item.as_str().expect("array of strings").to_owned())
                .collect();
            if key == "tags" {
                crate::config::validate_tags(&values).map_err(EditError::InvalidList)?;
            } else {
                crate::config::validate_winetricks(&values).map_err(EditError::InvalidList)?;
            }
            store_value(root, key, value);
            Ok(())
        }
        "arch" => Err(read_only_arch()),
        "gog_id" => Err(read_only_gog_id()),
        _ => {
            if let Some(var) = key.strip_prefix("env.") {
                let var = validate_env_var(key, var)?;
                let value = parse_string_value(key, raw)?;
                let table = ensure_table(root, "env", "env", false)?;
                store_value(table, var, value);
                Ok(())
            } else if let Some(rest) = key.strip_prefix("targets.") {
                set_target(root, key, rest, raw)
            } else if let Some(field) = key.strip_prefix("gamescope.") {
                set_gamescope_field(root, key, field, raw)
            } else if let Some(field) = key.strip_prefix("hooks.") {
                let value = parse_hooks_field(key, field, raw)?;
                let table = ensure_hooks(root, key)?;
                store_hooks_field(table, field, value);
                Ok(())
            } else {
                Err(EditError::UnknownKey(key.to_string()))
            }
        }
    }
}

fn set_global(root: &mut Table, key: &str, raw: &str) -> Result<(), EditError> {
    match key {
        "steam" => {
            if !matches!(raw, "native" | "flatpak") {
                return Err(EditError::InvalidValue {
                    key: key.to_owned(),
                    reason: format!("expected `native` or `flatpak`, got `{raw}`"),
                });
            }
            if root.get(key).and_then(Item::as_str) != Some(raw) {
                store_value(root, key, Value::from(raw));
            }
            Ok(())
        }
        "stop_timeout" | "log_limit_mb" => {
            let value = parse_count_value(key, raw)?;
            store_value(root, key, value);
            Ok(())
        }
        "defaults.runner" => {
            let value = parse_string_value(key, raw)?;
            let table = ensure_table(root, "defaults", "defaults", false)?;
            store_value(table, "runner", value);
            Ok(())
        }
        "defaults.arch" => {
            let value = parse_arch_value(key, raw)?;
            let table = ensure_table(root, "defaults", "defaults", false)?;
            store_value(table, "arch", value);
            Ok(())
        }
        "gog.language" => {
            crate::gog::language::validate(raw).map_err(|_| EditError::InvalidValue {
                key: key.to_owned(),
                reason: crate::gog::language::FORM_HINT.to_owned(),
            })?;
            let table = ensure_table(root, "gog", "gog", false)?;
            store_value(table, "language", Value::from(raw));
            Ok(())
        }
        "art.fetch" => {
            let fetch = raw.parse::<crate::config::ArtFetch>().map_err(|reason| {
                EditError::InvalidValue {
                    key: key.to_owned(),
                    reason,
                }
            })?;
            let value = match fetch {
                crate::config::ArtFetch::All => Value::from("all"),
                crate::config::ArtFetch::Icon => Value::from("icon"),
                crate::config::ArtFetch::Selected(_) => {
                    let mut array = toml_edit::Array::new();
                    for kind in fetch.kinds() {
                        array.push(kind.as_str());
                    }
                    Value::Array(array)
                }
            };
            let table = ensure_table(root, "art", "art", false)?;
            store_value(table, "fetch", value);
            Ok(())
        }
        "gamemode" | "wayland" | "mangohud" | "external_runners" => {
            store_value(root, key, Value::from(parse_bool(key, raw)?));
            Ok(())
        }
        "defaults.prefix_root"
        | "defaults.runner_root"
        | "defaults.dxvk_root"
        | "defaults.vkd3d_root"
        | "defaults.games_root" => {
            let value = parse_string_value(key, raw)?;
            let field = key.strip_prefix("defaults.").expect("checked above");
            let table = ensure_table(root, "defaults", "defaults", false)?;
            store_value(table, field, value);
            Ok(())
        }
        _ => {
            if let Some(var) = key.strip_prefix("env.") {
                let var = validate_env_var(key, var)?;
                let value = parse_string_value(key, raw)?;
                let table = ensure_table(root, "env", "env", false)?;
                store_value(table, var, value);
                Ok(())
            } else {
                Err(EditError::UnknownKey(key.to_string()))
            }
        }
    }
}

/// Handles `targets.<name>` and `targets.<name>.<field>` for `set`.
fn set_target(root: &mut Table, key: &str, rest: &str, raw: &str) -> Result<(), EditError> {
    let (name, field) = split_dotted(rest);
    if !crate::name::is_valid(name) {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("`{name}` is not a valid target name"),
        });
    }
    let Some(field) = field else {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("set a field such as `targets.{name}.exe`"),
        });
    };

    if field == "default" {
        return set_target_default(root, name, key, raw);
    }

    // Parse and validate before creating any table, so a rejected value
    // never leaves an empty `[targets]` or `[targets.<name>]` behind.
    if field == "gamescope" {
        parse_gamescope_whole(key, raw)?;
        let targets = ensure_table(root, "targets", "targets", true)?;
        let target = ensure_table(targets, name, &format!("targets.{name}"), false)?;
        return set_gamescope(target, key, raw);
    }
    if let Some(gamescope_field) = field.strip_prefix("gamescope.") {
        let value = parse_gamescope_field(key, gamescope_field, raw)?;
        let targets = ensure_table(root, "targets", "targets", true)?;
        let target = ensure_table(targets, name, &format!("targets.{name}"), false)?;
        let table = ensure_gamescope(target, key)?;
        store_value(table, gamescope_field, value);
        return Ok(());
    }
    if field == "hooks" {
        parse_hooks_whole(key, raw)?;
        let targets = ensure_table(root, "targets", "targets", true)?;
        let target = ensure_table(targets, name, &format!("targets.{name}"), false)?;
        return set_hooks(target, key, raw);
    }
    if let Some(hooks_field) = field.strip_prefix("hooks.") {
        let value = parse_hooks_field(key, hooks_field, raw)?;
        let targets = ensure_table(root, "targets", "targets", true)?;
        let target = ensure_table(targets, name, &format!("targets.{name}"), false)?;
        let table = ensure_hooks(target, key)?;
        store_hooks_field(table, hooks_field, value);
        return Ok(());
    }
    let value = match field {
        "exe" | "cwd" | "name" => parse_string_value(key, raw)?,
        "args" => parse_args_value(key, raw)?,
        "gamemode" | "wayland" | "mangohud" | "desktop" => Value::from(parse_bool(key, raw)?),
        _ => return Err(EditError::UnknownKey(key.to_string())),
    };

    let error_key = format!("targets.{name}");
    let targets = ensure_table(root, "targets", "targets", true)?;
    let target = ensure_table(targets, name, &error_key, false)?;
    store_value(target, field, value);
    Ok(())
}

/// `targets.<name>.default = true` clears `default` on every other target;
/// `= false` just drops the key instead of storing `false`.
fn set_target_default(root: &mut Table, name: &str, key: &str, raw: &str) -> Result<(), EditError> {
    let is_default = parse_bool(key, raw)?;
    let error_key = format!("targets.{name}");

    if !is_default {
        // Clearing a flag shouldn't conjure up an empty target that then fails validation.
        if let Some(targets) = existing_table(root, "targets", "targets")?
            && let Some(target) = existing_table(targets, name, &error_key)?
        {
            target.remove("default");
        }
        return Ok(());
    }
    let targets = ensure_table(root, "targets", "targets", true)?;
    let target = ensure_table(targets, name, &error_key, false)?;
    store_value(target, "default", Value::from(true));

    let others: Vec<String> = targets
        .iter()
        .filter(|&(other, _)| other != name)
        .map(|(other, _)| other.to_string())
        .collect();
    for other in others {
        if let Some(Item::Table(table)) = targets.get_mut(&other) {
            table.remove("default");
        }
    }
    Ok(())
}

/// What a whole `gamescope` value can be set to. There's no other way to
/// write a table with no fields, which runs plain `gamescope --`.
#[derive(Clone, Copy)]
enum Whole {
    Off,
    Empty,
}

fn parse_hooks_whole(key: &str, raw: &str) -> Result<Whole, EditError> {
    if raw == "false" {
        Ok(Whole::Off)
    } else if raw.trim() == "{}" {
        Ok(Whole::Empty)
    } else {
        Err(EditError::InvalidValue {
            key: key.to_owned(),
            reason: if raw == "true" {
                "hooks takes a [hooks] table or false, not true".to_owned()
            } else {
                "hooks takes a [hooks] table or false".to_owned()
            },
        })
    }
}

fn set_hooks(root: &mut Table, key: &str, raw: &str) -> Result<(), EditError> {
    match parse_hooks_whole(key, raw)? {
        Whole::Off => store_value(root, "hooks", Value::from(false)),
        Whole::Empty => ensure_hooks(root, key)?.clear(),
    }
    Ok(())
}

fn ensure_hooks<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table, EditError> {
    let old_decor = root
        .get("hooks")
        .and_then(Item::as_value)
        .filter(|value| value.as_bool() == Some(false))
        .map(|value| value.decor().clone());
    if old_decor.is_some() {
        root.remove("hooks");
    }
    let table = ensure_table(root, "hooks", key, false)?;
    // Only the comment carries over. The value's prefix is the space after
    // `=`, which would indent the table header.
    if let Some(suffix) = old_decor.and_then(|decor| decor.suffix().cloned()) {
        table.decor_mut().set_suffix(suffix);
    }
    Ok(table)
}

fn store_hooks_field(table: &mut Table, field: &str, value: Value) {
    // Preserve the original quoting too when the command has not changed.
    if table.get(field).and_then(Item::as_str) != value.as_str() {
        store_value(table, field, value);
    }
}

fn known_hooks_field(field: &str) -> bool {
    matches!(field, "before" | "after")
}

fn parse_hooks_field(key: &str, field: &str, raw: &str) -> Result<Value, EditError> {
    if !known_hooks_field(field) {
        return Err(EditError::UnknownKey(key.to_owned()));
    }
    if raw.trim().is_empty() {
        return Err(EditError::InvalidValue {
            key: key.to_owned(),
            reason: "empty command".to_owned(),
        });
    }
    parse_string_value(key, raw)
}

fn unset_hooks_field(root: &mut Table, key: &str, field: &str) -> Result<bool, EditError> {
    if !known_hooks_field(field) {
        return Err(EditError::UnknownKey(key.to_owned()));
    }
    // An empty target table must still override the environment's hooks.
    match root.get_mut("hooks") {
        Some(Item::Table(table)) => Ok(table.remove(field).is_some()),
        None | Some(Item::Value(_)) => Ok(false),
        Some(_) => Err(not_a_table(key)),
    }
}

fn parse_gamescope_whole(key: &str, raw: &str) -> Result<Whole, EditError> {
    if raw == "false" {
        Ok(Whole::Off)
    } else if raw.trim() == "{}" {
        Ok(Whole::Empty)
    } else {
        Err(EditError::InvalidValue {
            key: key.to_owned(),
            reason: if raw == "true" {
                "gamescope takes a [gamescope] table or false, not true".to_owned()
            } else {
                "gamescope takes a [gamescope] table or false".to_owned()
            },
        })
    }
}

fn set_gamescope(root: &mut Table, key: &str, raw: &str) -> Result<(), EditError> {
    match parse_gamescope_whole(key, raw)? {
        Whole::Off => store_value(root, "gamescope", Value::from(false)),
        Whole::Empty => ensure_gamescope(root, key)?.clear(),
    }
    Ok(())
}

fn ensure_gamescope<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table, EditError> {
    let old_decor = root
        .get("gamescope")
        .and_then(Item::as_value)
        .filter(|value| value.as_bool() == Some(false))
        .map(|value| value.decor().clone());
    if old_decor.is_some() {
        root.remove("gamescope");
    }
    let table = ensure_table(root, "gamescope", key, false)?;
    // Only the comment carries over. The value's prefix is the space after
    // `=`, which would indent the table header.
    if let Some(suffix) = old_decor.and_then(|decor| decor.suffix().cloned()) {
        table.decor_mut().set_suffix(suffix);
    }
    Ok(table)
}

fn set_gamescope_field(
    root: &mut Table,
    key: &str,
    field: &str,
    raw: &str,
) -> Result<(), EditError> {
    let value = parse_gamescope_field(key, field, raw)?;
    let table = ensure_gamescope(root, key)?;
    store_value(table, field, value);
    Ok(())
}

fn parse_gamescope_field(key: &str, field: &str, raw: &str) -> Result<Value, EditError> {
    match field {
        "width"
        | "height"
        | "output_width"
        | "output_height"
        | "fps_limit"
        | "fps_limit_unfocused" => parse_count_value(key, raw),
        "filter" | "scaler" | "window" => {
            use crate::gamescope::{Filter, Scaler, Window};
            let allowed = match field {
                "filter" => Filter::ALL.iter().any(|value| value.as_str() == raw),
                "scaler" => Scaler::ALL.iter().any(|value| value.as_str() == raw),
                _ => Window::ALL.iter().any(|value| value.as_str() == raw),
            };
            if !allowed {
                return Err(EditError::InvalidValue {
                    key: key.to_owned(),
                    reason: format!("unknown {field} `{raw}`"),
                });
            }
            parse_string_value(key, raw)
        }
        "grab_cursor" => Ok(Value::from(parse_bool(key, raw)?)),
        "args" => parse_args_value(key, raw),
        _ => Err(EditError::UnknownKey(key.to_owned())),
    }
}

fn known_gamescope_field(field: &str) -> bool {
    matches!(
        field,
        "width"
            | "height"
            | "output_width"
            | "output_height"
            | "fps_limit"
            | "fps_limit_unfocused"
            | "filter"
            | "scaler"
            | "window"
            | "grab_cursor"
            | "args"
    )
}

fn unset_gamescope_field(root: &mut Table, key: &str, field: &str) -> Result<bool, EditError> {
    if !known_gamescope_field(field) {
        return Err(EditError::UnknownKey(key.to_owned()));
    }
    // An empty gamescope table still turns gamescope on.
    match root.get_mut("gamescope") {
        Some(Item::Table(table)) => Ok(table.remove(field).is_some()),
        None | Some(Item::Value(_)) => Ok(false),
        Some(_) => Err(not_a_table(key)),
    }
}

fn unset_environment(root: &mut Table, key: &str) -> Result<bool, EditError> {
    match key {
        "prefix" | "runner" | "gamemode" | "wayland" | "mangohud" | "gamescope" | "hooks"
        | "gameid" | "tags" | "winetricks" | "store" | "dxvk" | "vkd3d" | "title" | "desktop"
        | "gog_id" => Ok(root.remove(key).is_some()),
        "arch" => Err(read_only_arch()),
        _ => {
            if let Some(var) = key.strip_prefix("env.") {
                let var = validate_env_var(key, var)?;
                unset_field(root, "env", "env", var)
            } else if let Some(rest) = key.strip_prefix("targets.") {
                unset_target(root, key, rest)
            } else if let Some(field) = key.strip_prefix("gamescope.") {
                unset_gamescope_field(root, key, field)
            } else if let Some(field) = key.strip_prefix("hooks.") {
                unset_hooks_field(root, key, field)
            } else {
                Err(EditError::UnknownKey(key.to_string()))
            }
        }
    }
}

fn unset_global(root: &mut Table, key: &str) -> Result<bool, EditError> {
    match key {
        "stop_timeout" | "log_limit_mb" | "gamemode" | "wayland" | "mangohud"
        | "external_runners" | "steam" => Ok(root.remove(key).is_some()),
        "defaults.runner" => unset_field(root, "defaults", "defaults", "runner"),
        "defaults.arch" => unset_field(root, "defaults", "defaults", "arch"),
        "defaults.prefix_root" => unset_field(root, "defaults", "defaults", "prefix_root"),
        "defaults.runner_root" => unset_field(root, "defaults", "defaults", "runner_root"),
        "defaults.dxvk_root" => unset_field(root, "defaults", "defaults", "dxvk_root"),
        "defaults.vkd3d_root" => unset_field(root, "defaults", "defaults", "vkd3d_root"),
        "defaults.games_root" => unset_field(root, "defaults", "defaults", "games_root"),
        "gog.language" => unset_field(root, "gog", "gog", "language"),
        "art.fetch" => unset_field(root, "art", "art", "fetch"),
        _ => {
            if let Some(var) = key.strip_prefix("env.") {
                let var = validate_env_var(key, var)?;
                unset_field(root, "env", "env", var)
            } else {
                Err(EditError::UnknownKey(key.to_string()))
            }
        }
    }
}

/// Removes `field` from the `table_name` table, dropping the table too if
/// that empties it. `Ok(false)` if the table or the field wasn't there.
fn unset_field(
    root: &mut Table,
    table_name: &str,
    error_key: &str,
    field: &str,
) -> Result<bool, EditError> {
    let Some(table) = existing_table(root, table_name, error_key)? else {
        return Ok(false);
    };
    let removed = table.remove(field).is_some();
    if removed && table.is_empty() {
        root.remove(table_name);
    }
    Ok(removed)
}

/// Handles `targets.<name>` and `targets.<name>.<field>` for `unset`.
fn unset_target(root: &mut Table, key: &str, rest: &str) -> Result<bool, EditError> {
    let (name, field) = split_dotted(rest);
    if !crate::name::is_valid(name) {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("`{name}` is not a valid target name"),
        });
    }
    if let Some(field) = field
        && !matches!(
            field,
            "exe"
                | "args"
                | "cwd"
                | "default"
                | "gamemode"
                | "wayland"
                | "mangohud"
                | "gamescope"
                | "hooks"
                | "desktop"
                | "name"
        )
        && !field
            .strip_prefix("gamescope.")
            .is_some_and(known_gamescope_field)
        && !field.strip_prefix("hooks.").is_some_and(known_hooks_field)
    {
        return Err(EditError::UnknownKey(key.to_string()));
    }

    let Some(targets) = existing_table(root, "targets", "targets")? else {
        return Ok(false);
    };

    match field {
        None => {
            let removed = targets.remove(name).is_some();
            if removed && targets.is_empty() {
                root.remove("targets");
            }
            Ok(removed)
        }
        Some(field) => {
            let error_key = format!("targets.{name}");
            let Some(target) = existing_table(targets, name, &error_key)? else {
                return Ok(false);
            };
            let removed = if let Some(gamescope_field) = field.strip_prefix("gamescope.") {
                unset_gamescope_field(target, key, gamescope_field)?
            } else if let Some(hooks_field) = field.strip_prefix("hooks.") {
                unset_hooks_field(target, key, hooks_field)?
            } else {
                target.remove(field).is_some()
            };
            if removed && target.is_empty() {
                targets.remove(name);
                if targets.is_empty() {
                    root.remove("targets");
                }
            }
            Ok(removed)
        }
    }
}

/// `gog_id` names the game folder an environment owns, so only the GOG
/// import sets it. Unsetting it stays allowed.
pub fn read_only_gog_id() -> EditError {
    EditError::Managed("gog_id is set by wineo import gog.".to_owned())
}

fn read_only_arch() -> EditError {
    EditError::ReadOnly {
        key: "arch".to_string(),
        reason: "arch is fixed when the prefix is created".to_string(),
    }
}

/// Splits `a.b.c` into (`a`, `Some("b.c")`), or (`a`, `None`) with no dot.
fn split_dotted(s: &str) -> (&str, Option<&str>) {
    match s.split_once('.') {
        Some((head, tail)) => (head, Some(tail)),
        None => (s, None),
    }
}

/// Stores `value` under `field`. If `field` already holds a value, this
/// keeps its position and copies over its decor (spacing, inline comments).
pub(crate) fn store_value(table: &mut Table, field: &str, mut value: Value) {
    match table.entry(field) {
        Entry::Occupied(mut entry) => {
            if let Item::Value(old) = entry.get() {
                // The same value in other quotes or spacing is left as written,
                // so a no-op edit doesn't rewrite a hand-written file.
                if same_value(old, &value) {
                    return;
                }
                *value.decor_mut() = old.decor().clone();
            }
            entry.insert(Item::Value(value));
        }
        Entry::Vacant(entry) => {
            entry.insert(Item::Value(value));
        }
    }
}

/// Whether two values mean the same, whatever their quoting or spacing.
fn same_value(old: &Value, new: &Value) -> bool {
    match (old, new) {
        (Value::String(old), Value::String(new)) => old.value() == new.value(),
        (Value::Integer(old), Value::Integer(new)) => old.value() == new.value(),
        (Value::Boolean(old), Value::Boolean(new)) => old.value() == new.value(),
        (Value::Array(old), Value::Array(new)) => {
            old.len() == new.len() && old.iter().zip(new).all(|(old, new)| same_value(old, new))
        }
        _ => false,
    }
}

/// Gets `name` as a table, creating an empty one (appended at the end) if
/// it's missing. Fails if `name` exists but isn't a table.
pub(crate) fn ensure_table<'a>(
    parent: &'a mut Table,
    name: &str,
    error_key: &str,
    implicit_if_new: bool,
) -> Result<&'a mut Table, EditError> {
    match parent.entry(name) {
        Entry::Occupied(entry) => match entry.into_mut() {
            Item::Table(table) => Ok(table),
            _ => Err(not_a_table(error_key)),
        },
        Entry::Vacant(entry) => {
            let mut table = Table::new();
            table.set_implicit(implicit_if_new);
            Ok(entry
                .insert(Item::Table(table))
                .as_table_mut()
                .expect("just inserted a table"))
        }
    }
}

/// Like `ensure_table`, but never creates the table: `Ok(None)` if missing.
pub(crate) fn existing_table<'a>(
    parent: &'a mut Table,
    name: &str,
    error_key: &str,
) -> Result<Option<&'a mut Table>, EditError> {
    match parent.get_mut(name) {
        None => Ok(None),
        Some(Item::Table(table)) => Ok(Some(table)),
        Some(_) => Err(not_a_table(error_key)),
    }
}

fn not_a_table(key: &str) -> EditError {
    EditError::InvalidValue {
        key: key.to_string(),
        reason: format!("`{key}` exists but isn't a table"),
    }
}

fn validate_env_var<'a>(key: &str, var: &'a str) -> Result<&'a str, EditError> {
    if var.is_empty() || var.contains('=') || var.contains('\0') {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("`{var}` is not a valid environment variable name"),
        });
    }
    Ok(var)
}

fn parse_string_value(key: &str, raw: &str) -> Result<Value, EditError> {
    if raw.contains('\0') {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: "value contains a NUL byte".to_string(),
        });
    }
    Ok(Value::from(raw.to_string()))
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, EditError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("expected `true` or `false`, got `{raw}`"),
        }),
    }
}

fn parse_count_value(key: &str, raw: &str) -> Result<Value, EditError> {
    let seconds: u32 = raw.parse().map_err(|_| EditError::InvalidValue {
        key: key.to_string(),
        reason: format!("expected a non-negative integer, got `{raw}`"),
    })?;
    Ok(Value::from(i64::from(seconds)))
}

fn parse_arch_value(key: &str, raw: &str) -> Result<Value, EditError> {
    if raw != "win64" && raw != "win32" {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("expected `win64` or `win32`, got `{raw}`"),
        });
    }
    Ok(Value::from(raw.to_string()))
}

fn parse_args_value(key: &str, raw: &str) -> Result<Value, EditError> {
    // The TOML parser's own message ("invalid float" and such) doesn't help here.
    let value: Value = raw.parse().map_err(|_| EditError::InvalidValue {
        key: key.to_string(),
        reason: r#"expected a list like ["-a", "-b"]"#.to_string(),
    })?;
    let Value::Array(array) = value else {
        return Err(EditError::InvalidValue {
            key: key.to_string(),
            reason: format!("expected an array of strings, got a {}", value.type_name()),
        });
    };
    for item in array.iter() {
        if item.as_str().is_none() {
            return Err(EditError::InvalidValue {
                key: key.to_string(),
                reason: format!("array items must be strings, got a {}", item.type_name()),
            });
        }
    }
    Ok(Value::Array(array))
}

#[cfg(test)]
mod tests;
