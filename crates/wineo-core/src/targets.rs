//! Changing an environment's targets as one batch, for the setup flows.
//!
//! Changes are collected against the config as it was read, without holding
//! any lock, and only written once they have been reviewed. The write reads
//! the config again under its lock and refuses when anything the batch touches
//! has changed in the meantime, so a `wineo set` that landed while someone was
//! choosing is never overwritten. Whatever the batch doesn't touch keeps its
//! value and its formatting.

use std::collections::BTreeMap;

use toml_edit::{Array, DocumentMut, Value};

use crate::art::TargetCleanup;
use crate::config::{self, EnvironmentConfig, Target};
use crate::edit::{ensure_table, existing_table, store_value};
use crate::error::{Error, IoContext, Result};
use crate::files::{self, write_atomic};
use crate::name;
use crate::operation::Locks;
use crate::paths::Paths;
use crate::winpath;

/// The settings the setup flows edit. Anything else a target has, like
/// `gamemode`, is left as it is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Spec {
    pub exe: String,
    pub args: Vec<String>,
    /// None runs it from the executable's own directory.
    pub cwd: Option<String>,
    /// What the desktop entry is called. None uses the target's name.
    pub name: Option<String>,
    pub desktop: bool,
}

impl Spec {
    pub fn of(target: &Target) -> Self {
        Self {
            exe: target.exe.clone(),
            args: target.args.clone(),
            cwd: target.cwd.clone(),
            name: target.name.clone(),
            desktop: target.desktop,
        }
    }
}

/// One line of a batch's review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added {
        target: String,
        spec: Spec,
    },
    Edited {
        target: String,
        before: Spec,
        after: Spec,
    },
    Removed {
        target: String,
        spec: Spec,
    },
    Default {
        before: Option<String>,
        after: Option<String>,
    },
}

/// Why a target could not be added to a batch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("`{0}` can't be a target name: use letters, digits, `-`, and `_`")]
    InvalidName(String),
    #[error("there is already a target called `{0}`")]
    Taken(String),
}

/// Target changes collected against the config as it was read.
#[derive(Debug, Clone)]
pub struct Batch {
    base: BTreeMap<String, Target>,
    /// Some is a target added or edited, None one removed.
    changes: BTreeMap<String, Option<Spec>>,
    /// Some once the default has been chosen, as a target or as none.
    default: Option<Option<String>>,
}

impl Batch {
    pub fn new(base: BTreeMap<String, Target>) -> Self {
        Self {
            base,
            changes: BTreeMap::new(),
            default: None,
        }
    }

    /// The targets as they will be once this batch is written.
    pub fn targets(&self) -> BTreeMap<String, Spec> {
        let mut targets: BTreeMap<String, Spec> = self
            .base
            .iter()
            .map(|(name, target)| (name.clone(), Spec::of(target)))
            .collect();
        for (name, change) in &self.changes {
            match change {
                Some(spec) => targets.insert(name.clone(), spec.clone()),
                None => targets.remove(name),
            };
        }
        targets
    }

    pub fn contains(&self, target: &str) -> bool {
        match self.changes.get(target) {
            Some(change) => change.is_some(),
            None => self.base.contains_key(target),
        }
    }

    /// The target `start <name>` will launch once this batch is written,
    /// when one is marked. A lone target is launched without being marked.
    pub fn default(&self) -> Option<String> {
        let chosen = match &self.default {
            Some(chosen) => chosen.clone(),
            None => base_default(&self.base),
        };
        chosen.filter(|target| self.contains(target))
    }

    /// Whether the batch would change anything at all.
    pub fn is_empty(&self) -> bool {
        self.summary().is_empty()
    }

    pub fn add(&mut self, target: &str, spec: Spec) -> Result<(), Refused> {
        if !name::is_valid(target) {
            return Err(Refused::InvalidName(target.to_owned()));
        }
        if self.contains(target) {
            return Err(Refused::Taken(target.to_owned()));
        }
        self.changes.insert(target.to_owned(), Some(spec));
        Ok(())
    }

    /// Replaces the editable settings of a target this batch has.
    pub fn edit(&mut self, target: &str, spec: Spec) {
        assert!(self.contains(target), "only an existing target is edited");
        if self.base.get(target).map(Spec::of).as_ref() == Some(&spec) {
            self.changes.remove(target);
        } else {
            self.changes.insert(target.to_owned(), Some(spec));
        }
    }

    pub fn remove(&mut self, target: &str) {
        if self.base.contains_key(target) {
            self.changes.insert(target.to_owned(), None);
        } else {
            self.changes.remove(target);
        }
    }

    /// Marks the target `start <name>` launches, or unmarks them all.
    pub fn set_default(&mut self, target: Option<&str>) {
        self.default = Some(target.map(str::to_owned));
    }

