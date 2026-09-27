//! The `[gamescope]` table on an environment, a target, or a template.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use toml_edit::{Item, Table, value};

/// What a config says about gamescope. `false` exists so a target can turn
/// off the environment's table; `true` would say nothing about how to run it.
#[derive(Debug, Clone, PartialEq)]
pub enum GamescopeSetting {
    Off,
    On(Gamescope),
}

impl GamescopeSetting {
    pub fn on(&self) -> Option<&Gamescope> {
        match self {
            GamescopeSetting::Off => None,
            GamescopeSetting::On(gamescope) => Some(gamescope),
        }
    }

    /// The setting as it is written to a config file.
    pub fn to_item(&self) -> Item {
        match self {
            GamescopeSetting::Off => value(false),
            GamescopeSetting::On(gamescope) => Item::Table(gamescope.to_table()),
        }
    }
}

impl<'de> Deserialize<'de> for GamescopeSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SettingVisitor;

        impl<'de> Visitor<'de> for SettingVisitor {
            type Value = GamescopeSetting;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a [gamescope] table or false")
            }

            fn visit_bool<E: de::Error>(self, on: bool) -> Result<Self::Value, E> {
                if on {
                    Err(E::custom(
                        "gamescope takes a [gamescope] table or false, not true",
                    ))
                } else {
                    Ok(GamescopeSetting::Off)
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Gamescope::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(GamescopeSetting::On)
            }
        }

        deserializer.deserialize_any(SettingVisitor)
    }
}

/// How gamescope runs a game. Every field is optional, so an empty table is
/// plain `gamescope --`.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gamescope {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub output_width: Option<u32>,
    pub output_height: Option<u32>,
    pub filter: Option<Filter>,
    pub scaler: Option<Scaler>,
    pub window: Option<Window>,
    pub fps_limit: Option<u32>,
    pub fps_limit_unfocused: Option<u32>,
    #[serde(default)]
    pub grab_cursor: bool,
    /// Flags without a field of their own, passed last.
    #[serde(default)]
    pub args: Vec<String>,
}

impl Gamescope {
    /// The flags before `--`. With `mangoapp`, gamescope runs MangoHud itself
    /// instead of it being a wrapper, as Heroic does.
    pub fn args(&self, mangoapp: bool) -> Vec<String> {
        let mut args = Vec::new();
        for (flag, set) in [
            ("-w", self.width),
            ("-h", self.height),
            ("-W", self.output_width),
            ("-H", self.output_height),
        ] {
            if let Some(set) = set {
                args.extend([flag.to_owned(), set.to_string()]);
            }
        }
        if let Some(filter) = self.filter {
            args.extend(["-F".to_owned(), filter.as_str().to_owned()]);
        }
        if let Some(scaler) = self.scaler {
            args.extend(["-S".to_owned(), scaler.as_str().to_owned()]);
        }
        match self.window {
            Some(Window::Fullscreen) => args.push("-f".to_owned()),
            Some(Window::Borderless) => args.push("-b".to_owned()),
            None => {}
        }
        for (flag, set) in [("-r", self.fps_limit), ("-o", self.fps_limit_unfocused)] {
            if let Some(set) = set {
                args.extend([flag.to_owned(), set.to_string()]);
            }
        }
        if self.grab_cursor {
            args.push("--force-grab-cursor".to_owned());
        }
        if mangoapp {
            args.push("--mangoapp".to_owned());
        }
        args.extend(self.args.iter().cloned());
        args
    }

    fn to_table(&self) -> Table {
        let mut table = Table::new();
        for (key, set) in [
            ("width", self.width),
            ("height", self.height),
            ("output_width", self.output_width),
            ("output_height", self.output_height),
        ] {
            if let Some(set) = set {
                table[key] = value(i64::from(set));
            }
        }
        for (key, set) in [
            ("filter", self.filter.map(Filter::as_str)),
            ("scaler", self.scaler.map(Scaler::as_str)),
            ("window", self.window.map(Window::as_str)),
        ] {
            if let Some(set) = set {
                table[key] = value(set);
            }
        }
        for (key, set) in [
            ("fps_limit", self.fps_limit),
            ("fps_limit_unfocused", self.fps_limit_unfocused),
        ] {
            if let Some(set) = set {
                table[key] = value(i64::from(set));
            }
        }
        if self.grab_cursor {
            table["grab_cursor"] = value(true);
        }
        if !self.args.is_empty() {
            table["args"] = value(self.args.iter().collect::<toml_edit::Array>());
        }
        table
    }
}

