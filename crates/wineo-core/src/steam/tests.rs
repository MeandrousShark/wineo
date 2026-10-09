use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::*;

const FLATPAK: &str = ".var/app/com.valvesoftware.Steam/.local/share/Steam";
const BASE: u64 = 76_561_197_960_265_728;
const UPDATING_BITS: [u32; 9] = [
    256, 512, 1024, 131_072, 262_144, 524_288, 1_048_576, 2_097_152, 4_194_304,
];

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A native install at `home` with its `steamapps` folder, and no other
/// libraries until `libraryfolders` is written.
fn native(home: &Path) -> PathBuf {
    let root = home.join(".local/share/Steam");
    fs::create_dir_all(root.join("steamapps")).unwrap();
    root
}

/// A `libraryfolders.vdf` listing `paths` as numbered libraries, each with a
/// stale `apps` list naming app 500000, which has no manifest anywhere.
fn libraryfolders(paths: &[&Path]) -> String {
    let mut text = String::from("\"libraryfolders\"\n{\n");
    for (index, path) in paths.iter().enumerate() {
        text.push_str(&format!(
            "\t\"{index}\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"500000\"\t\t\"1\"\n\t\t}}\n\t}}\n",
            path.display()
        ));
    }
    text.push_str("}\n");
    text
}

fn acf(app_id: &str, name: &str, flags: u32) -> String {
    format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\t\"Universe\"\t\t\"1\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"{flags}\"\n\t\"installdir\"\t\t\"{name}\"\n\t\"SizeOnDisk\"\t\t\"1024\"\n\t\"LastPlayed\"\t\t\"1700000000\"\n\t\"BytesToDownload\"\t\t\"0\"\n\t\"BytesDownloaded\"\t\t\"0\"\n\t\"BytesToStage\"\t\t\"0\"\n\t\"BytesStaged\"\t\t\"0\"\n}}\n"
    )
}

fn game(library: &Path, app_id: &str, name: &str, flags: u32) {
    write(
        &library
            .join("steamapps")
            .join(format!("appmanifest_{app_id}.acf")),
        &acf(app_id, name, flags),
    );
}

fn only(home: &Path) -> Install {
    let mut found = installs(home);
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

fn loginusers(root: &Path, accounts: &[(u64, bool)]) {
    let mut text = String::from("\"users\"\n{\n");
    for (steam_id, recent) in accounts {
        text.push_str(&format!(
            "\t\"{steam_id}\"\n\t{{\n\t\t\"AccountName\"\t\t\"someone\"\n\t\t\"MostRecent\"\t\t\"{}\"\n\t}}\n",
            u8::from(*recent)
        ));
    }
    text.push_str("}\n");
    write(&root.join("config/loginusers.vdf"), &text);
}

fn localconfig(root: &Path, account: u64, apps: &str) {
    let text = format!(
        "\"UserLocalConfigStore\"\n{{\n\t\"Software\"\n\t{{\n\t\t\"Valve\"\n\t\t{{\n\t\t\t\"Steam\"\n\t\t\t{{\n\t\t\t\t\"apps\"\n\t\t\t\t{{\n{apps}\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t}}\n}}\n"
    );
    write(
        &root
            .join("userdata")
            .join((account - BASE).to_string())
            .join("config/localconfig.vdf"),
        &text,
    );
}

fn app_entry(app_id: &str, playtime: &str, last_played: &str) -> String {
    format!(
        "\t\t\t\t\t\"{app_id}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"Playtime\"\t\t\"{playtime}\"\n\t\t\t\t\t\t\"LastPlayed\"\t\t\"{last_played}\"\n\t\t\t\t\t}}\n"
    )
}

#[test]
fn native_and_flatpak_are_both_found_and_tagged() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    native(home);
    fs::create_dir_all(home.join(FLATPAK).join("steamapps")).unwrap();

    let found = installs(home);
    let listed: Vec<(InstallKind, PathBuf)> = found
        .iter()
        .map(|install| (install.kind(), install.root().to_owned()))
        .collect();
    assert_eq!(
        listed,
        [
            (InstallKind::Native, home.join(".local/share/Steam")),
            (InstallKind::Flatpak, home.join(FLATPAK)),
        ]
    );
    let active = |preference| active_install(&found, preference).unwrap().kind();
    assert_eq!(active(None), InstallKind::Native);
    assert_eq!(active(Some(InstallKind::Native)), InstallKind::Native);
    assert_eq!(active(Some(InstallKind::Flatpak)), InstallKind::Flatpak);
}