    /// Targets that already run `exe`, compared the way Windows compares
    /// paths when both are Windows paths.
    pub fn using(&self, exe: &str) -> Vec<String> {
        self.targets()
            .into_iter()
            .filter(|(_, spec)| same_exe(&spec.exe, exe))
            .map(|(name, _)| name)
            .collect()
    }

    /// What writing this batch would change, for its review.
    pub fn summary(&self) -> Vec<Change> {
        let mut changes = Vec::new();
        for (target, change) in &self.changes {
            let before = self.base.get(target).map(Spec::of);
            match (before, change) {
                (None, Some(spec)) => changes.push(Change::Added {
                    target: target.clone(),
                    spec: spec.clone(),
                }),
                (Some(before), Some(after)) if &before != after => changes.push(Change::Edited {
                    target: target.clone(),
                    before,
                    after: after.clone(),
                }),
                (Some(spec), None) => changes.push(Change::Removed {
                    target: target.clone(),
                    spec,
                }),
                _ => {}
            }
        }
        let (before, after) = (base_default(&self.base), self.default());
        if before != after {
            changes.push(Change::Default { before, after });
        }
        changes
    }

    /// The same changes against the config as it is now. An edit carries over
    /// only the settings it changed, so what changed meanwhile in the others
    /// is kept. Changes that no longer apply are dropped, with a note for
    /// each saying why.
    pub fn rebase(&self, current: BTreeMap<String, Target>) -> (Batch, Vec<String>) {
        let mut rebased = Batch::new(current);
        let mut dropped = Vec::new();
        for (target, change) in &self.changes {
            let old = self.base.get(target).map(Spec::of);
            match (change, old) {
                (Some(spec), None) => {
                    if let Err(refused) = rebased.add(target, spec.clone()) {
                        dropped.push(format!("`{target}` was not added: {refused}"));
                    }
                }
                (Some(spec), Some(old)) => {
                    if let Some(now) = rebased.base.get(target).map(Spec::of) {
                        rebased.edit(target, merge(&old, spec, now));
                    } else {
                        dropped.push(format!(
                            "`{target}` was not changed: it was removed in the meantime"
                        ));
                    }
                }
                // Already gone is what the removal wanted.
                (None, _) => rebased.remove(target),
            }
        }
        if let Some(chosen) = &self.default {
            match chosen {
                Some(target) if !rebased.contains(target) => dropped.push(format!(
                    "`{target}` was not made the default: it no longer exists"
                )),
                chosen => rebased.default = Some(chosen.clone()),
            }
        }
        (rebased, dropped)
    }

    /// Whether a target gains a desktop entry it didn't already ask for.
    fn enables_desktop(&self) -> bool {
        self.changes.iter().any(|(target, change)| {
            change.as_ref().is_some_and(|spec| spec.desktop)
                && !self.base.get(target).is_some_and(|target| target.desktop)
        })
    }

    /// Whether anything this batch touches differs between the config it was
    /// collected against and `current`.
    fn is_stale(&self, current: &BTreeMap<String, Target>) -> bool {
        let touched = self
            .changes
            .keys()
            .any(|target| self.base.get(target) != current.get(target));
        let default_changes = base_default(&self.base) != self.default();
        touched || (default_changes && base_default(&self.base) != base_default(current))
    }

    fn apply(&self, doc: &mut DocumentMut) -> Result<()> {
        let root = doc.as_table_mut();
        let targets = ensure_table(root, "targets", "targets", true)?;
        for (target, change) in &self.changes {
            match change {
                None => {
                    targets.remove(target);
                }
                Some(spec) => {
                    let key = format!("targets.{target}");
                    let table = ensure_table(targets, target, &key, false)?;
                    let before = self.base.get(target).map(Spec::of);
                    write_spec(table, before.as_ref(), spec);
                }
            }
        }
        let default = self.default();
        if default != base_default(&self.base) {
            let names: Vec<String> = targets.iter().map(|(name, _)| name.to_owned()).collect();
            for name in names {
                if let Some(table) = existing_table(targets, &name, &format!("targets.{name}"))? {
                    table.remove("default");
                }
            }
            if let Some(default) = default {
                let key = format!("targets.{default}");
                let table = ensure_table(targets, &default, &key, false)?;
                store_value(table, "default", Value::from(true));
            }
        }
        if targets.is_empty() {
            root.remove("targets");
        }
        Ok(())
    }
}

/// `now` with whatever `edited` changed relative to `old`.
fn merge(old: &Spec, edited: &Spec, now: Spec) -> Spec {
    fn pick<T: PartialEq + Clone>(old: &T, edited: &T, now: T) -> T {
        if old == edited { now } else { edited.clone() }
    }
    Spec {
        exe: pick(&old.exe, &edited.exe, now.exe),
        args: pick(&old.args, &edited.args, now.args),
        cwd: pick(&old.cwd, &edited.cwd, now.cwd),
        name: pick(&old.name, &edited.name, now.name),
        desktop: pick(&old.desktop, &edited.desktop, now.desktop),
    }
}

