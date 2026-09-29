//! What the settings page edits, and how an edit becomes `settings::apply`'s
//! `pairs` and `unset`. Kept free of iced so it can be tested on its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use wineo_core::components::{Component, LATEST};
use wineo_core::config::{self, EnvironmentConfig, Target};
use wineo_core::gamescope::{self, GamescopeSetting};
use wineo_core::{name, targets, winpath};

/// The on/off launch settings an environment can override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Switch {
    Gamemode,
    Mangohud,
    Wayland,
}

impl Switch {
    pub(crate) const ALL: [Switch; 3] = [Switch::Gamemode, Switch::Mangohud, Switch::Wayland];
    pub(crate) fn key(self) -> &'static str {
        match self {
            Switch::Gamemode => "gamemode",
            Switch::Mangohud => "mangohud",
            Switch::Wayland => "wayland",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Switch::Gamemode => "Gamemode",
            Switch::Mangohud => "MangoHud",
            Switch::Wayland => "Wayland",
        }
    }
}

/// Where a switch's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Global,
    /// A target following its environment.
    Environment,
    Here,
}

impl Source {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Source::Global => "global",
            Source::Environment => "environment",
            Source::Here => "set here",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Width,
    Height,
    OutputWidth,
    OutputHeight,
    Filter,
    Scaler,
    Window,
    FpsLimit,
    FpsLimitUnfocused,
    GrabCursor,
    Args,
}

impl Field {
    pub(crate) const ALL: [Field; 11] = [
        Field::Width,
        Field::Height,
        Field::OutputWidth,
        Field::OutputHeight,
        Field::Filter,
        Field::Scaler,
        Field::Window,
        Field::FpsLimit,
        Field::FpsLimitUnfocused,
        Field::GrabCursor,
        Field::Args,
    ];
    pub(crate) fn key(self) -> &'static str {
        match self {
            Field::Width => "width",
            Field::Height => "height",
            Field::OutputWidth => "output_width",
            Field::OutputHeight => "output_height",
            Field::Filter => "filter",
            Field::Scaler => "scaler",
            Field::Window => "window",
            Field::FpsLimit => "fps_limit",
            Field::FpsLimitUnfocused => "fps_limit_unfocused",
            Field::GrabCursor => "grab_cursor",
            Field::Args => "args",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Field::Width => "Width",
            Field::Height => "Height",
            Field::OutputWidth => "Output width",
            Field::OutputHeight => "Output height",
            Field::Filter => "Filter",
            Field::Scaler => "Scaler",
            Field::Window => "Window",
            Field::FpsLimit => "FPS limit",
            Field::FpsLimitUnfocused => "FPS limit unfocused",
            Field::GrabCursor => "Grab cursor",
            Field::Args => "Extra args",
        }
    }
    /// The values a choice field offers, not counting unset.
    pub(crate) fn choices(self) -> Vec<&'static str> {
        match self {
            Field::Filter => gamescope::Filter::ALL
                .map(gamescope::Filter::as_str)
                .to_vec(),
            Field::Scaler => gamescope::Scaler::ALL
                .map(gamescope::Scaler::as_str)
                .to_vec(),
            Field::Window => gamescope::Window::ALL
                .map(gamescope::Window::as_str)
                .to_vec(),
            _ => Vec::new(),
        }
    }
}

/// A choice field's value in a pick list, where `None` leaves it unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Choice(pub(crate) Option<&'static str>);

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.unwrap_or("Default"))
    }
}

/// Gamescope's fields as the page shows them. An empty string, `None`,
/// `false`, or no args means the key is left out of the table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GamescopeFields {
    pub(crate) width: String,
    pub(crate) height: String,
    pub(crate) output_width: String,
    pub(crate) output_height: String,
    pub(crate) filter: Option<&'static str>,
    pub(crate) scaler: Option<&'static str>,
    pub(crate) window: Option<&'static str>,
    pub(crate) fps_limit: String,
    pub(crate) fps_limit_unfocused: String,
    pub(crate) grab_cursor: bool,
    pub(crate) args: Vec<String>,
}

impl GamescopeFields {
    fn from_config(gamescope: &gamescope::Gamescope) -> Self {
        let number = |value: Option<u32>| value.map(|value| value.to_string()).unwrap_or_default();
        Self {
            width: number(gamescope.width),
            height: number(gamescope.height),
            output_width: number(gamescope.output_width),
            output_height: number(gamescope.output_height),
            filter: gamescope.filter.map(gamescope::Filter::as_str),
            scaler: gamescope.scaler.map(gamescope::Scaler::as_str),
            window: gamescope.window.map(gamescope::Window::as_str),
            fps_limit: number(gamescope.fps_limit),
            fps_limit_unfocused: number(gamescope.fps_limit_unfocused),
            grab_cursor: gamescope.grab_cursor,
            args: gamescope.args.clone(),
        }
    }
    pub(crate) fn text(&self, field: Field) -> &str {
        match field {
            Field::Width => &self.width,
            Field::Height => &self.height,
            Field::OutputWidth => &self.output_width,
            Field::OutputHeight => &self.output_height,
            Field::FpsLimit => &self.fps_limit,
            Field::FpsLimitUnfocused => &self.fps_limit_unfocused,
            Field::Filter | Field::Scaler | Field::Window | Field::GrabCursor | Field::Args => "",
        }
    }
    pub(crate) fn choice(&self, field: Field) -> Choice {
        Choice(match field {
            Field::Filter => self.filter,
            Field::Scaler => self.scaler,
            Field::Window => self.window,
            _ => None,
        })
    }
    /// Takes typed digits, refusing anything else so the form never carries
    /// a value TOML would reject. `u32` parsing alone would let `+5` in.
    pub(crate) fn set_text(&mut self, field: Field, value: String) -> bool {
        let digits = value.chars().all(|character| character.is_ascii_digit());
        if !digits || (!value.is_empty() && value.parse::<u32>().is_err()) {
            return false;
        }
        let slot = match field {
            Field::Width => &mut self.width,
            Field::Height => &mut self.height,
            Field::OutputWidth => &mut self.output_width,
            Field::OutputHeight => &mut self.output_height,
            Field::FpsLimit => &mut self.fps_limit,
            Field::FpsLimitUnfocused => &mut self.fps_limit_unfocused,
            Field::Filter | Field::Scaler | Field::Window | Field::GrabCursor | Field::Args => {
                return false;
            }
        };
        *slot = value;
        true
    }
    pub(crate) fn set_choice(&mut self, field: Field, choice: Choice) {
        match field {
            Field::Filter => self.filter = choice.0,
            Field::Scaler => self.scaler = choice.0,
            Field::Window => self.window = choice.0,
            _ => {}
        }
    }
    /// The raw value `edit::set` takes for a field, or `None` to leave it
    /// unset. Numbers go through `u32`, so `05` and `5` are the same value.
    fn raw(&self, field: Field) -> Option<String> {
        match field {
            Field::Filter | Field::Scaler | Field::Window => {
                self.choice(field).0.map(str::to_owned)
            }
            Field::GrabCursor => self.grab_cursor.then(|| "true".to_owned()),
            Field::Args => (!self.args.is_empty()).then(|| toml_array(&self.args)),
            _ => self
                .text(field)
                .parse::<u32>()
                .ok()
                .map(|number| number.to_string()),
        }
    }
}

/// A chip list the environment has, other than gamescope's args.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum List {
    Winetricks,
    Tags,
}

impl List {
    pub(crate) fn key(self) -> &'static str {
        match self {
            List::Winetricks => "winetricks",
            List::Tags => "tags",
        }
    }
    /// Why `input` can't join `items`, by the rules the saved list is checked
    /// with. Nothing typed yet is no problem, it just can't be added.
    pub(crate) fn problem(self, items: &[String], input: &str) -> Option<String> {
        if input.is_empty() {
            return None;
        }
        let mut all = items.to_vec();
        all.push(input.to_owned());
        match self {
            List::Winetricks => config::validate_winetricks(&all),
            List::Tags => config::validate_tags(&all),
        }
        .err()
    }
}

/// One `[env]` variable as a row on the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EnvRow {
    pub(crate) key: String,
    pub(crate) value: String,
}

