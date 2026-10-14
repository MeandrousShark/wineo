use crate::test_script::write_script;
use std::ffi::OsString;

use super::*;

fn test_paths(root: &Path) -> Paths {
    let root = root.to_owned();
    Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(root.join("home"))),
        _ => None,
    })
    .unwrap()
}

fn fake_prefix(dir: &Path, arch: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("system.reg"),
        format!("WINE REGISTRY Version 2\n\n#arch={arch}\n"),
    )
    .unwrap();
}

fn new(name: &str, dir: &Path) -> NewEnvironment {
    NewEnvironment {
        name: name.to_owned(),
        dir: Some(dir.to_str().unwrap().to_owned()),
        ..Default::default()
    }
}

#[test]
fn adopting_a_prefix_writes_config_with_its_arch() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/Apps/Thing");
    fake_prefix(&prefix, "win64");

    let env = Environment::create(
        &paths,
        &GlobalConfig::default(),
        &Runner::fake("wine"),
        &new("thing", &prefix),
        &Token::new(),
        Output::Log(&paths.runtime_dir("thing").join("create.log")),
        |_| {},
    )
    .unwrap();
    let wine = env.config.wine().unwrap();
    assert_eq!(wine.prefix, "~/Apps/Thing");
    assert_eq!(wine.runner, "wine");
    assert_eq!(wine.arch, Arch::Win64);
    assert_eq!(
        fs::read_to_string(paths.environment_config("thing")).unwrap(),
        "schema_version = 1\nprefix = \"~/Apps/Thing\"\nrunner = \"wine\"\narch = \"win64\"\n"
    );
    assert_eq!(env.prefix(&paths).unwrap(), prefix);
    assert!(paths.runtime_dir("thing").join("create.log").exists());

    assert!(matches!(
        Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            &new("thing", &prefix),
            &Token::new(),
            Output::Log(&paths.runtime_dir("thing").join("create.log")),
            |_| {},
        ),
        Err(Error::EnvironmentExists(_))
    ));
}

#[test]
fn adopting_with_a_conflicting_arch_fails() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("old");
    fake_prefix(&prefix, "win32");

    let mut options = new("old", &prefix);
    options.arch = Some(Arch::Win64);
    assert!(matches!(
        Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            &options,
            &Token::new(),
            Output::Log(&paths.runtime_dir("old").join("create.log")),
            |_| {},
        ),
        Err(Error::ArchMismatch { .. })
    ));
    assert!(!paths.environment_config("old").exists());
}

#[test]
fn record_heroic_id_preserves_the_rest_of_the_file() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "# hand written\nprefix = \"~/Games/Game\"\nrunner = \"wine\"\narch = \"win64\"\n# keep this\ngamemode = true\n";
    fs::write(&path, original).unwrap();

    let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();
    Environment::record_heroic_id(&paths, "game", "abc123", &snapshot).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        format!("{original}heroic_id = \"abc123\"\nschema_version = 1\n")
    );
    let config = EnvironmentConfig::parse(&path, &text).unwrap();
    assert_eq!(config.heroic_id.as_deref(), Some("abc123"));

    Environment::record_heroic_id(&paths, "game", "abc123", &snapshot).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("heroic_id").count(), 1, "{text}");
    assert!(text.contains("heroic_id = \"abc123\"\n"), "{text}");
}

#[test]
fn record_heroic_id_refuses_a_concurrent_id_or_config_edit() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "prefix = \"/games/one\"\nrunner = \"wine\"\narch = \"win64\"\n";
    fs::write(&path, original).unwrap();
    let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();

    let changed = original.replace("/games/one", "/games/two");
    fs::write(&path, &changed).unwrap();
    assert!(matches!(
        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
        Err(Error::ConfigInvalid { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), changed);

    let changed_id = format!("{original}heroic_id = \"other\"\n");
    fs::write(&path, &changed_id).unwrap();
    assert!(matches!(
        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
        Err(Error::ConfigInvalid { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), changed_id);
}

#[test]
fn record_heroic_id_accepts_a_concurrent_backfill_of_the_same_id() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "prefix = \"/games/one\"\nrunner = \"wine\"\narch = \"win64\"\n",
    )
    .unwrap();
    let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();
    let concurrent = "# concurrent edit\nprefix = \"/games/two\"\nrunner = \"wine\"\narch = \"win64\"\nheroic_id = \"abc123\"\n";
    fs::write(&path, concurrent).unwrap();
    Environment::record_heroic_id(&paths, "game", "abc123", &snapshot).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), concurrent);
}