/// Only the settings that changed are written, so the rest keep their
/// formatting and any comments beside them.
fn write_spec(table: &mut toml_edit::Table, before: Option<&Spec>, spec: &Spec) {
    let changed = |same: bool| before.is_none() || !same;
    if changed(before.is_some_and(|before| before.exe == spec.exe)) {
        store_value(table, "exe", Value::from(spec.exe.as_str()));
    }
    if changed(before.is_some_and(|before| before.args == spec.args)) {
        if spec.args.is_empty() {
            table.remove("args");
        } else {
            let args: Array = spec.args.iter().map(String::as_str).collect();
            store_value(table, "args", Value::Array(args));
        }
    }
    store_optional(table, "cwd", &spec.cwd, before.map(|before| &before.cwd));
    if changed(before.is_some_and(|before| before.desktop == spec.desktop)) {
        if spec.desktop {
            store_value(table, "desktop", Value::from(true));
        } else {
            table.remove("desktop");
        }
    }
    store_optional(table, "name", &spec.name, before.map(|before| &before.name));
}

fn store_optional(
    table: &mut toml_edit::Table,
    key: &str,
    value: &Option<String>,
    before: Option<&Option<String>>,
) {
    if before == Some(value) {
        return;
    }
    match value {
        Some(value) => store_value(table, key, Value::from(value.as_str())),
        None => {
            table.remove(key);
        }
    }
}

fn base_default(targets: &BTreeMap<String, Target>) -> Option<String> {
    targets
        .iter()
        .find(|(_, target)| target.default)
        .map(|(name, _)| name.clone())
}

pub fn same_exe(left: &str, right: &str) -> bool {
    if winpath::is_windows_path(left) && winpath::is_windows_path(right) {
        left.replace('/', "\\")
            .eq_ignore_ascii_case(&right.replace('/', "\\"))
    } else {
        left == right
    }
}

/// How writing a batch went.
#[derive(Debug)]
pub enum Written {
    Saved,
    /// Something the batch touches changed since it was read, so nothing was
    /// written. These are the targets as they are now, to rebase onto.
    Changed(BTreeMap<String, Target>),
}

/// Writes a reviewed batch in one validated config write, under the config
/// lock `wineo set` shares.
pub fn write(paths: &Paths, environment: &str, batch: &Batch) -> Result<Written> {
    if !name::is_valid(environment) {
        return Err(Error::InvalidName(environment.to_owned()));
    }
    let path = paths.environment_config(environment);
    let _guard = Locks::new(paths).config(&path)?;
    let Some(text) = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?
    else {
        return Err(Error::UnknownEnvironment(environment.to_owned()));
    };
    let current = EnvironmentConfig::parse(&path, &text)?;
    if batch.is_stale(&current.targets) {
        return Ok(Written::Changed(current.targets));
    }
    if paths.portable_root().is_some() && batch.enables_desktop() {
        return Err(Error::PortableDesktop);
    }
    let mut doc: DocumentMut = text.parse().map_err(|source| Error::ConfigParse {
        path: path.clone(),
        source: Box::new(source),
    })?;
    batch.apply(&mut doc)?;
    let mut updated = doc.to_string();
    if updated != text {
        config::stamp_schema(&mut doc);
        updated = doc.to_string();
    }
    let written = EnvironmentConfig::parse(&path, &updated)?;
    let removed: Vec<String> = current
        .targets
        .keys()
        .filter(|target| !written.targets.contains_key(*target))
        .cloned()
        .collect();
    let cleanup = if removed.is_empty() {
        None
    } else {
        Some(TargetCleanup::prepare(paths, environment, removed)?)
    };
    if updated != text {
        write_atomic(&path, &updated)?;
    }
    if let Some(cleanup) = cleanup {
        cleanup.finish()?;
    }
    Ok(Written::Saved)
}

/// A target name for a program, from its label, like `hades-ii` for
/// `Hades II`. Falls back to the executable's name, then to `app`, when
/// neither has a usable character in it.
pub fn suggest_name(label: &str, exe: &str) -> String {
    let stem = exe
        .rsplit(['\\', '/'])
        .next()
        .map(|file| file.rsplit_once('.').map_or(file, |(stem, _)| stem))
        .unwrap_or_default();
    slug(label)
        .or_else(|| slug(stem))
        .unwrap_or_else(|| "app".to_owned())
}

fn slug(text: &str) -> Option<String> {
    let mut slug = String::new();
    let mut gap = false;
    for char in text.chars() {
        if char.is_ascii_alphanumeric() || char == '_' {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            gap = false;
            slug.push(char.to_ascii_lowercase());
        } else if char.is_whitespace() || matches!(char, '-' | '.' | ':' | '/' | '\\') {
            gap = true;
        }
    }
    (!slug.is_empty()).then_some(slug)
}

/// `name`, or the first of `name-2`, `name-3`, and so on that isn't taken.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| format!("{name}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("some suffix is always free")
}

#[cfg(test)]
mod tests;
