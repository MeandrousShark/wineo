use std::ffi::OsString;
use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;
use crate::config::GlobalConfig;
use crate::error::Error;
use crate::runners::Runners;

const STEAM: &str = ".local/share/Steam/compatibilitytools.d";
const HEROIC: &str = ".config/heroic/tools/proton";
const HEROIC_WINE: &str = ".config/heroic/tools/wine";
const HEROIC_FLATPAK: &str = ".var/app/com.heroicgameslauncher.hgl/config/heroic/tools/proton";
const LUTRIS: &str = ".local/share/lutris/runners/wine";
const LUTRIS_FLATPAK: &str = ".var/app/net.lutris.Lutris/data/lutris/runners/wine";

fn paths(home: &Path) -> Paths {
    let home = home.as_os_str().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.clone())).unwrap()
}

fn on() -> GlobalConfig {
    GlobalConfig {
        external_runners: Some(true),
        ..GlobalConfig::default()
    }
}

fn runners(home: &Path) -> Runners {
    Runners::new(&paths(home), &on()).unwrap()
}

fn file(path: &Path, contents: &str, executable: bool) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Heroic's folders only count once Heroic has written its config.
fn heroic_root(dir: &Path) {
    file(&dir.join("config.json"), "{}", false);
}

fn proton(dir: &Path, version: Option<&str>) {
    file(&dir.join("proton"), "#!/bin/sh\n", true);
    file(&dir.join("files/bin/wine"), "#!/bin/sh\n", true);
    file(&dir.join("files/bin/wineserver"), "#!/bin/sh\n", true);
    if let Some(version) = version {
        file(
            &dir.join("version"),
            &format!("1717451631 {version}\n"),
            false,
        );
    }
}

fn wine(dir: &Path, ntdll: Option<&str>) {
    file(&dir.join("bin/wine"), "#!/bin/sh\n", true);
    file(&dir.join("bin/wineserver"), "#!/bin/sh\n", true);
    if let Some(ntdll) = ntdll {
        file(&dir.join("lib/wine/x86_64-unix/ntdll.so"), ntdll, false);
    }
}

fn names(listing: &crate::runners::Listing) -> Vec<(&str, Source)> {
    listing
        .runners
        .iter()
        .map(|runner| (runner.name(), runner.source()))
        .collect()
}

#[test]
fn derived_names_replace_each_unusable_run_with_one_dash() {
    for (folder, derived) in [
        ("GE-Proton11-3", "GE-Proton11-3"),
        ("GE-Proton-latest", "GE-Proton-latest"),
        ("a--b", "a--b"),
        ("Proton 9.0", "Proton-9.0"),
        ("Proton - Experimental", "Proton-Experimental"),
        ("Proton 8.0 (beta)", "Proton-8.0-beta"),
        ("wine lutris-7.2 ", "wine-lutris-7.2"),
        (" -.x", "x"),
        ("\u{00e9}\u{00e9}", ""),
    ] {
        assert_eq!(derived_name(folder), derived, "{folder}");
    }
}

#[test]
fn library_paths_come_from_the_vdf() {
    let text = r#""libraryfolders"
{
	"0"
	{
		"path"		"/home/me/.local/share/Steam"
		"apps" { "228980" "1" }
	}
	"1"
	{
		"path"		"/mnt/games/My \"Steam\" \\Library"
	}
}"#;
    assert_eq!(
        vdf_paths(text),
        [
            "/home/me/.local/share/Steam",
            "/mnt/games/My \"Steam\" \\Library"
        ]
    );
}

#[test]
fn unset_or_off_means_nothing_external() {
    let home = tempfile::tempdir().unwrap();
    proton(
        &home.path().join(STEAM).join("GE-Proton11-3"),
        Some("GE-Proton11-3"),
    );
    let paths = paths(home.path());

    for setting in [None, Some(false)] {
        let global = GlobalConfig {
            external_runners: setting,
            ..GlobalConfig::default()
        };
        let runners = Runners::new(&paths, &global).unwrap();
        assert!(runners.list().unwrap().runners.is_empty(), "{setting:?}");
        assert!(runners.newest_proton().unwrap().is_none(), "{setting:?}");
        assert!(matches!(
            runners.resolve("GE-Proton11-3"),
            Err(Error::UnknownRunner { .. })
        ));
    }

    let runner = runners(home.path()).resolve("GE-Proton11-3").unwrap();
    assert_eq!(runner.source(), Source::Steam);
}