#[test]
fn record_heroic_id_refuses_removed_or_replaced_config() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "prefix = \"/games/one\"\nrunner = \"wine\"\narch = \"win64\"\n";
    fs::write(&path, original).unwrap();
    let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, original).unwrap();

    fs::remove_file(&path).unwrap();
    assert!(matches!(
        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
        Err(Error::UnknownEnvironment(_))
    ));
    assert!(!path.exists());

    fs::rename(&replacement, &path).unwrap();
    assert!(matches!(
        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
        Err(Error::ConfigInvalid { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn loading_a_legacy_config_leaves_it_alone() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "# mine\nprefix = \"~/Games/Game\"\nrunner = \"wine\"\narch = \"win64\"\n";
    fs::write(&path, original).unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        Environment::load(&paths, "game")
            .unwrap()
            .config
            .schema_version,
        0
    );
    Environment::load_for_heroic_match(&paths, "game").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn heroic_matching_refuses_an_unsupported_version() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original =
        "schema_version = 2\nprefix = \"~/Games/Game\"\nrunner = \"wine\"\narch = \"win64\"\n";
    fs::write(&path, original).unwrap();
    assert!(matches!(
        Environment::load_for_heroic_match(&paths, "game"),
        Err(Error::SchemaVersion { .. })
    ));
    assert!(matches!(
        Environment::load(&paths, "game"),
        Err(Error::SchemaVersion { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn record_heroic_id_on_unknown_environment_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let path = paths.environment_config("nope");
    let snapshot = HeroicIdSnapshot {
        text: String::new(),
        device: 0,
        inode: 0,
    };
    assert!(matches!(
        Environment::record_heroic_id(&paths, "nope", "abc123", &snapshot),
        Err(Error::UnknownEnvironment(_))
    ));
    assert!(!path.exists());
}

#[test]
fn non_empty_non_prefix_dirs_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let dir = root.path().join("stuff");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("notes.txt"), "keep me").unwrap();

    assert!(matches!(
        Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            &new("stuff", &dir),
            &Token::new(),
            Output::Log(&paths.runtime_dir("stuff").join("create.log")),
            |_| {},
        ),
        Err(Error::NotAPrefix(_))
    ));
    assert!(dir.join("notes.txt").exists());
}

fn hand_written(paths: &Paths, name: &str, prefix: &Path) -> (Environment, String) {
    let path = paths.environment_config(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = format!(
        "# mine\nprefix = \"{}\"\nrunner = \"wine\"\narch = \"win32\"\n",
        prefix.display()
    );
    fs::write(&path, &text).unwrap();
    (Environment::load(paths, name).unwrap(), text)
}

#[test]
fn a_relative_prefix_is_refused_outside_portable_mode() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let (env, _) = hand_written(&paths, "game", Path::new("[PREFIX-DIR]"));
    assert!(matches!(env.prefix(&paths), Err(Error::RelativePrefix(_))));
    let (env, _) = hand_written(&paths, "home", Path::new("~/Games/Game"));
    assert_eq!(
        env.prefix(&paths).unwrap(),
        root.path().join("home/Games/Game")
    );
}

fn native(paths: &Paths, name: &str) -> Environment {
    let path = paths.environment_config(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "kind = \"native\"\n[targets.game]\nexe = \"/usr/bin/dusklight\"\n",
    )
    .unwrap();
    Environment::load(paths, name).unwrap()
}

#[test]
fn a_native_environment_has_no_prefix() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let env = native(&paths, "dusklight");
    let err = env.prefix(&paths).unwrap_err();
    assert!(matches!(err, Error::NoPrefix(_)));
    assert_eq!(
        err.to_string(),
        "dusklight is a native environment, so it has no prefix"
    );
    assert!(matches!(env.wine(), Err(Error::NoPrefix(_))));
    assert!(matches!(
        env.build_prefix(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("dusklight").join("create.log")),
            |_| panic!("nothing to build"),
        ),
        Err(Error::NoPrefix(_))
    ));
}

#[test]
fn waiting_for_a_native_environment_to_go_idle_returns_at_once() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let env = native(&paths, "dusklight");
    env.wait_idle(&paths, &GlobalConfig::default(), || {
        panic!("no wine to wait for")
    })
    .unwrap();
}

