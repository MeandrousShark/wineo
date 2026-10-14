use crate::test_script::write_script;
use std::ffi::OsString;
use std::os::unix::fs::symlink;

use super::*;
use crate::components::Component;
use crate::config::Defaults;

// The unreviewed forms, which only tests call.
fn execute(
    paths: &Paths,
    global: &GlobalConfig,
    request: ImportRequest,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    execute_reviewed(paths, global, request, None, None, cancel, progress)
}

fn copy_runner(
    source: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
    name: &str,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    copy_runner_with_review(
        source.as_ref(),
        destination_root.as_ref(),
        name,
        None,
        cancel,
        progress,
    )
}

fn paths(root: &Path) -> Paths {
    let root = root.to_owned();
    Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(root.join("home"))),
        _ => None,
    })
    .unwrap()
}

#[test]
fn a_failed_copy_removes_staging_that_holds_read_only_directories() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir_all(source.join("a-locked")).unwrap();
    fs::write(source.join("a-locked/file"), "data").unwrap();
    fs::set_permissions(source.join("a-locked"), fs::Permissions::from_mode(0o555)).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        source.join("z-fifo"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let parent = root.path().join("prefixes");
    fs::create_dir(&parent).unwrap();
    let staging = Staging::new_in(".wineo-clone-", &parent).unwrap();
    let staged = staging.path().join("prefix");
    assert!(
        copy_tree(
            &source,
            &staged,
            &parent.join("game"),
            &Token::new(),
            &mut |_| {}
        )
        .is_err()
    );
    assert_eq!(
        fs::metadata(staged.join("a-locked"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    drop(staging);
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    fs::set_permissions(source.join("a-locked"), fs::Permissions::from_mode(0o755)).unwrap();
}
#[test]
fn imported_targets_inside_a_portable_folder_are_stored_relative() {
    let paths = Paths::select(
        |key| (key == "HOME").then(|| OsString::from("/home/me")),
        Some(Path::new("/mnt/usb/wineo")),
    )
    .unwrap();
    let target = |exe: &str, cwd: &str| Target {
        exe: exe.to_owned(),
        args: Vec::new(),
        cwd: Some(cwd.to_owned()),
        default: false,
        gamemode: None,
        wayland: None,
        mangohud: None,
        gamescope: None,
        hooks: None,
        desktop: false,
        name: None,
    };
    let settings = ImportedSettings {
        targets: BTreeMap::from([
            (
                "inside".to_owned(),
                target(
                    "/mnt/usb/wineo/prefixes/game/drive_c/game.exe",
                    "/mnt/usb/wineo/prefixes/game/drive_c",
                ),
            ),
            (
                "outside".to_owned(),
                target("/home/me/Games/game.exe", "C:\\Games"),
            ),
        ]),
        ..ImportedSettings::default()
    };
    let stored = stored_settings(&paths, &settings).unwrap();
    let inside = &stored.targets["inside"];
    assert_eq!(inside.exe, "prefixes/game/drive_c/game.exe");
    assert_eq!(inside.cwd.as_deref(), Some("prefixes/game/drive_c"));
    let outside = &stored.targets["outside"];
    assert_eq!(outside.exe, "/home/me/Games/game.exe");
    assert_eq!(outside.cwd.as_deref(), Some("C:\\Games"));
}

#[test]
fn relative_paths_are_taken_from_where_the_link_sits() {
    let cases = [
        ("/usb/prefixes/game", "/usb/prefixes/game", "."),
        (
            "/usb/prefixes/game/drive_c/x",
            "/usb/prefixes/game/drive_c",
            "x",
        ),
        (
            "/usb/prefixes/game/drive_c",
            "/usb/prefixes/game/pfx/a",
            "../../drive_c",
        ),
        (
            "/usb/prefixes/game/drive_c/x",
            "/usb/prefixes/.wineo-snapshots/game/one/prefix",
            "../../../../game/drive_c/x",
        ),
    ];
    for (target, base, expected) in cases {
        assert_eq!(
            relative_path(Path::new(target), Path::new(base)),
            Path::new(expected),
            "{target} from {base}"
        );
    }
}

fn prefix(root: &Path, arch: &str) {
    fs::create_dir_all(root.join("drive_c")).unwrap();
    fs::write(
        root.join("system.reg"),
        format!("WINE REGISTRY Version 2\n\n#arch={arch}\n"),
    )
    .unwrap();
}

fn wine_runner(root: &Path, name: &str) {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("bin")).unwrap();
    for executable in ["wine", "wineserver"] {
        let path = dir.join("bin").join(executable);
        write_script(&path, "#!/bin/sh\n");
    }
}

fn proton_runner(root: &Path, name: &str) {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("files/bin")).unwrap();
    let proton = dir.join("proton");
    write_script(&proton, "#!/bin/sh\n");
    for executable in ["wine", "wineserver"] {
        let path = dir.join("files/bin").join(executable);
        write_script(&path, "#!/bin/sh\n");
    }
    fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
}

fn base_global(runner_root: &Path, prefix_root: &Path) -> GlobalConfig {
    GlobalConfig {
        defaults: Defaults {
            runner: "wine-build".to_owned(),
            runner_root: Some(runner_root.to_string_lossy().into_owned()),
            prefix_root: Some(prefix_root.to_string_lossy().into_owned()),
            ..Defaults::default()
        },
        ..GlobalConfig::default()
    }
}

fn component_build(root: &Path, component: Component, name: &str) {
    let (wide, narrow) = component.arch_dirs();
    let marker = match component {
        Component::Dxvk => "dxgi.dll",
        Component::Vkd3d => "d3d12.dll",
    };
    for arch in [wide, narrow] {
        let dir = root.join(name).join(arch);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(marker), "build").unwrap();
    }
}