#[test]
fn a_portable_folder_means_nothing_external() {
    let home = tempfile::tempdir().unwrap();
    proton(
        &home.path().join(STEAM).join("GE-Proton11-3"),
        Some("GE-Proton11-3"),
    );
    let lookup_home: OsString = home.path().as_os_str().to_owned();
    let portable = home.path().join("portable");
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| lookup_home.clone()),
        Some(&portable),
    )
    .unwrap();
    let runners = Runners::new(&paths, &on()).unwrap();
    assert!(runners.list().unwrap().runners.is_empty());
    assert!(runners.resolve("GE-Proton11-3").is_err());
}

#[test]
fn wineo_then_steam_then_heroic_then_lutris() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    heroic_root(&home.join(".config/heroic"));
    let wineo = paths(home).default_runner_root().join("shared");
    let steam = home.join(STEAM).join("shared");
    let heroic = home.join(HEROIC_WINE).join("shared");
    let lutris = home.join(LUTRIS).join("shared");
    for dir in [&wineo, &steam, &heroic, &lutris] {
        wine(dir, Some("same"));
    }
    wine(&home.join(LUTRIS).join("lutris-only"), None);
    proton(&home.join(HEROIC).join("heroic-only"), None);

    let listing = runners(home).list().unwrap();
    assert_eq!(
        names(&listing),
        [
            ("heroic-only", Source::Heroic),
            ("lutris-only", Source::Lutris),
            ("shared", Source::Wineo),
        ]
    );
    let hidden: Vec<Source> = listing.hidden.iter().map(|hidden| hidden.source).collect();
    assert_eq!(hidden, [Source::Steam, Source::Heroic, Source::Lutris]);
    assert!(listing.different_builds("shared").is_empty());

    for (dir, next) in [
        (&wineo, Source::Steam),
        (&steam, Source::Heroic),
        (&heroic, Source::Lutris),
    ] {
        fs::remove_dir_all(dir).unwrap();
        let runner = runners(home).resolve("shared").unwrap();
        assert_eq!(runner.source(), next);
        assert_eq!(
            runner.wine_path(),
            Some(
                fs::canonicalize(
                    home.join(match next {
                        Source::Steam => STEAM,
                        Source::Heroic => HEROIC_WINE,
                        _ => LUTRIS,
                    })
                    .join("shared/bin/wine")
                )
                .unwrap()
                .as_path()
            )
        );
    }
}

#[test]
fn within_a_source_the_first_folder_wins() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    heroic_root(&home.join(".config/heroic"));
    heroic_root(&home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"));
    proton(
        &home.join(HEROIC).join("GE-Proton-latest"),
        Some("GE-Proton10-3"),
    );
    proton(
        &home.join(HEROIC_FLATPAK).join("GE-Proton-latest"),
        Some("GE-Proton11-7"),
    );
    wine(&home.join(LUTRIS_FLATPAK).join("lutris-flatpak"), None);

    let listing = runners(home).list().unwrap();
    assert_eq!(
        names(&listing),
        [
            ("GE-Proton-latest", Source::Heroic),
            ("lutris-flatpak", Source::Lutris),
        ]
    );
    // The Flatpak copy is hidden, and compared like any other.
    assert_eq!(listing.hidden.len(), 1);
    assert_eq!(
        listing.hidden[0].directory,
        home.join(HEROIC_FLATPAK).join("GE-Proton-latest")
    );
    assert_eq!(
        listing.different_builds("GE-Proton-latest"),
        [Source::Heroic]
    );
    let runner = runners(home).resolve("GE-Proton-latest").unwrap();
    assert_eq!(runner.version().as_deref(), Some("GE-Proton10-3"));
}