#[test]
fn a_lone_install_is_active_whatever_the_preference() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    fs::create_dir_all(home.join(FLATPAK)).unwrap();
    let found = installs(home);
    assert_eq!(found.len(), 1);
    for preference in [None, Some(InstallKind::Native), Some(InstallKind::Flatpak)] {
        let active = active_install(&found, preference).unwrap();
        assert_eq!(active.kind(), InstallKind::Flatpak, "{preference:?}");
    }

    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    native(home);
    let found = installs(home);
    let active = active_install(&found, Some(InstallKind::Flatpak)).unwrap();
    assert_eq!(active.kind(), InstallKind::Native);

    let empty = tempfile::tempdir().unwrap();
    assert!(installs(empty.path()).is_empty());
    assert!(active_install(&[], None).is_none());
}

#[test]
fn symlinked_roots_are_one_install_under_the_first_path() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    fs::create_dir_all(home.join(".steam")).unwrap();
    symlink(&root, home.join(".steam/steam")).unwrap();
    symlink(&root, home.join(".steam/root")).unwrap();
    symlink(&root, home.join(".steam/steam-again")).unwrap();
    fs::create_dir_all(home.join(FLATPAK)).unwrap();

    let found = installs(home);
    let listed: Vec<(InstallKind, PathBuf)> = found
        .iter()
        .map(|install| (install.kind(), install.root().to_owned()))
        .collect();
    assert_eq!(
        listed,
        [
            (InstallKind::Native, home.join(".steam/root")),
            (InstallKind::Flatpak, home.join(FLATPAK)),
        ]
    );
}

#[test]
fn a_dangling_symlink_is_not_an_install() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    fs::create_dir_all(home.join(".steam")).unwrap();
    symlink(home.join("nowhere"), home.join(".steam/root")).unwrap();
    symlink(&root, home.join(".steam/steam")).unwrap();

    let install = only(home);
    assert_eq!(install.kind(), InstallKind::Native);
    assert_eq!(install.root(), home.join(".steam/steam"));
}

#[test]
fn the_root_is_always_a_library_and_unavailable_ones_are_marked() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    let ok = home.join("games/ok");
    fs::create_dir_all(ok.join("steamapps")).unwrap();
    let gone = home.join("mnt/gone");
    let broken = home.join("games/broken");
    fs::create_dir_all(&broken).unwrap();
    write(&broken.join("steamapps"), "not a folder");
    write(
        &root.join("steamapps/libraryfolders.vdf"),
        &libraryfolders(&[&root, &ok, &gone, &broken]),
    );

    let listing = libraries(&only(home));
    assert_eq!(listing.problem, None);
    let listed: Vec<(PathBuf, bool)> = listing
        .entries
        .iter()
        .map(|library| (library.path.clone(), library.available))
        .collect();
    assert_eq!(
        listed,
        [
            (root.clone(), true),
            (ok.clone(), true),
            (gone.clone(), false),
            (broken.clone(), false),
        ]
    );
}

#[test]
fn a_missing_libraryfolders_file_leaves_just_the_root() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    let listing = libraries(&only(home));
    assert_eq!(listing.problem, None);
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, root);
    assert!(listing.entries[0].available);
}