#[test]
fn a_native_environment_shares_no_prefix() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/Games/game");
    hand_written(&paths, "game", &prefix);
    native(&paths, "dusklight");
    assert!(sharing_prefix(&paths, "other", &prefix).unwrap().names == ["game"]);
    assert!(sharing_prefix(&paths, "game", &prefix).unwrap().is_empty());
}

/// Stands in for wine, which the fake runner doesn't run, by writing the
/// registry file `prefix::create` checks for.
fn wine_builds_it(dir: &Path) {
    fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
}

#[test]
fn build_prefix_builds_once_and_never_writes_the_config() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let global = GlobalConfig::default();
    let prefix = global.prefix_root(&paths).unwrap().join("palworld");
    let (env, text) = hand_written(&paths, "palworld", &prefix);
    let config = paths.environment_config("palworld");
    let before = fs::metadata(&config).unwrap().modified().unwrap();

    let mut announced = None;
    let built = env
        .build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |event| {
                if let PrefixEvent::Started { dir } = event {
                    announced = Some(dir.clone());
                    wine_builds_it(&dir);
                }
            },
        )
        .unwrap();
    assert!(built);
    assert_eq!(announced.as_deref(), Some(prefix.as_path()));
    assert!(prefix::is_prefix(&prefix));
    assert!(paths.runtime_dir("palworld").join("create.log").exists());
    assert_eq!(fs::read_to_string(&config).unwrap(), text);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), before);

    let built = env
        .build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |_| panic!("already built"),
        )
        .unwrap();
    assert!(!built);
    assert_eq!(fs::read_to_string(&config).unwrap(), text);
}

#[test]
fn failed_build_removes_a_dir_it_made() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let global = GlobalConfig::default();
    let prefix = global.prefix_root(&paths).unwrap().join("palworld");
    let (env, text) = hand_written(&paths, "palworld", &prefix);

    let err = env
        .build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |_| {},
        )
        .unwrap_err();
    assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
    assert!(!prefix.exists());
    assert_eq!(
        fs::read_to_string(paths.environment_config("palworld")).unwrap(),
        text
    );
}

#[test]
fn failed_build_keeps_an_empty_dir_it_found() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let global = GlobalConfig::default();
    let prefix = global.prefix_root(&paths).unwrap().join("palworld");
    fs::create_dir_all(&prefix).unwrap();
    let (env, _) = hand_written(&paths, "palworld", &prefix);

    let err = env
        .build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |_| {},
        )
        .unwrap_err();
    assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
    assert!(prefix.is_dir());
}

#[test]
fn build_prefix_refuses_before_touching_anything() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let global = GlobalConfig::default();
    let prefix = root.path().join("mnt/game-ssd");
    fs::create_dir_all(&prefix).unwrap();
    let (env, _) = hand_written(&paths, "palworld", &prefix);

    assert!(matches!(
        env.build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |_| { panic!("refused") }
        ),
        Err(Error::EmptyPrefix(_))
    ));
    assert!(!paths.runtime_dir("palworld").exists());

    let built = env
        .build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            true,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |event| {
                if let PrefixEvent::Started { dir } = event {
                    wine_builds_it(&dir);
                }
            },
        )
        .unwrap();
    assert!(built);
    assert!(prefix::is_prefix(&prefix));
}

