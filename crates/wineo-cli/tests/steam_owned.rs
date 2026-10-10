mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::steam_api::{self, SteamApi};
use common::{Fixture, assert_success, stderr, stdout};

const KEY: &str = "sekrit-key-0123456789";
const WITCHER: u64 = 292030;
const NATIVE_STEAM: &str = ".local/share/Steam";

fn steam_root(f: &Fixture) -> PathBuf {
    f.root.path().join(NATIVE_STEAM)
}

/// Steam's login file, which names the account whose library is asked for.
fn login(f: &Fixture) {
    let config = steam_root(f).join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("loginusers.vdf"),
        "\"users\" { \"76561197960265729\" { \"MostRecent\" \"1\" } }\n",
    )
    .unwrap();
}

fn manifest(root: &Path, app_id: &str, title: &str, flags: u32, to_download: u64, done: u64) {
    let steamapps = root.join("steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join(format!("appmanifest_{app_id}.acf")),
        format!(
            "\"AppState\"\n{{\n\"appid\" \"{app_id}\"\n\"name\" \"{title}\"\n\"StateFlags\" \"{flags}\"\n\"SizeOnDisk\" \"1024\"\n\"BytesToDownload\" \"{to_download}\"\n\"BytesDownloaded\" \"{done}\"\n}}\n"
        ),
    )
    .unwrap();
}

fn installed(f: &Fixture, app_id: &str, title: &str) {
    manifest(&steam_root(f), app_id, title, 4, 0, 0);
}

fn cache_path(f: &Fixture) -> PathBuf {
    f.root
        .path()
        .join("cache/wineo/steam-owned-76561197960265729.json")
}

fn config_path(f: &Fixture, name: &str) -> PathBuf {
    f.config_dir().join(format!("environments/{name}.toml"))
}

/// Runs wineo with the fake API and, when given, a key in the environment.
fn run(f: &Fixture, api: Option<&SteamApi>, key: Option<&str>, args: &[&str]) -> Output {
    command(f, api, key, args).output().unwrap()
}

fn command(f: &Fixture, api: Option<&SteamApi>, key: Option<&str>, args: &[&str]) -> Command {
    let mut command = f.wineo_at_home();
    if let Some(api) = api {
        command.env("WINEO_TEST_STEAM_API", &api.base);
    }
    if let Some(key) = key {
        command.env("WINEO_STEAM_KEY", key);
    }
    command.args(args);
    command
}

fn assert_no_key(output: &Output) {
    let all = format!("{}{}", stdout(output), stderr(output));
    assert!(!all.contains(KEY), "the key was printed: {all}");
}

fn library() -> String {
    steam_api::owned(&[
        (WITCHER, "The Witcher 3"),
        (10, "Hades II"),
        (500, "Alpha Centauri"),
    ])
}

#[test]
fn list_shows_owned_games_with_their_state_installed_ones_first() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");
    manifest(&steam_root(&f), "11", "Half Way", 1026, 200, 84);
    manifest(&steam_root(&f), "12", "Waiting Room", 2, 0, 0);
    let api = SteamApi::start(vec![steam_api::owned(&[
        (WITCHER, "The Witcher 3"),
        (10, "Hades II"),
        (11, "Half Way"),
        (12, "Waiting Room"),
        (500, "Alpha Centauri"),
    ])]);
    // A queued game is listed among the uninstalled ones, sorted by title.
    // An imported game shows its environment name.
    fs::create_dir_all(config_path(&f, "x").parent().unwrap()).unwrap();
    fs::write(
        config_path(&f, "hades"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"10\"\n",
    )
    .unwrap();

    let output = run(&f, Some(&api), Some(KEY), &["import", "steam", "--list"]);

    assert_success(&output);
    let text = stdout(&output);
    let rows: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert_eq!(
        rows[0],
        ["APP", "ID", "TITLE", "SIZE", "STATE", "IMPORTED"],
        "{text}"
    );
    let order: Vec<_> = [
        "Hades II",
        "Half Way",
        "Alpha Centauri",
        "The Witcher 3",
        "Waiting Room",
    ]
    .iter()
    .map(|title| {
        text.find(title)
            .unwrap_or_else(|| panic!("{title}: {text}"))
    })
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    let row = |title: &str| text.lines().find(|line| line.contains(title)).unwrap();
    assert!(row("Hades II").contains("installed"), "{text}");
    assert!(row("Hades II").trim_end().ends_with("hades"), "{text}");
    assert!(row("Half Way").contains("installing 42%"), "{text}");
    assert!(row("Waiting Room").contains("install queued"), "{text}");
    assert!(row("The Witcher 3").contains("not installed"), "{text}");
    assert!(row("The Witcher 3").contains("  -  "), "{text}");
    assert_eq!(stderr(&output), "");
    assert_no_key(&output);
    let requests = api.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].contains("steamid=76561197960265729"),
        "{requests:?}"
    );
    assert!(requests[0].contains(&format!("key={KEY}")), "{requests:?}");
}