#[test]
fn an_unparsable_libraryfolders_file_is_a_problem_not_a_crash() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    write(
        &root.join("steamapps/libraryfolders.vdf"),
        "\"libraryfolders\" { \"0\" { \"path\" \"/x\"",
    );
    let install = only(home);
    let listing = libraries(&install);
    assert!(listing.problem.unwrap().contains("can't be parsed"));
    assert_eq!(listing.entries.len(), 1);

    // Without the list, a game that isn't in the root can't be ruled out.
    assert!(matches!(
        app_state(Some(&install), "1086940"),
        AppState::Unknown(_)
    ));
    game(&root, "1086940", "Baldur's Gate 3", 4);
    assert_eq!(app_state(Some(&install), "1086940"), AppState::Installed);
}

#[test]
fn a_stale_apps_list_is_not_trusted_either_way() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    // Listed under `apps` in libraryfolders.vdf, but with no manifest.
    write(
        &root.join("steamapps/libraryfolders.vdf"),
        &libraryfolders(&[&root]),
    );
    // Has a manifest, but the `apps` list doesn't name it.
    game(&root, "1086940", "Baldur's Gate 3", 4);
    let install = only(home);

    let ids: Vec<String> = manifests(&install)
        .into_iter()
        .map(|entry| entry.app_id)
        .collect();
    assert_eq!(ids, ["1086940"]);
    assert_eq!(app_state(Some(&install), "500000"), AppState::NotInstalled);
    assert_eq!(app_state(Some(&install), "1086940"), AppState::Installed);
}

#[test]
fn a_malformed_manifest_only_costs_its_own_app() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    game(&root, "1070560", "Steam Linux Runtime 3.0 (sniper)", 4);
    game(&root, "1086940", "Baldur's Gate 3", 4);
    write(
        &root.join("steamapps/appmanifest_1245620.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\" \"Broken",
    );
    game(&root, "228980", "Steamworks Common Redistributables", 4);
    write(&root.join("steamapps/appmanifest_abc.acf"), "ignored");
    write(&root.join("steamapps/notes.txt"), "ignored");
    let install = only(home);

    let entries = manifests(&install);
    let ids: Vec<&str> = entries.iter().map(|entry| entry.app_id.as_str()).collect();
    assert_eq!(ids, ["1070560", "1086940", "1245620", "228980"]);
    assert!(entries.iter().all(|entry| entry.library == root));

    let error = entries[2].manifest.as_ref().unwrap_err();
    assert!(error.contains("appmanifest_1245620.acf"), "{error}");
    assert!(error.contains("line 4"), "{error}");

    let healthy: Vec<(&str, bool)> = entries
        .iter()
        .filter_map(|entry| entry.manifest.as_ref().ok())
        .map(|manifest| (manifest.name.as_str(), manifest.is_tool()))
        .collect();
    assert_eq!(
        healthy,
        [
            ("Steam Linux Runtime 3.0 (sniper)", true),
            ("Baldur's Gate 3", false),
            ("Steamworks Common Redistributables", true),
        ]
    );
    assert_eq!(
        app_state(Some(&install), "1245620"),
        AppState::Unknown(error.clone())
    );
    assert_eq!(app_state(Some(&install), "1086940"), AppState::Installed);
}

/// Puts a file's mode back when dropped, so the temporary folder can be removed
/// even when a test fails part way through.
struct Restore(PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o644));
    }
}