/// Why a variable can't be saved as `key`. `others` are the names above it,
/// so only the later of two duplicates is marked.
pub(crate) fn env_problem(key: &str, value: &str, wine: bool, others: &[&str]) -> Option<String> {
    if key.is_empty() {
        return Some("Name can't be empty.".to_owned());
    }
    if let Err(error) =
        config::validate_env_key(key, wine).and_then(|()| config::validate_env_var(key, value))
    {
        return Some(error);
    }
    others
        .contains(&key)
        .then(|| format!("Duplicate variable {key:?}."))
}

/// A DXVK or vkd3d-proton build in a pick list. `None` leaves it unset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build(pub(crate) Option<String>);

impl fmt::Display for Build {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0.as_deref() {
            None => "None",
            Some(LATEST) => "Latest",
            Some(name) => name,
        })
    }
}

/// None, Latest, and the installed builds, plus the configured one when it
/// isn't installed, so the list can still show it.
pub(crate) fn build_choices(installed: &[String], current: Option<&str>) -> Vec<Build> {
    let mut choices = vec![Build(None), Build(Some(LATEST.to_owned()))];
    choices.extend(installed.iter().map(|name| Build(Some(name.clone()))));
    if let Some(current) = current
        && !choices
            .iter()
            .any(|choice| choice.0.as_deref() == Some(current))
    {
        choices.push(Build(Some(current.to_owned())));
    }
    choices
}

/// Why DXVK can't be dropped while vkd3d-proton is set.
pub(crate) const VKD3D_NEEDS_DXVK: &str =
    "vkd3d-proton needs DXVK. Set vkd3d-proton to None first.";

/// Why DXVK can't be removed while vkd3d-proton is set, where there's no
/// picker to set it with.
pub(crate) const REMOVE_VKD3D_FIRST: &str = "vkd3d-proton needs DXVK. Remove vkd3d-proton first.";

/// Every control's value, read from a config or edited on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Values {
    /// `None` on a native environment, which has no runner.
    pub(crate) runner: Option<String>,
    pub(crate) gamemode: Option<bool>,
    pub(crate) mangohud: Option<bool>,
    pub(crate) wayland: Option<bool>,
    /// Whether the environment has a gamescope table. A hand-written
    /// `gamescope = false` reads as off, the same as no key.
    pub(crate) gamescope: bool,
    /// Kept while gamescope is off, so switching it back on restores them.
    pub(crate) fields: GamescopeFields,
    /// Empty on a native environment. An empty `gameid` or `store` is unset.
    pub(crate) prefix: String,
    pub(crate) gameid: String,
    pub(crate) store: String,
    pub(crate) dxvk: Option<String>,
    pub(crate) vkd3d: Option<String>,
    pub(crate) winetricks: Vec<String>,
    pub(crate) tags: Vec<String>,
    /// In the order the rows show, which renames keep.
    pub(crate) env: Vec<EnvRow>,
    pub(crate) targets: BTreeMap<String, TargetValues>,
    /// The target marked `default`. Core allows only one.
    pub(crate) default_target: Option<String>,
}

impl Values {
    pub(crate) fn from_config(config: &EnvironmentConfig) -> Self {
        let (gamescope, fields) = match &config.gamescope {
            Some(GamescopeSetting::On(table)) => (true, GamescopeFields::from_config(table)),
            None | Some(GamescopeSetting::Off) => (false, GamescopeFields::default()),
        };
        let wine = config.wine();
        let text = |value: Option<&String>| value.cloned().unwrap_or_default();
        Self {
            runner: wine.map(|wine| wine.runner.clone()),
            gamemode: config.gamemode,
            mangohud: config.mangohud,
            wayland: config.wayland,
            gamescope,
            fields,
            prefix: text(wine.map(|wine| &wine.prefix)),
            gameid: text(wine.and_then(|wine| wine.gameid.as_ref())),
            store: text(wine.and_then(|wine| wine.store.as_ref())),
            dxvk: wine.and_then(|wine| wine.dxvk.clone()),
            vkd3d: wine.and_then(|wine| wine.vkd3d.clone()),
            winetricks: config.winetricks.clone(),
            tags: config.tags.clone(),
            env: config
                .env
                .iter()
                .map(|(key, value)| EnvRow {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            targets: config
                .targets
                .iter()
                .map(|(name, target)| (name.clone(), TargetValues::from_config(target)))
                .collect(),
            default_target: config
                .targets
                .iter()
                .find(|(_, target)| target.default)
                .map(|(name, _)| name.clone()),
        }
    }
    /// A native environment has no runner.
    pub(crate) fn wine(&self) -> bool {
        self.runner.is_some()
    }
    pub(crate) fn list(&self, list: List) -> &[String] {
        match list {
            List::Winetricks => &self.winetricks,
            List::Tags => &self.tags,
        }
    }
    pub(crate) fn list_mut(&mut self, list: List) -> &mut Vec<String> {
        match list {
            List::Winetricks => &mut self.winetricks,
            List::Tags => &mut self.tags,
        }
    }
    pub(crate) fn build(&self, component: Component) -> Option<&str> {
        match component {
            Component::Dxvk => self.dxvk.as_deref(),
            Component::Vkd3d => self.vkd3d.as_deref(),
        }
    }
    /// The variables as they'd be saved. A later duplicate wins, but a draft
    /// with one can't be saved anyway.
    fn env_map(&self) -> BTreeMap<&str, &str> {
        self.env
            .iter()
            .map(|row| (row.key.as_str(), row.value.as_str()))
            .collect()
    }
    /// Each row's problem, in row order.
    pub(crate) fn env_problems(&self) -> Vec<Option<String>> {
        let wine = self.wine();
        self.env
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let above: Vec<&str> = self.env[..index]
                    .iter()
                    .map(|row| row.key.as_str())
                    .collect();
                env_problem(&row.key, &row.value, wine, &above)
            })
            .collect()
    }
    /// Why a new row can't be added with this name and value.
    pub(crate) fn new_env_problem(&self, key: &str, value: &str) -> Option<String> {
        let keys: Vec<&str> = self.env.iter().map(|row| row.key.as_str()).collect();
        env_problem(key, value, self.wine(), &keys)
    }
    pub(crate) fn switch(&self, switch: Switch) -> Option<bool> {
        match switch {
            Switch::Gamemode => self.gamemode,
            Switch::Mangohud => self.mangohud,
            Switch::Wayland => self.wayland,
        }
    }
    pub(crate) fn switch_mut(&mut self, switch: Switch) -> &mut Option<bool> {
        match switch {
            Switch::Gamemode => &mut self.gamemode,
            Switch::Mangohud => &mut self.mangohud,
            Switch::Wayland => &mut self.wayland,
        }
    }
}

/// The global values environments fall back to. Unset globally means off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Inherited {
    pub(crate) gamemode: Option<bool>,
    pub(crate) mangohud: Option<bool>,
    pub(crate) wayland: Option<bool>,
}

impl Inherited {
    pub(crate) fn get(&self, switch: Switch) -> bool {
        match switch {
            Switch::Gamemode => self.gamemode,
            Switch::Mangohud => self.mangohud,
            Switch::Wayland => self.wayland,
        }
        .unwrap_or(false)
    }
}

/// The value in effect and where it comes from. A key that is present counts
/// as set here even when it matches what it would inherit, so a hand-written
/// value keeps its reset button.
pub(crate) fn switch_state(value: Option<bool>, inherited: bool) -> (bool, Source) {
    match value {
        Some(value) => (value, Source::Here),
        None => (inherited, Source::Global),
    }
}

/// How a target sets gamescope. Its table replaces the environment's whole
/// table, and `false` turns gamescope off for just this target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TargetGamescope {
    #[default]
    Inherit,
    Off,
    On,
}

/// One target's controls. An empty `cwd` or `name` is unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TargetValues {
    pub(crate) exe: String,
    pub(crate) args: Vec<String>,
    /// Empty runs it from the exe's own directory.
    pub(crate) cwd: String,
    /// The desktop entry's name. Empty uses the target's own name.
    pub(crate) name: String,
    pub(crate) desktop: bool,
    pub(crate) gamemode: Option<bool>,
    pub(crate) mangohud: Option<bool>,
    pub(crate) wayland: Option<bool>,
    pub(crate) gamescope: TargetGamescope,
    /// Kept while gamescope isn't on here, like the environment's.
    pub(crate) fields: GamescopeFields,
}

