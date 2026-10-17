use std::ffi::OsString;
use std::fs;

use super::*;
use crate::art::ArtConfig;
use crate::templates::Templates;

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    (root, paths)
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const ENV: &str = "schema_version = 1\nprefix = '/p'\nrunner = 'wine'\narch = 'win64'\n";

fn moved(old: &str, new: &str) -> Moved {
    Moved {
        old: old.to_owned(),
        new: new.to_owned(),
        since: "0.0.0",
    }
}

#[test]
fn every_entry_renames_only_its_last_part() {
    for entry in TABLE {
        let (old_parent, _) = entry.old.rsplit_once('.').unwrap_or(("", entry.old));
        let (new_parent, _) = entry.new.rsplit_once('.').unwrap_or(("", entry.new));
        assert_eq!(old_parent, new_parent, "{}", entry.old);
        assert_ne!(entry.old_leaf(), entry.new_leaf());
        assert!(!entry.old_leaf().contains('*'), "{}", entry.old);
    }
}

#[test]
fn an_old_key_loads_as_the_new_one_and_is_recorded() {
    let path = Path::new("/env/hades.toml");
    let config =
        EnvironmentConfig::parse(path, &format!("{ENV}test_old_gamemode = true\n")).unwrap();
    assert_eq!(config.gamemode, Some(true));
    assert_eq!(config.deprecated, [moved("test_old_gamemode", "gamemode")]);
}

#[test]
fn a_new_key_records_nothing() {
    let config = EnvironmentConfig::parse(
        Path::new("/env/hades.toml"),
        &format!("{ENV}gamemode = true\n"),
    )
    .unwrap();
    assert_eq!(config.gamemode, Some(true));
    assert!(config.deprecated.is_empty());
}

#[test]
fn both_spellings_at_once_name_both_keys_and_the_file() {
    let err = EnvironmentConfig::parse(
        Path::new("/env/hades.toml"),
        &format!("{ENV}gamemode = false\ntest_old_gamemode = true\n"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::ConfigInvalid { .. }), "{err:?}");
    let message = err.to_string();
    assert!(message.contains("/env/hades.toml"), "{message}");
    assert!(message.contains("`test_old_gamemode`"), "{message}");
    assert!(message.contains("`gamemode`"), "{message}");
}

#[test]
fn a_key_in_a_nested_table_is_moved() {
    let global = GlobalConfig::parse(
        Path::new("/config.toml"),
        "[defaults]\ntest_old_runner = 'ge'\n",
    )
    .unwrap();
    assert_eq!(global.defaults.runner, "ge");
    assert_eq!(
        global.deprecated,
        [moved("defaults.test_old_runner", "defaults.runner")]
    );

    let config = EnvironmentConfig::parse(
        Path::new("/env/hades.toml"),
        &format!("{ENV}hooks = {{ test_old_before = 'echo hi' }}\n"),
    )
    .unwrap();
    assert_eq!(
        config
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.on()?.before.as_deref()),
        Some("echo hi")
    );
    assert_eq!(
        config.deprecated,
        [moved("hooks.test_old_before", "hooks.before")]
    );
}

#[test]
fn a_key_in_every_target_is_moved() {
    let config = EnvironmentConfig::parse(
        Path::new("/env/hades.toml"),
        &format!(
            "{ENV}[targets.game]\ntest_old_exe = 'C:/game.exe'\n[targets.tool]\nexe = 'C:/tool.exe'\n[targets.editor]\ntest_old_exe = 'C:/editor.exe'\n"
        ),
    )
    .unwrap();
    assert_eq!(config.targets["game"].exe, "C:/game.exe");
    assert_eq!(config.targets["tool"].exe, "C:/tool.exe");
    assert_eq!(config.targets["editor"].exe, "C:/editor.exe");
    let mut found = config.deprecated.clone();
    found.sort_by(|a, b| a.old.cmp(&b.old));
    assert_eq!(
        found,
        [
            moved("targets.editor.test_old_exe", "targets.editor.exe"),
            moved("targets.game.test_old_exe", "targets.game.exe"),
        ]
    );

    let err = EnvironmentConfig::parse(
        Path::new("/env/hades.toml"),
        &format!("{ENV}[targets.game]\nexe = 'a'\ntest_old_exe = 'b'\n"),
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("`targets.game.test_old_exe`"), "{message}");
    assert!(message.contains("`targets.game.exe`"), "{message}");
}

#[test]
fn a_template_key_is_moved() {
    let (_root, paths) = setup();
    write(
        &paths.templates_dir().join("coop.toml"),
        "schema_version = 1\ntest_old_tags = ['game']\n",
    );
    let template = Templates::new(&paths).resolve("coop").unwrap();
    assert_eq!(template.tags, ["game"]);
    assert_eq!(template.deprecated, [moved("test_old_tags", "tags")]);

    write(
        &paths.templates_dir().join("both.toml"),
        "tags = ['a']\ntest_old_tags = ['b']\n",
    );
    let err = Templates::new(&paths).resolve("both").unwrap_err();
    assert!(err.to_string().contains("`test_old_tags`"), "{err}");
}