#[test]
fn an_unreadable_manifest_only_costs_its_own_app() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    game(&root, "1086940", "Baldur's Gate 3", 4);
    let locked = root.join("steamapps/appmanifest_1245620.acf");
    write(&locked, &acf("1245620", "Locked", 4));
    let _restore = Restore(locked.clone());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_to_string(&locked).is_ok() {
        // Running as root, or on a filesystem that ignores modes, still reads it,
        // so there's no unreadable file to test.
        return;
    }
    let install = only(home);

    let entries = manifests(&install);
    let ids: Vec<&str> = entries.iter().map(|entry| entry.app_id.as_str()).collect();
    assert_eq!(ids, ["1086940", "1245620"]);
    assert_eq!(
        entries[0].manifest.as_ref().unwrap().state(),
        InstallState::Installed
    );
    let error = entries[1].manifest.as_ref().unwrap_err();
    assert!(error.contains("appmanifest_1245620.acf"), "{error}");
    assert!(error.contains("can't be read"), "{error}");
    match app_state(Some(&install), "1245620") {
        AppState::Unknown(reason) => assert!(reason.contains("can't be read"), "{reason}"),
        other => panic!("expected unknown, got {other:?}"),
    }
    assert_eq!(app_state(Some(&install), "1086940"), AppState::Installed);
}

#[test]
fn a_manifest_stating_another_app_id_is_an_error() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    write(
        &root.join("steamapps/appmanifest_1086940.acf"),
        &acf("999", "Other", 4),
    );
    let entries = manifests(&only(home));
    let error = entries[0].manifest.as_ref().unwrap_err();
    assert!(error.contains("says appid 999"), "{error}");
}

#[test]
fn missing_numbers_default_to_zero_and_bad_ones_fail() {
    let sparse = "\"AppState\" { \"appid\" \"1\" \"name\" \"Sparse\" \"StateFlags\" \"4\" }";
    let manifest = parse_manifest(sparse, "1").unwrap();
    assert_eq!(manifest.name, "Sparse");
    assert_eq!(manifest.install_dir, "");
    assert_eq!(manifest.size_on_disk, 0);
    assert_eq!(manifest.last_played, 0);
    assert_eq!(manifest.bytes_staged, 0);
    assert_eq!(manifest.state(), InstallState::Installed);

    let no_flags = "\"AppState\" { \"appid\" \"1\" \"name\" \"NoFlags\" }";
    assert_eq!(
        parse_manifest(no_flags, "1").unwrap_err(),
        "has no StateFlags"
    );
    let bad_size = "\"AppState\" { \"StateFlags\" \"4\" \"SizeOnDisk\" \"lots\" }";
    assert_eq!(
        parse_manifest(bad_size, "1").unwrap_err(),
        "SizeOnDisk isn't a number"
    );
    assert_eq!(
        parse_manifest("\"Other\" { }", "1").unwrap_err(),
        "has no AppState section"
    );
}

#[test]
fn state_flags_table() {
    let cases = [
        (0, InstallState::NotInstalled),
        (1, InstallState::NotInstalled),
        (2, InstallState::NotInstalled),
        (64, InstallState::NotInstalled),
        (2 | 64, InstallState::NotInstalled),
        (4, InstallState::Installed),
        (6, InstallState::Installed),
        (4 | 2, InstallState::Installed),
        (4 | 32, InstallState::Installed),
        (4 | 128, InstallState::Installed),
        (4 | 64, InstallState::Installed),
        (4 | 8 | 16, InstallState::Installed),
        (4 | 2 | 32 | 128, InstallState::Installed),
        (2048, InstallState::NotInstalled),
        (4 | 2048, InstallState::NotInstalled),
        (2048 | 1_048_576, InstallState::NotInstalled),
    ];
    for (flags, expected) in cases {
        assert_eq!(state_from_flags(flags), expected, "flags {flags}");
    }
    for bit in UPDATING_BITS {
        assert_eq!(state_from_flags(bit), InstallState::Updating, "bit {bit}");
        assert_eq!(
            state_from_flags(bit | 4),
            InstallState::Updating,
            "bit {bit} with FullyInstalled"
        );
        assert_eq!(
            state_from_flags(bit | 2),
            InstallState::Updating,
            "bit {bit} with UpdateRequired"
        );
    }
    assert_eq!(
        state_from_flags(4 | 256 | 1_048_576 | 4_194_304),
        InstallState::Updating
    );
}

