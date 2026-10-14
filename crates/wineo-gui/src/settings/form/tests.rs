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
    assert!(!form.edit_target_fields("game", |fields| fields.set_text(Field::Height, "x".into())));
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
fn a_removed_target_name_stays_taken_until_save() {
    let mut form = Form::new(values(TARGETS));
    assert!(form.remove_target("tool"));
    assert_eq!(
        form.new_target_problem("tool").as_deref(),
        Some("`tool` was removed. Save first to reuse its name.")
    );
    assert_eq!(form.suggest_target("Tool", r"C:\x\Tool.exe"), "tool-2");
    assert_eq!(
        form.add_target("tool", TargetValues::default())
            .unwrap_err(),
        "`tool` was removed. Save first to reuse its name."
    );
    assert_eq!(form.edits().unset, ["targets.tool"]);
}

#[test]
fn adding_then_removing_a_target_leaves_no_edits() {
    let mut form = Form::new(values(TARGETS));
    form.add_target(
        "new",
        TargetValues {
            exe: "/bin/sh".into(),
            ..TargetValues::default()
        },
    )
    .unwrap();
    assert!(form.remove_target("new"));
    assert_eq!(form.changes(), 0);
    assert!(form.edits().is_empty());
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