#[test]
fn a_broken_wineo_build_is_not_replaced_by_steams() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let wineo = paths(home).default_runner_root().join("GE-Proton11-3");
    fs::create_dir_all(&wineo).unwrap();
    let steam = home.join(STEAM).join("GE-Proton11-3");
    proton(&steam, Some("GE-Proton11-3"));

    let runners = runners(home);
    assert!(matches!(
        runners.resolve("GE-Proton11-3"),
        Err(Error::NotARunner { .. })
    ));
    let listing = runners.list().unwrap();
    assert!(listing.runners.is_empty());
    assert_eq!(
        listing.skipped,
        [("GE-Proton11-3".to_owned(), crate::runners::Skipped::NoWine)]
    );
    assert_eq!(listing.hidden.len(), 1);
    assert_eq!(listing.hidden[0].source, Source::Steam);
    assert_eq!(listing.hidden[0].directory, steam);
    assert_eq!(listing.hidden[0].comparison, Comparison::Unknown);
}

#[test]
fn a_broken_first_folder_owns_the_name_within_a_source() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    heroic_root(&home.join(".config/heroic"));
    heroic_root(&home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"));
    let native = home.join(HEROIC).join("GE-Proton-latest");
    fs::create_dir_all(&native).unwrap();
    let flatpak = home.join(HEROIC_FLATPAK).join("GE-Proton-latest");
    proton(&flatpak, Some("GE-Proton11-7"));

    let runners = runners(home);
    assert!(matches!(
        runners.resolve("GE-Proton-latest"),
        Err(Error::NotARunner { .. })
    ));
    let listing = runners.list().unwrap();
    assert!(listing.runners.is_empty());
    assert_eq!(listing.external_skipped.len(), 1);
    assert_eq!(listing.external_skipped[0].directory, native);
    assert_eq!(listing.hidden.len(), 1);
    assert_eq!(listing.hidden[0].directory, flatpak);
    assert_eq!(listing.hidden[0].comparison, Comparison::Unknown);
}

#[test]
fn only_proton_counts_in_a_steam_library() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let common = home.join(".local/share/Steam/steamapps/common");
    wine(&common.join("Proton Wine Tree"), Some("ntdll"));

    let runners = runners(home);
    let listing = runners.list().unwrap();
    assert!(listing.runners.is_empty());
    assert!(listing.external_skipped.is_empty());
    assert!(listing.hidden.is_empty());
    assert!(matches!(
        runners.resolve("Proton-Wine-Tree"),
        Err(Error::UnknownRunner { .. })
    ));
}

#[test]
fn symlinked_folders_count_once() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    fs::create_dir_all(home.join(STEAM)).unwrap();
    fs::create_dir_all(home.join(".steam")).unwrap();
    symlink(home.join(".local/share/Steam"), home.join(".steam/root")).unwrap();
    symlink(home.join(".local/share/Steam"), home.join(".steam/steam")).unwrap();
    heroic_root(&home.join(".config/heroic"));
    let flatpak = home.join(".var/app/com.heroicgameslauncher.hgl/config");
    fs::create_dir_all(&flatpak).unwrap();
    symlink(home.join(".config/heroic"), flatpak.join("heroic")).unwrap();
    fs::create_dir_all(home.join(HEROIC)).unwrap();

    let found: Vec<(Source, PathBuf)> = folders(&paths(home))
        .into_iter()
        .map(|folder| (folder.source, folder.path))
        .collect();
    assert_eq!(
        found,
        [
            (Source::Steam, home.join(".steam/root/compatibilitytools.d")),
            (Source::Heroic, home.join(HEROIC)),
        ]
    );

    // A Steam entry that points into Heroic's tools is the same build there.
    proton(&home.join(HEROIC).join("Proton-CachyOS-latest"), None);
    symlink(
        home.join(HEROIC).join("Proton-CachyOS-latest"),
        home.join(STEAM).join("Proton-CachyOS-latest"),
    )
    .unwrap();
    let listing = runners(home).list().unwrap();
    assert_eq!(names(&listing), [("Proton-CachyOS-latest", Source::Steam)]);
    assert_eq!(listing.hidden.len(), 1);
    assert_eq!(listing.hidden[0].comparison, Comparison::Same);
}