#[test]
fn a_staging_only_stretch_shows_progress() {
    let snapshot = |downloaded: u64, staged: u64, flags: u32| {
        let text = format!(
            "\"AppState\" {{ \"appid\" \"1086940\" \"StateFlags\" \"{flags}\" \"BytesDownloaded\" \"{downloaded}\" \"BytesStaged\" \"{staged}\" }}"
        );
        parse_manifest(&text, "1086940").unwrap().progress()
    };
    // Staging: the bytes are already downloaded, and only BytesStaged moves.
    let before = snapshot(5_000, 100, 2_097_152 | 4);
    let after = snapshot(5_000, 200, 2_097_152 | 4);
    assert!(after.progressed_since(&before));
    assert!(!before.progressed_since(&before));
    assert!(!snapshot(5_000, 50, 2_097_152 | 4).progressed_since(&before));
    assert!(snapshot(5_000, 50, 2_097_152).progressed_since(&before));

    assert!(snapshot(6_000, 100, 1_048_576).progressed_since(&before));
    assert!(snapshot(5_000, 100, 1_048_576 | 4).progressed_since(&before));
    assert!(!snapshot(4_000, 90, 2_097_152 | 4).progressed_since(&before));
}

#[test]
fn tools_are_known_ids_or_names() {
    for app_id in TOOL_APP_IDS {
        assert!(is_tool(app_id, "Anything"), "{app_id}");
    }
    for name in [
        "Proton Experimental",
        "Proton 9.0-4",
        "Steam Linux Runtime 2.0 (scout)",
        "Steamworks Shared",
    ] {
        assert!(is_tool("1", name), "{name}");
    }
    for name in [
        "Baldur's Gate 3",
        "Hades",
        "Half-Life",
        "Protonic Pulse",
        "Steamworld Dig",
    ] {
        assert!(!is_tool("1086940", name), "{name}");
    }
}

#[test]
fn app_state_covers_each_state() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    let gone = home.join("mnt/gone");
    write(
        &root.join("steamapps/libraryfolders.vdf"),
        &libraryfolders(&[&root, &gone]),
    );
    game(&root, "1", "Installed", 4);
    game(&root, "2", "Updating", 1_048_576);
    game(&root, "3", "Uninstalling", 2048 | 4);
    game(&root, "4", "Paused", 512 | 2);
    let install = only(home);
    let state = |app_id| app_state(Some(&install), app_id);

    assert_eq!(state("1"), AppState::Installed);
    assert_eq!(state("2"), AppState::Updating);
    assert_eq!(state("3"), AppState::NotInstalled);
    assert_eq!(state("4"), AppState::Updating);
    assert_eq!(state("999"), AppState::Unavailable(gone.clone()));
    assert_eq!(state("500000"), AppState::Unavailable(gone));
    assert!(
        matches!(app_state(None, "1"), AppState::Unknown(text) if text.contains("isn't found"))
    );
    assert!(
        matches!(state("../1"), AppState::Unknown(text) if text.contains("isn't a Steam app id"))
    );
}

#[test]
fn an_unreadable_manifest_is_unknown_and_a_missing_one_is_not_installed() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    write(
        &root.join("steamapps/libraryfolders.vdf"),
        &libraryfolders(&[&root]),
    );
    write(
        &root.join("steamapps/appmanifest_77.acf"),
        "\"AppState\" { \"StateFlags\" \"4\"",
    );
    let install = only(home);
    match app_state(Some(&install), "77") {
        AppState::Unknown(reason) => {
            assert!(reason.contains("appmanifest_77.acf"), "{reason}");
            assert!(reason.contains("never closed"), "{reason}");
        }
        other => panic!("expected unknown, got {other:?}"),
    }
    assert_eq!(app_state(Some(&install), "78"), AppState::NotInstalled);
}

