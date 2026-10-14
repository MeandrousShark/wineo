use crate::test_script::{write_executable, write_script};

use std::os::unix::fs::symlink;

use super::*;

fn runners(root: &Path) -> Runners {
    Runners::at(root.join("runners"))
}

/// A runner tree of stub executables, enough for discovery without wine.
fn install(root: &Path, name: &str, files: &[(&str, bool)]) {
    let dir = root.join("runners").join(name);
    for (path, executable) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mode = if *executable { 0o755 } else { 0o644 };
        write_executable(&path, "#!/bin/sh\n", mode);
    }
}

fn wine_build(root: &Path, name: &str) {
    install(root, name, &[("bin/wine", true), ("bin/wineserver", true)]);
}

fn skipped(listing: &Listing, name: &str) -> Option<Skipped> {
    listing
        .skipped
        .iter()
        .find(|(found, _)| found == name)
        .map(|(_, reason)| *reason)
}

#[test]
fn a_missing_runners_root_just_means_nothing_is_installed() {
    let root = tempfile::tempdir().unwrap();
    let listing = runners(root.path()).list().unwrap();
    assert!(listing.runners.is_empty());
    assert!(listing.skipped.is_empty());
}

#[test]
fn a_wine_build_is_a_runner_and_everything_else_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    wine_build(root.path(), "wine-tkg");
    wine_build(root.path(), "wine-10.0");
    install(root.path(), "half", &[("bin/wine", true)]);
    install(root.path(), "no-wine", &[("bin/wineserver", true)]);
    install(
        root.path(),
        "not-executable",
        &[("bin/wine", false), ("bin/wineserver", true)],
    );
    install(root.path(), "empty", &[("README", false)]);
    // Proton is not a wine build as far as this plan is concerned.
    install(root.path(), "GE-Proton9-20", &[("files/bin/wine", true)]);
    wine_build(root.path(), "wine");
    fs::create_dir_all(root.path().join("runners/.tmp-extract/bin")).unwrap();
    install(root.path(), "bad name", &[("bin/wine", true)]);

    let listing = runners(root.path()).list().unwrap();
    let names: Vec<&str> = listing.runners.iter().map(|runner| runner.name()).collect();
    assert_eq!(names, ["wine-10.0", "wine-tkg"]);

    assert_eq!(skipped(&listing, "half"), Some(Skipped::NoWineserver));
    assert_eq!(skipped(&listing, "no-wine"), Some(Skipped::NoWine));
    assert_eq!(
        skipped(&listing, "not-executable"),
        Some(Skipped::NotExecutable)
    );
    assert_eq!(skipped(&listing, "empty"), Some(Skipped::NoWine));
    assert_eq!(skipped(&listing, "GE-Proton9-20"), Some(Skipped::NoWine));
    assert_eq!(skipped(&listing, "wine"), Some(Skipped::Reserved));
    assert_eq!(
        skipped(&listing, ".tmp-extract"),
        Some(Skipped::InvalidName)
    );
    assert_eq!(skipped(&listing, "bad name"), Some(Skipped::InvalidName));
}

fn proton_build(root: &Path, name: &str, version: &str) {
    install(
        root,
        name,
        &[
            ("proton", true),
            ("files/bin/wine", true),
            ("files/bin/wineserver", true),
            ("version", false),
        ],
    );
    fs::write(
        root.join("runners").join(name).join("version"),
        format!("1717451631 {version}\n"),
    )
    .unwrap();
}

#[test]
fn a_proton_tree_is_recognized_by_its_proton_script() {
    let root = tempfile::tempdir().unwrap();
    proton_build(root.path(), "GE-Proton9-20", "GE-Proton9-20");
    let runner = runners(root.path()).resolve("GE-Proton9-20").unwrap();

    assert_eq!(runner.kind_name(), "proton");
    assert!(runner.is_proton());
    // Nothing reaches for Proton's own wine: umu runs everything.
    assert_eq!(runner.wine_path(), None);
    assert_eq!(runner.wineserver_path(), None);
    // Proton keeps its prefix under the compatdata directory it is given.
    assert_eq!(
        runner.wine_prefix(Path::new("/games/hades")),
        Path::new("/games/hades/pfx")
    );
    assert_eq!(runner.version().as_deref(), Some("GE-Proton9-20"));
}

#[test]
fn proton_wins_over_the_wine_binaries_in_the_same_tree() {
    let root = tempfile::tempdir().unwrap();
    // A real Proton tree has bin/wine too, and must not read as a wine build.
    proton_build(root.path(), "GE-Proton9-20", "GE-Proton9-20");
    install(
        root.path(),
        "GE-Proton9-20",
        &[("bin/wine", true), ("bin/wineserver", true)],
    );
    let runner = runners(root.path()).resolve("GE-Proton9-20").unwrap();
    assert_eq!(runner.kind_name(), "proton");
}

#[test]
fn a_proton_script_that_cannot_run_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "half-proton", &[("proton", false)]);
    let listing = runners(root.path()).list().unwrap();
    assert!(listing.runners.is_empty());
    assert_eq!(
        skipped(&listing, "half-proton"),
        Some(Skipped::NotExecutable)
    );
}

