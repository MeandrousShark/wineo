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

fn set_environment(root: &mut Table, key: &str, raw: &str) -> Result<(), EditError> {
    match key {
        "prefix" => {
            let value = parse_string_value(key, raw)?;
            store_value(root, "prefix", value);
            Ok(())
        }
        "runner" => {
            let value = parse_string_value(key, raw)?;
            store_value(root, "runner", value);
            Ok(())
        }
        "arch" => Err(read_only_arch()),
        _ => {
            if let Some(var) = key.strip_prefix("env.") {
                let var = validate_env_var(key, var)?;
                let value = parse_string_value(key, raw)?;
                let table = ensure_table(root, "env", "env", false)?;
                store_value(table, var, value);
                Ok(())
            } else if let Some(rest) = key.strip_prefix("targets.") {
                set_target(root, key, rest, raw)
            } else {
                Err(EditError::UnknownKey(key.to_string()))
            }
        }
    }
}

fn set_global(root: &mut Table, key: &str, raw: &str) -> Result<(), EditError> {
    match key {
        "stop_timeout" => {
            let value = parse_stop_timeout_value(key, raw)?;
            store_value(root, "stop_timeout", value);
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
        "defaults.prefix_root" => {
            let value = parse_string_value(key, raw)?;
            let table = ensure_table(root, "defaults", "defaults", false)?;
            store_value(table, "prefix_root", value);
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
    let value = match field {
        "exe" | "cwd" => parse_string_value(key, raw)?,
        "args" => parse_args_value(key, raw)?,
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
    let targets = ensure_table(root, "targets", "targets", true)?;
    let target = ensure_table(targets, name, &error_key, false)?;

    if !is_default {
        target.remove("default");
        return Ok(());
    }
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

fn unset_environment(root: &mut Table, key: &str) -> Result<bool, EditError> {
    match key {
        "prefix" | "runner" => Ok(root.remove(key).is_some()),
        "arch" => Err(read_only_arch()),
        _ => {
            if let Some(var) = key.strip_prefix("env.") {
                let var = validate_env_var(key, var)?;
                unset_field(root, "env", "env", var)
            } else if let Some(rest) = key.strip_prefix("targets.") {
                unset_target(root, key, rest)
            } else {
                Err(EditError::UnknownKey(key.to_string()))
            }
        }
    }
}

fn unset_global(root: &mut Table, key: &str) -> Result<bool, EditError> {
    match key {
        "stop_timeout" => Ok(root.remove(key).is_some()),
        "defaults.runner" => unset_field(root, "defaults", "defaults", "runner"),
        "defaults.arch" => unset_field(root, "defaults", "defaults", "arch"),
        "defaults.prefix_root" => unset_field(root, "defaults", "defaults", "prefix_root"),
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
        && !matches!(field, "exe" | "args" | "cwd" | "default")
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
            let removed = target.remove(field).is_some();
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
fn store_value(table: &mut Table, field: &str, mut value: Value) {
    match table.entry(field) {
        Entry::Occupied(mut entry) => {
            if let Item::Value(old) = entry.get() {
                *value.decor_mut() = old.decor().clone();
            }
            entry.insert(Item::Value(value));
        }
        Entry::Vacant(entry) => {
            entry.insert(Item::Value(value));
        }
    }
}

/// Gets `name` as a table, creating an empty one (appended at the end) if
/// it's missing. Fails if `name` exists but isn't a table.
fn ensure_table<'a>(
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
fn existing_table<'a>(
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

fn parse_stop_timeout_value(key: &str, raw: &str) -> Result<Value, EditError> {
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
mod tests {
    use super::*;

    fn parse(s: &str) -> DocumentMut {
        s.parse().expect("valid toml")
    }

    // -- parse_assignment --

    #[test]
    fn parse_assignment_splits_at_first_equals() {
        assert_eq!(parse_assignment("key=value").unwrap(), ("key", "value"));
    }

    #[test]
    fn parse_assignment_keeps_extra_equals_in_the_value() {
        assert_eq!(parse_assignment("key=a=b").unwrap(), ("key", "a=b"));
    }

    #[test]
    fn parse_assignment_requires_an_equals() {
        let err = parse_assignment("no-equals-here").unwrap_err();
        assert!(matches!(err, EditError::MissingEquals(s) if s == "no-equals-here"));
    }

    // -- set: top-level keys --

    #[test]
    fn set_top_level_key_in_empty_doc() {
        let mut doc = parse("");
        set(&mut doc, Scope::Environment, "prefix", "/home/x/prefix").unwrap();
        assert_eq!(doc.to_string(), "prefix = \"/home/x/prefix\"\n");
    }

    #[test]
    fn set_top_level_key_renders_before_existing_tables() {
        let mut doc = parse("[env]\nFOO = \"1\"\n");
        set(&mut doc, Scope::Environment, "runner", "wine").unwrap();
        // `[env]` keeps its own original spacing (none, since it was the
        // first line); only the new key is inserted ahead of it.
        assert_eq!(doc.to_string(), "runner = \"wine\"\n[env]\nFOO = \"1\"\n");
    }

    #[test]
    fn set_updates_existing_key_and_keeps_other_comments() {
        let mut doc = parse("# top comment\nprefix = \"/old\"\nrunner = \"wine\" # keep me\n");
        set(&mut doc, Scope::Environment, "prefix", "/new").unwrap();
        assert_eq!(
            doc.to_string(),
            "# top comment\nprefix = \"/new\"\nrunner = \"wine\" # keep me\n"
        );
    }

    // -- set: creating tables --

    #[test]
    fn set_creates_env_table() {
        let mut doc = parse("prefix = \"/x\"\n");
        set(&mut doc, Scope::Environment, "env.WINEDEBUG", "fixme-all").unwrap();
        assert_eq!(
            doc.to_string(),
            "prefix = \"/x\"\n\n[env]\nWINEDEBUG = \"fixme-all\"\n"
        );
    }

    #[test]
    fn set_creates_target_with_implicit_parent() {
        let mut doc = parse("");
        set(
            &mut doc,
            Scope::Environment,
            "targets.bridge.exe",
            "bridge.exe",
        )
        .unwrap();
        assert_eq!(doc.to_string(), "[targets.bridge]\nexe = \"bridge.exe\"\n");
    }

    #[test]
    fn set_whole_target_key_is_invalid() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Environment, "targets.bridge", "x").unwrap_err();
        assert!(matches!(
            err,
            EditError::InvalidValue { key, .. } if key == "targets.bridge"
        ));
    }

    // -- set: string formatting --

    #[test]
    fn set_string_with_backslash_renders_as_literal() {
        let mut doc = parse("");
        set(
            &mut doc,
            Scope::Environment,
            "targets.bridge.exe",
            "bridge.exe",
        )
        .unwrap();
        set(
            &mut doc,
            Scope::Environment,
            "targets.bridge.cwd",
            "C:\\Program Files\\Bridge",
        )
        .unwrap();
        assert_eq!(
            doc.to_string(),
            "[targets.bridge]\nexe = \"bridge.exe\"\ncwd = 'C:\\Program Files\\Bridge'\n"
        );
    }

    // -- set: args arrays --

    #[test]
    fn set_args_array_of_strings() {
        let mut doc = parse("");
        set(
            &mut doc,
            Scope::Environment,
            "targets.bridge.args",
            r#"["-a", "-b"]"#,
        )
        .unwrap();
        assert_eq!(
            doc.to_string(),
            "[targets.bridge]\nargs = [\"-a\", \"-b\"]\n"
        );
    }

    #[test]
    fn set_args_rejects_non_array() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Environment, "targets.bridge.args", "5").unwrap_err();
        assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bridge.args"));
        // the failed parse must not have left a stray empty table behind
        assert_eq!(doc.to_string(), "");
    }

    #[test]
    fn set_args_rejects_non_string_item() {
        let mut doc = parse("");
        let err = set(
            &mut doc,
            Scope::Environment,
            "targets.bridge.args",
            r#"["-a", 5]"#,
        )
        .unwrap_err();
        assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bridge.args"));
    }

    // -- set: bool parsing --

    #[test]
    fn set_default_rejects_non_bool() {
        let mut doc = parse("");
        let err = set(
            &mut doc,
            Scope::Environment,
            "targets.bridge.default",
            "maybe",
        )
        .unwrap_err();
        assert!(
            matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bridge.default")
        );
    }

    // -- set: stop_timeout --

    #[test]
    fn set_stop_timeout_valid() {
        let mut doc = parse("");
        set(&mut doc, Scope::Global, "stop_timeout", "10").unwrap();
        assert_eq!(doc.to_string(), "stop_timeout = 10\n");
    }

    #[test]
    fn set_stop_timeout_rejects_negative() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Global, "stop_timeout", "-1").unwrap_err();
        assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "stop_timeout"));
    }

    #[test]
    fn set_stop_timeout_rejects_non_number() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Global, "stop_timeout", "soon").unwrap_err();
        assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "stop_timeout"));
    }

    // -- set: defaults.arch --

    #[test]
    fn set_defaults_arch_valid() {
        let mut doc = parse("");
        set(&mut doc, Scope::Global, "defaults.arch", "win64").unwrap();
        assert_eq!(doc.to_string(), "[defaults]\narch = \"win64\"\n");
    }

    #[test]
    fn set_defaults_arch_rejects_unknown_arch() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Global, "defaults.arch", "win95").unwrap_err();
        assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "defaults.arch"));
    }

    // -- set: default target switching --

    #[test]
    fn set_default_switches_between_targets() {
        let mut doc = parse("");
        set(&mut doc, Scope::Environment, "targets.a.exe", "a.exe").unwrap();
        set(&mut doc, Scope::Environment, "targets.a.default", "true").unwrap();
        set(&mut doc, Scope::Environment, "targets.b.exe", "b.exe").unwrap();
        set(&mut doc, Scope::Environment, "targets.b.default", "true").unwrap();
        assert_eq!(
            doc.to_string(),
            "[targets.a]\nexe = \"a.exe\"\n\n[targets.b]\nexe = \"b.exe\"\ndefault = true\n"
        );
    }

    // -- unset --

    #[test]
    fn unset_env_var_removes_empty_table() {
        let mut doc = parse("[env]\nFOO = \"1\"\n");
        let removed = unset(&mut doc, Scope::Environment, "env.FOO").unwrap();
        assert!(removed);
        assert_eq!(doc.to_string(), "");
    }

    #[test]
    fn unset_whole_target() {
        let mut doc = parse(
            "[targets.bridge]\nexe = \"bridge.exe\"\n\n[targets.other]\nexe = \"other.exe\"\n",
        );
        let removed = unset(&mut doc, Scope::Environment, "targets.bridge").unwrap();
        assert!(removed);
        // `[targets.other]` keeps the blank line it was originally written
        // with, even though it's now the first table in the document.
        assert_eq!(doc.to_string(), "\n[targets.other]\nexe = \"other.exe\"\n");
    }

    #[test]
    fn unset_missing_key_returns_false() {
        let mut doc = parse("");
        assert!(!unset(&mut doc, Scope::Environment, "prefix").unwrap());
        assert!(!unset(&mut doc, Scope::Environment, "env.FOO").unwrap());
        assert!(!unset(&mut doc, Scope::Environment, "targets.bridge").unwrap());
        assert!(!unset(&mut doc, Scope::Environment, "targets.bridge.exe").unwrap());
    }

    // -- unknown keys --

    #[test]
    fn unknown_key_env_scope() {
        let mut doc = parse("");
        assert!(matches!(
            set(&mut doc, Scope::Environment, "envv.X", "1"),
            Err(EditError::UnknownKey(k)) if k == "envv.X"
        ));
        assert!(matches!(
            unset(&mut doc, Scope::Environment, "envv.X"),
            Err(EditError::UnknownKey(k)) if k == "envv.X"
        ));
        assert!(matches!(
            set(&mut doc, Scope::Environment, "targets.a.bogus", "1"),
            Err(EditError::UnknownKey(k)) if k == "targets.a.bogus"
        ));
        assert!(matches!(
            set(&mut doc, Scope::Environment, "stop_timeout", "1"),
            Err(EditError::UnknownKey(k)) if k == "stop_timeout"
        ));
    }

    #[test]
    fn unknown_key_global_scope() {
        let mut doc = parse("");
        assert!(matches!(
            set(&mut doc, Scope::Global, "prefix", "/x"),
            Err(EditError::UnknownKey(k)) if k == "prefix"
        ));
        assert!(matches!(
            set(&mut doc, Scope::Global, "envv.X", "1"),
            Err(EditError::UnknownKey(k)) if k == "envv.X"
        ));
    }

    // -- read-only arch --

    #[test]
    fn arch_is_read_only() {
        let mut doc = parse("");
        assert!(matches!(
            set(&mut doc, Scope::Environment, "arch", "win64"),
            Err(EditError::ReadOnly { key, .. }) if key == "arch"
        ));
        assert!(matches!(
            unset(&mut doc, Scope::Environment, "arch"),
            Err(EditError::ReadOnly { key, .. }) if key == "arch"
        ));
    }

    // -- invalid names --

    #[test]
    fn set_rejects_invalid_target_name() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Environment, "targets.bad name.exe", "x").unwrap_err();
        assert!(
            matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bad name.exe")
        );
    }

    #[test]
    fn set_rejects_invalid_env_var_name() {
        let mut doc = parse("");
        let err = set(&mut doc, Scope::Environment, "env.FOO=BAR", "x").unwrap_err();
        assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "env.FOO=BAR"));
    }
}