#[test]
fn list_without_a_key_shows_installed_games_and_a_hint_and_never_asks_steam() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");
    let api = SteamApi::start(vec![library()]);

    let output = run(&f, Some(&api), None, &["import", "steam", "--list"]);

    assert_success(&output);
    assert!(stdout(&output).contains("Hades II"));
    assert!(!stdout(&output).contains("The Witcher 3"));
    assert_eq!(
        stderr(&output),
        "Add a Steam Web API key to see uninstalled games.\n"
    );
    assert!(api.requests().is_empty());
}

#[test]
fn a_saved_list_is_used_until_refresh_and_kept_when_the_refresh_fails() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");
    let api = SteamApi::start(vec![library(), steam_api::status(500)]);

    assert_success(&run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--list"],
    ));
    let cached = run(&f, Some(&api), Some(KEY), &["import", "steam", "--list"]);
    assert_success(&cached);
    assert!(stdout(&cached).contains("The Witcher 3"));
    assert_eq!(stderr(&cached), "");
    assert_eq!(api.requests().len(), 1, "a fresh list needs no request");

    let refreshed = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--list", "--refresh"],
    );

    assert_success(&refreshed);
    assert_eq!(api.requests().len(), 2);
    assert!(
        stdout(&refreshed).contains("The Witcher 3"),
        "the old list stands in"
    );
    assert_eq!(
        stderr(&refreshed),
        "Couldn't refresh owned games: Steam answered with status 500\n"
    );
    assert_no_key(&refreshed);
}

#[test]
fn a_failed_fetch_without_a_saved_list_shows_installed_games_and_the_reason() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");
    let api = SteamApi::start(vec![steam_api::status(401)]);

    let output = run(&f, Some(&api), Some(KEY), &["import", "steam", "--list"]);

    assert_success(&output);
    assert!(stdout(&output).contains("Hades II"));
    assert!(!stdout(&output).contains("The Witcher 3"));
    assert_eq!(
        stderr(&output),
        "Couldn't get owned games: Steam rejected the Web API key\n"
    );
    assert_no_key(&output);
}

#[test]
fn an_empty_response_is_a_failure_that_keeps_the_saved_list() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![library(), steam_api::json("{\"response\":{}}")]);
    assert_success(&run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--list"],
    ));

    let output = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--list", "--refresh"],
    );

    assert!(stdout(&output).contains("The Witcher 3"));
    assert_eq!(
        stderr(&output),
        "Couldn't refresh owned games: the key can't see this library\n"
    );
}