#[test]
fn an_app_only_the_inactive_install_has_is_not_installed() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let native_root = native(home);
    let flatpak_root = home.join(FLATPAK);
    fs::create_dir_all(flatpak_root.join("steamapps")).unwrap();
    game(&flatpak_root, "1086940", "Baldur's Gate 3", 4);
    game(&native_root, "1245620", "Elden Ring", 4);

    let found = installs(home);
    let native_active = active_install(&found, None);
    assert_eq!(app_state(native_active, "1086940"), AppState::NotInstalled);
    assert_eq!(app_state(native_active, "1245620"), AppState::Installed);
    let flatpak_active = active_install(&found, Some(InstallKind::Flatpak));
    assert_eq!(app_state(flatpak_active, "1086940"), AppState::Installed);
    assert_eq!(app_state(flatpak_active, "1245620"), AppState::NotInstalled);
}

#[test]
fn a_mixed_fixture_lists_installs_libraries_and_manifests() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let native_root = native(home);
    let games = home.join("games/SteamLibrary");
    fs::create_dir_all(games.join("steamapps")).unwrap();
    let gone = home.join("mnt/gone");
    write(
        &native_root.join("steamapps/libraryfolders.vdf"),
        &libraryfolders(&[&native_root, &games, &gone]),
    );
    game(&native_root, "1086940", "Baldur's Gate 3", 4);
    game(&games, "1245620", "Elden Ring", 1_048_576 | 4);
    game(&games, "1070560", "Steam Linux Runtime 3.0 (sniper)", 4);
    game(&games, "1245621", "Proton 9.0", 4);
    write(
        &games.join("steamapps/appmanifest_1300000.acf"),
        "\"AppState\" { \"StateFlags\" \"4\"",
    );
    fs::create_dir_all(home.join(".steam")).unwrap();
    symlink(&native_root, home.join(".steam/root")).unwrap();
    let flatpak_root = home.join(FLATPAK);
    fs::create_dir_all(flatpak_root.join("steamapps")).unwrap();

    let found = installs(home);
    assert_eq!(
        found.iter().map(Install::kind).collect::<Vec<_>>(),
        [InstallKind::Native, InstallKind::Flatpak]
    );
    let install = active_install(&found, None).unwrap();
    assert_eq!(install.root(), home.join(".steam/root"));

    let listing = libraries(install);
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|library| (library.path.as_path(), library.available))
            .collect::<Vec<_>>(),
        [
            (install.root(), true),
            (games.as_path(), true),
            (gone.as_path(), false),
        ]
    );

    let entries = manifests(install);
    let summary: Vec<(&str, bool, bool)> = entries
        .iter()
        .map(|entry| match &entry.manifest {
            Ok(manifest) => (manifest.app_id.as_str(), true, manifest.is_tool()),
            Err(_) => (entry.app_id.as_str(), false, false),
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("1086940", true, false),
            ("1070560", true, true),
            ("1245620", true, false),
            ("1245621", true, true),
            ("1300000", false, false),
        ]
    );
    assert!(matches!(
        app_state(Some(install), "1300000"),
        AppState::Unknown(_)
    ));
    assert_eq!(app_state(Some(install), "1245620"), AppState::Updating);
    assert_eq!(app_state(Some(install), "1086940"), AppState::Installed);
    assert_eq!(
        app_state(Some(install), "9999999"),
        AppState::Unavailable(gone)
    );
}

#[test]
fn the_most_recent_account_is_the_one_read() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    loginusers(&root, &[(BASE + 1, false), (BASE + 2, true)]);
    let install = only(home);
    assert_eq!(active_account(&install).unwrap(), 2);
}

#[test]
fn a_single_account_is_used_without_a_flag() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    loginusers(&root, &[(BASE + 7, false)]);
    assert_eq!(active_account(&only(home)).unwrap(), 7);
}