fn import_fails_when_reviewed_component_disappears(component: Component) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    let dxvk_root = root.path().join("dxvk");
    wine_runner(&runner_root, "wine-build");
    let source = root.path().join("source");
    prefix(&source, "win64");
    let mut global = base_global(&runner_root, &prefix_root);
    global.defaults.dxvk_root = Some(dxvk_root.to_string_lossy().into_owned());

    let dxvk = "dxvk-2.7";
    let vkd3d = "vkd3d-proton-2.14";
    component_build(&dxvk_root, Component::Dxvk, dxvk);
    let removed = match component {
        Component::Dxvk => dxvk_root.join(dxvk),
        Component::Vkd3d => {
            let vkd3d_root = root.path().join("vkd3d");
            global.defaults.vkd3d_root = Some(vkd3d_root.to_string_lossy().into_owned());
            component_build(&vkd3d_root, Component::Vkd3d, vkd3d);
            vkd3d_root.join(vkd3d)
        }
    };

    let runners = Runners::new(&paths, &global).unwrap();
    let runner = runners.resolve("wine-build").unwrap();
    let reviewed = inspect(&source, Some(&runner), None).unwrap();
    let settings = ImportedSettings {
        dxvk: Some(dxvk.to_owned()),
        vkd3d: (component == Component::Vkd3d).then(|| vkd3d.to_owned()),
        ..ImportedSettings::default()
    };
    fs::remove_dir_all(removed).unwrap();

    let error = execute_reviewed(
        &paths,
        &global,
        ImportRequest {
            source,
            name: "copied".to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        Some(&reviewed),
        Some(&settings),
        &Token::new(),
        |_| {},
    )
    .unwrap_err()
    .to_string();

    let component_name = match component {
        Component::Dxvk => dxvk,
        Component::Vkd3d => vkd3d,
    };
    let build_root = match component {
        Component::Dxvk => dxvk_root,
        Component::Vkd3d => root.path().join("vkd3d"),
    };
    assert_eq!(
        error,
        format!(
            "prefix {} is ready, but registration failed: {} build `{component_name}` is not installed, and nothing is installed in {}\nGet one with: wineo download {}\nThen retry this import with --adopt.",
            prefix_root.join("copied").display(),
            component.as_str(),
            build_root.display(),
            component.as_str(),
        )
    );
    assert!(prefix_root.join("copied/system.reg").is_file());
    assert!(!paths.environment_config("copied").exists());
}

#[test]
fn reviewed_dxvk_disappearance_does_not_register_the_import() {
    import_fails_when_reviewed_component_disappears(Component::Dxvk);
}

#[test]
fn reviewed_vkd3d_disappearance_does_not_register_the_import() {
    import_fails_when_reviewed_component_disappears(Component::Vkd3d);
}

#[test]
fn reviewed_import_registers_installed_dxvk_and_vkd3d_builds() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    let dxvk_root = root.path().join("dxvk");
    let vkd3d_root = root.path().join("vkd3d");
    wine_runner(&runner_root, "wine-build");
    let source = root.path().join("source");
    prefix(&source, "win64");
    let mut global = base_global(&runner_root, &prefix_root);
    global.defaults.dxvk_root = Some(dxvk_root.to_string_lossy().into_owned());
    global.defaults.vkd3d_root = Some(vkd3d_root.to_string_lossy().into_owned());

    let dxvk = "dxvk-2.7";
    let vkd3d = "vkd3d-proton-2.14";
    component_build(&dxvk_root, Component::Dxvk, dxvk);
    component_build(&vkd3d_root, Component::Vkd3d, vkd3d);
    let reviewed = inspect(&source, None, None).unwrap();
    let settings = ImportedSettings {
        dxvk: Some(dxvk.to_owned()),
        vkd3d: Some(vkd3d.to_owned()),
        ..ImportedSettings::default()
    };

    let env = execute_reviewed(
        &paths,
        &global,
        ImportRequest {
            source,
            name: "copied".to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        Some(&reviewed),
        Some(&settings),
        &Token::new(),
        |_| {},
    )
    .unwrap();

    let wine = env.config.wine().unwrap();
    assert_eq!(wine.dxvk.as_deref(), Some(dxvk));
    assert_eq!(wine.vkd3d.as_deref(), Some(vkd3d));
    let config_path = paths.environment_config("copied");
    assert!(config_path.is_file());
    let config_text = fs::read_to_string(&config_path).unwrap();
    let config = EnvironmentConfig::parse(&config_path, &config_text).unwrap();
    let wine = config.wine().unwrap();
    assert_eq!(wine.dxvk.as_deref(), Some(dxvk));
    assert_eq!(wine.vkd3d.as_deref(), Some(vkd3d));
}

