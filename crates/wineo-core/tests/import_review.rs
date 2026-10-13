use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::import::{Cancelled, ImportProgress, ImportRequest, copy_runner, execute};
use wineo_core::paths::Paths;

#[path = "common/test_script.rs"]
mod test_script;
use test_script::write_script;

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_script(path, "not executed by import");
}
fn paths(root: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
}
fn setup(root: &Path) -> (Paths, std::path::PathBuf) {
    let paths = paths(root);
    executable(&paths.default_runner_root().join("test-wine/bin/wine"));
    executable(&paths.default_runner_root().join("test-wine/bin/wineserver"));
    let source = root.join("original/prefix");
    fs::create_dir_all(source.join("drive_c")).unwrap();
    fs::write(
        source.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    (paths, source)
}
fn request(source: &Path) -> ImportRequest {
    ImportRequest {
        source: source.to_owned(),
        name: "copied".into(),
        runner: "test-wine".into(),
        destination: None,
        adopt: false,
        arch: None,
    }
}

#[test]
fn copying_a_managed_runner_holds_its_source_lock() {
    use wineo_core::build_lock::{Kind, try_exclusive};
    use wineo_core::heroic_import::RunnerCopy;
    use wineo_core::runners::Runners;

    let temp = tempfile::tempdir().unwrap();
    let (paths, _) = setup(temp.path());
    let runners = Runners::new(&paths, &GlobalConfig::default()).unwrap();
    let runner = runners.resolve("test-wine").unwrap();
    let source = runners.root().join(runner.name());
    let copy = RunnerCopy {
        inspection: wineo_core::import::inspect_runner_copy(&source).unwrap(),
        name: "wine-copy".to_owned(),
        source,
    };
    let guard = copy.lock_source(&paths, &GlobalConfig::default()).unwrap();
    assert!(guard.is_some());
    let mut copied = false;
    let receipt = copy_runner(
        &copy.source,
        runners.root(),
        "wine-copy",
        &Token::new(),
        |event| {
            if matches!(event, ImportProgress::Copied { .. }) {
                copied = true;
                assert!(
                    try_exclusive(&paths, Kind::Runner, runner.name())
                        .unwrap()
                        .is_none()
                );
                assert!(
                    try_exclusive(&paths, Kind::Runner, "wine-copy")
                        .unwrap()
                        .is_some()
                );
            }
        },
    )
    .unwrap();
    assert!(copied);
    assert!(receipt.path.join("bin/wine").is_file());
    drop(guard);
    assert!(
        try_exclusive(&paths, Kind::Runner, runner.name())
            .unwrap()
            .is_some()
    );
}

#[test]
fn runner_receipt_symlink_does_not_overwrite_external_data() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("runner");
    executable(&source.join("proton"));
    executable(&source.join("files/bin/wine"));
    executable(&source.join("files/bin/wineserver"));
    let sentinel = temp.path().join("external-settings");
    fs::write(&sentinel, "preserve me").unwrap();
    symlink(&sentinel, source.join(".wineo-install.json")).unwrap();
    let _ = copy_runner(
        &source,
        temp.path().join("installed"),
        "GE-Proton11-7",
        &Token::new(),
        |_| {},
    );
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "preserve me");
}

#[test]
fn relative_external_links_keep_pointing_at_original_data() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    fs::write(source.parent().unwrap().join("external-save"), "saved game").unwrap();
    symlink("../external-save", source.join("save")).unwrap();
    execute(
        &paths,
        &GlobalConfig::default(),
        request(&source),
        &Token::new(),
        |_| {},
    )
    .unwrap();
    let copied = paths.default_prefix_root().join("copied");
    assert_eq!(
        fs::read_to_string(copied.join("save")).unwrap(),
        "saved game"
    );
    assert_eq!(
        fs::canonicalize(copied.join("save")).unwrap(),
        source.parent().unwrap().join("external-save")
    );
}

#[test]
fn absolute_internal_registry_link_survives_staging_and_publication() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    fs::rename(source.join("system.reg"), source.join("registry-data")).unwrap();
    symlink(source.join("registry-data"), source.join("system.reg")).unwrap();
    execute(
        &paths,
        &GlobalConfig::default(),
        request(&source),
        &Token::new(),
        |_| {},
    )
    .unwrap();
    let copied = paths.default_prefix_root().join("copied");
    assert_eq!(
        fs::canonicalize(copied.join("system.reg")).unwrap(),
        copied.join("registry-data")
    );
}