#[test]
fn valve_proton_in_steam_libraries_gets_a_derived_name() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let steam = home.join(".local/share/Steam");
    let library = home.join("games/SteamLibrary");
    file(
        &steam.join("steamapps/libraryfolders.vdf"),
        &format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            steam.display(),
            library.display()
        ),
        false,
    );
    proton(
        &steam.join("steamapps/common/Proton 9.0"),
        Some("proton-9.0-4"),
    );
    proton(
        &library.join("steamapps/common/Proton - Experimental"),
        Some("experimental-9.0"),
    );
    // Not Proton, or not a runner: neither is reported.
    fs::create_dir_all(library.join("steamapps/common/Proton EasyAntiCheat Runtime")).unwrap();
    proton(&library.join("steamapps/common/Hades"), None);

    let runners = runners(home);
    let listing = runners.list().unwrap();
    assert_eq!(
        names(&listing),
        [
            ("Proton-9.0", Source::Steam),
            ("Proton-Experimental", Source::Steam),
        ]
    );
    assert!(listing.external_skipped.is_empty());
    for name in ["Proton-9.0", "Proton-Experimental"] {
        let runner = runners.resolve(name).unwrap();
        assert_eq!(runner.name(), name);
        assert!(runner.is_proton());
    }
    assert!(runners.resolve("Hades").is_err());
}

#[test]
fn unusable_external_folders_are_reported() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    fs::create_dir_all(home.join(STEAM).join("broken")).unwrap();
    proton(&home.join(STEAM).join("GE-Proton-Latest"), None);
    wine(&home.join(STEAM).join("wine"), None);
    fs::create_dir_all(home.join(STEAM).join(".hidden")).unwrap();

    let listing = runners(home).list().unwrap();
    assert!(listing.runners.is_empty());
    let mut reasons: Vec<(String, crate::runners::Skipped)> = listing
        .external_skipped
        .iter()
        .map(|skipped| {
            (
                skipped
                    .directory
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                skipped.reason,
            )
        })
        .collect();
    reasons.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        reasons,
        [
            (
                "GE-Proton-Latest".to_owned(),
                crate::runners::Skipped::AliasReserved
            ),
            ("broken".to_owned(), crate::runners::Skipped::NoWine),
            ("wine".to_owned(), crate::runners::Skipped::Reserved),
        ]
    );
}

#[test]
fn identity_tells_same_different_and_unknown_apart() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    heroic_root(&home.join(".config/heroic"));
    let steam = home.join(STEAM);
    let heroic = home.join(HEROIC);
    let heroic_wine = home.join(HEROIC_WINE);

    proton(&steam.join("proton-same"), Some("GE-Proton10-3"));
    proton(&heroic.join("proton-same"), Some("GE-Proton10-3"));
    proton(&steam.join("proton-different"), Some("GE-Proton10-3"));
    proton(&heroic.join("proton-different"), Some("GE-Proton11-7"));
    proton(&steam.join("proton-unknown"), Some("GE-Proton10-3"));
    proton(&heroic.join("proton-unknown"), None);
    wine(&steam.join("wine-same"), Some("ntdll"));
    wine(&heroic_wine.join("wine-same"), Some("ntdll"));
    wine(&steam.join("wine-different"), Some("ntdll"));
    wine(&heroic_wine.join("wine-different"), Some("ntdll, patched"));
    wine(&steam.join("wine-unknown"), Some("ntdll"));
    wine(&heroic_wine.join("wine-unknown"), None);
    // The lib64 layout counts too.
    wine(&steam.join("wine-lib64"), None);
    file(
        &steam.join("wine-lib64/lib64/wine/x86_64-unix/ntdll.so"),
        "a",
        false,
    );
    wine(&heroic_wine.join("wine-lib64"), None);
    file(
        &heroic_wine.join("wine-lib64/lib64/wine/x86_64-unix/ntdll.so"),
        "b",
        false,
    );

    let listing = runners(home).list().unwrap();
    let comparison = |name: &str| {
        listing
            .hidden
            .iter()
            .find(|hidden| hidden.name == name)
            .map(|hidden| hidden.comparison)
            .unwrap()
    };
    assert_eq!(comparison("proton-same"), Comparison::Same);
    assert_eq!(comparison("proton-different"), Comparison::Different);
    assert_eq!(comparison("proton-unknown"), Comparison::Unknown);
    assert_eq!(comparison("wine-same"), Comparison::Same);
    assert_eq!(comparison("wine-different"), Comparison::Different);
    assert_eq!(comparison("wine-unknown"), Comparison::Unknown);
    assert_eq!(comparison("wine-lib64"), Comparison::Different);

    assert_eq!(
        listing.different_builds("proton-different"),
        [Source::Heroic]
    );
    assert_eq!(
        listing
            .hidden
            .iter()
            .find(|h| h.name == "proton-different")
            .unwrap()
            .directory,
        heroic.join("proton-different")
    );
    for quiet in ["proton-same", "proton-unknown", "wine-same", "wine-unknown"] {
        assert!(listing.different_builds(quiet).is_empty(), "{quiet}");
    }
}