impl TargetValues {
    fn from_config(target: &Target) -> Self {
        let (gamescope, fields) = match &target.gamescope {
            None => (TargetGamescope::Inherit, GamescopeFields::default()),
            Some(GamescopeSetting::Off) => (TargetGamescope::Off, GamescopeFields::default()),
            Some(GamescopeSetting::On(table)) => {
                (TargetGamescope::On, GamescopeFields::from_config(table))
            }
        };
        Self {
            exe: target.exe.clone(),
            args: target.args.clone(),
            cwd: target.cwd.clone().unwrap_or_default(),
            name: target.name.clone().unwrap_or_default(),
            desktop: target.desktop,
            gamemode: target.gamemode,
            mangohud: target.mangohud,
            wayland: target.wayland,
            gamescope,
            fields,
        }
    }
    pub(crate) fn switch(&self, switch: Switch) -> Option<bool> {
        match switch {
            Switch::Gamemode => self.gamemode,
            Switch::Mangohud => self.mangohud,
            Switch::Wayland => self.wayland,
        }
    }
    pub(crate) fn switch_mut(&mut self, switch: Switch) -> &mut Option<bool> {
        match switch {
            Switch::Gamemode => &mut self.gamemode,
            Switch::Mangohud => &mut self.mangohud,
            Switch::Wayland => &mut self.wayland,
        }
    }
    /// The fields a gamescope table starts from when this saves.
    fn gamescope_base(&self) -> GamescopeFields {
        if self.gamescope == TargetGamescope::On {
            self.fields.clone()
        } else {
            GamescopeFields::default()
        }
    }
    /// How many controls differ from `before`.
    fn changes(&self, before: &TargetValues) -> usize {
        let mut count = [
            before.exe != self.exe,
            before.args != self.args,
            before.cwd != self.cwd,
            before.name != self.name,
            before.desktop != self.desktop,
            before.gamescope != self.gamescope,
        ]
        .into_iter()
        .filter(|changed| *changed)
        .count();
        for switch in Switch::ALL {
            count += usize::from(before.switch(switch) != self.switch(switch));
        }
        if self.gamescope == TargetGamescope::On {
            let base = before.gamescope_base();
            count += Field::ALL
                .iter()
                .filter(|field| base.raw(**field) != self.fields.raw(**field))
                .count();
        }
        count
    }
    /// Writes what differs from `before` under `targets.<target>.`. The exe
    /// goes first, so a new target's table starts with it.
    fn edits(&self, target: &str, before: &TargetValues, edits: &mut Edits) {
        let key = |field: &str| format!("targets.{target}.{field}");
        if before.exe != self.exe {
            edits.pairs.push((key("exe"), self.exe.clone()));
        }
        if before.args != self.args {
            edits.write(
                key("args"),
                (!self.args.is_empty()).then(|| toml_array(&self.args)),
            );
        }
        for (field, old, new) in [
            ("cwd", &before.cwd, &self.cwd),
            ("name", &before.name, &self.name),
        ] {
            if old != new {
                edits.write(key(field), (!new.is_empty()).then(|| new.clone()));
            }
        }
        if before.desktop != self.desktop {
            edits.write(key("desktop"), self.desktop.then(|| "true".to_owned()));
        }
        for switch in Switch::ALL {
            let value = self.switch(switch);
            if before.switch(switch) != value {
                edits.write(key(switch.key()), value.map(|on| on.to_string()));
            }
        }
        if before.gamescope != self.gamescope {
            match self.gamescope {
                TargetGamescope::Inherit => edits.unset.push(key("gamescope")),
                TargetGamescope::Off => edits.pairs.push((key("gamescope"), "false".to_owned())),
                TargetGamescope::On => edits.pairs.push((key("gamescope"), "{}".to_owned())),
            }
        }
        if self.gamescope == TargetGamescope::On {
            let base = before.gamescope_base();
            for field in Field::ALL {
                let value = self.fields.raw(field);
                if base.raw(field) != value {
                    edits.write(key(&format!("gamescope.{}", field.key())), value);
                }
            }
        }
    }
}

/// Whether a typed exe or working dir would be resolved from wherever the
/// GUI was started, the same reason a relative prefix is refused. An exe
/// that's a bare name is looked up on PATH instead.
fn relative_path(value: &str, exe: bool) -> bool {
    !value.is_empty()
        && (value.contains('/') || !exe)
        && !winpath::is_windows_path(value)
        && !std::path::Path::new(value).is_absolute()
        && !value.starts_with("~/")
}

/// Why several targets can't be saved without a default: `wineo start`
/// with no `--target` refuses to guess, so `setup` asks for one.
pub(crate) const NEEDS_DEFAULT: &str = "Several targets need a default. Mark one as the default.";

/// What a save writes. A reset or emptied control unsets its key.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Edits {
    pub(crate) pairs: Vec<(String, String)>,
    pub(crate) unset: Vec<String>,
}

impl Edits {
    pub(crate) fn is_empty(&self) -> bool {
        self.pairs.is_empty() && self.unset.is_empty()
    }
    fn write(&mut self, key: String, value: Option<String>) {
        match value {
            Some(value) => self.pairs.push((key, value)),
            None => self.unset.push(key),
        }
    }
}