#[test]
fn a_proton_build_without_a_version_file_just_has_no_version() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "nameless", &[("proton", true)]);
    let runner = runners(root.path()).resolve("nameless").unwrap();
    assert_eq!(runner.version(), None);
}

#[test]
fn resolving_finds_the_absolute_binaries() {
    let root = tempfile::tempdir().unwrap();
    wine_build(root.path(), "wine-tkg");
    let runner = runners(root.path()).resolve("wine-tkg").unwrap();
    assert_eq!(runner.name(), "wine-tkg");
    assert_eq!(runner.kind_name(), "wine");
    assert_eq!(
        runner.wine_path(),
        Some(root.path().join("runners/wine-tkg/bin/wine").as_path())
    );
    assert_eq!(
        runner.wineserver_path(),
        Some(
            root.path()
                .join("runners/wine-tkg/bin/wineserver")
                .as_path()
        )
    );
    // A wine prefix is the configured directory itself.
    assert_eq!(runner.wine_prefix(Path::new("/p")), Path::new("/p"));
}

#[test]
fn resolving_a_runner_alias_keeps_the_concrete_executable_paths() {
    let root = tempfile::tempdir().unwrap();
    let concrete = root.path().join("builds/wine-tkg");
    for name in ["wine", "wineserver"] {
        let path = concrete.join("bin").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_script(&path, "#!/bin/sh\n");
    }
    fs::create_dir_all(root.path().join("runners")).unwrap();
    symlink(&concrete, root.path().join("runners/current")).unwrap();

    let runner = runners(root.path()).resolve("current").unwrap();
    let concrete_wine = concrete.join("bin/wine").canonicalize().unwrap();
    assert_eq!(runner.name(), "current");
    assert_eq!(runner.wine_path(), Some(concrete_wine.as_path()));
}

#[test]
fn wine_always_means_the_system_one() {
    let root = tempfile::tempdir().unwrap();
    // Installed under the reserved name, and still ignored.
    wine_build(root.path(), "wine");
    let resolved = runners(root.path()).resolve("wine");
    match Runner::system() {
        Ok(system) => assert_eq!(
            resolved.unwrap().wine_path().map(Path::to_owned),
            system.wine_path().map(Path::to_owned)
        ),
        // No wine on this machine, so it fails the same way either way.
        Err(_) => assert!(resolved.is_err()),
    }
}

#[test]
fn an_unknown_runner_says_what_is_installed() {
    let root = tempfile::tempdir().unwrap();
    let runners = runners(root.path());

    let err = runners.resolve("nope").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("is not installed"), "{message}");
    assert!(message.contains("runners"), "{message}");

    wine_build(root.path(), "wine-tkg");
    let message = runners.resolve("nope").unwrap_err().to_string();
    assert!(message.contains("wine-tkg"), "{message}");
}

#[test]
fn a_directory_that_is_not_a_wine_build_says_what_was_missing() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "half", &[("bin/wine", true)]);
    let err = runners(root.path()).resolve("half").unwrap_err();
    assert!(
        matches!(&err, Error::NotARunner { name, .. } if name == "half"),
        "{err}"
    );
    assert!(err.to_string().contains("bin/wineserver"), "{err}");
}

#[test]
fn an_unusable_name_is_refused_before_looking_at_the_disk() {
    let root = tempfile::tempdir().unwrap();
    for name in ["..", ".", ".hidden", "has space", "a/b"] {
        assert!(
            matches!(
                runners(root.path()).resolve(name),
                Err(Error::InvalidRunnerName(_))
            ),
            "{name}"
        );
    }
}

#[test]
fn the_runner_root_follows_the_global_config() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
            .unwrap();

    let default = Runners::new(&paths, &GlobalConfig::default()).unwrap();
    assert_eq!(
        default.root(),
        Path::new("/home/me/.local/share/wineo/runners")
    );

    let mut global = GlobalConfig::default();
    global.defaults.runner_root = Some(root.path().join("elsewhere").display().to_string());
    let moved = Runners::new(&paths, &global).unwrap();
    assert_eq!(moved.root(), root.path().join("elsewhere"));
}