#[test]
fn accounts_that_cannot_be_told_apart_are_unknown() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    let install = only(home);
    assert!(
        active_account(&install)
            .unwrap_err()
            .contains("can't be read")
    );
    assert!(matches!(playtime(&install, "1"), Playtime::Unknown(_)));

    loginusers(&root, &[]);
    assert_eq!(
        active_account(&install).unwrap_err(),
        "Steam has no logged-in account"
    );

    loginusers(&root, &[(BASE + 1, false), (BASE + 2, false)]);
    assert_eq!(
        active_account(&install).unwrap_err(),
        "Steam doesn't say which account was used last"
    );
    assert!(matches!(playtime(&install, "1"), Playtime::Unknown(_)));
}

#[test]
fn playtime_is_known_zero_or_unknown() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    loginusers(&root, &[(BASE + 1, true), (BASE + 2, false)]);
    let install = only(home);

    let apps = format!(
        "{}{}",
        app_entry("1086940", "90", "1700000000"),
        app_entry("1245620", "0", "0")
    );
    localconfig(&root, BASE + 1, &apps);
    assert_eq!(
        playtime(&install, "1086940"),
        Playtime::Known {
            seconds: 5400,
            last_played: Some(1_700_000_000),
        }
    );
    assert_eq!(
        playtime(&install, "1245620"),
        Playtime::Known {
            seconds: 0,
            last_played: None,
        }
    );
    assert_eq!(
        playtime(&install, "1300000"),
        Playtime::Known {
            seconds: 0,
            last_played: None,
        }
    );

    // Only the most recent account's file counts, and it's missing for this one.
    localconfig(&root, BASE + 2, &app_entry("1086940", "600", "1"));
    fs::remove_file(root.join("userdata/1/config/localconfig.vdf")).unwrap();
    assert!(
        matches!(playtime(&install, "1086940"), Playtime::Unknown(text) if text.contains("can't be read"))
    );
    loginusers(&root, &[(BASE + 1, false), (BASE + 2, true)]);
    assert_eq!(
        playtime(&install, "1086940"),
        Playtime::Known {
            seconds: 36_000,
            last_played: Some(1),
        }
    );

    localconfig(
        &root,
        BASE + 2,
        "\t\t\t\t\t\"1086940\" { \"Playtime\" \"lots\" }\n",
    );
    assert!(
        matches!(playtime(&install, "1086940"), Playtime::Unknown(text) if text.contains("Playtime isn't a number"))
    );
    write(
        &root.join("userdata/2/config/localconfig.vdf"),
        "\"UserLocalConfigStore\" {",
    );
    assert!(
        matches!(playtime(&install, "1086940"), Playtime::Unknown(text) if text.contains("can't be parsed"))
    );
}

#[test]
fn playtime_keys_ignore_case() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    loginusers(&root, &[(BASE + 3, true)]);
    write(
        &root.join("userdata/3/config/localconfig.vdf"),
        "\"userlocalconfigstore\" { \"software\" { \"valve\" { \"steam\" { \"APPS\" { \"1086940\" { \"playtime\" \"2\" \"lastplayed\" \"5\" } } } } } }",
    );
    assert_eq!(
        playtime(&only(home), "1086940"),
        Playtime::Known {
            seconds: 120,
            last_played: Some(5),
        }
    );
}

#[test]
fn compatibility_tools_lists_mappings_and_keeps_empty_entries() {
    let home = tempfile::tempdir().unwrap();
    let root = native(home.path());
    let install = only(home.path());
    write(
        &root.join("config/config.vdf"),
        "\"InstallConfigStore\" { \"Software\" { \"Valve\" { \"Steam\" { \"CompatToolMapping\" { \"0\" { \"name\" \"proton_default\" } \"AbC\" { \"name\" \"GE-Proton\" } \"1245620\" { \"config\" \"\" } \"broken\" \"not a block\" } } } } }",
    );

    let tools = compat_tools(&install);
    assert_eq!(tools.len(), 4);
    assert_eq!(tools.get("0").map(String::as_str), Some("proton_default"));
    assert_eq!(tools.get("abc").map(String::as_str), Some("GE-Proton"));
    assert_eq!(tools.get("1245620").map(String::as_str), Some(""));
    assert_eq!(tools.get("broken").map(String::as_str), Some(""));
}