#[test]
fn an_art_key_is_moved() {
    let art = ArtConfig::parse(
        Path::new("/art/hades/art.toml"),
        "test_old_game = 5\n[targets.main.test_old_icon]\nfile = 'icon.png'\nmanual = 'a.png'\n",
    )
    .unwrap();
    assert_eq!(art.game, Some(5));
    assert!(art.record(Some("main"), crate::art::Kind::Icon).is_some());
    let mut found = art.deprecated.clone();
    found.sort_by(|a, b| a.old.cmp(&b.old));
    assert_eq!(
        found,
        [
            moved("targets.main.test_old_icon", "targets.main.icon"),
            moved("test_old_game", "game"),
        ]
    );
}

#[test]
fn reading_never_rewrites_the_file() {
    let (_root, paths) = setup();
    let path = paths.environment_config("hades");
    let text = format!("# mine\n{ENV}test_old_gamemode = true\n");
    write(&path, &text);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    let environment = crate::environment::Environment::load(&paths, "hades").unwrap();
    assert_eq!(environment.config.gamemode, Some(true));
    assert_eq!(count_configs(&paths), 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn an_unknown_key_keeps_its_line_and_column() {
    let err = EnvironmentConfig::parse(
        Path::new("/env/hades.toml"),
        &format!("{ENV}gamemod = true\n"),
    )
    .unwrap_err();
    assert!(err.to_string().contains("line 5"), "{err}");
}

#[test]
fn rename_keeps_comments_formatting_and_order() {
    let text = "# top\nschema_version = 1\nprefix = '/p'\n# the switch\ntest_old_gamemode   = true # on\nrunner = 'wine'\narch = 'win64'\n\n[targets.game]\nname = 'Game'\n\"test_old_exe\" = 'C:/game.exe' # here\ndefault = true\n";
    let mut doc: DocumentMut = text.parse().unwrap();
    let renamed = rename_document(FileKind::Environment, Path::new("/e.toml"), &mut doc).unwrap();
    assert_eq!(
        doc.to_string(),
        "# top\nschema_version = 1\nprefix = '/p'\n# the switch\ngamemode   = true # on\nrunner = 'wine'\narch = 'win64'\n\n[targets.game]\nname = 'Game'\nexe = 'C:/game.exe' # here\ndefault = true\n"
    );
    restore_document(&mut doc, &renamed);
    assert_eq!(doc.to_string(), text);
}

#[test]
fn rename_works_in_inline_and_dotted_tables() {
    let text = "targets.game.test_old_exe = 'a'\ntargets.game.args = []\nhooks = { test_old_before = 'x', after = 'y' }\n";
    let mut doc: DocumentMut = text.parse().unwrap();
    let renamed = rename_document(FileKind::Environment, Path::new("/e.toml"), &mut doc).unwrap();
    assert_eq!(
        doc.to_string(),
        "targets.game.exe = 'a'\ntargets.game.args = []\nhooks = { before = 'x', after = 'y' }\n"
    );
    restore_document(&mut doc, &renamed);
    assert_eq!(doc.to_string(), text);
}

#[test]
fn rename_in_place_renames_and_stamps_a_legacy_file() {
    let (_root, paths) = setup();
    let path = paths.environment_config("hades");
    write(
        &path,
        "# mine\nprefix = '/p'\ntest_old_gamemode = true # keep\nrunner = 'wine'\narch = 'win64'\n",
    );
    let config = Config::Environment("hades".into());
    assert!(rename_in_place(&paths, &config).unwrap());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# mine\nprefix = '/p'\ngamemode = true # keep\nrunner = 'wine'\narch = 'win64'\nschema_version = 1\n"
    );
    assert!(!rename_in_place(&paths, &config).unwrap());
}

#[test]
fn current_name_maps_old_spellings_only() {
    let kind = FileKind::Environment;
    assert_eq!(current_name(kind, "test_old_gamemode"), "gamemode");
    assert_eq!(current_name(kind, "gamemode"), "gamemode");
    assert_eq!(
        current_name(kind, "targets.game.test_old_exe"),
        "targets.game.exe"
    );
    assert_eq!(current_name(kind, "hooks.test_old_before"), "hooks.before");
    assert_eq!(
        current_name(kind, "targets.test_old_exe"),
        "targets.test_old_exe"
    );
    assert_eq!(
        current_name(FileKind::Global, "test_old_gamemode"),
        "test_old_gamemode"
    );
}

#[test]
fn the_summary_counts_configs_not_keys() {
    let (_root, paths) = setup();
    assert_eq!(count_configs(&paths), 0);
    assert_eq!(summary(0), None);
    write(
        &paths.environment_config("hades"),
        &format!("{ENV}test_old_gamemode = true\n[targets.a]\ntest_old_exe = 'a'\n"),
    );
    write(&paths.environment_config("tool"), ENV);
    assert_eq!(count_configs(&paths), 1);
    assert_eq!(
        summary(1).unwrap(),
        "1 config uses deprecated keys; see wineo validate"
    );
    write(&paths.global_config(), "test_old_stop_timeout = 3\n");
    write(
        &paths.templates_dir().join("coop.toml"),
        "test_old_tags = ['x']\n",
    );
    write(
        &paths.art_dir("tool").join("art.toml"),
        "test_old_game = 5\n",
    );
    // Unreadable files are validate's to report.
    write(&paths.environment_config("broken"), "nope = ");
    assert_eq!(count_configs(&paths), 4);
    assert_eq!(
        summary(4).unwrap(),
        "4 configs use deprecated keys; see wineo validate"
    );
}
