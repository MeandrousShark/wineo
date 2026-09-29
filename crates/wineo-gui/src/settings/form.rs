//! What the settings page edits, and how an edit becomes `settings::apply`'s
//! `pairs` and `unset`. Kept free of iced so it can be tested on its own.

use std::fmt;

use wineo_core::config::EnvironmentConfig;
use wineo_core::gamescope::{self, GamescopeSetting};

/// The on/off launch settings an environment can override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Switch {
    Gamemode,
    Mangohud,
    Wayland,
}

impl Switch {
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
    Default,
    Here,
}

impl Source {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Source::Global => "global",
            Source::Default => "default",
            Source::Here => "set here",
        }
    }
}

/// What the environment's `gamescope` key holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GamescopeState {
    Unset,
    Off,
    On,
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
    pub(crate) fn is_number(self) -> bool {
        matches!(
            self,
            Field::Width
                | Field::Height
                | Field::OutputWidth
                | Field::OutputHeight
                | Field::FpsLimit
                | Field::FpsLimitUnfocused
        )
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

/// Gamescope's fields as the page shows them. An empty string, `None`, or
/// `false` means the key is left out of the table.
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
    /// Split on whitespace when written.
    pub(crate) args: String,
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
            args: gamescope.args.join(" "),
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
            Field::Args => &self.args,
            Field::Filter | Field::Scaler | Field::Window | Field::GrabCursor => "",
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
    /// Takes typed text, refusing anything a number field can't hold so the
    /// form never carries a value TOML would reject.
    pub(crate) fn set_text(&mut self, field: Field, value: String) -> bool {
        if field.is_number() && !value.is_empty() && value.parse::<u32>().is_err() {
            return false;
        }
        let slot = match field {
            Field::Width => &mut self.width,
            Field::Height => &mut self.height,
            Field::OutputWidth => &mut self.output_width,
            Field::OutputHeight => &mut self.output_height,
            Field::FpsLimit => &mut self.fps_limit,
            Field::FpsLimitUnfocused => &mut self.fps_limit_unfocused,
            Field::Args => &mut self.args,
            Field::Filter | Field::Scaler | Field::Window | Field::GrabCursor => return false,
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
    /// The raw value `edit::set` takes for a field, or `None` to leave it unset.
    fn raw(&self, field: Field) -> Option<String> {
        let text = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        match field {
            Field::Filter | Field::Scaler | Field::Window => {
                self.choice(field).0.map(str::to_owned)
            }
            Field::GrabCursor => self.grab_cursor.then(|| "true".to_owned()),
            Field::Args => {
                let args: Vec<&str> = self.args.split_whitespace().collect();
                (!args.is_empty()).then(|| toml_array(&args))
            }
            _ => text(self.text(field)),
        }
    }
}

/// Every control's value, read from a config or edited on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Values {
    /// `None` on a native environment, which has no runner.
    pub(crate) runner: Option<String>,
    pub(crate) gamemode: Option<bool>,
    pub(crate) mangohud: Option<bool>,
    pub(crate) wayland: Option<bool>,
    pub(crate) gamescope: GamescopeState,
    pub(crate) fields: GamescopeFields,
}

impl Values {
    pub(crate) fn from_config(config: &EnvironmentConfig) -> Self {
        let (gamescope, fields) = match &config.gamescope {
            None => (GamescopeState::Unset, GamescopeFields::default()),
            Some(GamescopeSetting::Off) => (GamescopeState::Off, GamescopeFields::default()),
            Some(GamescopeSetting::On(table)) => {
                (GamescopeState::On, GamescopeFields::from_config(table))
            }
        };
        Self {
            runner: config.wine().map(|wine| wine.runner.clone()),
            gamemode: config.gamemode,
            mangohud: config.mangohud,
            wayland: config.wayland,
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

/// Gamescope has no global setting, so unset means off.
pub(crate) fn gamescope_state(state: GamescopeState) -> (bool, Source) {
    match state {
        GamescopeState::Unset => (false, Source::Default),
        GamescopeState::Off => (false, Source::Here),
        GamescopeState::On => (true, Source::Here),
    }
}

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
    pub(crate) fn revert(&mut self) {
        self.draft = self.loaded.clone();
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.changes() > 0
    }
    /// How many controls differ from the file. Gamescope's fields only count
    /// while it's on, since switching it off drops them anyway.
    pub(crate) fn changes(&self) -> usize {
        let (loaded, draft) = (&self.loaded, &self.draft);
        let mut count = usize::from(loaded.runner != draft.runner);
        for switch in [Switch::Gamemode, Switch::Mangohud, Switch::Wayland] {
            count += usize::from(loaded.switch(switch) != draft.switch(switch));
        }
        count += usize::from(loaded.gamescope != draft.gamescope);
        if draft.gamescope == GamescopeState::On {
            let base = self.gamescope_base();
            count += Field::ALL
                .iter()
                .filter(|field| base.raw(**field) != draft.fields.raw(**field))
                .count();
        }
        count
    }
    /// The fields a gamescope table starts from when this saves: the file's
    /// own table, or an empty one when it had none.
    fn gamescope_base(&self) -> GamescopeFields {
        if self.loaded.gamescope == GamescopeState::On {
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
        match draft.gamescope {
            GamescopeState::Unset if loaded.gamescope != GamescopeState::Unset => {
                edits.unset.push("gamescope".to_owned());
            }
            GamescopeState::Off if loaded.gamescope != GamescopeState::Off => {
                edits
                    .pairs
                    .push(("gamescope".to_owned(), "false".to_owned()));
            }
            GamescopeState::On => {
                let base = self.gamescope_base();
                let before = edits.pairs.len();
                for field in Field::ALL {
                    let value = draft.fields.raw(field);
                    if base.raw(field) != value {
                        edits.write(format!("gamescope.{}", field.key()), value);
                    }
                }
                // `set` can only write a table through one of its fields, so
                // an empty table is a field written and then removed again.
                if loaded.gamescope != GamescopeState::On && edits.pairs.len() == before {
                    edits
                        .pairs
                        .push(("gamescope.grab_cursor".to_owned(), "false".to_owned()));
                    edits.unset.push("gamescope.grab_cursor".to_owned());
                }
            }
            GamescopeState::Unset | GamescopeState::Off => {}
        }
        edits
    }
}

/// A TOML array of basic strings, the form `edit::set` parses list values from.
fn toml_array(items: &[&str]) -> String {
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
    fn empty_gamescope_fields_stay_unset() {
        let mut form = Form::new(values(WINE));
        form.draft.gamescope = GamescopeState::On;
        form.draft.fields.width = "1920".into();
        form.draft.fields.filter = Some("fsr");
        form.draft.fields.args = "--hdr-enabled  --expose-wayland".into();
        let edits = form.edits();
        assert_eq!(
            pairs(&edits),
            [
                ("gamescope.width", "1920"),
                ("gamescope.filter", "fsr"),
                (
                    "gamescope.args",
                    "[\"--hdr-enabled\", \"--expose-wayland\"]"
                ),
            ]
        );
        assert!(edits.unset.is_empty());
        // The switch and three fields.
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
        form.draft.fields.args = " ".into();
        let edits = form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(
            edits.unset,
            ["gamescope.width", "gamescope.grab_cursor", "gamescope.args"]
        );
    }

    #[test]
    fn switching_gamescope_on_with_no_fields_writes_an_empty_table() {
        let mut form = Form::new(values(WINE));
        form.draft.gamescope = GamescopeState::On;
        let edits = form.edits();
        assert_eq!(pairs(&edits), [("gamescope.grab_cursor", "false")]);
        assert_eq!(edits.unset, ["gamescope.grab_cursor"]);
        assert_eq!(form.changes(), 1);
    }

    #[test]
    fn switching_gamescope_off_and_resetting() {
        let mut form = Form::new(values(&format!("{WINE}[gamescope]\nwidth = 1920\n")));
        form.draft.gamescope = GamescopeState::Off;
        assert_eq!(pairs(&form.edits()), [("gamescope", "false")]);
        assert_eq!(form.changes(), 1);
        form.draft.gamescope = GamescopeState::Unset;
        assert_eq!(form.edits().unset, ["gamescope"]);
        // Turning it back on keeps the fields it had.
        form.draft.gamescope = GamescopeState::On;
        assert!(!form.is_dirty());
    }

    #[test]
    fn gamescope_off_in_the_file_is_set_here() {
        let form = Form::new(values(&format!("{WINE}gamescope = false\n")));
        assert_eq!(gamescope_state(form.draft.gamescope), (false, Source::Here));
        assert_eq!(
            gamescope_state(GamescopeState::Unset),
            (false, Source::Default)
        );
    }

    #[test]
    fn number_fields_refuse_anything_but_digits() {
        let mut fields = GamescopeFields::default();
        assert!(!fields.set_text(Field::Width, "19x".into()));
        assert!(!fields.set_text(Field::Width, "-1".into()));
        assert!(fields.set_text(Field::Width, "1920".into()));
        assert!(fields.set_text(Field::Width, String::new()));
        assert!(fields.set_text(Field::Args, "-x y".into()));
    }

    #[test]
    fn args_are_quoted_for_toml() {
        assert_eq!(toml_array(&["a\"b", "c\\d"]), r#"["a\"b", "c\\d"]"#);
    }

    #[test]
    fn native_has_no_runner() {
        let form = Form::new(values("kind = \"native\"\n"));
        assert_eq!(form.draft.runner, None);
    }
}