#[test]
fn inspection_normalizes_a_proton_pfx_path_and_reports_external_links() {
    let root = tempfile::tempdir().unwrap();
    let compatdata = root.path().join("compatdata/123");
    prefix(&compatdata.join("pfx"), "win64");
    fs::create_dir_all(compatdata.join("dosdevices")).unwrap();
    symlink("/mnt/games", compatdata.join("dosdevices/z:")).unwrap();

    let inspection = inspect(
        compatdata.join("pfx"),
        Some(&Runner::fake_proton("GE-Proton9-20")),
        None,
    )
    .unwrap();
    assert_eq!(inspection.source_root, compatdata);
    assert_eq!(inspection.layout, ImportLayout::Proton);
    assert_eq!(inspection.arch, Arch::Win64);
    assert_eq!(inspection.external_dependencies.len(), 1);
    assert_eq!(
        inspection.external_dependencies[0].target,
        Path::new("/mnt/games")
    );
}

#[test]
fn import_fingerprint_sees_a_same_size_rewrite_without_reading_content() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    prefix(&source, "win64");
    let file = source.join("drive_c/save.dat");
    fs::write(&file, "aaaa").unwrap();
    let cancel = Token::new();
    let fingerprint = || import_fingerprint(&source, &mut Check::new(&cancel, &mut |_| {}));
    let before = fingerprint().unwrap();
    assert_eq!(fingerprint().unwrap(), before);

    // Coarse kernel timestamps could otherwise give the rewrite the same ctime.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let metadata = fs::metadata(&file).unwrap();
    fs::write(&file, "bbbb").unwrap();
    filetime::set_file_mtime(
        &file,
        filetime::FileTime::from_last_modification_time(&metadata),
    )
    .unwrap();
    let after = fs::metadata(&file).unwrap();
    assert_eq!(after.len(), metadata.len());
    assert_eq!(after.mtime_nsec(), metadata.mtime_nsec());
    assert_ne!(fingerprint().unwrap(), before);
}

#[test]
fn content_fingerprint_of_a_copy_matches_the_original() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    prefix(&source, "win64");
    fs::write(source.join("drive_c/game.exe"), "game").unwrap();
    let copy = root.path().join("copy");
    copy_tree(&source, &copy, &copy, &Token::new(), &mut |_| {}).unwrap();
    assert_eq!(fingerprint(&copy).unwrap(), fingerprint(&source).unwrap());

    let cancel = Token::new();
    let mut quiet = |_| {};
    assert_ne!(
        import_fingerprint(&copy, &mut Check::new(&cancel, &mut quiet)).unwrap(),
        import_fingerprint(&source, &mut Check::new(&cancel, &mut quiet)).unwrap()
    );
}

#[test]
fn a_folder_of_prefixes_is_refused_but_a_proton_prefix_is_not() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Prefixes/default");
    prefix(&folder, "win64");
    prefix(&folder.join("Game One"), "win64");
    prefix(&folder.join("Game Two/pfx"), "win64");
    let error = inspect(&folder, None, None).unwrap_err().to_string();
    assert_eq!(
        error,
        format!(
            "{} holds other prefixes; choose the game's own prefix",
            fs::canonicalize(&folder).unwrap().display()
        )
    );

    let proton = root.path().join("compatdata/123");
    prefix(&proton, "win64");
    prefix(&proton.join("pfx"), "win64");
    let inspection = inspect(&proton, Some(&Runner::fake_proton("GE-Proton9-20")), None).unwrap();
    assert_eq!(inspection.layout, ImportLayout::Proton);
}

