//! The global settings controls and their changes to the config file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use wineo_core::config::{Arch, ArtFetch, GlobalConfig};
use wineo_core::edit;

use super::form::{Edits, EnvRow, Switch, env_problem};
use super::load::RunnerChoice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    PrefixRoot,
    RunnerRoot,
    DxvkRoot,
    Vkd3dRoot,
    StopTimeout,
    LogLimitMb,
}

impl Field {
    pub(crate) const ROOTS: [Self; 4] = [
        Self::PrefixRoot,
        Self::RunnerRoot,
        Self::DxvkRoot,
        Self::Vkd3dRoot,
    ];
    pub(crate) const NUMBERS: [Self; 2] = [Self::StopTimeout, Self::LogLimitMb];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::PrefixRoot => "defaults.prefix_root",
            Self::RunnerRoot => "defaults.runner_root",
            Self::DxvkRoot => "defaults.dxvk_root",
            Self::Vkd3dRoot => "defaults.vkd3d_root",
            Self::StopTimeout => "stop_timeout",
            Self::LogLimitMb => "log_limit_mb",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::PrefixRoot => "Prefix root",
            Self::RunnerRoot => "Runner root",
            Self::DxvkRoot => "DXVK root",
            Self::Vkd3dRoot => "vkd3d root",
            Self::StopTimeout => "Stop timeout (seconds)",
            Self::LogLimitMb => "Log limit (MB)",
        }
    }
    pub(crate) fn number(self) -> bool {
        matches!(self, Self::StopTimeout | Self::LogLimitMb)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Values {
    pub(crate) gamemode: Option<bool>,
    pub(crate) mangohud: Option<bool>,
    pub(crate) wayland: Option<bool>,
    pub(crate) runner: String,
    pub(crate) arch: Arch,
    pub(crate) prefix_root: String,
    pub(crate) runner_root: String,
    pub(crate) dxvk_root: String,
    pub(crate) vkd3d_root: String,
    pub(crate) stop_timeout: String,
    pub(crate) log_limit_mb: String,
    pub(crate) art_fetch: ArtFetch,
    pub(crate) env: Vec<EnvRow>,
}

impl Values {
    pub(crate) fn from_config(config: &GlobalConfig, text: Option<&str>) -> Self {
        let shown = |key: &str, value: u32| {
            text.filter(|text| edit::global_has_key(text, key))
                .map(|_| value.to_string())
                .unwrap_or_default()
        };
        Self {
            gamemode: config.gamemode,
            mangohud: config.mangohud,
            wayland: config.wayland,
            runner: config.defaults.runner.clone(),
            arch: config.defaults.arch,
            prefix_root: config.defaults.prefix_root.clone().unwrap_or_default(),
            runner_root: config.defaults.runner_root.clone().unwrap_or_default(),
            dxvk_root: config.defaults.dxvk_root.clone().unwrap_or_default(),
            vkd3d_root: config.defaults.vkd3d_root.clone().unwrap_or_default(),
            stop_timeout: shown("stop_timeout", config.stop_timeout),
            log_limit_mb: shown("log_limit_mb", config.log_limit_mb),
            art_fetch: config.art.fetch,
            env: config
                .env
                .iter()
                .map(|(key, value)| EnvRow {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
        }
    }
    pub(crate) fn switch(&self, key: Switch) -> Option<bool> {
        match key {
            Switch::Gamemode => self.gamemode,
            Switch::Mangohud => self.mangohud,
            Switch::Wayland => self.wayland,
        }
    }
    fn switch_mut(&mut self, key: Switch) -> &mut Option<bool> {
        match key {
            Switch::Gamemode => &mut self.gamemode,
            Switch::Mangohud => &mut self.mangohud,
            Switch::Wayland => &mut self.wayland,
        }
    }
    pub(crate) fn field(&self, field: Field) -> &str {
        match field {
            Field::PrefixRoot => &self.prefix_root,
            Field::RunnerRoot => &self.runner_root,
            Field::DxvkRoot => &self.dxvk_root,
            Field::Vkd3dRoot => &self.vkd3d_root,
            Field::StopTimeout => &self.stop_timeout,
            Field::LogLimitMb => &self.log_limit_mb,
        }
    }
    fn field_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::PrefixRoot => &mut self.prefix_root,
            Field::RunnerRoot => &mut self.runner_root,
            Field::DxvkRoot => &mut self.dxvk_root,
            Field::Vkd3dRoot => &mut self.vkd3d_root,
            Field::StopTimeout => &mut self.stop_timeout,
            Field::LogLimitMb => &mut self.log_limit_mb,
        }
    }
    pub(crate) fn set_field(&mut self, field: Field, value: String) -> bool {
        if field.number()
            && (!value.chars().all(|character| character.is_ascii_digit())
                || (!value.is_empty() && value.parse::<u32>().is_err()))
        {
            return false;
        }
        *self.field_mut(field) = value;
        true
    }
    pub(crate) fn env_problems(&self) -> Vec<Option<String>> {
        self.env
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let above: Vec<&str> = self.env[..index]
                    .iter()
                    .map(|row| row.key.as_str())
                    .collect();
                env_problem(&row.key, &row.value, true, &above)
            })
            .collect()
    }
    pub(crate) fn new_env_problem(&self, key: &str, value: &str) -> Option<String> {
        let keys: Vec<&str> = self.env.iter().map(|row| row.key.as_str()).collect();
        env_problem(key, value, true, &keys)
    }
    fn env_map(&self) -> BTreeMap<&str, &str> {
        self.env
            .iter()
            .map(|row| (row.key.as_str(), row.value.as_str()))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Form {
    pub(crate) loaded: Values,
    pub(crate) draft: Values,
}

impl Form {
    pub(crate) fn new(values: Values) -> Self {
        Self {
            loaded: values.clone(),
            draft: values,
        }
    }
    pub(crate) fn revert(&mut self) {
        self.draft = self.loaded.clone();
    }
    pub(crate) fn rebase(&mut self, values: Values) {
        self.loaded = values;
    }
    pub(crate) fn set_switch(&mut self, key: Switch, on: bool) {
        *self.draft.switch_mut(key) = if on {
            Some(true)
        } else if self.loaded.switch(key) == Some(false) {
            Some(false)
        } else {
            None
        };
    }
    pub(crate) fn changes(&self) -> usize {
        let edits = self.edits();
        edits.pairs.len() + edits.unset.len()
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.changes() != 0
    }
    pub(crate) fn problem(&self) -> Option<String> {
        if let Some(problem) = self.draft.env_problems().into_iter().flatten().next() {
            return Some(problem);
        }
        for field in Field::ROOTS {
            let value = self.draft.field(field);
            if value != self.loaded.field(field)
                && !value.is_empty()
                && !Path::new(value).is_absolute()
                && !value.starts_with("~/")
            {
                return Some("The prefix must be absolute or start with ~/.".to_owned());
            }
        }
        None
    }
    pub(crate) fn edits(&self) -> Edits {
        let (before, after) = (&self.loaded, &self.draft);
        let mut edits = Edits::default();
        for key in Switch::ALL {
            if before.switch(key) != after.switch(key) {
                if after.switch(key) == Some(true) {
                    edits.pairs.push((key.key().to_owned(), "true".to_owned()));
                } else {
                    edits.unset.push(key.key().to_owned());
                }
            }
        }
        if before.runner != after.runner {
            edits
                .pairs
                .push(("defaults.runner".to_owned(), after.runner.clone()));
        }
        if before.arch != after.arch {
            edits
                .pairs
                .push(("defaults.arch".to_owned(), after.arch.as_str().to_owned()));
        }
        for field in Field::ROOTS.into_iter().chain(Field::NUMBERS) {
            let (old, new) = (before.field(field), after.field(field));
            let changed = if field.number() {
                old.parse::<u32>().ok() != new.parse::<u32>().ok()
            } else {
                old != new
            };
            if changed {
                if new.is_empty() {
                    edits.unset.push(field.key().to_owned());
                } else {
                    let value = if field.number() {
                        new.parse::<u32>().expect("validated number").to_string()
                    } else {
                        new.to_owned()
                    };
                    edits.pairs.push((field.key().to_owned(), value));
                }
            }
        }
        if before.art_fetch != after.art_fetch {
            let value = match after.art_fetch {
                ArtFetch::All => "all",
                ArtFetch::Icon => "icon",
            };
            edits.pairs.push(("art.fetch".to_owned(), value.to_owned()));
        }
        let (old, new) = (before.env_map(), after.env_map());
        let keys: BTreeSet<&&str> = old.keys().chain(new.keys()).collect();
        for key in keys {
            match (old.get(key), new.get(key)) {
                (Some(_), None) => edits.unset.push(format!("env.{key}")),
                (old, Some(value)) if old != Some(value) => {
                    edits
                        .pairs
                        .push((format!("env.{key}"), (*value).to_owned()));
                }
                _ => {}
            }
        }
        edits
    }
}

pub(crate) struct Editor {
    pub(crate) form: Form,
    pub(crate) runners: Vec<RunnerChoice>,
    pub(crate) new_env: EnvRow,
}

impl Editor {
    pub(crate) fn new(values: Values, runners: Vec<RunnerChoice>) -> Self {
        Self {
            form: Form::new(values),
            runners,
            new_env: EnvRow::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtChoice(pub(crate) ArtFetch);

impl fmt::Display for ArtChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            ArtFetch::All => "All kinds",
            ArtFetch::Icon => "Icon only",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_form(text: &str) -> Form {
        let config = GlobalConfig::parse(Path::new("global.toml"), text).unwrap();
        Form::new(Values::from_config(&config, Some(text)))
    }

    #[test]
    fn launch_switches_write_true_and_off_unsets_only_a_true_key() {
        let mut form = parsed_form("gamemode = true\nmangohud = false\n");
        form.set_switch(Switch::Gamemode, false);
        form.set_switch(Switch::Mangohud, false);
        form.set_switch(Switch::Wayland, true);
        let edits = form.edits();
        assert_eq!(edits.unset, ["gamemode"]);
        assert_eq!(edits.pairs, [("wayland".to_owned(), "true".to_owned())]);
        form.set_switch(Switch::Wayland, false);
        assert_eq!(form.edits().unset, ["gamemode"]);
        let untouched = parsed_form("mangohud = false\n");
        assert_eq!(untouched.changes(), 0);
    }

    #[test]
    fn runner_arch_and_art_write_their_core_values() {
        let mut form = parsed_form("");
        form.draft.runner = "GE-Proton-Latest".to_owned();
        form.draft.arch = Arch::Win32;
        form.draft.art_fetch = ArtFetch::Icon;
        assert_eq!(
            form.edits().pairs,
            [
                ("defaults.runner".to_owned(), "GE-Proton-Latest".to_owned()),
                ("defaults.arch".to_owned(), "win32".to_owned()),
                ("art.fetch".to_owned(), "icon".to_owned()),
            ]
        );
        form.revert();
        assert!(!form.is_dirty());
    }

    #[test]
    fn every_root_writes_and_clears() {
        for field in Field::ROOTS {
            let mut form = parsed_form("");
            assert!(form.draft.set_field(field, "~/custom".to_owned()));
            assert_eq!(
                form.edits().pairs,
                [(field.key().to_owned(), "~/custom".to_owned())]
            );
            assert!(form.problem().is_none());
            form.rebase(form.draft.clone());
            assert!(form.draft.set_field(field, String::new()));
            assert_eq!(form.edits().unset, [field.key()]);
        }
    }

    #[test]
    fn roots_require_absolute_or_home_paths_when_edited() {
        for field in Field::ROOTS {
            let mut form = parsed_form("");
            form.draft.set_field(field, "relative/path".to_owned());
            assert_eq!(
                form.problem().as_deref(),
                Some("The prefix must be absolute or start with ~/.")
            );
            form.draft.set_field(field, "/absolute/path".to_owned());
            assert!(form.problem().is_none());
        }
        let form = parsed_form("[defaults]\nrunner_root = \"relative/path\"\n");
        assert!(form.problem().is_none());
    }

    #[test]
    fn numbers_accept_digits_compare_parsed_values_and_unset() {
        let mut form = parsed_form("stop_timeout = 5\nlog_limit_mb = 50\n");
        assert!(form.draft.set_field(Field::StopTimeout, "005".to_owned()));
        assert_eq!(form.changes(), 0);
        assert!(!form.draft.set_field(Field::StopTimeout, "+5".to_owned()));
        assert!(
            !form
                .draft
                .set_field(Field::StopTimeout, "4294967296".to_owned())
        );
        assert!(form.draft.set_field(Field::StopTimeout, "6".to_owned()));
        assert!(form.draft.set_field(Field::LogLimitMb, String::new()));
        assert_eq!(
            form.edits().pairs,
            [("stop_timeout".to_owned(), "6".to_owned())]
        );
        assert_eq!(form.edits().unset, ["log_limit_mb"]);
        let absent = parsed_form("");
        assert!(absent.draft.stop_timeout.is_empty());
        assert!(absent.draft.log_limit_mb.is_empty());
    }

    #[test]
    fn env_rows_change_rename_and_refuse_wineprefix() {
        let mut form = parsed_form("[env]\nOLD = \"one\"\n");
        form.draft.env[0].key = "NEW".to_owned();
        form.draft.env[0].value = "two".to_owned();
        assert_eq!(form.edits().unset, ["env.OLD"]);
        assert_eq!(
            form.edits().pairs,
            [("env.NEW".to_owned(), "two".to_owned())]
        );
        assert!(
            form.draft
                .new_env_problem("WINEPREFIX", "/tmp/pfx")
                .is_some()
        );
        form.draft.env[0].key = "WINEPREFIX".to_owned();
        assert!(form.problem().is_some());
    }
}