/// The page's copy of what's on disk next to what's been edited.
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
    /// A fresh read of the file, keeping gamescope fields typed while it was
    /// off so they survive a save or reload until the page is left.
    pub(crate) fn reloaded(values: Values, previous: Option<&Form>) -> Self {
        let mut form = Self::new(values);
        if let Some(previous) = previous {
            if !form.draft.gamescope {
                form.draft.fields = previous.draft.fields.clone();
                form.loaded.fields = previous.draft.fields.clone();
            }
            keep_target_fields(&mut form.draft, &previous.draft);
            keep_target_fields(&mut form.loaded, &previous.draft);
        }
        form
    }
    /// Takes a fresh read as the base but keeps the edits on top of it.
    pub(crate) fn rebase(&mut self, values: Values) {
        let fields = (!values.gamescope).then(|| self.draft.fields.clone());
        self.loaded = values;
        if let Some(fields) = fields {
            self.loaded.fields = fields;
        }
        keep_target_fields(&mut self.loaded, &self.draft);
    }
    pub(crate) fn revert(&mut self) {
        let previous = self.draft.clone();
        self.draft = self.loaded.clone();
        if !self.loaded.gamescope {
            self.draft.fields = previous.fields.clone();
        }
        keep_target_fields(&mut self.draft, &previous);
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.changes() > 0
    }
    /// How many controls differ from the file. Gamescope's fields only count
    /// while it's on, since they aren't written while it's off.
    pub(crate) fn changes(&self) -> usize {
        let (loaded, draft) = (&self.loaded, &self.draft);
        let mut count = usize::from(loaded.runner != draft.runner);
        for switch in [Switch::Gamemode, Switch::Mangohud, Switch::Wayland] {
            count += usize::from(loaded.switch(switch) != draft.switch(switch));
        }
        count += usize::from(loaded.gamescope != draft.gamescope);
        if draft.gamescope {
            let base = self.gamescope_base();
            count += Field::ALL
                .iter()
                .filter(|field| base.raw(**field) != draft.fields.raw(**field))
                .count();
        }
        count += [
            loaded.prefix != draft.prefix,
            loaded.gameid != draft.gameid,
            loaded.store != draft.store,
            loaded.dxvk != draft.dxvk,
            loaded.vkd3d != draft.vkd3d,
            loaded.winetricks != draft.winetricks,
            loaded.tags != draft.tags,
        ]
        .into_iter()
        .filter(|changed| *changed)
        .count();
        let (before, after) = (loaded.env_map(), draft.env_map());
        count += before
            .keys()
            .chain(after.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|key| before.get(*key) != after.get(*key))
            .count();
        count + self.target_changes()
    }
    /// How many target controls differ from the file. An added or removed
    /// target is one change.
    fn target_changes(&self) -> usize {
        let (loaded, draft) = (&self.loaded, &self.draft);
        let names: BTreeSet<&String> = loaded.targets.keys().chain(draft.targets.keys()).collect();
        let mut count = usize::from(self.default_changed());
        for name in names {
            count += match (loaded.targets.get(name), draft.targets.get(name)) {
                (Some(before), Some(after)) => after.changes(before),
                _ => 1,
            };
        }
        count
    }
    /// Whether the default moved. Removing the default target takes its mark
    /// with it, which isn't a second change.
    fn default_changed(&self) -> bool {
        let (loaded, draft) = (&self.loaded, &self.draft);
        loaded.default_target != draft.default_target
            && (draft.default_target.is_some()
                || loaded
                    .default_target
                    .as_ref()
                    .is_some_and(|old| draft.targets.contains_key(old)))
    }
    /// Why the draft can't be saved as it stands, so Save stays off rather
    /// than sending something core would refuse. `runner_known` is whether
    /// the runner resolves, since core only checks components when it does.
    pub(crate) fn problem(&self, runner_known: bool) -> Option<String> {
        let draft = &self.draft;
        if let Some(problem) = draft.env_problems().into_iter().flatten().next() {
            return Some(problem);
        }
        if let Some(problem) = self.prefix_problem() {
            return Some(problem.to_owned());
        }
        // Only reachable from a hand-written config, since picking DXVK's
        // None is refused while vkd3d-proton is set.
        if runner_known && draft.vkd3d.is_some() && draft.dxvk.is_none() {
            return Some(VKD3D_NEEDS_DXVK.to_owned());
        }
        None
    }
    pub(crate) fn protonfixes_problem(&self) -> Option<&'static str> {
        let (loaded, draft) = (&self.loaded, &self.draft);
        ((draft.gameid != loaded.gameid && !draft.gameid.is_empty())
            || (draft.store != loaded.store && !draft.store.is_empty()))
        .then_some(
            "Game id and store only apply to Proton runners. Pick a Proton runner or clear them.",
        )
    }
    /// Why an edited prefix can't be saved. A typed relative path would be
    /// resolved from wherever the GUI was started, so it's refused the way
    /// the example config asks. A hand-written one that isn't edited is left
    /// alone, so it can't block unrelated saves.
    pub(crate) fn prefix_problem(&self) -> Option<&'static str> {
        let prefix = &self.draft.prefix;
        if !self.draft.wine() || *prefix == self.loaded.prefix {
            return None;
        }
        if prefix.is_empty() {
            return Some("The prefix can't be empty.");
        }
        if !std::path::Path::new(prefix).is_absolute() && !prefix.starts_with("~/") {
            return Some("The prefix must be absolute or start with ~/.");
        }
        None
    }
    /// Why the targets can't be saved as they stand. Only edited paths are
    /// checked, so a hand-written one can't block unrelated saves, and a
    /// portable folder resolves relative ones from its root.
    pub(crate) fn targets_problem(&self, portable: bool) -> Option<String> {
        for (name, target) in &self.draft.targets {
            let before = self.loaded.targets.get(name);
            if target.exe.is_empty() {
                return Some(format!("Target {name} needs an exe."));
            }
            if portable {
                continue;
            }
            for (label, value, old) in [
                ("exe", &target.exe, before.map(|before| &before.exe)),
                ("working dir", &target.cwd, before.map(|before| &before.cwd)),
            ] {
                if Some(value) != old && relative_path(value, label == "exe") {
                    return Some(format!(
                        "Target {name}'s {label} must be absolute or start with ~/."
                    ));
                }
            }
        }
        // A config that already has no default isn't blocked for other edits.
        if self.target_changes() > 0
            && self.draft.targets.len() > 1
            && self.draft.default_target.is_none()
        {
            return Some(NEEDS_DEFAULT.to_owned());
        }
        None
    }
    /// A target's on/off setting in effect and where it comes from: its own
    /// key, the environment's draft value, or the global one.
    pub(crate) fn target_switch(
        &self,
        target: &str,
        switch: Switch,
        inherited: &Inherited,
    ) -> (bool, Source) {
        let own = self
            .draft
            .targets
            .get(target)
            .and_then(|values| values.switch(switch));
        match (own, self.draft.switch(switch)) {
            (Some(on), _) => (on, Source::Here),
            (None, Some(on)) => (on, Source::Environment),
            (None, None) => (inherited.get(switch), Source::Global),
        }
    }
    /// Whether gamescope runs for a target and where that comes from. There's
    /// no global gamescope, so an inheriting target follows the environment.
    pub(crate) fn target_gamescope(&self, target: &str) -> (bool, Source) {
        match self
            .draft
            .targets
            .get(target)
            .map(|values| values.gamescope)
        {
            Some(TargetGamescope::On) => (true, Source::Here),
            Some(TargetGamescope::Off) => (false, Source::Here),
            _ => (self.draft.gamescope, Source::Environment),
        }
    }
    /// The gamescope fields a target runs with, its own or the environment's.
    pub(crate) fn target_fields(&self, target: &str) -> Option<&GamescopeFields> {
        let values = self.draft.targets.get(target)?;
        match values.gamescope {
            TargetGamescope::On => Some(&values.fields),
            TargetGamescope::Inherit if self.draft.gamescope => Some(&self.draft.fields),
            _ => None,
        }
    }
    /// Turns gamescope on or off for a target. Turning it on with no fields
    /// of its own starts from the environment's, since the target's table
    /// replaces the environment's rather than adding to it.
    pub(crate) fn set_target_gamescope(&mut self, target: &str, on: bool) -> bool {
        let env_fields = self.draft.gamescope.then(|| self.draft.fields.clone());
        let Some(values) = self.draft.targets.get_mut(target) else {
            return false;
        };
        if !on {
            values.gamescope = TargetGamescope::Off;
            return true;
        }
        if values.gamescope != TargetGamescope::On
            && values.fields == GamescopeFields::default()
            && let Some(fields) = env_fields
        {
            values.fields = fields;
        }
        values.gamescope = TargetGamescope::On;
        true
    }
    /// Gives an inheriting target its own copy of the environment's fields.
    pub(crate) fn override_target_gamescope(&mut self, target: &str) -> bool {
        self.edit_target_fields(target, |_| true)
    }
    /// Edits a target's gamescope fields. One that inherits from an
    /// environment with gamescope on gets the environment's fields first, so
    /// the rest of them aren't lost when its own table replaces them.
    /// Nothing changes when `edit` refuses the value.
    pub(crate) fn edit_target_fields(
        &mut self,
        target: &str,
        edit: impl FnOnce(&mut GamescopeFields) -> bool,
    ) -> bool {
        let env_on = self.draft.gamescope;
        let env_fields = &self.draft.fields;
        let Some(values) = self.draft.targets.get(target) else {
            return false;
        };
        let mut fields = match values.gamescope {
            TargetGamescope::On => values.fields.clone(),
            TargetGamescope::Inherit if env_on => env_fields.clone(),
            _ => return false,
        };
        if !edit(&mut fields) {
            return false;
        }
        let values = self.draft.targets.get_mut(target).expect("checked above");
        values.fields = fields;
        values.gamescope = TargetGamescope::On;
        true
    }
    /// Marks or unmarks a target as the default. Marking one clears the
    /// others, the same as core does when it's saved.
    pub(crate) fn set_default(&mut self, target: &str, on: bool) {
        if !self.draft.targets.contains_key(target) {
            return;
        }
        if on {
            self.draft.default_target = Some(target.to_owned());
        } else if self.draft.default_target.as_deref() == Some(target) {
            self.draft.default_target = None;
        }
    }
    /// Why `name` can't be a new target, checked as it's typed. Nothing typed
    /// yet is no problem, it just can't be added.
    pub(crate) fn new_target_problem(&self, name: &str) -> Option<String> {
        if name.is_empty() {
            return None;
        }
        if let Some(reason) = name::problem(name) {
            return Some(format!("Invalid input: {reason}."));
        }
        self.draft
            .targets
            .contains_key(name)
            .then(|| format!("There is already a target called `{name}`."))
    }
    pub(crate) fn using_exe(&self, exe: &str) -> Option<&str> {
        self.draft
            .targets
            .iter()
            .find(|(_, target)| targets::same_exe(&target.exe, exe))
            .map(|(name, _)| name.as_str())
    }
    /// A free target name for a program, the way `setup` names one.
    pub(crate) fn suggest_target(&self, label: &str, exe: &str) -> String {
        let suggested = targets::suggest_name(label, exe);
        targets::free_name(&suggested, |taken| self.draft.targets.contains_key(taken))
    }
    pub(crate) fn add_target(&mut self, name: &str, values: TargetValues) -> Result<(), String> {
        if name.is_empty() {
            return Err("Enter a name.".to_owned());
        }
        if let Some(problem) = self.new_target_problem(name) {
            return Err(problem);
        }
        self.draft.targets.insert(name.to_owned(), values);
        Ok(())
    }
    pub(crate) fn remove_target(&mut self, name: &str) -> bool {
        if self.draft.targets.remove(name).is_none() {
            return false;
        }
        if self.draft.default_target.as_deref() == Some(name) {
            self.draft.default_target = None;
        }
        true
    }
    /// Picks a build, refusing to drop DXVK from under vkd3d-proton.
    pub(crate) fn pick_build(
        &mut self,
        component: Component,
        build: Option<String>,
    ) -> Result<(), &'static str> {
        let draft = &mut self.draft;
        match component {
            Component::Dxvk if build.is_none() && draft.vkd3d.is_some() => {
                return Err(VKD3D_NEEDS_DXVK);
            }
            Component::Dxvk => draft.dxvk = build,
            Component::Vkd3d => draft.vkd3d = build,
        }
        Ok(())
    }
    /// Unsets a build, which is all a Proton environment can do with one.
    pub(crate) fn remove_build(&mut self, component: Component) -> Result<(), &'static str> {
        match component {
            Component::Dxvk if self.draft.vkd3d.is_some() => Err(REMOVE_VKD3D_FIRST),
            Component::Dxvk => {
                self.draft.dxvk = None;
                Ok(())
            }
            Component::Vkd3d => {
                self.draft.vkd3d = None;
                Ok(())
            }
        }
    }
    /// vkd3d-proton is off until DXVK is set, unless it holds a value that
    /// has to stay clearable.
    pub(crate) fn vkd3d_enabled(&self) -> bool {
        self.draft.dxvk.is_some() || self.draft.vkd3d.is_some()
    }
    /// The fields a gamescope table starts from when this saves: the file's
    /// own table, or an empty one when it had none.
    fn gamescope_base(&self) -> GamescopeFields {
        if self.loaded.gamescope {
            self.loaded.fields.clone()
        } else {
            GamescopeFields::default()
        }
    }
    pub(crate) fn edits(&self) -> Edits {
        let (loaded, draft) = (&self.loaded, &self.draft);
        let mut edits = Edits::default();
        if loaded.runner != draft.runner
            && let Some(runner) = &draft.runner
        {
            edits.pairs.push(("runner".to_owned(), runner.clone()));
        }
        for switch in [Switch::Gamemode, Switch::Mangohud, Switch::Wayland] {
            let value = draft.switch(switch);
            if loaded.switch(switch) != value {
                edits.write(switch.key().to_owned(), value.map(|on| on.to_string()));
            }
        }
        match (loaded.gamescope, draft.gamescope) {
            // Off removes the key, so a hand-written `gamescope = false` goes
            // too; the environment level never writes `false`.
            (true, false) => edits.unset.push("gamescope".to_owned()),
            (false, true) => {
                edits.pairs.push(("gamescope".to_owned(), "{}".to_owned()));
            }
            _ => {}
        }
        if draft.gamescope {
            let base = self.gamescope_base();
            for field in Field::ALL {
                let value = draft.fields.raw(field);
                if base.raw(field) != value {
                    edits.write(format!("gamescope.{}", field.key()), value);
                }
            }
        }
        // Stored through core the way `wineo new` stores it.
        if draft.wine() && loaded.prefix != draft.prefix {
            edits
                .pairs
                .push(("prefix".to_owned(), draft.prefix.clone()));
        }
        for (key, before, after) in [
            ("gameid", &loaded.gameid, &draft.gameid),
            ("store", &loaded.store, &draft.store),
        ] {
            if before != after {
                edits.write(key.to_owned(), (!after.is_empty()).then(|| after.clone()));
            }
        }
        for component in Component::ALL {
            let value = draft.build(component);
            if loaded.build(component) != value {
                edits.write(component.as_str().to_owned(), value.map(str::to_owned));
            }
        }
        for list in [List::Winetricks, List::Tags] {
            let items = draft.list(list);
            if loaded.list(list) != items {
                edits.write(
                    list.key().to_owned(),
                    (!items.is_empty()).then(|| toml_array(items)),
                );
            }
        }
        // Values are taken as written, so nothing here needs quoting. A
        // rename is the old name unset and the new one set.
        let (before, after) = (loaded.env_map(), draft.env_map());
        for key in before.keys().filter(|key| !after.contains_key(*key)) {
            edits.unset.push(format!("env.{key}"));
        }
        for (key, value) in &after {
            if before.get(key) != Some(value) {
                edits
                    .pairs
                    .push((format!("env.{key}"), (*value).to_owned()));
            }
        }
        self.target_edits(&mut edits);
        edits
    }
    /// Removed targets go first, so their art is cleaned up by core, then
    /// each target's changes, then the default, which core moves with one
    /// `default = true`.
    fn target_edits(&self, edits: &mut Edits) {
        let (loaded, draft) = (&self.loaded, &self.draft);
        for name in loaded.targets.keys() {
            if !draft.targets.contains_key(name) {
                edits.unset.push(format!("targets.{name}"));
            }
        }
        let empty = TargetValues::default();
        for (name, values) in &draft.targets {
            values.edits(name, loaded.targets.get(name).unwrap_or(&empty), edits);
        }
        if self.default_changed() {
            match (&draft.default_target, &loaded.default_target) {
                (Some(target), _) => edits
                    .pairs
                    .push((format!("targets.{target}.default"), "true".to_owned())),
                (None, Some(old)) => edits.unset.push(format!("targets.{old}.default")),
                (None, None) => {}
            }
        }
    }
}