#[test]
fn both_layout_markers_require_a_runner_choice() {
    let root = tempfile::tempdir().unwrap();
    prefix(root.path(), "win64");
    prefix(&root.path().join("pfx"), "win64");
    assert!(inspect(root.path(), None, None).is_err());
    assert_eq!(
        inspect(
            root.path(),
            Some(&Runner::fake_proton("GE-Proton9-20")),
            None
        )
        .unwrap()
        .layout,
        ImportLayout::Proton
    );
    assert_eq!(
        inspect(root.path(), Some(&Runner::fake("wine")), None)
            .unwrap()
            .layout,
        ImportLayout::Wine
    );
}

#[test]
fn proton_import_preserves_a_pfx_dot_link_in_a_compatdata_root() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    proton_runner(&runner_root, "GE-Proton9-20");
    let source = root.path().join("compatdata/123");
    prefix(&source, "win64");
    symlink(".", source.join("pfx")).unwrap();
    let global = {
        let mut global = base_global(&runner_root, &prefix_root);
        global.defaults.runner = "GE-Proton9-20".to_owned();
        global
    };
    let selected_runner = Runners::at(runner_root.clone())
        .resolve("GE-Proton9-20")
        .unwrap();
    let inspection = inspect(&source, Some(&selected_runner), None).unwrap();
    assert_eq!(inspection.layout, ImportLayout::Proton);
    execute_reviewed(
        &paths,
        &global,
        ImportRequest {
            source: source.clone(),
            name: "stellar".to_owned(),
            runner: "GE-Proton9-20".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        Some(&inspection),
        None,
        &Token::new(),
        |_| {},
    )
    .unwrap();
    let copied = prefix_root.join("stellar");
    assert_eq!(fs::read_link(copied.join("pfx")).unwrap(), Path::new("."));
}

#[test]
fn proton_import_rebases_an_absolute_pfx_self_link() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    proton_runner(&runner_root, "GE-Proton9-20");
    let source = root.path().join("compatdata/123");
    prefix(&source, "win64");
    symlink(&source, source.join("pfx")).unwrap();
    let global = base_global(&runner_root, &prefix_root);

    execute(
        &paths,
        &global,
        ImportRequest {
            source: source.clone(),
            name: "stellar".to_owned(),
            runner: "GE-Proton9-20".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        &Token::new(),
        |_| {},
    )
    .unwrap();

    let copied = prefix_root.join("stellar");
    assert_eq!(fs::read_link(source.join("pfx")).unwrap(), source);
    assert_eq!(fs::read_link(copied.join("pfx")).unwrap(), copied);
    assert!(copied.join("pfx/system.reg").is_file());
    assert!(copied.join("pfx/drive_c").is_dir());
    assert!(paths.environment_config("stellar").exists());
}

#[test]
fn import_reports_checking_and_stops_when_canceled_during_a_check() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    wine_runner(&runner_root, "wine-build");
    let source = root.path().join("source");
    prefix(&source, "win64");
    for index in 0..1200 {
        fs::write(source.join(format!("drive_c/{index}")), "x").unwrap();
    }
    let global = base_global(&runner_root, &prefix_root);
    let request = |name: &str| ImportRequest {
        source: source.clone(),
        name: name.to_owned(),
        runner: "wine-build".to_owned(),
        destination: None,
        adopt: false,
        arch: None,
    };

    let canceled = Token::new();
    let result = execute(&paths, &global, request("stopped"), &canceled, |event| {
        if matches!(event, ImportProgress::Checking { .. }) {
            canceled.cancel();
        }
    });
    assert!(result.unwrap_err().is::<Cancelled>());
    assert!(!prefix_root.join("stopped").exists());
    assert!(!paths.environment_config("stopped").exists());

    let mut events = Vec::new();
    execute(&paths, &global, request("done"), &Token::new(), |event| {
        events.push(event)
    })
    .unwrap();
    let started = events
        .iter()
        .position(|event| matches!(event, ImportProgress::Started { .. }))
        .unwrap();
    let checks = |events: &[ImportProgress]| {
        events
            .iter()
            .filter_map(|event| match event {
                ImportProgress::Checking { entries } => Some(*entries),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // Two inspections before the copy, each with a periodic and a final count.
    assert_eq!(checks(&events[..started]), [1000, 1203, 1000, 1203]);
    assert_eq!(checks(&events[started..]), [1000, 1203]);

    // Canceling on the last check after the copy must still stop before
    // publication.
    let late = Token::new();
    let mut copying = false;
    let result = execute(
        &paths,
        &global,
        request("late"),
        &late,
        |event| match event {
            ImportProgress::Started { .. } => copying = true,
            ImportProgress::Checking { entries: 1203 } if copying => late.cancel(),
            _ => {}
        },
    );
    assert!(result.unwrap_err().is::<Cancelled>());
    assert!(!prefix_root.join("late").exists());
    assert!(!paths.environment_config("late").exists());
    let staging = fs::read_dir(&prefix_root)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".wineo-import-")
        });
    assert!(!staging, "staging directory left behind");
}