#[test]
fn the_newest_proton_goes_by_version_numbers() {
    let root = tempfile::tempdir().unwrap();
    assert!(runners(root.path()).newest_proton().unwrap().is_none());
    for name in ["GE-Proton9-20", "GE-Proton11-7-x86_64", "GE-Proton10-3"] {
        proton_build(root.path(), name, name);
    }
    let newest = runners(root.path()).newest_proton().unwrap().unwrap();
    assert_eq!(newest.name(), "GE-Proton11-7-x86_64");
}
#[test]
fn aliases_select_stable_family_versions_and_retain_resolved_runners() {
    let root = tempfile::tempdir().unwrap();
    let installed = runners(root.path());
    assert!(installed.resolve("GE-Proton-Latest").is_err());
    for name in [
        "GE-Proton9-20",
        "GE-Proton11-7-x86_64",
        "GE-Proton11-99-rc1",
        "proton-cachyos-12.0-20260921-slr-x86_64",
        "GE-Proton99-1-aarch64",
    ] {
        proton_build(root.path(), name, name);
    }
    let before = installed.resolve("GE-Proton-Latest").unwrap();
    assert_eq!(before.name(), "GE-Proton11-7-x86_64");
    assert_eq!(before.selection(), "GE-Proton-Latest");
    let serialized = toml::to_string(&before).unwrap();
    proton_build(root.path(), "GE-Proton12-1", "GE-Proton12-1");
    proton_build(root.path(), "GE-Proton8-1", "GE-Proton8-1");
    assert_eq!(
        installed.resolve("GE-Proton-Latest").unwrap().name(),
        "GE-Proton12-1"
    );
    assert_eq!(toml::from_str::<Runner>(&serialized).unwrap(), before);
    assert_eq!(
        installed.resolve("GE-Proton9-20").unwrap().selection(),
        "GE-Proton9-20"
    );
    assert_eq!(
        installed.resolve("Proton-CachyOS-Latest").unwrap().name(),
        "proton-cachyos-12.0-20260921-slr-x86_64"
    );
}

#[test]
fn alias_re_resolves_when_the_selected_build_disappears_before_locking() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    proton_build(root.path(), "GE-Proton9-20", "GE-Proton9-20");
    proton_build(root.path(), "GE-Proton10-1", "GE-Proton10-1");
    let runners = runners(root.path());
    let selected = runners.resolve("GE-Proton-Latest").unwrap();
    assert_eq!(selected.name(), "GE-Proton10-1");
    fs::remove_dir_all(root.path().join("runners/GE-Proton10-1")).unwrap();
    let (runner, _guard) = runners
        .lock_resolved(&paths, "GE-Proton-Latest", selected)
        .unwrap();
    assert_eq!(runner.name(), "GE-Proton9-20");
    assert!(
        crate::build_lock::try_exclusive(&paths, crate::build_lock::Kind::Runner, runner.name(),)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_missing_concrete_build_after_resolution_uses_the_existing_error() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    wine_build(root.path(), "wine-tkg");
    let runners = runners(root.path());
    let selected = runners.resolve("wine-tkg").unwrap();
    fs::remove_dir_all(root.path().join("runners/wine-tkg")).unwrap();
    assert!(matches!(
        runners.lock_resolved(&paths, "wine-tkg", selected),
        Err(Error::UnknownRunner { .. })
    ));
}

#[test]
fn alias_receipts_allow_renames_but_not_incompatible_or_corrupt_builds() {
    let root = tempfile::tempdir().unwrap();
    proton_build(root.path(), "renamed", "GE-Proton12-1");
    let path = root.path().join("runners/renamed/.wineo-install.json");
    let mut receipt = serde_json::json!({"provider":"ge-proton", "version":"GE-Proton12-1", "architecture":"x86_64", "variant":"baseline", "stable":true});
    fs::write(&path, receipt.to_string()).unwrap();
    assert_eq!(
        runners(root.path())
            .resolve("GE-Proton-Latest")
            .unwrap()
            .name(),
        "renamed"
    );
    receipt["stable"] = false.into();
    fs::write(&path, receipt.to_string()).unwrap();
    assert!(runners(root.path()).resolve("GE-Proton-Latest").is_err());
    fs::write(&path, "broken").unwrap();
    assert!(runners(root.path()).resolve("GE-Proton-Latest").is_err());
    fs::write(root.path().join("runners/GE-Proton-Latest"), "reserved").unwrap();
    assert!(
        runners(root.path())
            .resolve("GE-Proton-Latest")
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
}

#[test]
fn candidates_are_system_then_installed_then_aliases_keeping_failures() {
    let root = tempfile::tempdir().unwrap();
    wine_build(root.path(), "alpha");
    proton_build(root.path(), "GE-Proton10-1", "GE-Proton10-1");
    let installed = runners(root.path());
    let listing = installed.list().unwrap();
    let candidates = installed.candidates(&listing);

    let shape: Vec<_> = candidates
        .iter()
        .map(|candidate| (candidate.kind, candidate.name.as_str()))
        .collect();
    let mut expected = vec![(CandidateKind::System, Runner::SYSTEM)];
    expected.extend(
        listing
            .runners
            .iter()
            .map(|runner| (CandidateKind::Installed, runner.name())),
    );
    expected.extend([
        (CandidateKind::Alias, "GE-Proton-Latest"),
        (CandidateKind::Alias, "Proton-CachyOS-Latest"),
    ]);
    assert_eq!(shape, expected);
    assert_eq!(listing.runners.len(), 2);
    assert_eq!(candidates[0].runner.is_ok(), Runner::system().is_ok());

    // An alias keeps its own name as the selection but runs its build.
    let ge = candidates[3].runner.as_ref().unwrap();
    assert_eq!(ge.selection(), "GE-Proton-Latest");
    assert_eq!(ge.name(), "GE-Proton10-1");
    let cachyos = candidates[4].runner.as_ref().unwrap_err();
    assert_eq!(
        cachyos.to_string(),
        installed
            .resolve("Proton-CachyOS-Latest")
            .unwrap_err()
            .to_string()
    );
}
