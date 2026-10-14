//! What the settings page edits, and how an edit becomes `settings::apply`'s
//! `pairs` and `unset`. Kept free of iced so it can be tested on its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use wineo_core::components::{Component, LATEST};
use wineo_core::config::{self, EnvironmentConfig, Kind, Target};
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
    /// Settings that exist only on a proxied environment.
    pub(crate) proxied: Option<ProxiedValues>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxiedValues {
    pub(crate) title: Option<String>,
    pub(crate) desktop: bool,
}

impl Values {
    pub(crate) fn from_config(config: &EnvironmentConfig) -> Self {
        let (gamescope, fields) = match &config.gamescope {
            Some(GamescopeSetting::On(table)) => (true, GamescopeFields::from_config(table)),
            None | Some(GamescopeSetting::Off) => (false, GamescopeFields::default()),
        };
        let wine = config.wine();
        let proxied = match &config.kind {
            Kind::Proxied(proxied) => Some(ProxiedValues {
                title: proxied.title.clone(),
                desktop: proxied.desktop,
            }),
            Kind::Wine(_) | Kind::Native => None,
        };
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
            proxied,
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
        if let (Some(before), Some(after)) = (&loaded.proxied, &draft.proxied) {
            count += usize::from(before.title != after.title);
            count += usize::from(before.desktop != after.desktop);
        }
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
        if self.draft.targets.contains_key(name) {
            return Some(format!("There is already a target called `{name}`."));
        }
        (self.loaded.targets.contains_key(name))
            .then(|| format!("`{name}` was removed. Save first to reuse its name."))
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
        targets::free_name(&suggested, |taken| {
            self.draft.targets.contains_key(taken) || self.loaded.targets.contains_key(taken)
        })
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
        if let (Some(before), Some(after)) = (&loaded.proxied, &draft.proxied) {
            if before.title != after.title {
                edits.write("title".to_owned(), after.title.clone());
            }
            if before.desktop != after.desktop {
                edits.write(
                    "desktop".to_owned(),
                    after.desktop.then(|| "true".to_owned()),
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
mod tests;