#[test]
fn staged_validation_rejects_missing_targets_and_link_cycles() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("destination");
    fs::create_dir(&staging).unwrap();
    symlink(&destination, staging.join("pfx")).unwrap();
    assert!(validate_staged_prefix(&staging, &destination, ImportLayout::Proton).is_err());
    assert!(
        staged_path_if_exists(&staging.join("pfx/system.reg"), &staging, &destination).is_err()
    );

    fs::remove_file(staging.join("pfx")).unwrap();
    symlink(destination.join("loop"), staging.join("pfx")).unwrap();
    symlink(destination.join("pfx"), staging.join("loop")).unwrap();
    let error = validate_staged_prefix(&staging, &destination, ImportLayout::Proton)
        .unwrap_err()
        .to_string();
    assert!(error.contains("link chain is too deep"), "{error}");
    assert!(!destination.exists());
}

#[test]
fn copy_preserves_hardlinks_and_rebases_internal_absolute_links() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    prefix(&source, "win64");
    let file = source.join("drive_c/data.bin");
    fs::write(&file, "same").unwrap();
    fs::hard_link(&file, source.join("drive_c/data-copy.bin")).unwrap();
    symlink(
        source.join("drive_c/data.bin"),
        source.join("drive_c/absolute-link"),
    )
    .unwrap();
    symlink("/outside/save", source.join("drive_c/external-link")).unwrap();

    copy_tree(
        &source,
        &destination,
        &destination,
        &Token::new(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(destination.join("drive_c/data.bin")).unwrap(),
        b"same"
    );
    let one = fs::metadata(destination.join("drive_c/data.bin")).unwrap();
    let two = fs::metadata(destination.join("drive_c/data-copy.bin")).unwrap();
    assert_eq!((one.dev(), one.ino()), (two.dev(), two.ino()));
    assert_eq!(
        fs::read_link(destination.join("drive_c/absolute-link")).unwrap(),
        destination.join("drive_c/data.bin")
    );
    assert_eq!(
        fs::read_link(destination.join("drive_c/external-link")).unwrap(),
        Path::new("/outside/save")
    );
}

#[test]
fn regular_file_copy_uses_reflink_and_falls_back_only_when_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"contents").unwrap();
    let cloned = root.path().join("cloned");
    copy_regular_file_with(&source, &cloned, |output, input| {
        std::io::copy(input, output).unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(&cloned).unwrap(), b"contents");

    let fallback = root.path().join("fallback");
    copy_regular_file_with(&source, &fallback, |_, _| Err(Errno::OPNOTSUPP)).unwrap();
    assert_eq!(fs::read(&fallback).unwrap(), b"contents");

    let failed = root.path().join("failed");
    let error = copy_regular_file_with(&source, &failed, |_, _| Err(Errno::IO)).unwrap_err();
    assert!(error.to_string().contains("failed to reflink"));
    assert_eq!(fs::metadata(&failed).unwrap().len(), 0);
}

#[test]
fn strict_reflink_copy_fails_when_reflinks_are_unsupported() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"contents").unwrap();
    let destination = root.path().join("destination");

    let error =
        copy_regular_file_with_mode(&source, &destination, |_, _| Err(Errno::OPNOTSUPP), true)
            .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("filesystem does not support reflinks")
    );
    assert_eq!(fs::metadata(destination).unwrap().len(), 0);
}

