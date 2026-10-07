mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use rustix::fs::{FlockOperation, flock};

use common::snapshot::Snapshot;
use common::{Fixture, assert_success, stderr, stdout};

const NATIVE_STEAM: &str = ".local/share/Steam";
const FLATPAK_STEAM: &str = ".var/app/com.valvesoftware.Steam/.local/share/Steam";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDATx\x9cc\x10\x14\xd7\xfa\x0f\0\x01\xe1\x01Re\x03\x38\x0e\0\0\0\0IEND\xaeB`\x82";

fn steam_root(f: &Fixture, relative: &str) -> PathBuf {
    f.root.path().join(relative)
}

fn add_game(root: &Path, app_id: &str, title: &str) {
    let steamapps = root.join("steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join(format!("appmanifest_{app_id}.acf")),
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\t\"name\"\t\t\"{title}\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"{app_id}\"\n\t\"SizeOnDisk\"\t\t\"1024\"\n}}\n"
        ),
    )
    .unwrap();
}

fn add_tool(root: &Path, app_id: &str, title: &str) {
    add_game(root, app_id, title);
}

fn imported_config(f: &Fixture, name: &str, app_id: &str) -> PathBuf {
    let dir = f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.toml"));
    fs::write(
        &path,
        format!(
            "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"{app_id}\"\ntitle = \"Stored title\"\ntags = [\"game\"]\ndesktop = false\n"
        ),
    )
    .unwrap();
    path
}

fn run_at_home(f: &Fixture, args: &[&str]) -> Output {
    f.run_at_home(args)
}

fn dry_run_at_home(f: &Fixture, args: &[&str]) -> Output {
    let before = Snapshot::capture(f.root.path()).expect("capture fixture before import");
    let output = run_at_home(f, args);
    before.assert_unchanged(f.root.path());
    output
}

#[test]
fn steam_list_hides_tools_and_keeps_unreadable_rows_beside_healthy_games() {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "1086940", "Hades II");
    add_tool(&steam, "1070560", "Steam Linux Runtime 3.0 (sniper)");
    let steamapps = steam.join("steamapps");
    fs::write(
        steamapps.join("appmanifest_1245620.acf"),
        "\"AppState\"\n{\n\t\"appid\" \"1245620\"\n\t\"name\" \"Broken",
    )
    .unwrap();
    imported_config(&f, "my-own-name", "1086940");

    let output = run_at_home(&f, &["import", "steam", "--list"]);
    assert_success(&output);
    let listing = stdout(&output);
    for expected in ["1086940", "Hades II", "1 KiB", "my-own-name"] {
        assert!(
            listing.contains(expected),
            "missing {expected:?}: {listing}"
        );
    }
    assert!(
        listing.to_ascii_lowercase().contains("installed"),
        "{listing}"
    );
    assert!(listing.contains("1245620"), "{listing}");
    assert!(
        listing.to_ascii_lowercase().contains("unreadable"),
        "{listing}"
    );
    assert!(!listing.contains("Steam Linux Runtime"), "{listing}");
    assert!(!listing.contains("1070560"), "{listing}");
}

#[test]
fn steam_single_import_writes_only_the_planned_proxied_config() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "1086940", "Hades II");
    let templates = f.config_dir().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(
        templates.join("gaming.toml"),
        "kind = \"native\"\ntags = [\"template\"]\n",
    )
    .unwrap();

    let output = run_at_home(&f, &["import", "steam", "--game", "1086940"]);
    assert_success(&output);
    let path = f.config_dir().join("environments/hades-ii.toml");
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\ntitle = \"Hades II\"\ntags = [\"game\"]\ndesktop = false\n"
    );
    let config = wineo_core::config::EnvironmentConfig::parse(&path, &text).unwrap();
    assert_eq!(config.schema_version, 1);
    assert_eq!(config.tags, ["game"]);
    assert!(config.targets.is_empty());
    assert!(config.winetricks.is_empty());
    assert!(config.env.is_empty());
    match config.kind {
        wineo_core::config::Kind::Proxied(proxied) => {
            assert_eq!(proxied.launcher.as_str(), "steam");
            assert_eq!(proxied.app_id, "1086940");
            assert_eq!(proxied.title.as_deref(), Some("Hades II"));
            assert!(!proxied.desktop);
        }
        other => panic!("expected proxied config, got {}", other.as_str()),
    }
}

#[test]
fn steam_import_accepts_a_custom_name() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "10", "Hades II");

    let output = run_at_home(&f, &["import", "steam", "--game", "10", "-n", "bg2"]);
    assert_success(&output);
    let path = f.config_dir().join("environments/bg2.toml");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("app_id = \"10\""), "{text}");
    assert!(text.contains("title = \"Hades II\""), "{text}");
    assert!(!f.config_dir().join("environments/hades-ii.toml").exists());
}

#[test]
fn steam_import_rejects_a_name_taken_by_another_environment() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "10", "Hades II");
    let occupied = f.config_dir().join("environments/hades-ii.toml");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, "kind = \"native\"\ntags = [\"work\"]\n").unwrap();
    let original = fs::read_to_string(&occupied).unwrap();

    let output = run_at_home(&f, &["import", "steam", "--game", "10"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("environment `hades-ii` already exists"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read_to_string(occupied).unwrap(), original);
}

#[test]
fn steam_import_skips_a_game_already_imported_under_a_custom_name() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "10", "Hades II");
    imported_config(&f, "renamed-game", "10");

    let output = run_at_home(&f, &["import", "steam", "--game", "10"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Already imported as renamed-game."),
        "{}",
        stdout(&output)
    );
    assert!(!f.config_dir().join("environments/hades-ii.toml").exists());
}

#[test]
fn steam_import_reports_not_installed_ids_and_missing_steam() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "10", "Hades II");

    let absent = run_at_home(&f, &["import", "steam", "--game", "999"]);
    assert_eq!(absent.status.code(), Some(1), "{}", stderr(&absent));
    assert!(
        stderr(&absent).contains("Steam lists 999 as not installed."),
        "{}",
        stderr(&absent)
    );

    let no_steam = Fixture::new();
    let missing = run_at_home(&no_steam, &["import", "steam", "--list"]);
    assert_eq!(missing.status.code(), Some(1), "{}", stderr(&missing));
    assert!(
        stderr(&missing).contains("Steam wasn't found."),
        "{}",
        stderr(&missing)
    );
}

#[test]
fn steam_all_imports_healthy_games_and_exits_one_for_a_failed_game() {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "10", "Hades II");
    add_game(&steam, "20", "Stardew Valley");
    add_tool(&steam, "1070560", "Steam Linux Runtime 3.0 (sniper)");
    let occupied = f.config_dir().join("environments/hades-ii.toml");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, "kind = \"native\"\n").unwrap();

    let output = run_at_home(&f, &["import", "steam", "--all", "--yes"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        f.config_dir()
            .join("environments/stardew-valley.toml")
            .is_file()
    );
    assert_eq!(
        fs::read_to_string(&occupied).unwrap(),
        "kind = \"native\"\n"
    );
    let report = format!("{}{}", stdout(&output), stderr(&output));
    assert!(report.contains("hades-ii"), "{report}");
    assert!(report.contains("already exists"), "{report}");
    assert!(
        !f.config_dir()
            .join("environments/steam-linux-runtime-3-0-sniper.toml")
            .exists()
    );
}

#[test]
fn steam_all_imports_healthy_games_skips_imported_games_and_reports_bad_manifests() {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "10", "Hades II");
    add_game(&steam, "20", "Stardew Valley");
    fs::write(
        steam.join("steamapps/appmanifest_30.acf"),
        "\"AppState\"\n{\n\t\"appid\" \"30\"\n\t\"name\" \"Broken",
    )
    .unwrap();
    imported_config(&f, "my-save", "20");

    let output = run_at_home(&f, &["import", "steam", "--all", "--yes"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(f.config_dir().join("environments/hades-ii.toml").is_file());
    assert!(
        !f.config_dir()
            .join("environments/stardew-valley.toml")
            .exists()
    );
    let report = format!("{}{}", stdout(&output), stderr(&output));
    assert!(report.contains("Already imported as my-save."), "{report}");
    assert!(report.contains("Failed 30:"), "{report}");
}

#[test]
fn steam_all_skips_an_app_imported_between_review_and_apply() {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "10", "Hades II");
    add_game(&steam, "20", "Stardew Valley");
    let locks = f.root.path().join("state/wineo/.locks");
    fs::create_dir_all(&locks).unwrap();
    let lock = fs::File::create(locks.join("steam-10.lock")).unwrap();
    flock(&lock, FlockOperation::LockExclusive).unwrap();
    let mut child = f
        .wineo_at_home()
        .args(["import", "steam", "--all", "--yes"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut review = String::new();
    let mut reviewed = 0;
    while reviewed < 2 {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0, "{review}");
        if line.contains("  art       Steam cache") {
            reviewed += 1;
        }
        review.push_str(&line);
    }
    let existing = imported_config(&f, "another-name", "10");
    let before = fs::read(&existing).unwrap();
    flock(&lock, FlockOperation::Unlock).unwrap();
    let mut remaining = String::new();
    reader.read_to_string(&mut remaining).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_success(&output);
    assert!(
        remaining.contains("Already imported as another-name."),
        "{remaining}"
    );
    assert!(
        remaining.contains("Imported stardew-valley."),
        "{remaining}"
    );
    assert!(!remaining.contains("Imported hades-ii."), "{remaining}");
    assert!(
        !stderr(&output).contains("Failed 10:"),
        "{}",
        stderr(&output)
    );
    assert!(!f.config_dir().join("environments/hades-ii.toml").exists());
    assert!(!f.root.path().join("data/wineo/art/hades-ii").exists());
    assert_eq!(fs::read(existing).unwrap(), before);
}

#[test]
fn steam_all_summary_excludes_plan_and_apply_skips_and_reports_apply_failures() {
    assert_bulk_summary_after_apply_race(true);
}

#[test]
fn steam_all_summary_excludes_plan_and_apply_skips_without_failing() {
    assert_bulk_summary_after_apply_race(false);
}

fn assert_bulk_summary_after_apply_race(failing_game: bool) {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "10", "Hades II");
    add_game(&steam, "20", "Stardew Valley");
    add_game(&steam, "30", "Celeste");
    let planned_skip = imported_config(&f, "my-save", "30");
    let planned_skip_before = fs::read(&planned_skip).unwrap();
    if failing_game {
        add_game(&steam, "40", "Broken Game");
    }
    let locks = f.root.path().join("state/wineo/.locks");
    fs::create_dir_all(&locks).unwrap();
    let lock = fs::File::create(locks.join("steam-10.lock")).unwrap();
    flock(&lock, FlockOperation::LockExclusive).unwrap();
    let mut child = f
        .wineo_at_home()
        .args(["import", "steam", "--all", "--yes"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut report = String::new();
    let mut reviewed = 0;
    while reviewed < if failing_game { 3 } else { 2 } {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0, "{report}");
        if line.contains("  art       Steam cache") {
            reviewed += 1;
        }
        report.push_str(&line);
    }
    let applied_skip = imported_config(&f, "another-name", "10");
    let applied_skip_before = fs::read(&applied_skip).unwrap();
    let occupied = f.config_dir().join("environments/broken-game.toml");
    if failing_game {
        fs::write(&occupied, "kind = \"native\"\n").unwrap();
    }
    flock(&lock, FlockOperation::Unlock).unwrap();
    reader.read_to_string(&mut report).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(i32::from(failing_game)),
        "{}",
        stderr(&output)
    );
    let summary = &report[report.rfind("Imported 1 of ").expect(&report)..];
    assert_eq!(
        summary,
        if failing_game {
            "Imported 1 of 2 games.\n1 already imported: another-name\n1 failed: broken-game\n"
        } else {
            "Imported 1 of 1 game.\n1 already imported: another-name\n"
        }
    );
    assert!(report.contains("Already imported as my-save."), "{report}");
    assert!(
        report.contains("Already imported as another-name."),
        "{report}"
    );
    assert!(report.contains("Imported stardew-valley."), "{report}");
    assert!(
        f.config_dir()
            .join("environments/stardew-valley.toml")
            .is_file()
    );
    assert!(!f.config_dir().join("environments/hades-ii.toml").exists());
    assert!(!f.config_dir().join("environments/celeste.toml").exists());
    assert_eq!(fs::read(planned_skip).unwrap(), planned_skip_before);
    assert_eq!(fs::read(applied_skip).unwrap(), applied_skip_before);
    if failing_game {
        assert!(
            stderr(&output).contains("Failed 40:"),
            "{}",
            stderr(&output)
        );
        assert_eq!(fs::read_to_string(occupied).unwrap(), "kind = \"native\"\n");
    } else {
        assert!(!stderr(&output).contains("Failed"), "{}", stderr(&output));
    }
}

#[test]
fn steam_single_and_bulk_dry_runs_write_nothing() {
    let single = Fixture::new();
    add_game(&steam_root(&single, NATIVE_STEAM), "10", "Hades II");
    let output = dry_run_at_home(
        &single,
        &["import", "steam", "--game", "10", "--art", "--dry-run"],
    );
    assert_success(&output);
    assert!(
        stdout(&output).contains("Dry run, nothing was imported."),
        "{}",
        stdout(&output)
    );
    assert!(!single.root.path().join("state/wineo/.locks").exists());

    let bulk = Fixture::new();
    add_game(&steam_root(&bulk, NATIVE_STEAM), "20", "Stardew Valley");
    let output = dry_run_at_home(&bulk, &["import", "steam", "--all", "--dry-run"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Dry run, nothing was imported."),
        "{}",
        stdout(&output)
    );
    assert!(!bulk.root.path().join("state/wineo/.locks").exists());
}

#[test]
fn steam_bulk_dry_run_reports_duplicate_names_without_writing() {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "10", "Hades II");
    add_game(&steam, "20", "Hades II");

    let output = dry_run_at_home(&f, &["import", "steam", "--all", "--dry-run"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        report.contains("Dry run, nothing was imported."),
        "{report}"
    );
    assert!(report.contains("already exists"), "{report}");
    assert!(!f.config_dir().join("environments/hades-ii.toml").exists());
}

#[test]
fn steam_import_copies_valid_cached_art_and_skips_invalid_images() {
    let f = Fixture::new();
    let steam = steam_root(&f, NATIVE_STEAM);
    add_game(&steam, "10", "Hades II");
    let cache = steam.join("appcache/librarycache/10");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("library_600x900.jpg"), PNG).unwrap();
    fs::write(cache.join("library_hero.jpg"), b"not an image").unwrap();
    let before = Snapshot::capture(&steam).unwrap();

    let output = run_at_home(&f, &["import", "steam", "--game", "10"]);
    assert_success(&output);
    let art = f.root.path().join("data/wineo/art/hades-ii");
    assert_eq!(fs::read(art.join("grid.png")).unwrap(), PNG);
    assert!(!art.join("hero.png").exists());
    let warning = format!("{}{}", stdout(&output), stderr(&output));
    assert!(warning.to_ascii_lowercase().contains("image"), "{warning}");
    before.assert_unchanged(&steam);
}

#[test]
fn steam_import_with_art_without_a_key_succeeds_without_fetching() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "10", "Hades II");

    let output = run_at_home(&f, &["import", "steam", "--game", "10", "--art"]);
    assert_success(&output);
    assert!(f.config_dir().join("environments/hades-ii.toml").is_file());
    assert!(!f.root.path().join("data/wineo/steamgriddb.key").exists());
}

#[test]
fn steam_import_uses_only_the_active_native_or_flatpak_install() {
    let f = Fixture::new();
    let native = steam_root(&f, NATIVE_STEAM);
    let flatpak = steam_root(&f, FLATPAK_STEAM);
    add_game(&native, "10", "Native Game");
    add_game(&native, "11", "Native Other");
    add_game(&flatpak, "20", "Flatpak Game");

    let native_import = run_at_home(&f, &["import", "steam", "--game", "10"]);
    assert_success(&native_import);
    assert!(
        f.config_dir()
            .join("environments/native-game.toml")
            .is_file()
    );

    f.global_config("steam = \"flatpak\"\n");
    let flatpak_import = run_at_home(&f, &["import", "steam", "--game", "20"]);
    assert_success(&flatpak_import);
    assert!(
        f.config_dir()
            .join("environments/flatpak-game.toml")
            .is_file()
    );

    let inactive = run_at_home(&f, &["import", "steam", "--game", "11"]);
    assert_eq!(inactive.status.code(), Some(1), "{}", stderr(&inactive));
    assert!(
        stderr(&inactive).contains("Steam lists 11 as not installed."),
        "{}",
        stderr(&inactive)
    );
}

#[test]
fn steam_import_is_refused_for_a_portable_folder() {
    let f = Fixture::new();
    add_game(&steam_root(&f, NATIVE_STEAM), "10", "Hades II");
    let portable = f.root.path().join("portable");
    let output = f
        .wineo_at_home()
        .env("WINEO_PORTABLE_DIR", &portable)
        .args(["import", "steam", "--game", "10"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Steam games can't be imported into a portable folder."),
        "{}",
        stderr(&output)
    );
    assert!(!portable.exists());
}