#[test]
fn an_uninstalled_owned_game_imports_and_shows_as_not_installed() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![library(), library()]);

    let dry = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "292030", "--dry-run"],
    );
    assert_success(&dry);
    let text = stdout(&dry);
    assert!(text.contains("The Witcher 3 (292030)"), "{text}");
    assert!(text.contains("state     not installed"), "{text}");
    assert!(text.contains("Dry run, nothing was imported."), "{text}");
    assert!(!config_path(&f, "the-witcher-3").exists());
    assert!(!cache_path(&f).exists(), "a dry run saves nothing");

    let output = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "292030"],
    );

    assert_success(&output);
    assert!(stdout(&output).contains("Imported the-witcher-3."));
    assert_eq!(
        fs::read_to_string(config_path(&f, "the-witcher-3")).unwrap(),
        "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"292030\"\ntitle = \"The Witcher 3\"\ntags = [\"game\"]\ndesktop = false\n"
    );
    assert_eq!(api.requests().len(), 2, "the dry run left nothing saved");
    assert!(cache_path(&f).is_file());
    let status = run(&f, None, None, &["status", "the-witcher-3"]);
    assert!(
        stdout(&status).starts_with("the-witcher-3: not installed"),
        "{}",
        stdout(&status)
    );
    assert_no_key(&output);
}

#[test]
fn a_game_that_is_neither_installed_nor_owned_is_refused() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![library()]);

    let output = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "12345"],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: App 12345 isn't installed or owned.\n"
    );
}

#[test]
fn importing_an_uninstalled_game_fails_with_the_reason_when_the_list_is_unavailable() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![steam_api::status(403)]);

    let output = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "292030"],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: Couldn't get owned games: Steam rejected the Web API key\n"
    );
    assert_no_key(&output);
    assert!(!config_path(&f, "the-witcher-3").exists());
}

#[test]
fn importing_an_uninstalled_game_without_a_key_keeps_the_old_message() {
    let f = Fixture::new();
    login(&f);

    let output = run(&f, None, None, &["import", "steam", "--game", "292030"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: Steam lists 292030 as not installed.\n"
    );
}

#[test]
fn installed_games_import_with_a_key_set_and_never_ask_steam() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");
    installed(&f, "20", "Stardew Valley");
    let api = SteamApi::start(vec![library()]);

    let single = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "10"],
    );
    assert_success(&single);
    let all = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--all", "--yes"],
    );
    assert_success(&all);

    assert!(config_path(&f, "hades-ii").is_file());
    assert!(config_path(&f, "stardew-valley").is_file());
    assert!(!config_path(&f, "the-witcher-3").exists());
    assert!(api.requests().is_empty());
}

#[test]
fn all_with_owned_imports_the_whole_library_with_one_fetch() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");
    let api = SteamApi::start(vec![library()]);

    let output = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--all", "--owned", "--yes"],
    );

    assert_success(&output);
    assert!(
        stdout(&output).contains("Imported 3 of 3 games."),
        "{}",
        stdout(&output)
    );
    for name in ["hades-ii", "the-witcher-3", "alpha-centauri"] {
        assert!(config_path(&f, name).is_file(), "{name}");
    }
    assert_eq!(api.requests().len(), 1);
    assert_no_key(&output);
}

#[test]
fn all_with_owned_needs_a_key_and_a_readable_list() {
    let f = Fixture::new();
    login(&f);
    installed(&f, "10", "Hades II");

    let no_key = run(
        &f,
        None,
        None,
        &["import", "steam", "--all", "--owned", "--yes"],
    );
    assert_eq!(no_key.status.code(), Some(1));
    assert_eq!(
        stderr(&no_key),
        "error: Add a Steam Web API key to import uninstalled games.\n"
    );

    let api = SteamApi::start(vec![steam_api::status(500)]);
    let failed = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--all", "--owned", "--yes"],
    );
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(
        stderr(&failed),
        "error: Couldn't get owned games: Steam answered with status 500\n"
    );
    assert!(
        !config_path(&f, "hades-ii").exists(),
        "nothing imports on a failed list"
    );

    let no_all = run(&f, None, None, &["import", "steam", "--owned"]);
    assert_eq!(no_all.status.code(), Some(2));
}