#[test]
fn execute_copies_prefix_and_registers_heroic_settings_atomically() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    wine_runner(&runner_root, "wine-build");
    let source = root.path().join("old-prefix");
    prefix(&source, "win64");
    let global = base_global(&runner_root, &prefix_root);
    let reviewed = inspect(&source, None, None).unwrap();
    let settings = ImportedSettings {
        targets: BTreeMap::from([(
            "game".to_owned(),
            Target {
                exe: "Z:\\games\\game.exe".to_owned(),
                args: vec!["--launcher".to_owned()],
                cwd: Some("Z:\\games".to_owned()),
                default: true,
                gamemode: None,
                wayland: None,
                mangohud: None,
                gamescope: None,
                hooks: None,
                desktop: false,
                name: Some("Game".to_owned()),
            },
        )]),
        env: BTreeMap::from([("GAME_MODE".to_owned(), "heroic".to_owned())]),
        heroic_id: Some("kZXLzzKAFWBP4dpRatpPHu".to_owned()),
        ..ImportedSettings::default()
    };
    let env = execute_reviewed(
        &paths,
        &global,
        ImportRequest {
            source: source.clone(),
            name: "game".to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        Some(&reviewed),
        Some(&settings),
        &Token::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(env.config.targets["game"].exe, "Z:\\games\\game.exe");
    assert_eq!(env.config.env["GAME_MODE"], "heroic");
    assert_eq!(
        env.config.heroic_id.as_deref(),
        Some("kZXLzzKAFWBP4dpRatpPHu")
    );
    assert!(prefix_root.join("game/system.reg").is_file());
    assert!(paths.environment_config("game").is_file());
}

#[test]
fn import_holds_the_install_lock_through_registration() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    wine_runner(&runner_root, "wine-build");
    let source = root.path().join("old-prefix");
    prefix(&source, "win64");
    let global = base_global(&runner_root, &prefix_root);
    let destination = prefix_root.join("game");
    let checked = std::rc::Rc::new(std::cell::Cell::new(false));
    BEFORE_CONFIG_WRITE.with(|hook| {
        let destination = destination.clone();
        let checked = checked.clone();
        *hook.borrow_mut() = Some(Box::new(move |_| {
            assert!(destination.join("system.reg").is_file());
            assert!(InstallLock::acquire(&destination).is_err());
            checked.set(true);
        }));
    });
    let result = execute(
        &paths,
        &global,
        ImportRequest {
            source,
            name: "game".to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        &Token::new(),
        |_| {},
    );
    BEFORE_CONFIG_WRITE.with(|hook| hook.borrow_mut().take());
    result.unwrap();
    assert!(checked.get());
    assert!(InstallLock::acquire(&destination).is_ok());
}

#[test]
fn imported_configs_start_with_the_version() {
    let heroic = ImportedSettings {
        heroic_id: Some("abc".to_owned()),
        ..ImportedSettings::default()
    };
    for settings in [None, Some(&heroic)] {
        let text = config_text("~/p", "wine", Arch::Win64, settings).unwrap();
        assert!(
            text.starts_with("schema_version = 1\nprefix = \"~/p\"\n"),
            "{text}"
        );
        let config = EnvironmentConfig::parse(Path::new("/x.toml"), &text).unwrap();
        assert_eq!(config.schema_version, 1);
    }
}

#[test]
fn execute_allows_an_idle_shared_prefix_but_rejects_a_lost_run() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_root = root.path().join("runners");
    let prefix_root = root.path().join("prefixes");
    wine_runner(&runner_root, "wine-build");
    let source = root.path().join("shared");
    prefix(&source, "win64");
    let global = base_global(&runner_root, &prefix_root);
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("old"),
        format!(
            "prefix = \"{}\"\nrunner = \"wine-build\"\narch = \"win64\"\n",
            source.display()
        ),
    )
    .unwrap();
    let first = execute(
        &paths,
        &global,
        ImportRequest {
            source: source.clone(),
            name: "new".to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        &Token::new(),
        |_| {},
    );
    assert!(first.is_ok(), "idle shared prefix should import: {first:?}");

    let state = crate::runtime::RunState {
        status: crate::runtime::RunStatus::Running,
        lost: false,
        phase: None,
        mode: crate::runtime::RunMode::Attached,
        target: "game".to_owned(),
        runner: None,
        prefix: None,
        native: false,
        launcher: None,
        pid: 1,
        group: None,
        boot_id: None,
        leader_start: None,
        started_at: "2026-09-21T00:00:00Z".parse().unwrap(),
        running_at: None,
        heartbeat_at: None,
        program_exited_at: None,
        ended_at: None,
        exit_code: None,
        requested_target: None,
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    Runtime::new(&paths, "old").write_state(&state).unwrap();
    let lost = execute(
        &paths,
        &global,
        ImportRequest {
            source,
            name: "lost".to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        },
        &Token::new(),
        |_| {},
    );
    assert!(lost.is_err());
}

#[test]
fn copy_runner_writes_alias_compatible_provenance_and_refuses_alias_names() {
    let root = tempfile::tempdir().unwrap();
    proton_runner(root.path(), "GE-Proton9-20");
    let source = root.path().join("GE-Proton9-20");
    let runners = root.path().join("installed");
    let receipt = copy_runner(&source, &runners, "GE-Proton9-20", &Token::new(), |_| {}).unwrap();
    assert_eq!(
        crate::runner_alias::version(&runners, "GE-Proton9-20", "GE-Proton-Latest"),
        Some(vec![9, 20])
    );
    assert!(receipt.receipt.is_file());
    assert!(copy_runner(&source, &runners, "GE-Proton-Latest", &Token::new(), |_| {},).is_err());
}

#[test]
fn runner_copy_preflight_rejects_unavailable_and_invalid_sources() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("app");
    assert!(inspect_runner_copy(&missing).is_err());
    fs::create_dir(&missing).unwrap();
    assert!(inspect_runner_copy(&missing).is_err());

    wine_runner(root.path(), "wine-build");
    let wine = inspect_runner_copy(root.path().join("wine-build")).unwrap();
    assert!(wine.supports_wine);
    assert!(!wine.supports_proton);

    proton_runner(root.path(), "GE-Proton9-20");
    let proton = inspect_runner_copy(root.path().join("GE-Proton9-20")).unwrap();
    assert!(proton.supports_proton);
}