#[test]
fn compatibility_tools_keep_the_first_duplicate_entry() {
    let home = tempfile::tempdir().unwrap();
    let root = native(home.path());
    let install = only(home.path());
    write(
        &root.join("config/config.vdf"),
        "\"InstallConfigStore\" { \"Software\" { \"Valve\" { \"Steam\" { \"CompatToolMapping\" { \"42\" \"malformed\" \"42\" { \"name\" \"GE-Proton\" } } } } } }",
    );

    assert_eq!(
        compat_tools(&install).get("42").map(String::as_str),
        Some("")
    );
    assert_eq!(compat_tool(&install, "42"), None);
}

#[test]
fn compatibility_tool_in_falls_back_to_the_default() {
    let tools = HashMap::from([("0".to_owned(), "proton_default".to_owned())]);
    assert_eq!(
        compat_tool_in(&tools, "42").as_deref(),
        Some("proton_default")
    );
}

#[test]
fn compatibility_tool_in_empty_name_falls_back_to_the_default() {
    let tools = HashMap::from([
        ("0".to_owned(), "proton_default".to_owned()),
        ("42".to_owned(), String::new()),
    ]);
    assert_eq!(
        compat_tool_in(&tools, "42").as_deref(),
        Some("proton_default")
    );
}

#[test]
fn compatibility_tool_in_app_ids_ignore_case() {
    let tools = HashMap::from([
        ("0".to_owned(), "proton_default".to_owned()),
        ("abc".to_owned(), "GE-Proton".to_owned()),
    ]);
    for app_id in ["abc", "AbC", "ABC"] {
        assert_eq!(compat_tool_in(&tools, app_id).as_deref(), Some("GE-Proton"));
    }
}

#[test]
fn the_compatibility_tool_falls_back_to_the_default() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    let install = only(home);
    assert_eq!(compat_tool(&install, "1086940"), None);

    write(
        &root.join("config/config.vdf"),
        "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t}\n\t\t\t\t\t\"1086940\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"GE-Proton9-20\"\n\t\t\t\t\t}\n\t\t\t\t\t\"1245620\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n",
    );
    assert_eq!(
        compat_tool(&install, "1086940").as_deref(),
        Some("GE-Proton9-20")
    );
    assert_eq!(
        compat_tool(&install, "1300000").as_deref(),
        Some("proton_experimental")
    );
    assert_eq!(
        compat_tool(&install, "1245620").as_deref(),
        Some("proton_experimental")
    );

    write(&root.join("config/config.vdf"), "\"InstallConfigStore\" {");
    assert_eq!(compat_tool(&install, "1086940"), None);
}

#[test]
fn art_maps_to_wineo_kinds_and_lists_only_what_exists() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let root = native(home);
    let cache = root.join("appcache/librarycache");
    write(&cache.join("1086940/library_600x900.jpg"), "jpg");
    write(&cache.join("1086940/library_hero.jpg"), "jpg");
    write(&cache.join("1086940/logo.png"), "png");
    write(&cache.join("1086940/0123456789abcdef.ico"), "icon");
    write(&cache.join("1245620/logo.png"), "png");
    let install = only(home);

    assert_eq!(
        art_files(&install, "1086940"),
        [
            (art::Kind::Grid, cache.join("1086940/library_600x900.jpg")),
            (art::Kind::Hero, cache.join("1086940/library_hero.jpg")),
            (art::Kind::Logo, cache.join("1086940/logo.png")),
        ]
    );
    assert_eq!(
        art_files(&install, "1245620"),
        [(art::Kind::Logo, cache.join("1245620/logo.png"))]
    );
    assert!(art_files(&install, "1300000").is_empty());
    assert!(art_files(&install, "../1086940").is_empty());
}