#[test]
fn key_is_checked_online_then_saved_and_never_printed() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![library()]);
    let mut child = command(&f, Some(&api), None, &["import", "steam", "--key"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{KEY}").unwrap();
    let output = child.wait_with_output().unwrap();

    assert_success(&output);
    let saved = f.root.path().join("data/wineo/steam.key");
    assert_eq!(fs::read_to_string(&saved).unwrap(), format!("{KEY}\n"));
    assert!(
        stderr(&output).contains("stored in plain text"),
        "{}",
        stderr(&output)
    );
    assert_no_key(&output);
    let requests = api.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains(&format!("key={KEY}")));

    // The saved key now lists the library with no variable set.
    api.push(library());
    let list = run(&f, Some(&api), None, &["import", "steam", "--list"]);
    assert_success(&list);
    assert!(stdout(&list).contains("The Witcher 3"));
    assert_no_key(&list);
}

#[test]
fn key_is_not_saved_when_steam_refuses_it() {
    let f = Fixture::new();
    login(&f);
    for (reply, reason) in [
        (steam_api::status(403), "Steam rejected the Web API key"),
        (
            steam_api::json("{\"response\":{}}"),
            "the key can't see this library",
        ),
    ] {
        let api = SteamApi::start(vec![reply]);
        let mut child = command(&f, Some(&api), None, &["import", "steam", "--key"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{KEY}").unwrap();
        let output = child.wait_with_output().unwrap();

        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            stderr(&output),
            format!("error: Couldn't check the key: {reason}\n")
        );
        assert_no_key(&output);
        assert!(!f.root.path().join("data/wineo/steam.key").exists());
    }
}

#[test]
fn key_with_nothing_entered_never_reaches_steam() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![library()]);

    let output = run(&f, Some(&api), None, &["import", "steam", "--key"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "error: no key entered\n");
    assert!(api.requests().is_empty());
}

#[test]
fn remove_key_deletes_the_saved_key() {
    let f = Fixture::new();
    let file = f.root.path().join("data/wineo/steam.key");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, format!("{KEY}\n")).unwrap();

    let removed = run(&f, None, None, &["import", "steam", "--remove-key"]);
    assert_success(&removed);
    assert_eq!(stdout(&removed), "Removed the Steam Web API key\n");
    assert!(!file.exists());

    let again = run(&f, None, None, &["import", "steam", "--remove-key"]);
    assert_success(&again);
    assert_eq!(stdout(&again), "No Steam Web API key was saved\n");

    let overridden = run(&f, None, Some("abc"), &["import", "steam", "--remove-key"]);
    assert_eq!(
        stderr(&overridden),
        "note: WINEO_STEAM_KEY is set and overrides the saved key\n"
    );
}

#[test]
fn key_flags_conflict_with_the_other_import_flags() {
    let f = Fixture::new();
    for args in [
        &["--key", "--list"][..],
        &["--key", "--game", "10"],
        &["--key", "--all"],
        &["--key", "--remove-key"],
        &["--remove-key", "--refresh"],
        &["--remove-key", "--dry-run"],
        &["--remove-key", "--art"],
    ] {
        let mut full = vec!["import", "steam"];
        full.extend(args);
        let output = run(&f, None, None, &full);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(stderr(&output).contains("cannot be used with"), "{args:?}");
    }
}