#[test]
fn runner_copy_retains_external_dependencies_and_rebases_internal_links() {
    let root = tempfile::tempdir().unwrap();
    wine_runner(root.path(), "wine-build");
    let source = root.path().join("wine-build");
    fs::rename(source.join("bin/wine"), source.join("bin/wine-real")).unwrap();
    symlink(source.join("bin/wine-real"), source.join("bin/wine")).unwrap();
    let external = root.path().join("shared-lib");
    fs::write(&external, "host data").unwrap();
    symlink("../../shared-lib", source.join("bin/lib-shared")).unwrap();

    let inspection = inspect_runner_copy(&source).unwrap();
    assert_eq!(inspection.external_dependencies.len(), 1);
    assert_eq!(inspection.external_dependencies[0].target, external);

    let receipt = copy_runner(
        &source,
        root.path().join("installed"),
        "wine-build",
        &Token::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(receipt.external_dependencies.len(), 1);
    assert_eq!(
        receipt.external_dependencies[0].source,
        receipt.path.join("bin/lib-shared")
    );
    assert_eq!(receipt.external_dependencies[0].target, external);
    assert_eq!(
        fs::read_to_string(receipt.path.join("bin/lib-shared")).unwrap(),
        "host data"
    );
    assert_eq!(
        fs::canonicalize(receipt.path.join("bin/wine")).unwrap(),
        receipt.path.join("bin/wine-real")
    );
    assert_eq!(fs::read_to_string(&external).unwrap(), "host data");
}

#[test]
fn reviewed_runner_copy_rejects_changes_before_copy() {
    let root = tempfile::tempdir().unwrap();
    wine_runner(root.path(), "wine-build");
    let source = root.path().join("wine-build");
    let reviewed = inspect_runner_copy(&source).unwrap();
    fs::remove_dir_all(source.join("bin")).unwrap();
    write_script(source.join("proton"), "#!/bin/sh\n");
    fs::create_dir_all(source.join("files/bin")).unwrap();
    for name in ["wine", "wineserver"] {
        let path = source.join("files/bin").join(name);
        write_script(&path, "#!/bin/sh\n");
    }
    let destination = root.path().join("installed");
    let error = copy_runner_reviewed(&reviewed, &destination, "wine-build", &Token::new(), |_| {})
        .unwrap_err();
    assert!(error.to_string().contains("reviewed runner changed"));
    assert!(!destination.exists());
}

#[test]
fn runner_copy_cancellation_and_source_change_leave_no_publication() {
    let root = tempfile::tempdir().unwrap();
    wine_runner(root.path(), "wine-build");
    let source = root.path().join("wine-build");
    let destination = root.path().join("installed");
    let canceled = Token::new();
    let result = copy_runner(&source, &destination, "canceled", &canceled, |event| {
        if matches!(event, ImportProgress::Copied { .. }) {
            canceled.cancel();
        }
    });
    assert!(result.unwrap_err().is::<Cancelled>());
    assert!(!destination.join("canceled").exists());
    assert!(fs::read_dir(&destination).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".wineo-runner-")
    }));

    let changed = source.join("bin/wine");
    let result = copy_runner(&source, &destination, "changed", &Token::new(), |event| {
        if let ImportProgress::Copied { path } = event
            && path == changed
        {
            fs::write(&changed, "#!/bin/sh\necho changed\n").unwrap();
        }
    });
    assert!(result.unwrap_err().to_string().contains("changed"));
    assert!(!destination.join("changed").exists());
    assert!(fs::read_dir(&destination).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".wineo-runner-")
    }));
}

#[test]
fn concurrent_runner_copies_publish_only_one_complete_build() {
    use std::sync::{Arc, Barrier};

    let root = tempfile::tempdir().unwrap();
    proton_runner(root.path(), "GE-Proton9-20");
    let source = root.path().join("GE-Proton9-20");
    let destination = root.path().join("installed");
    let barrier = Arc::new(Barrier::new(2));
    let copies = (0..2)
        .map(|_| {
            let source = source.clone();
            let destination = destination.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                copy_runner(source, destination, "GE-Proton9-20", &Token::new(), |_| {})
            })
        })
        .collect::<Vec<_>>();
    let results = copies
        .into_iter()
        .map(|copy| copy.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert!(destination.join("GE-Proton9-20/proton").is_file());
    assert!(destination.join("GE-Proton9-20/files/bin/wine").is_file());
    assert!(
        destination
            .join("GE-Proton9-20/.wineo-install.json")
            .is_file()
    );
    assert_eq!(
        fs::read_dir(&destination)
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
            .count(),
        1
    );
}