#[test]
fn aliases_ignore_external_builds() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    proton(
        &home.join(STEAM).join("GE-Proton11-7"),
        Some("GE-Proton11-7"),
    );
    let runners = runners(home);
    assert!(matches!(
        runners.resolve("GE-Proton-Latest"),
        Err(Error::RunnerAlias { .. })
    ));
    proton(
        &paths(home).default_runner_root().join("GE-Proton9-20"),
        Some("GE-Proton9-20"),
    );
    let root = paths(home).default_runner_root().join("GE-Proton9-20");
    for bin in ["wine", "wineserver"] {
        file(&root.join("files/bin").join(bin), "#!/bin/sh\n", true);
    }
    let alias = runners.resolve("GE-Proton-Latest").unwrap();
    assert_eq!(alias.name(), "GE-Proton9-20");
    assert_eq!(alias.source(), Source::Wineo);
}

#[test]
fn the_newest_proton_comes_from_any_source() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    heroic_root(&home.join(".config/heroic"));
    proton(
        &paths(home).default_runner_root().join("GE-Proton9-20"),
        Some("GE-Proton9-20"),
    );
    proton(
        &home.join(HEROIC).join("GE-Proton11-1"),
        Some("GE-Proton11-1"),
    );
    let newest = runners(home).newest_proton().unwrap().unwrap();
    assert_eq!(newest.name(), "GE-Proton11-1");
    assert_eq!(newest.source(), Source::Heroic);
}

#[test]
fn external_builds_lock_by_name_and_are_never_removed() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let paths = paths(home);
    let dir = home.join(STEAM).join("GE-Proton11-3");
    proton(&dir, Some("GE-Proton11-3"));
    crate::settings::set_external_runners(&paths, true).unwrap();
    let runners = Runners::new(&paths, &GlobalConfig::load(&paths).unwrap()).unwrap();

    let (runner, guard) = runners.resolve_locked(&paths, "GE-Proton11-3").unwrap();
    assert_eq!(runner.source(), Source::Steam);
    assert!(guard.is_some());
    assert!(
        crate::build_lock::try_exclusive(&paths, crate::build_lock::Kind::Runner, "GE-Proton11-3")
            .unwrap()
            .is_none()
    );
    drop(guard);

    let error = crate::runners::remove(&paths, "GE-Proton11-3").unwrap_err();
    assert_eq!(error.to_string(), "GE-Proton11-3 belongs to Steam.");
    assert!(matches!(
        crate::runners::size(&paths, "GE-Proton11-3"),
        Err(Error::ExternalBuild { .. })
    ));
    assert!(dir.join("proton").is_file());

    // Steam deleting it shows up as the usual unknown runner.
    let selected = runners.resolve("GE-Proton11-3").unwrap();
    fs::remove_dir_all(&dir).unwrap();
    assert!(matches!(
        runners.lock_resolved(&paths, "GE-Proton11-3", selected),
        Err(Error::UnknownRunner { .. })
    ));
}

/// Points wineo's runners root at `root` in the global config, whatever the
/// `external_runners` setting is.
fn root_at(paths: &Paths, root: &Path, setting: Option<bool>) {
    let mut text = String::new();
    if let Some(setting) = setting {
        text.push_str(&format!("external_runners = {setting}\n\n"));
    }
    text.push_str(&format!("[defaults]\nrunner_root = '{}'\n", root.display()));
    file(&paths.global_config(), &text, false);
}

#[test]
fn a_root_that_is_a_launcher_folder_refuses_removal_whatever_the_setting() {
    for setting in [None, Some(false), Some(true)] {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let paths = paths(home);
        let steam = home.join(STEAM);
        proton(&steam.join("GE-Proton11-3"), Some("GE-Proton11-3"));
        root_at(&paths, &steam, setting);

        // It still launches as wineo's own.
        let global = GlobalConfig::load(&paths).unwrap();
        let runner = Runners::new(&paths, &global)
            .unwrap()
            .resolve("GE-Proton11-3")
            .unwrap();
        assert_eq!(runner.source(), Source::Wineo, "{setting:?}");

        let error = crate::runners::remove(&paths, "GE-Proton11-3").unwrap_err();
        assert_eq!(
            error.to_string(),
            "GE-Proton11-3 belongs to Steam.",
            "{setting:?}"
        );
        assert!(matches!(
            crate::runners::size(&paths, "GE-Proton11-3"),
            Err(Error::ExternalBuild { .. })
        ));
        assert!(steam.join("GE-Proton11-3/proton").is_file());
    }
}