#[test]
fn build_prefix_reports_a_busy_environment() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let global = GlobalConfig::default();
    let prefix = global.prefix_root(&paths).unwrap().join("palworld");
    let (env, _) = hand_written(&paths, "palworld", &prefix);

    let _held = Locks::new(&paths)
        .acquire(Operation::Create, "palworld", &[&prefix])
        .unwrap();
    assert!(matches!(
        env.build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |_| { panic!("busy") }
        ),
        Err(Error::Busy { .. })
    ));
    assert!(!prefix.exists());
}

#[test]
fn build_prefix_refuses_a_removed_environment() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let global = GlobalConfig::default();
    let prefix = global.prefix_root(&paths).unwrap().join("palworld");
    let (env, _) = hand_written(&paths, "palworld", &prefix);
    fs::remove_file(paths.environment_config("palworld")).unwrap();

    assert!(matches!(
        env.build_prefix(
            &paths,
            &global,
            &Runner::fake("wine"),
            false,
            &Token::new(),
            Output::Log(&paths.runtime_dir("palworld").join("create.log")),
            |_| { panic!("removed") }
        ),
        Err(Error::UnknownEnvironment(_))
    ));
    assert!(!prefix.exists());
}

#[test]
fn failed_create_removes_the_dir_it_made() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/prefixes/fails");

    let err = Environment::create(
        &paths,
        &GlobalConfig::default(),
        &Runner::fake("wine"),
        &new("fails", &prefix),
        &Token::new(),
        Output::Log(&paths.runtime_dir("fails").join("create.log")),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
    assert!(!prefix.exists());
    assert!(!paths.environment_config("fails").exists());
}

#[test]
fn canceling_prefix_creation_from_another_thread_removes_partial_state() {
    use std::thread;
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/prefixes/canceled");
    let marker = prefix.join("partial");
    let runner_path = root.path().join("fake-wine");
    write_script(
        &runner_path,
        "#!/bin/sh\ncase \"$1\" in\n  wineboot)\n    printf partial > \"$WINEPREFIX/partial\"\n    trap 'exit 0' TERM\n    while :; do sleep 1; done\n    ;;\n  reg|-w|-k) exit 0 ;;\nesac\nexit 0\n",
    );
    let runner = Runner::wine_build("fake".to_owned(), runner_path.clone(), runner_path);
    let token = Token::new();
    let worker_token = token.clone();
    let new = new("canceled", &prefix);
    let worker_paths = paths.clone();
    let worker_runner = runner.clone();
    let worker = thread::spawn(move || {
        Environment::create(
            &worker_paths,
            &GlobalConfig::default(),
            &worker_runner,
            &new,
            &worker_token,
            Output::Log(&worker_paths.runtime_dir("canceled").join("create.log")),
            |_| {},
        )
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "fake wineboot did not start");
    token.cancel();

    let error = worker.join().unwrap().unwrap_err();
    assert!(
        matches!(error, Error::Canceled("prefix creation")),
        "{error}"
    );
    assert!(!prefix.exists());
    assert!(!paths.environment_config("canceled").exists());
}

#[test]
fn empty_dir_keeps_going_after_a_failed_entry() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let stuck = root.path().join("stuck");
    fs::create_dir(&stuck).unwrap();
    fs::write(stuck.join("inner"), "x").unwrap();
    fs::set_permissions(&stuck, fs::Permissions::from_mode(0o500)).unwrap();
    for name in ["a", "b", "c"] {
        fs::write(root.path().join(name), "x").unwrap();
    }

    let result = empty_dir(root.path());
    fs::set_permissions(&stuck, fs::Permissions::from_mode(0o700)).unwrap();
    // Running as root removes the stuck directory too.
    if result.is_ok() {
        return;
    }
    let left: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, ["stuck"]);
}