/// Keeps gamescope fields typed for targets that don't have it on, so
/// switching it back on restores them until the page is left.
fn keep_target_fields(values: &mut Values, from: &Values) {
    for (name, target) in &mut values.targets {
        if target.gamescope != TargetGamescope::On
            && let Some(previous) = from.targets.get(name)
        {
            target.fields = previous.fields.clone();
        }
    }
}

/// A TOML array of basic strings, the form `edit::set` parses list values from.
pub(crate) fn toml_array(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| toml_string(item)).collect();
    format!("[{}]", quoted.join(", "))
}

fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            character if character.is_control() => {
                out.push_str(&format!("\\u{:04X}", u32::from(character)));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn values(text: &str) -> Values {
        let config = EnvironmentConfig::parse(Path::new("/x/game.toml"), text).unwrap();
        Values::from_config(&config)
    }

    const WINE: &str = "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n";

    fn pairs(edits: &Edits) -> Vec<(&str, &str)> {
        edits
            .pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    #[test]
    fn a_hand_written_value_equal_to_global_is_still_set_here() {
        let form = Form::new(values(&format!("{WINE}gamemode = false\n")));
        let inherited = Inherited::default();
        assert_eq!(
            switch_state(form.draft.gamemode, inherited.get(Switch::Gamemode)),
            (false, Source::Here)
        );
        assert_eq!(
            switch_state(form.draft.mangohud, inherited.get(Switch::Mangohud)),
            (false, Source::Global)
        );
        let on = Inherited {
            mangohud: Some(true),
            ..Inherited::default()
        };
        assert_eq!(
            switch_state(form.draft.mangohud, on.get(Switch::Mangohud)),
            (true, Source::Global)
        );
    }

    #[test]
    fn flips_write_and_resets_unset() {
        let mut form = Form::new(values(&format!("{WINE}gamemode = true\n")));
        form.draft.gamemode = None;
        form.draft.mangohud = Some(true);
        let edits = form.edits();
        assert_eq!(pairs(&edits), [("mangohud", "true")]);
        assert_eq!(edits.unset, ["gamemode"]);
        assert_eq!(form.changes(), 2);
    }

    #[test]
    fn flipping_back_to_the_file_value_is_no_change() {
        let mut form = Form::new(values(&format!("{WINE}gamemode = true\n")));
        form.draft.gamemode = Some(false);
        assert!(form.is_dirty());
        form.draft.gamemode = Some(true);
        assert!(!form.is_dirty());
        assert!(form.edits().is_empty());
    }

    #[test]
    fn revert_restores_the_file_values() {
        let mut form = Form::new(values(WINE));
        form.draft.runner = Some("other".into());
        form.draft.wayland = Some(true);
        assert_eq!(form.changes(), 2);
        form.revert();
        assert!(!form.is_dirty());
        assert_eq!(form.draft, form.loaded);
    }

    #[test]
    fn runner_changes_write_the_name() {
        let mut form = Form::new(values(WINE));
        form.draft.runner = Some("GE-Proton-Latest".into());
        assert_eq!(pairs(&form.edits()), [("runner", "GE-Proton-Latest")]);
    }

    #[test]
    fn switching_gamescope_on_writes_a_table_and_skips_empty_fields() {
        let mut form = Form::new(values(WINE));
        form.draft.gamescope = true;
        assert_eq!(pairs(&form.edits()), [("gamescope", "{}")]);
        assert_eq!(form.changes(), 1);
        form.draft.fields.width = "1920".into();
        form.draft.fields.filter = Some("fsr");
        form.draft.fields.args = vec!["--backend".into(), "sdl".into(), "a b".into()];
        let edits = form.edits();
        assert_eq!(
            pairs(&edits),
            [
                ("gamescope", "{}"),
                ("gamescope.width", "1920"),
                ("gamescope.filter", "fsr"),
                ("gamescope.args", "[\"--backend\", \"sdl\", \"a b\"]"),
            ]
        );
        assert!(edits.unset.is_empty());
        assert_eq!(form.changes(), 4);
    }

    #[test]
    fn emptying_a_gamescope_field_unsets_it() {
        let mut form = Form::new(values(&format!(
            "{WINE}[gamescope]\nwidth = 1920\ngrab_cursor = true\nargs = [\"-e\"]\n"
        )));
        assert_eq!(form.draft.fields.width, "1920");
        form.draft.fields.width.clear();
        form.draft.fields.grab_cursor = false;
        form.draft.fields.args.clear();
        let edits = form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(
            edits.unset,
            ["gamescope.width", "gamescope.grab_cursor", "gamescope.args"]
        );
    }

    #[test]
    fn switching_gamescope_off_unsets_it_and_on_again_restores_fields() {
        let mut form = Form::new(values(&format!("{WINE}[gamescope]\nwidth = 1920\n")));
        form.draft.gamescope = false;
        let edits = form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(edits.unset, ["gamescope"]);
        assert_eq!(form.changes(), 1);
        form.draft.gamescope = true;
        assert!(!form.is_dirty());
    }

    #[test]
    fn a_hand_written_false_reads_as_off() {
        let mut form = Form::new(values(&format!("{WINE}gamescope = false\n")));
        assert!(!form.draft.gamescope);
        assert!(form.edits().is_empty());
        form.draft.gamescope = true;
        assert_eq!(pairs(&form.edits()), [("gamescope", "{}")]);
    }

    #[test]
    fn fields_typed_while_off_survive_a_reload() {
        let mut form = Form::new(values(WINE));
        form.draft.fields.width = "1280".into();
        let form = Form::reloaded(values(WINE), Some(&form));
        assert!(!form.is_dirty());
        assert_eq!(form.draft.fields.width, "1280");
    }

    #[test]
    fn numbers_compare_by_value() {
        let mut form = Form::new(values(&format!("{WINE}[gamescope]\nwidth = 5\n")));
        assert!(form.draft.fields.set_text(Field::Width, "05".into()));
        assert!(!form.is_dirty());
        assert!(form.edits().is_empty());
    }

    #[test]
    fn number_fields_refuse_anything_but_digits() {
        let mut fields = GamescopeFields::default();
        assert!(!fields.set_text(Field::Width, "19x".into()));
        assert!(!fields.set_text(Field::Width, "-1".into()));
        assert!(!fields.set_text(Field::Width, "+5".into()));
        assert!(!fields.set_text(Field::Width, "4294967296".into()));
        assert!(fields.set_text(Field::Width, "1920".into()));
        assert!(fields.set_text(Field::Width, String::new()));
        assert!(!fields.set_text(Field::Args, "1".into()));
    }

    #[test]
    fn args_are_quoted_for_toml() {
        assert_eq!(
            toml_array(&["a\"b".into(), "c\\d".into()]),
            r#"["a\"b", "c\\d"]"#
        );
    }

    #[test]
    fn native_has_no_runner() {
        let form = Form::new(values("kind = \"native\"\n"));
        assert_eq!(form.draft.runner, None);
    }

    fn row(key: &str, value: &str) -> EnvRow {
        EnvRow {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn builds_write_their_name_and_none_unsets() {
        let mut form = Form::new(values(&format!("{WINE}dxvk = \"2.3\"\n")));
        form.pick_build(Component::Dxvk, Some("latest".into()))
            .unwrap();
        form.pick_build(Component::Vkd3d, Some("2.12".into()))
            .unwrap();
        assert_eq!(
            pairs(&form.edits()),
            [("dxvk", "latest"), ("vkd3d", "2.12")]
        );
        assert_eq!(form.changes(), 2);
        form.pick_build(Component::Vkd3d, None).unwrap();
        form.pick_build(Component::Dxvk, None).unwrap();
        let edits = form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(edits.unset, ["dxvk"]);
    }

    #[test]
    fn vkd3d_needs_dxvk() {
        let mut form = Form::new(values(WINE));
        assert!(!form.vkd3d_enabled());
        form.pick_build(Component::Dxvk, Some("latest".into()))
            .unwrap();
        assert!(form.vkd3d_enabled());
        form.pick_build(Component::Vkd3d, Some("latest".into()))
            .unwrap();
        assert_eq!(
            form.pick_build(Component::Dxvk, None),
            Err(VKD3D_NEEDS_DXVK)
        );
        assert_eq!(form.draft.dxvk.as_deref(), Some("latest"));
        assert_eq!(form.problem(true), None);
        // A hand-written vkd3d without DXVK can still be cleared, but not saved.
        let mut form = Form::new(values(&format!("{WINE}vkd3d = \"latest\"\n")));
        assert!(form.vkd3d_enabled());
        assert_eq!(form.problem(true).as_deref(), Some(VKD3D_NEEDS_DXVK));
        assert_eq!(form.problem(false), None);
        form.pick_build(Component::Vkd3d, None).unwrap();
        assert_eq!(form.problem(true), None);
        assert_eq!(form.edits().unset, ["vkd3d"]);
    }

    #[test]
    fn build_choices_keep_an_uninstalled_value() {
        let installed = vec!["2.3".to_owned(), "2.4".to_owned()];
        let shown: Vec<String> = build_choices(&installed, Some("1.0"))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(shown, ["None", "Latest", "2.3", "2.4", "1.0"]);
        assert_eq!(build_choices(&installed, Some("latest")).len(), 4);
        assert_eq!(build_choices(&installed, Some("2.4")).len(), 4);
    }

    #[test]
    fn lists_write_arrays_and_emptied_ones_unset() {
        let mut form = Form::new(values(&format!(
            "{WINE}tags = [\"rpg\"]\nwinetricks = [\"corefonts\"]\n"
        )));
        form.draft.winetricks.push("vcrun2019".into());
        form.draft.tags.clear();
        let edits = form.edits();
        assert_eq!(
            pairs(&edits),
            [("winetricks", "[\"corefonts\", \"vcrun2019\"]")]
        );
        assert_eq!(edits.unset, ["tags"]);
        assert_eq!(form.changes(), 2);
    }

    #[test]
    fn list_input_is_checked_by_core_rules() {
        let verbs = vec!["corefonts".to_owned()];
        assert_eq!(List::Winetricks.problem(&verbs, ""), None);
        assert_eq!(List::Winetricks.problem(&verbs, "d3dx9"), None);
        for bad in ["a b", "-q", "corefonts"] {
            assert!(List::Winetricks.problem(&verbs, bad).is_some(), "{bad}");
        }
        let tags = vec!["rpg".to_owned()];
        assert_eq!(List::Tags.problem(&tags, "co-op_2"), None);
        for bad in ["a b", "a.b", "rpg", "é"] {
            assert!(List::Tags.problem(&tags, bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn env_values_are_written_as_typed() {
        let mut form = Form::new(values(WINE));
        let value = r#"a "b" \c = d"#;
        form.draft.env.push(row("OPTS", value));
        assert_eq!(pairs(&form.edits()), [("env.OPTS", value)]);
        assert_eq!(form.changes(), 1);
    }

    #[test]
    fn env_rows_rename_remove_and_change() {
        let mut form = Form::new(values(&format!(
            "{WINE}[env]\nA = \"1\"\nB = \"2\"\nC = \"3\"\n"
        )));
        form.draft.env[0].key = "AA".into();
        form.draft.env.remove(1);
        form.draft.env[1].value = "4".into();
        let edits = form.edits();
        assert_eq!(edits.unset, ["env.A", "env.B"]);
        assert_eq!(pairs(&edits), [("env.AA", "1"), ("env.C", "4")]);
        assert_eq!(form.changes(), 4);
        // Renaming back is no change.
        form.revert();
        form.draft.env[0].key = "X".into();
        form.draft.env[0].key = "A".into();
        assert!(!form.is_dirty());
    }

    #[test]
    fn bad_env_rows_block_saving() {
        let mut form = Form::new(values(&format!("{WINE}[env]\nA = \"1\"\n")));
        form.draft.env.push(row("A", "2"));
        assert_eq!(form.draft.env_problems()[0], None);
        assert_eq!(
            form.draft.env_problems()[1].as_deref(),
            Some("Duplicate variable \"A\".")
        );
        assert!(form.problem(true).is_some());
        form.draft.env[1].key = "WINEPREFIX".into();
        assert!(form.problem(true).unwrap().contains("WINEPREFIX"));
        form.draft.env[1].key = String::new();
        assert_eq!(form.problem(true).as_deref(), Some("Name can't be empty."));
        form.draft.env[1].key = "A=B".into();
        assert!(form.problem(true).is_some());
        form.draft.env[1].key = "B".into();
        assert_eq!(form.problem(true), None);
        assert_eq!(
            form.draft.new_env_problem("B", "x").as_deref(),
            Some("Duplicate variable \"B\".")
        );
        assert!(form.draft.new_env_problem("WINEPREFIX", "x").is_some());
        assert_eq!(form.draft.new_env_problem("C", "x"), None);
    }

    #[test]
    fn native_may_set_wineprefix_and_writes_no_prefix() {
        let mut form = Form::new(values("kind = \"native\"\n"));
        assert!(!form.draft.wine());
        assert_eq!(form.draft.new_env_problem("WINEPREFIX", "/x"), None);
        form.draft.env.push(row("WINEPREFIX", "/x"));
        assert_eq!(form.problem(true), None);
        assert_eq!(pairs(&form.edits()), [("env.WINEPREFIX", "/x")]);
    }

    #[test]
    fn advanced_fields_set_and_emptied_ones_unset() {
        let mut form = Form::new(values(&format!("{WINE}gameid = \"umu-1\"\n")));
        assert_eq!(form.draft.prefix, "~/p");
        form.draft.prefix = "~/elsewhere".into();
        form.draft.gameid.clear();
        form.draft.store = "gog".into();
        let edits = form.edits();
        assert_eq!(pairs(&edits), [("prefix", "~/elsewhere"), ("store", "gog")]);
        assert_eq!(edits.unset, ["gameid"]);
        assert_eq!(form.changes(), 3);
        form.draft.prefix.clear();
        assert_eq!(
            form.problem(true).as_deref(),
            Some("The prefix can't be empty.")
        );
        for bad in ["prefixes/game", "~", "~other/p", "./p"] {
            form.draft.prefix = bad.into();
            assert_eq!(
                form.prefix_problem(),
                Some("The prefix must be absolute or start with ~/."),
                "{bad}"
            );
        }
        for good in ["/games/p", "~/games/p"] {
            form.draft.prefix = good.into();
            assert_eq!(form.problem(true), None, "{good}");
        }
    }

    #[test]
    fn an_unedited_relative_prefix_does_not_block_saving() {
        let mut form = Form::new(values(&WINE.replace("~/p", "prefixes/p")));
        form.draft.gamemode = Some(true);
        assert_eq!(form.problem(true), None);
        form.draft.prefix = "prefixes/q".into();
        assert!(form.problem(true).is_some());
    }

    const TARGETS: &str = "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n[targets.game]\nexe = 'C:\\g.exe'\ndefault = true\n[targets.tool]\nexe = '/usr/bin/tool'\nargs = ['-a']\ncwd = '/tmp'\nname = 'Tool'\ndesktop = true\n";

    fn target<'a>(form: &'a mut Form, name: &str) -> &'a mut TargetValues {
        form.draft.targets.get_mut(name).unwrap()
    }

    #[test]
    fn target_fields_write_and_emptied_ones_unset() {
        let mut form = Form::new(values(TARGETS));
        assert_eq!(form.draft.default_target.as_deref(), Some("game"));
        let game = target(&mut form, "game");
        game.exe = r"C:\other.exe".into();
        game.args = vec!["-x".into(), "a b".into()];
        game.cwd = "~/games".into();
        game.name = "Game".into();
        game.desktop = true;
        let tool = target(&mut form, "tool");
        tool.args.clear();
        tool.cwd.clear();
        tool.name.clear();
        tool.desktop = false;
        let edits = form.edits();
        assert_eq!(
            pairs(&edits),
            [
                ("targets.game.exe", r"C:\other.exe"),
                ("targets.game.args", "[\"-x\", \"a b\"]"),
                ("targets.game.cwd", "~/games"),
                ("targets.game.name", "Game"),
                ("targets.game.desktop", "true"),
            ]
        );
        assert_eq!(
            edits.unset,
            [
                "targets.tool.args",
                "targets.tool.cwd",
                "targets.tool.name",
                "targets.tool.desktop"
            ]
        );
        assert_eq!(form.changes(), 9);
        form.revert();
        assert!(!form.is_dirty());
    }

    #[test]
    fn marking_a_default_moves_it_with_one_pair() {
        let mut form = Form::new(values(TARGETS));
        form.set_default("tool", true);
        assert_eq!(form.draft.default_target.as_deref(), Some("tool"));
        assert_eq!(pairs(&form.edits()), [("targets.tool.default", "true")]);
        assert!(form.edits().unset.is_empty());
        assert_eq!(form.changes(), 1);
        // Unmarking the one that isn't marked does nothing.
        form.set_default("game", false);
        assert_eq!(form.draft.default_target.as_deref(), Some("tool"));
        form.set_default("tool", false);
        let edits = form.edits();
        assert_eq!(edits.unset, ["targets.game.default"]);
        assert!(edits.pairs.is_empty());
        // Several targets and no default can't be saved.
        assert_eq!(form.targets_problem(false).as_deref(), Some(NEEDS_DEFAULT));
        form.set_default("game", true);
        assert!(!form.is_dirty());
        assert_eq!(form.targets_problem(false), None);
    }

    #[test]
    fn a_config_without_a_default_only_blocks_target_edits() {
        let mut form = Form::new(values(&TARGETS.replace("default = true\n", "")));
        form.draft.gamemode = Some(true);
        assert_eq!(form.targets_problem(false), None);
        target(&mut form, "tool").args.push("-b".into());
        assert_eq!(form.targets_problem(false).as_deref(), Some(NEEDS_DEFAULT));
        // A lone target is launched without one.
        let mut form = Form::new(values(&TARGETS.replace("default = true\n", "")));
        form.remove_target("tool");
        assert_eq!(form.targets_problem(false), None);
    }

    #[test]
    fn a_lone_target_needs_no_default() {
        let single = TARGETS
            .split("[targets.tool]")
            .next()
            .unwrap()
            .replace("default = true\n", "");
        let mut form = Form::new(values(&single));
        form.draft
            .targets
            .get_mut("game")
            .unwrap()
            .args
            .push("-a".into());
        assert_eq!(form.targets_problem(false), None);
        assert!(form.is_dirty());
    }

    #[test]
    fn target_switches_override_and_reset() {
        let mut form = Form::new(values(
            &TARGETS.replace("desktop = true\n", "desktop = true\nmangohud = false\n"),
        ));
        *target(&mut form, "game").switch_mut(Switch::Gamemode) = Some(true);
        *target(&mut form, "tool").switch_mut(Switch::Mangohud) = None;
        let edits = form.edits();
        assert_eq!(pairs(&edits), [("targets.game.gamemode", "true")]);
        assert_eq!(edits.unset, ["targets.tool.mangohud"]);
        assert_eq!(form.changes(), 2);
    }

    #[test]
    fn target_labels_follow_the_environment_draft() {
        let mut form = Form::new(values(TARGETS));
        let global = Inherited {
            gamemode: Some(true),
            ..Inherited::default()
        };
        assert_eq!(
            form.target_switch("game", Switch::Gamemode, &global),
            (true, Source::Global)
        );
        form.draft.gamemode = Some(false);
        assert_eq!(
            form.target_switch("game", Switch::Gamemode, &global),
            (false, Source::Environment)
        );
        *target(&mut form, "game").switch_mut(Switch::Gamemode) = Some(true);
        assert_eq!(
            form.target_switch("game", Switch::Gamemode, &global),
            (true, Source::Here)
        );
        // Gamescope has no global, so an inheriting target follows the
        // environment's draft switch.
        assert_eq!(form.target_gamescope("tool"), (false, Source::Environment));
        form.draft.gamescope = true;
        assert_eq!(form.target_gamescope("tool"), (true, Source::Environment));
        assert_eq!(form.target_fields("tool"), Some(&form.draft.fields));
    }

    #[test]
    fn editing_an_inherited_gamescope_copies_the_environments_fields() {
        let mut form = Form::new(values(&TARGETS.replace(
            "[targets.game]",
            "[gamescope]\nwidth = 1920\nfilter = \"fsr\"\n[targets.game]",
        )));
        // Refused text changes nothing, copy included.
        assert!(
            !form.edit_target_fields("game", |fields| fields.set_text(Field::Height, "x".into()))
        );
        assert!(!form.is_dirty());
        assert!(form.edit_target_fields("game", |fields| {
            fields.set_text(Field::Height, "1080".into())
        }));
        let game = &form.draft.targets["game"];
        assert_eq!(game.gamescope, TargetGamescope::On);
        assert_eq!(
            (game.fields.width.as_str(), game.fields.filter),
            ("1920", Some("fsr"))
        );
        assert_eq!(
            pairs(&form.edits()),
            [
                ("targets.game.gamescope", "{}"),
                ("targets.game.gamescope.width", "1920"),
                ("targets.game.gamescope.height", "1080"),
                ("targets.game.gamescope.filter", "fsr"),
            ]
        );
        assert_eq!(form.changes(), 4);
        // The environment's fields stay its own.
        assert_eq!(form.draft.fields.height, "");
        // With the environment's gamescope off there's nothing to edit.
        let mut form = Form::new(values(TARGETS));
        assert!(!form.edit_target_fields("game", |_| true));
        assert!(!form.override_target_gamescope("game"));
    }

    #[test]
    fn target_gamescope_moves_between_false_unset_and_a_table() {
        let text = TARGETS.replace("[targets.tool]", "gamescope = false\n[targets.tool]");
        let mut form = Form::new(values(&format!(
            "{text}[targets.tool.gamescope]\nwidth = 800\n"
        )));
        assert_eq!(form.draft.targets["game"].gamescope, TargetGamescope::Off);
        assert_eq!(form.draft.targets["tool"].gamescope, TargetGamescope::On);
        // false to a table of its own, and a table to false.
        assert!(form.set_target_gamescope("game", true));
        assert!(form.set_target_gamescope("tool", false));
        let edits = form.edits();
        assert_eq!(
            pairs(&edits),
            [
                ("targets.game.gamescope", "{}"),
                ("targets.tool.gamescope", "false"),
            ]
        );
        assert!(edits.unset.is_empty());
        // Reset unsets either one.
        target(&mut form, "game").gamescope = TargetGamescope::Inherit;
        target(&mut form, "tool").gamescope = TargetGamescope::Inherit;
        let edits = form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(
            edits.unset,
            ["targets.game.gamescope", "targets.tool.gamescope"]
        );
        // Back on, a table keeps the fields it had.
        assert!(form.set_target_gamescope("tool", true));
        assert_eq!(form.draft.targets["tool"].fields.width, "800");
        assert_eq!(form.edits().unset, ["targets.game.gamescope"]);
    }

    #[test]
    fn turning_a_target_gamescope_on_starts_from_the_environments() {
        let mut form = Form::new(values(&TARGETS.replace(
            "[targets.game]",
            "[gamescope]\nwidth = 1920\n[targets.game]",
        )));
        assert!(form.set_target_gamescope("game", false));
        assert!(form.set_target_gamescope("game", true));
        assert_eq!(form.draft.targets["game"].fields.width, "1920");
    }

    #[test]
    fn target_gamescope_fields_typed_while_off_survive_a_reload() {
        let mut form = Form::new(values(TARGETS));
        target(&mut form, "game").fields.width = "640".into();
        let form = Form::reloaded(values(TARGETS), Some(&form));
        assert!(!form.is_dirty());
        assert_eq!(form.draft.targets["game"].fields.width, "640");
    }

    #[test]
    fn adding_and_removing_targets() {
        let mut form = Form::new(values(TARGETS));
        assert_eq!(
            form.new_target_problem("my game").as_deref(),
            Some("Invalid input: spaces not allowed.")
        );
        assert_eq!(
            form.new_target_problem("game").as_deref(),
            Some("There is already a target called `game`.")
        );
        assert_eq!(form.new_target_problem(""), None);
        assert_eq!(form.suggest_target("Game", r"C:\x\Game.exe"), "game-2");
        assert_eq!(form.suggest_target("", r"C:\x\Hades II.exe"), "hades-ii");
        assert!(form.add_target("game", TargetValues::default()).is_err());
        let new = TargetValues {
            exe: r"C:\new.exe".into(),
            desktop: true,
            name: "New".into(),
            mangohud: Some(true),
            ..TargetValues::default()
        };
        form.add_target("new", new).unwrap();
        form.set_default("new", true);
        assert!(form.remove_target("tool"));
        assert!(!form.remove_target("tool"));
        let edits = form.edits();
        assert_eq!(edits.unset, ["targets.tool"]);
        assert_eq!(
            pairs(&edits),
            [
                ("targets.new.exe", r"C:\new.exe"),
                ("targets.new.name", "New"),
                ("targets.new.desktop", "true"),
                ("targets.new.mangohud", "true"),
                ("targets.new.default", "true"),
            ]
        );
        assert_eq!(form.changes(), 3);
        // Removing the default takes its mark along, as one change.
        let mut form = Form::new(values(TARGETS));
        form.remove_target("game");
        assert_eq!(form.draft.default_target, None);
        assert_eq!(form.changes(), 1);
        assert_eq!(form.edits().unset, ["targets.game"]);
        assert_eq!(form.targets_problem(false), None);
    }

    #[test]
    fn edited_target_paths_are_checked() {
        let mut form = Form::new(values(TARGETS));
        target(&mut form, "tool").exe.clear();
        assert_eq!(
            form.targets_problem(false).as_deref(),
            Some("Target tool needs an exe.")
        );
        target(&mut form, "tool").exe = "bin/tool".into();
        assert_eq!(
            form.targets_problem(false).as_deref(),
            Some("Target tool's exe must be absolute or start with ~/.")
        );
        assert_eq!(form.targets_problem(true), None);
        for good in ["tool", "~/bin/tool", "/bin/tool", r"C:\tool.exe"] {
            target(&mut form, "tool").exe = good.into();
            assert_eq!(form.targets_problem(false), None, "{good}");
        }
        for bad in ["work", "./work"] {
            target(&mut form, "tool").cwd = bad.into();
            assert!(
                form.targets_problem(false).unwrap().contains("working dir"),
                "{bad}"
            );
        }
        target(&mut form, "tool").cwd = "~/work".into();
        assert_eq!(form.targets_problem(false), None);
    }
}