#[test]
fn proton_runner_copy_preserves_executable_modes_and_internal_links() {
    let root = tempfile::tempdir().unwrap();
    proton_runner(root.path(), "GE-Proton9-20");
    let source = root.path().join("GE-Proton9-20");
    fs::rename(source.join("proton"), source.join("proton-real")).unwrap();
    symlink(source.join("proton-real"), source.join("proton")).unwrap();
    let reviewed = inspect_runner_copy(&source).unwrap();
    let receipt = copy_runner_reviewed(
        &reviewed,
        root.path().join("installed"),
        "GE-Proton9-20",
        &Token::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::canonicalize(receipt.path.join("proton")).unwrap(),
        receipt.path.join("proton-real")
    );
    for path in ["proton-real", "files/bin/wine", "files/bin/wineserver"] {
        assert_ne!(
            fs::metadata(receipt.path.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
}
struct Preflight {
    _root: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    global: GlobalConfig,
    inspection: Inspection,
}

impl Preflight {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let source = root.join("source");
        prefix(&source, "win64");
        Self {
            paths: paths(&root),
            global: base_global(&root.join("runners"), &root.join("prefixes")),
            inspection: inspect(&source, None, None).unwrap(),
            root,
            _root: dir,
        }
    }

    fn request(&self, name: &str, destination: Option<PathBuf>, adopt: bool) -> ImportRequest {
        ImportRequest {
            source: self.inspection.source_root.clone(),
            name: name.to_owned(),
            runner: "wine-build".to_owned(),
            destination,
            adopt,
            arch: None,
        }
    }

    fn check(&self, request: &ImportRequest) -> Result<PathBuf> {
        preflight(&self.paths, &self.global, request, &self.inspection)
    }

    fn error(&self, request: &ImportRequest) -> String {
        self.check(request).unwrap_err().to_string()
    }
}

#[test]
fn preflight_returns_the_destination_as_given() {
    let f = Preflight::new();
    let given = f.root.join("missing/../new");
    assert_eq!(
        f.check(&f.request("game", Some(given.clone()), false))
            .unwrap(),
        given
    );
    assert_eq!(
        f.check(&f.request("game", None, false)).unwrap(),
        f.root.join("prefixes/game")
    );
    assert_eq!(
        f.check(&f.request("game", None, true)).unwrap(),
        f.inspection.source_root
    );
}

#[test]
fn preflight_checks_a_destination_where_it_resolves() {
    let f = Preflight::new();
    let taken = f.root.join("taken");
    fs::create_dir(&taken).unwrap();
    let expected = format!("import destination already exists: {}", taken.display());

    let dotted = f.root.join("missing/../taken");
    assert!(!dotted.exists());
    assert_eq!(f.error(&f.request("game", Some(dotted), false)), expected);

    let link = f.root.join("link");
    symlink(&taken, &link).unwrap();
    assert_eq!(f.error(&f.request("game", Some(link), false)), expected);
}

#[test]
fn preflight_refuses_in_order() {
    let f = Preflight::new();
    let source = f.inspection.source_root.clone();
    let config = f.paths.environment_config("game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "").unwrap();

    // An invalid name is refused before anything about the paths.
    let error = f.error(&f.request("bad/name", Some(source.clone()), false));
    assert_eq!(error, "invalid environment name `bad/name`");
    // Overlap is refused before the existing destination and environment.
    let error = f.error(&f.request("game", Some(source.join("inside")), false));
    assert!(error.contains("overlaps destination"), "{error}");
    let taken = f.root.join("taken");
    fs::create_dir(&taken).unwrap();
    let error = f.error(&f.request("game", Some(taken), false));
    assert!(
        error.starts_with("import destination already exists"),
        "{error}"
    );
    let error = f.error(&f.request("game", Some(f.root.join("free")), false));
    assert_eq!(error, "environment `game` already exists");
}

#[test]
fn preflight_refuses_a_dangling_config_symlink_as_an_existing_environment() {
    let f = Preflight::new();
    let config = f.paths.environment_config("game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    symlink(f.root.join("nowhere.toml"), &config).unwrap();
    assert_eq!(
        f.error(&f.request("game", None, false)),
        "environment `game` already exists"
    );
}

#[test]
fn preflight_refuses_a_destination_another_environment_names() {
    let f = Preflight::new();
    let other = f.paths.environment_config("other");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    let claimed = f.root.join("claimed");
    fs::write(
        &other,
        format!(
            "schema_version = 1\nprefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n",
            claimed.display()
        ),
    )
    .unwrap();
    let error = f.error(&f.request("game", Some(claimed), false));
    assert!(error.contains("other"), "{error}");
    f.check(&f.request("game", Some(f.root.join("free")), false))
        .unwrap();
}