#[test]
fn list_and_status_show_installing_percent_in_text() {
    let f = Fixture::new();
    let root = steam_root(&f);
    manifest(&root, "11", "Half Way", 1026, 200, 84);
    fs::create_dir_all(config_path(&f, "x").parent().unwrap()).unwrap();
    fs::write(
        config_path(&f, "half"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"11\"\n",
    )
    .unwrap();

    let status = run(&f, None, None, &["status", "half"]);
    assert!(
        stdout(&status).starts_with("half: installing 42%"),
        "{}",
        stdout(&status)
    );
    let list = run(&f, None, None, &["list"]);
    assert!(
        stdout(&list).contains("installing 42%"),
        "{}",
        stdout(&list)
    );
}

#[test]
fn dry_runs_ask_about_ownership_but_never_write_the_saved_list() {
    for args in [
        &["import", "steam", "--game", "292030", "--dry-run"][..],
        &["import", "steam", "--all", "--owned", "--dry-run"],
    ] {
        // No saved list: it is fetched, used, and not saved.
        let f = Fixture::new();
        login(&f);
        let api = SteamApi::start(vec![library()]);
        let output = run(&f, Some(&api), Some(KEY), args);
        assert_success(&output);
        assert!(stdout(&output).contains("The Witcher 3"), "{args:?}");
        assert_eq!(api.requests().len(), 1);
        assert!(!cache_path(&f).exists(), "{args:?}");

        // A stale saved list is fetched over, and left byte for byte.
        let stale = r#"{"fetched_at":1,"games":[{"app_id":"9","name":"Old"}]}"#;
        fs::create_dir_all(cache_path(&f).parent().unwrap()).unwrap();
        fs::write(cache_path(&f), stale).unwrap();
        api.push(library());
        let output = run(&f, Some(&api), Some(KEY), args);
        assert_success(&output);
        assert!(stdout(&output).contains("The Witcher 3"), "{args:?}");
        assert_eq!(api.requests().len(), 2);
        assert_eq!(fs::read_to_string(cache_path(&f)).unwrap(), stale);
        assert!(!config_path(&f, "the-witcher-3").exists());
        assert_no_key(&output);
    }
}

#[test]
fn a_dry_run_uses_a_fresh_saved_list_without_asking() {
    let f = Fixture::new();
    login(&f);
    let api = SteamApi::start(vec![library()]);
    assert_success(&run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--list"],
    ));
    let before = fs::read(cache_path(&f)).unwrap();

    let output = run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "292030", "--dry-run"],
    );

    assert_success(&output);
    assert_eq!(api.requests().len(), 1);
    assert_eq!(fs::read(cache_path(&f)).unwrap(), before);
}

/// Two accounts and none marked most recent.
fn ambiguous_accounts(f: &Fixture) {
    let config = steam_root(f).join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("loginusers.vdf"),
        "\"users\" { \"76561197960265729\" { \"MostRecent\" \"0\" } \"76561197960265730\" { \"MostRecent\" \"0\" } }\n",
    )
    .unwrap();
}

#[test]
fn several_accounts_with_none_most_recent_is_an_error_when_a_key_is_set() {
    let f = Fixture::new();
    ambiguous_accounts(&f);
    installed(&f, "10", "Hades II");
    let api = SteamApi::start(vec![library()]);
    for args in [
        &["import", "steam", "--list"][..],
        &["import", "steam", "--game", "292030"],
        &["import", "steam", "--all", "--owned", "--yes"],
    ] {
        let output = run(&f, Some(&api), Some(KEY), args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            stderr(&output),
            "error: Steam doesn't say which account was used last\n",
            "{args:?}"
        );
        assert_no_key(&output);
    }
    assert!(api.requests().is_empty());
    assert!(!config_path(&f, "hades-ii").exists());

    // Without a key no account is needed, and installed games list as before.
    let no_key = run(&f, Some(&api), None, &["import", "steam", "--list"]);
    assert_success(&no_key);
    assert!(stdout(&no_key).contains("Hades II"));
    // Installed games import with a key set, since they need no list.
    assert_success(&run(
        &f,
        Some(&api),
        Some(KEY),
        &["import", "steam", "--game", "10"],
    ));
}

#[test]
fn a_failed_key_check_never_prints_the_key_even_with_verbose() {
    let f = Fixture::new();
    login(&f);
    for reply in [
        steam_api::status(401),
        steam_api::status(500),
        steam_api::json("not json"),
        steam_api::json("{\"response\":{}}"),
    ] {
        let api = SteamApi::start(vec![reply]);
        let mut child = command(&f, Some(&api), None, &["-v", "import", "steam", "--key"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{KEY}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_no_key(&output);
    }
    // A server that is not there at all.
    let gone = SteamApi::start(vec![]);
    let base = gone.base.clone();
    drop(gone);
    let mut child = f
        .wineo_at_home()
        .env("WINEO_TEST_STEAM_API", base)
        .args(["-v", "import", "steam", "--key"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{KEY}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_no_key(&output);
}
