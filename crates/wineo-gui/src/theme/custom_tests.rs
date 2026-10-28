use iced::theme::Mode;
use wineo_core::themes::{Colors, ThemeFile, ThemeKind};

use super::*;

fn preset(id: &str) -> &'static Preset {
    Preset::find(id).unwrap()
}

fn file(id: &str, kind: Option<ThemeKind>, based_on: Option<&str>, colors: Colors) -> ThemeFile {
    ThemeFile {
        id: id.into(),
        name: id.into(),
        kind,
        based_on: based_on.map(str::to_owned),
        colors,
        newer_version: false,
    }
}

#[test]
fn missing_colors_come_from_based_on() {
    let colors = Colors {
        primary: Some("#123456".into()),
        ..Colors::default()
    };
    let custom = Custom::from_file(&file("mine", Some(ThemeKind::Dark), Some("cellar"), colors));
    let expected = Palette {
        primary: rgb(0x123456),
        ..preset("cellar").palette
    };
    assert_eq!(custom.palette(), expected);
    assert_eq!(custom.id, "custom:mine");
    assert_eq!(custom.file_id(), "mine");
}

#[test]
fn a_missing_kind_is_dark_and_a_missing_based_on_is_the_kinds_default() {
    let custom = Custom::from_file(&file("mine", None, None, Colors::default()));
    assert_eq!(custom.kind, PresetKind::Dark);
    assert_eq!(custom.palette(), preset("midnight").palette);
    let light = Custom::from_file(&file(
        "mine",
        Some(ThemeKind::Light),
        None,
        Colors::default(),
    ));
    assert_eq!(light.kind, PresetKind::Light);
    assert_eq!(light.palette(), preset("daylight").palette);
    // A kind with another kind's base keeps both as written.
    let mixed = Custom::from_file(&file("mine", None, Some("daylight"), Colors::default()));
    assert_eq!(mixed.kind, PresetKind::Dark);
    assert_eq!(mixed.palette(), preset("daylight").palette);
}

#[test]
fn every_color_given_needs_no_base() {
    let colors = colors_of(preset("dracula").palette);
    let custom = Custom::from_file(&file("mine", Some(ThemeKind::Dark), None, colors));
    assert_eq!(custom.palette(), preset("dracula").palette);
    assert_eq!(hex(rgb(0x0a0b0c)), "#0a0b0c");
}

#[test]
fn custom_ids_resolve_and_fall_back_by_slot() {
    let dark = Custom::from_file(&file(
        "night",
        Some(ThemeKind::Dark),
        Some("dracula"),
        Colors::default(),
    ));
    let light = Custom::from_file(&file(
        "day",
        Some(ThemeKind::Light),
        Some("catppuccin-latte"),
        Colors::default(),
    ));
    let customs = [dark, light];
    let appearance = Appearance {
        dark: "custom:night".into(),
        light: "custom:day".into(),
        ..Appearance::default()
    };
    assert_eq!(
        resolve(&appearance, Mode::Dark, &customs),
        preset("dracula").palette
    );
    assert_eq!(
        resolve(&appearance, Mode::Light, &customs),
        preset("catppuccin-latte").palette
    );
    assert_eq!(shown("custom:night", None, &customs).name(), "night");
    // Gone, or the wrong kind for the slot, falls back as for a built-in.
    let swapped = Appearance {
        dark: "custom:day".into(),
        light: "custom:gone".into(),
        ..Appearance::default()
    };
    assert_eq!(
        resolve(&swapped, Mode::Dark, &customs),
        preset("midnight").palette
    );
    assert_eq!(
        resolve(&swapped, Mode::Light, &customs),
        preset("daylight").palette
    );
    // Without the list, nothing custom resolves.
    assert_eq!(
        resolve(&appearance, Mode::Dark, &[]),
        preset("midnight").palette
    );
    let fixed = Appearance {
        follow_system: false,
        theme: "custom:day".into(),
        improve_readability: true,
        ..Appearance::default()
    };
    assert_eq!(
        resolve(&fixed, Mode::Dark, &customs),
        improve(preset("catppuccin-latte").palette, PresetKind::Light)
    );
}

#[test]
fn a_file_that_does_not_parse_is_left_out() {
    let themes = wineo_core::themes::Themes {
        themes: vec![file("good", None, None, Colors::default())],
        invalid: vec![wineo_core::themes::InvalidTheme {
            id: "bad".into(),
            path: "bad.toml".into(),
            error: "line 1".into(),
        }],
    };
    let listed: Vec<_> = customs(&themes).into_iter().map(|theme| theme.id).collect();
    assert_eq!(listed, ["custom:good"]);
}