/// `-F`, how the game's frames are filtered when scaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    Linear,
    Nearest,
    Fsr,
    Nis,
    Pixel,
    Sgsr,
}

impl Filter {
    pub fn as_str(self) -> &'static str {
        match self {
            Filter::Linear => "linear",
            Filter::Nearest => "nearest",
            Filter::Fsr => "fsr",
            Filter::Nis => "nis",
            Filter::Pixel => "pixel",
            Filter::Sgsr => "sgsr",
        }
    }
}

/// `-S`, how the game's frames are fitted to the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scaler {
    Auto,
    Integer,
    Fit,
    Fill,
    Stretch,
}

impl Scaler {
    pub fn as_str(self) -> &'static str {
        match self {
            Scaler::Auto => "auto",
            Scaler::Integer => "integer",
            Scaler::Fit => "fit",
            Scaler::Fill => "fill",
            Scaler::Stretch => "stretch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    Fullscreen,
    Borderless,
}

impl Window {
    pub fn as_str(self) -> &'static str {
        match self {
            Window::Fullscreen => "fullscreen",
            Window::Borderless => "borderless",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Holder {
        gamescope: Option<GamescopeSetting>,
    }

    fn parse(text: &str) -> Result<Option<GamescopeSetting>, toml::de::Error> {
        toml::from_str::<Holder>(text).map(|holder| holder.gamescope)
    }

    const FULL: &str = r#"
        [gamescope]
        width = 1920
        height = 1080
        output_width = 2560
        output_height = 1440
        filter = "fsr"
        scaler = "fit"
        window = "fullscreen"
        fps_limit = 60
        fps_limit_unfocused = 10
        grab_cursor = true
        args = ["--hdr-enabled"]
    "#;

    #[test]
    fn every_field_becomes_its_flag_in_order() {
        let setting = parse(FULL).unwrap().unwrap();
        let gamescope = setting.on().unwrap();
        assert_eq!(
            gamescope.args(false),
            [
                "-w",
                "1920",
                "-h",
                "1080",
                "-W",
                "2560",
                "-H",
                "1440",
                "-F",
                "fsr",
                "-S",
                "fit",
                "-f",
                "-r",
                "60",
                "-o",
                "10",
                "--force-grab-cursor",
                "--hdr-enabled",
            ]
        );
        let with_hud = gamescope.args(true);
        assert_eq!(
            &with_hud[with_hud.len() - 2..],
            ["--mangoapp", "--hdr-enabled"]
        );
    }

    #[test]
    fn an_empty_table_has_no_flags() {
        let setting = parse("[gamescope]\n").unwrap().unwrap();
        assert_eq!(setting, GamescopeSetting::On(Gamescope::default()));
        assert!(setting.on().unwrap().args(false).is_empty());
        assert_eq!(setting.on().unwrap().args(true), ["--mangoapp"]);
    }

    #[test]
    fn borderless_is_dash_b() {
        let setting = parse("[gamescope]\nwindow = \"borderless\"\n")
            .unwrap()
            .unwrap();
        assert_eq!(setting.on().unwrap().args(false), ["-b"]);
    }

    #[test]
    fn false_is_off_and_true_is_refused() {
        assert_eq!(
            parse("gamescope = false\n").unwrap(),
            Some(GamescopeSetting::Off)
        );
        let err = parse("gamescope = true\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("gamescope takes a [gamescope] table or false, not true"),
            "{err}"
        );
        assert!(parse("gamescope = 1\n").is_err());
    }

    #[test]
    fn bad_values_and_unknown_fields_are_refused() {
        for text in [
            "filter = \"bilinear\"",
            "scaler = \"zoom\"",
            "window = \"windowed\"",
            "window = \"Fullscreen\"",
            "width = -1",
            "widht = 1920",
            "args = \"-f\"",
        ] {
            assert!(parse(&format!("[gamescope]\n{text}\n")).is_err(), "{text}");
        }
    }

    #[test]
    fn a_setting_round_trips_through_its_item() {
        for text in [FULL, "[gamescope]\n", "gamescope = false\n"] {
            let setting = parse(text).unwrap().unwrap();
            let mut doc = toml_edit::DocumentMut::new();
            doc["gamescope"] = setting.to_item();
            assert_eq!(parse(&doc.to_string()).unwrap().unwrap(), setting, "{doc}");
        }
    }
}