#[test]
fn canceling_into_an_existing_empty_dir_empties_it_and_keeps_it() {
    use std::thread;
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/prefixes/existing");
    fs::create_dir_all(&prefix).unwrap();
    let outside = root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    let marker = prefix.join("partial");
    let runner_path = root.path().join("fake-wine");
    // Leaves a subdirectory and a symlink out of the prefix behind.
    write_script(
        &runner_path,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  wineboot)\n    mkdir -p \"$WINEPREFIX/sub/deeper\"\n    ln -s '{}' \"$WINEPREFIX/link\"\n    printf partial > \"$WINEPREFIX/partial\"\n    trap 'exit 0' TERM\n    while :; do sleep 1; done\n    ;;\n  reg|-w|-k) exit 0 ;;\nesac\nexit 0\n",
            outside.display()
        ),
    );
    let runner = Runner::wine_build("fake".to_owned(), runner_path.clone(), runner_path);
    let token = Token::new();
    let worker_token = token.clone();
    let new = new("existing", &prefix);
    let worker_paths = paths.clone();
    let worker_runner = runner.clone();
    let worker = thread::spawn(move || {
        Environment::create(
            &worker_paths,
            &GlobalConfig::default(),
            &worker_runner,
            &new,
            &worker_token,
            Output::Log(&worker_paths.runtime_dir("existing").join("create.log")),
            |_| {},
        )
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "fake wineboot did not start");
    token.cancel();

    let error = worker.join().unwrap().unwrap_err();
    assert!(
        matches!(error, Error::Canceled("prefix creation")),
        "{error}"
    );
    assert!(prefix.is_dir());
    assert_eq!(fs::read_dir(&prefix).unwrap().count(), 0);
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
    assert!(!paths.environment_config("existing").exists());
}

#[test]
fn names_lists_toml_files_with_valid_names() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    assert!(names(&paths).unwrap().is_empty());

    let dir = paths.environments_dir();
    fs::create_dir_all(dir.join("dir.toml")).unwrap();
    for file in ["zed.toml", "broken.toml", "notes.txt", "bad name.toml"] {
        fs::write(dir.join(file), "not even toml").unwrap();
    }
    assert_eq!(names(&paths).unwrap(), ["broken", "zed"]);

    assert!(Environment::ensure_exists(&paths, "broken").is_ok());
    assert!(matches!(
        Environment::ensure_exists(&paths, "dir"),
        Err(Error::UnknownEnvironment(_))
    ));
    assert!(matches!(
        Environment::ensure_exists(&paths, "../zed"),
        Err(Error::InvalidName(_))
    ));
}

#[test]
fn load_reports_missing_and_invalid_names() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    assert!(matches!(
        Environment::load(&paths, "nope"),
        Err(Error::UnknownEnvironment(_))
    ));
    assert!(matches!(
        Environment::load(&paths, "../escape"),
        Err(Error::InvalidName(_))
    ));
}

#[test]
#[ignore = "runs wine"]
fn creates_a_real_prefix() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/prefixes/real");

    let env = Environment::create(
        &paths,
        &GlobalConfig::default(),
        &Runner::system().unwrap(),
        &new("real", &prefix),
        &Token::new(),
        Output::Log(&paths.runtime_dir("real").join("create.log")),
        |_| {},
    )
    .unwrap();
    assert!(prefix::is_prefix(&prefix));
    assert_eq!(env.config.wine().unwrap().arch, Arch::Win64);
}

#[test]
#[ignore = "runs wine"]
fn failed_creation_removes_the_new_dir() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let prefix = root.path().join("home/prefixes/win32");

    let mut options = new("win32", &prefix);
    options.arch = Some(Arch::Win32);
    let err = Environment::create(
        &paths,
        &GlobalConfig::default(),
        &Runner::system().unwrap(),
        &options,
        &Token::new(),
        Output::Log(&paths.runtime_dir("win32").join("create.log")),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
    assert!(!prefix.exists());
    assert!(!paths.environment_config("win32").exists());
}
