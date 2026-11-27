//! An Epic game's main program comes from Epic, so the settings page can't
//! save a new one; everything else about the target still saves.

use super::*;

const EPIC: &str = "schema_version = 1\nepic_id = \"AppOffline\"\nprefix = \"~/prefixes/game\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.game]\nexe = \"/games/Game.exe\"\n";

fn edits(key: &str, value: &str) -> form::Edits {
    form::Edits {
        pairs: vec![(key.to_owned(), value.to_owned())],
        unset: Vec::new(),
    }
}

#[test]
fn an_epic_games_main_program_cant_be_saved_but_its_arguments_can() {
    let (_root, paths) = setup();
    write(&paths, EPIC);
    let config = paths.environment_config("game");
    assert_eq!(
        load::save(
            &paths,
            "game",
            &edits("targets.game.exe", "/games/Other.exe")
        )
        .err(),
        Some("game's main program comes from Epic and can't be changed.".to_owned())
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), EPIC);
    assert!(load::save(&paths, "game", &edits("targets.game.args", "[\"-x\"]")).is_ok());
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("args = [\"-x\"]")
    );
}