#[test]
fn registered_missing_destination_with_unresolved_operation_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    let destination = paths.default_prefix_root().join("copied");
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("existing"),
        format!(
            "prefix = {:?}\nrunner = \"test-wine\"\narch = \"win64\"\n",
            destination.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::create_dir_all(paths.runtime_dir("existing")).unwrap();
    fs::write(
        paths.runtime_dir("existing").join("operation.toml"),
        "interrupted operation record",
    )
    .unwrap();
    assert!(
        execute(
            &paths,
            &GlobalConfig::default(),
            request(&source),
            &Token::new(),
            |_| {}
        )
        .is_err()
    );
    assert!(!destination.exists());
}

#[test]
fn new_environment_name_with_unresolved_operation_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    fs::create_dir_all(paths.runtime_dir("copied")).unwrap();
    fs::write(
        paths.runtime_dir("copied").join("operation.toml"),
        "interrupted operation record",
    )
    .unwrap();
    assert!(
        execute(
            &paths,
            &GlobalConfig::default(),
            request(&source),
            &Token::new(),
            |_| {}
        )
        .is_err()
    );
    assert!(!paths.default_prefix_root().join("copied").exists());
}

#[test]
fn explicitly_selected_wine_can_import_a_prefix_named_pfx() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    let pfx = source.parent().unwrap().join("pfx");
    fs::rename(&source, &pfx).unwrap();
    execute(
        &paths,
        &GlobalConfig::default(),
        request(&pfx),
        &Token::new(),
        |_| {},
    )
    .unwrap();
    assert!(
        paths
            .default_prefix_root()
            .join("copied/system.reg")
            .is_file()
    );
}

#[test]
fn cancellation_during_copy_removes_only_staging() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    fs::write(source.join("drive_c/save"), "saved game").unwrap();
    let canceled = Token::new();
    let result = execute(
        &paths,
        &GlobalConfig::default(),
        request(&source),
        &canceled,
        |event| {
            if matches!(event, ImportProgress::Copied { .. }) {
                canceled.cancel();
            }
        },
    );
    assert!(result.unwrap_err().is::<Cancelled>());
    assert!(!paths.environment_config("copied").exists());
    assert!(!paths.default_prefix_root().join("copied").exists());
    assert_eq!(
        fs::read(source.join("drive_c/save")).unwrap(),
        b"saved game"
    );
    assert!(
        fs::read_dir(paths.default_prefix_root())
            .unwrap()
            .all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".wineo-import-")
            })
    );
}

#[test]
fn a_source_content_change_during_copy_prevents_publication() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    let save = source.join("drive_c/save");
    fs::write(&save, "before").unwrap();
    let original_time =
        filetime::FileTime::from_last_modification_time(&fs::metadata(&save).unwrap());
    let result = execute(
        &paths,
        &GlobalConfig::default(),
        request(&source),
        &Token::new(),
        |event| {
            if matches!(event, ImportProgress::Copied { .. }) {
                fs::write(&save, "after!").unwrap();
                filetime::set_file_mtime(&save, original_time).unwrap();
            }
        },
    );
    assert!(result.unwrap_err().to_string().contains("changed"));
    assert!(!paths.environment_config("copied").exists());
    assert!(!paths.default_prefix_root().join("copied").exists());
}

#[test]
fn a_registration_collision_retains_the_complete_published_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let (paths, source) = setup(temp.path());
    fs::write(source.join("drive_c/save"), "saved game").unwrap();
    let config = paths.environment_config("copied");
    let result = execute(
        &paths,
        &GlobalConfig::default(),
        request(&source),
        &Token::new(),
        |event| {
            if matches!(event, ImportProgress::Finished { .. }) {
                fs::create_dir_all(config.parent().unwrap()).unwrap();
                fs::write(&config, "concurrent configuration").unwrap();
            }
        },
    );
    let error = result.unwrap_err().to_string();
    assert!(error.contains("--adopt"), "{error}");
    assert!(error.contains(paths.default_prefix_root().join("copied").to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "concurrent configuration"
    );
    assert_eq!(
        fs::read(paths.default_prefix_root().join("copied/drive_c/save")).unwrap(),
        b"saved game"
    );
}
