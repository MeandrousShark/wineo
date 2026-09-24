//! Changing an environment's targets as one batch, for the setup flows.
//!
//! Changes are collected against the config as it was read, without holding
//! any lock, and only written once they have been reviewed. The write reads
//! the config again under its lock and refuses when anything the batch touches
//! has changed in the meantime, so a `wineo set` that landed while someone was
//! choosing is never overwritten. Whatever the batch doesn't touch keeps its
//! value and its formatting.

use std::collections::BTreeMap;
use std::fs;
use std::io;

use toml_edit::{Array, DocumentMut, Value};

use crate::config::{self, EnvironmentConfig, Target};
use crate::edit::{ensure_table, existing_table, store_value};
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
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

fn same_exe(left: &str, right: &str) -> bool {
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
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(Error::UnknownEnvironment(environment.to_owned()));
        }
        Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
    };
    let current = EnvironmentConfig::parse(&path, &text)?;
    if batch.is_stale(&current.targets) {
        return Ok(Written::Changed(current.targets));
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
    EnvironmentConfig::parse(&path, &updated)?;
    if updated != text {
        write_atomic(&path, &updated)?;
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
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::*;

    fn parse(text: &str) -> BTreeMap<String, Target> {
        let text = format!("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n{text}");
        EnvironmentConfig::parse(Path::new("/x.toml"), &text)
            .unwrap()
            .targets
    }

    fn spec(exe: &str) -> Spec {
        Spec {
            exe: exe.to_owned(),
            ..Spec::default()
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        paths: Paths,
    }

    impl Fixture {
        fn new(config: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().to_owned();
            let paths = Paths::from_lookup(move |key| {
                (key == "HOME").then(|| OsString::from(home.as_os_str()))
            })
            .unwrap();
            let path = paths.environment_config("hades");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, config).unwrap();
            Self { _root: root, paths }
        }

        fn text(&self) -> String {
            fs::read_to_string(self.paths.environment_config("hades")).unwrap()
        }

        fn targets(&self) -> BTreeMap<String, Target> {
            EnvironmentConfig::parse(Path::new("/x.toml"), &self.text())
                .unwrap()
                .targets
        }
    }

    const HEADER: &str = "# my notes\nprefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\ngamemode = true\n\n[env]\nDXVK_HUD = \"fps\"\n";

    #[test]
    fn names_come_from_labels_then_executables() {
        assert_eq!(suggest_name("Hades II", r"C:\x\Hades2.exe"), "hades-ii");
        assert_eq!(
            suggest_name("Hades: Battle Out of Hell", ""),
            "hades-battle-out-of-hell"
        );
        assert_eq!(suggest_name("My_Game v1.2", ""), "my_game-v1-2");
        assert_eq!(suggest_name("  --Fly Corp--  ", ""), "fly-corp");
        // Nothing usable in the label, so the executable names it.
        assert_eq!(suggest_name("游戏", r"C:\Games\Game.exe"), "game");
        assert_eq!(suggest_name("", "/opt/tools/run.sh"), "run");
        assert_eq!(suggest_name("™", "™.exe"), "app");
        for label in ["Hades II", "My_Game v1.2", "  --Fly Corp--  "] {
            assert!(name::is_valid(&suggest_name(label, "")), "{label}");
        }
    }

    #[test]
    fn a_taken_name_gets_the_first_free_suffix() {
        let taken = ["hades", "hades-2"];
        assert_eq!(free_name("hades", |name| taken.contains(&name)), "hades-3");
        assert_eq!(free_name("config", |name| taken.contains(&name)), "config");
    }

    #[test]
    fn a_batch_shows_the_targets_it_will_leave() {
        let mut batch = Batch::new(parse(
            "[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndefault = true\n[targets.old]\nexe = 'old.exe'\n",
        ));
        batch.add("config", spec(r"C:\Hades\Config.exe")).unwrap();
        batch.remove("old");
        let names: Vec<String> = batch.targets().into_keys().collect();
        assert_eq!(names, ["config", "hades"]);
        assert_eq!(batch.default().as_deref(), Some("hades"));
        assert!(!batch.contains("old"));

        assert_eq!(
            batch.add("hades", spec("x")),
            Err(Refused::Taken("hades".to_owned()))
        );
        assert_eq!(
            batch.add("bad name", spec("x")),
            Err(Refused::InvalidName("bad name".to_owned()))
        );
        // A removed name is free again.
        assert!(batch.add("old", spec("new.exe")).is_ok());
    }

    #[test]
    fn undoing_a_change_leaves_nothing_to_write() {
        let base = parse("[targets.hades]\nexe = 'Hades.exe'\n");
        let mut batch = Batch::new(base.clone());
        let mut edited = Spec::of(&base["hades"]);
        edited.args = vec!["-windowed".to_owned()];
        batch.edit("hades", edited);
        assert!(!batch.is_empty());
        batch.edit("hades", Spec::of(&base["hades"]));
        assert!(batch.is_empty());

        batch.add("tmp", spec("tmp.exe")).unwrap();
        batch.remove("tmp");
        assert!(batch.is_empty());
    }

    #[test]
    fn removing_the_default_leaves_none_until_one_is_chosen() {
        let mut batch = Batch::new(parse(
            "[targets.a]\nexe = 'a'\ndefault = true\n[targets.b]\nexe = 'b'\n[targets.c]\nexe = 'c'\n",
        ));
        batch.remove("a");
        assert_eq!(batch.default(), None);
        batch.set_default(Some("c"));
        assert_eq!(
            batch.summary(),
            [
                Change::Removed {
                    target: "a".to_owned(),
                    spec: spec("a"),
                },
                Change::Default {
                    before: Some("a".to_owned()),
                    after: Some("c".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn writing_refuses_an_unsupported_version() {
        let text = format!("schema_version = 2\n{HEADER}");
        let f = Fixture::new(&text);
        let mut batch = Batch::new(BTreeMap::new());
        batch.add("game", spec("game.exe")).unwrap();
        assert!(matches!(
            write(&f.paths, "hades", &batch),
            Err(Error::SchemaVersion { .. })
        ));
        assert_eq!(f.text(), text);
    }

    #[test]
    fn writing_keeps_everything_the_batch_does_not_touch() {
        let f = Fixture::new(&format!(
            "{HEADER}\n[targets.hades]\n# the real game\nexe = 'C:\\Hades\\Hades.exe' # keep me\ngamemode = false\ndefault = true\n\n[targets.old]\nexe = 'old.exe'\n"
        ));
        let mut batch = Batch::new(f.targets());
        let mut hades = Spec::of(&f.targets()["hades"]);
        hades.args = vec!["-config".to_owned(), r"C:\My Games\settings.ini".to_owned()];
        hades.desktop = true;
        hades.name = Some("Hades".to_owned());
        batch.edit("hades", hades);
        batch
            .add(
                "config",
                Spec {
                    cwd: Some(r"C:\Hades".to_owned()),
                    ..spec(r"C:\Hades\Config.exe")
                },
            )
            .unwrap();
        batch.remove("old");
        assert!(matches!(
            write(&f.paths, "hades", &batch).unwrap(),
            Written::Saved
        ));

        let text = f.text();
        let stamped = HEADER.replace("gamemode = true\n", "gamemode = true\nschema_version = 1\n");
        assert!(text.starts_with(&stamped), "{text}");
        assert!(text.contains("# the real game\n"), "{text}");
        assert!(
            text.contains("exe = 'C:\\Hades\\Hades.exe' # keep me\n"),
            "{text}"
        );
        assert!(text.contains("gamemode = false\n"), "{text}");
        assert!(!text.contains("old.exe"), "{text}");
        let targets = f.targets();
        // Each argument stays one value, spaces and all.
        assert_eq!(
            targets["hades"].args,
            ["-config", r"C:\My Games\settings.ini"]
        );
        assert!(targets["hades"].desktop);
        assert!(targets["hades"].default);
        assert_eq!(targets["hades"].gamemode, Some(false));
        assert_eq!(targets["config"].cwd.as_deref(), Some(r"C:\Hades"));
        assert!(!targets["config"].desktop);
    }

    #[test]
    fn moving_the_default_is_one_write() {
        let f = Fixture::new(&format!(
            "{HEADER}\n[targets.a]\nexe = 'a'\ndefault = true\n\n[targets.b]\nexe = 'b'\n"
        ));
        let mut batch = Batch::new(f.targets());
        batch.set_default(Some("b"));
        write(&f.paths, "hades", &batch).unwrap();
        let targets = f.targets();
        assert!(!targets["a"].default);
        assert!(targets["b"].default);
    }

    #[test]
    fn a_change_made_meanwhile_is_not_overwritten() {
        let f = Fixture::new(&format!(
            "{HEADER}\n[targets.hades]\nexe = 'Hades.exe'\n\n[targets.other]\nexe = 'other.exe'\n"
        ));
        let mut batch = Batch::new(f.targets());
        batch.edit(
            "hades",
            Spec {
                args: vec!["-windowed".to_owned()],
                ..Spec::of(&f.targets()["hades"])
            },
        );

        // Something the batch doesn't touch is fine to change meanwhile.
        crate::settings::update(
            &f.paths,
            crate::settings::ConfigFile::Environment("hades"),
            &[("targets.other.args", r#"["-x"]"#)],
            &[],
        )
        .unwrap();
        assert!(matches!(
            write(&f.paths, "hades", &batch).unwrap(),
            Written::Saved
        ));
        assert_eq!(f.targets()["other"].args, ["-x"]);
        assert_eq!(f.targets()["hades"].args, ["-windowed"]);

        // Something it does touch refuses the write, and rebasing carries the
        // change over for a second review, keeping what changed meanwhile.
        let mut batch = Batch::new(f.targets());
        batch.edit(
            "hades",
            Spec {
                cwd: Some("C:\\".to_owned()),
                ..Spec::of(&f.targets()["hades"])
            },
        );
        crate::settings::update(
            &f.paths,
            crate::settings::ConfigFile::Environment("hades"),
            &[("targets.hades.args", r#"["-safe"]"#)],
            &[],
        )
        .unwrap();
        let before = f.text();
        let Written::Changed(current) = write(&f.paths, "hades", &batch).unwrap() else {
            panic!("a stale batch was written");
        };
        assert_eq!(f.text(), before);
        let (rebased, dropped) = batch.rebase(current);
        assert!(dropped.is_empty(), "{dropped:?}");
        assert!(matches!(
            rebased.summary().as_slice(),
            [Change::Edited { before, after, .. }]
                if before.args == ["-safe"] && after.args == ["-safe"] && after.cwd.as_deref() == Some("C:\\")
        ));
    }

    #[test]
    fn a_default_chosen_meanwhile_refuses_a_different_one() {
        let f = Fixture::new(&format!(
            "{HEADER}\n[targets.a]\nexe = 'a'\n\n[targets.b]\nexe = 'b'\n"
        ));
        let mut batch = Batch::new(f.targets());
        batch.set_default(Some("a"));
        crate::settings::update(
            &f.paths,
            crate::settings::ConfigFile::Environment("hades"),
            &[("targets.b.default", "true")],
            &[],
        )
        .unwrap();
        assert!(matches!(
            write(&f.paths, "hades", &batch).unwrap(),
            Written::Changed(_)
        ));
    }

    #[test]
    fn rebasing_drops_what_no_longer_applies() {
        let base = parse("[targets.a]\nexe = 'a'\n[targets.b]\nexe = 'b'\n");
        let mut batch = Batch::new(base);
        batch.edit(
            "a",
            Spec {
                args: vec!["-x".to_owned()],
                ..spec("a")
            },
        );
        batch.add("c", spec("c")).unwrap();
        batch.set_default(Some("a"));
        // Meanwhile `a` was removed and a `c` was added.
        let current = parse("[targets.b]\nexe = 'b'\n[targets.c]\nexe = 'other'\n");
        let (rebased, dropped) = batch.rebase(current);
        assert_eq!(dropped.len(), 3, "{dropped:?}");
        assert!(rebased.is_empty());
    }

    #[test]
    fn executables_already_in_use_are_found_whatever_the_case() {
        let batch = Batch::new(parse(
            "[targets.hades]\nexe = 'C:\\Games\\Hades\\Hades.exe'\n[targets.tool]\nexe = '/opt/Tool.exe'\n",
        ));
        assert_eq!(batch.using(r"c:\games\hades\HADES.EXE"), ["hades"]);
        assert_eq!(batch.using("C:/Games/Hades/Hades.exe"), ["hades"]);
        assert_eq!(batch.using("/opt/Tool.exe"), ["tool"]);
        assert!(batch.using("/opt/tool.exe").is_empty());
    }
}