#[test]
fn a_root_reached_through_a_symlink_into_a_launcher_folder_is_still_theirs() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let paths = paths(home);
    heroic_root(&home.join(".config/heroic"));
    let tools = home.join(HEROIC);
    proton(&tools.join("GE-Proton11-1"), Some("GE-Proton11-1"));

    // A link to the folder itself, and a link to a folder inside it.
    let link = home.join("runners-link");
    symlink(&tools, &link).unwrap();
    root_at(&paths, &link, None);
    let error = crate::runners::remove(&paths, "GE-Proton11-1").unwrap_err();
    assert_eq!(error.to_string(), "GE-Proton11-1 belongs to Heroic.");

    let nested = tools.join("nested");
    proton(&nested.join("GE-Proton11-2"), Some("GE-Proton11-2"));
    let link = home.join("nested-link");
    symlink(&nested, &link).unwrap();
    root_at(&paths, &link, None);
    let error = crate::runners::remove(&paths, "GE-Proton11-2").unwrap_err();
    assert_eq!(error.to_string(), "GE-Proton11-2 belongs to Heroic.");
    assert!(nested.join("GE-Proton11-2/proton").is_file());
}

#[test]
fn an_unrelated_root_still_removes_and_one_above_a_launcher_folder_is_unaffected() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let paths = paths(home);
    heroic_root(&home.join(".config/heroic"));
    proton(
        &home.join(HEROIC).join("GE-Proton11-1"),
        Some("GE-Proton11-1"),
    );

    let own = home.join("my-runners");
    proton(&own.join("GE-Proton9-20"), Some("GE-Proton9-20"));
    root_at(&paths, &own, Some(true));
    crate::runners::remove(&paths, "GE-Proton9-20").unwrap();
    assert!(!own.join("GE-Proton9-20").exists());

    // A root that only contains a launcher folder further down is just a root.
    let above = home.join(".config");
    proton(&above.join("GE-Proton9-21"), Some("GE-Proton9-21"));
    // The gaming template follows the newest Proton, so keep this one off it.
    proton(&above.join("GE-Proton10-1"), Some("GE-Proton10-1"));
    root_at(&paths, &above, None);
    crate::runners::remove(&paths, "GE-Proton9-21").unwrap();
    assert!(!above.join("GE-Proton9-21").exists());
    assert!(home.join(HEROIC).join("GE-Proton11-1/proton").is_file());
}

#[test]
fn the_display_owner_is_the_launcher_for_a_root_inside_its_folder() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let paths = paths(home);
    let steam = home.join(STEAM);
    proton(&steam.join("GE-Proton11-3"), Some("GE-Proton11-3"));
    let own = home.join("my-runners");
    proton(&own.join("GE-Proton9-20"), Some("GE-Proton9-20"));
    heroic_root(&home.join(".config/heroic"));
    proton(
        &home.join(HEROIC).join("GE-Proton11-1"),
        Some("GE-Proton11-1"),
    );

    root_at(&paths, &steam, Some(true));
    let global = GlobalConfig::load(&paths).unwrap();
    let runners = Runners::new(&paths, &global).unwrap();
    let runner = runners.resolve("GE-Proton11-3").unwrap();
    assert_eq!(runner.source(), Source::Wineo);
    assert_eq!(runner.owner(&paths), Source::Steam);
    let heroic = runners.resolve("GE-Proton11-1").unwrap();
    assert_eq!(heroic.owner(&paths), Source::Heroic);

    root_at(&paths, &own, Some(true));
    let global = GlobalConfig::load(&paths).unwrap();
    let runners = Runners::new(&paths, &global).unwrap();
    let runner = runners.resolve("GE-Proton9-20").unwrap();
    assert_eq!(runner.owner(&paths), Source::Wineo);
}
