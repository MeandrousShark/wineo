//! `wineo import gog --game` installing games that aren't installed, against
//! a fake GOG: native and Windows installs, reviews, dry runs, the wizard
//! fallback, interrupted installs, and `--forget` of one that never got a
//! config.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::gog_api::{self, CODE, GogApi, owned_game, product, success_address};
use common::gog_world::{DISCO, Item, RIMWORLD, RIMWORLD_DLC, gog_package};
use common::pty::Pty;
use common::snapshot::Snapshot;
use common::{Fixture, assert_success, stderr, stdout};
use wineo_core::gog::folder::{
    FolderLock, FolderState, Journal, JournalKind, Platform, Source, Target, Track,
};

const V1: &str = "1.6.4871 rev588";

fn logged_in(f: &Fixture, api: &GogApi) {
    let mut command = f.gog_command(api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

/// RimWorld on GOG with a Linux installer, and two DLC of which only
/// Royalty is owned.
fn rimworld(api: &GogApi) {
    let base = api.linux_package(
        RIMWORLD,
        "en",
        V1,
        &gog_package(&[
            Item::Exe("start.sh", "#!/bin/sh\n"),
            Item::File("Data/core.txt", "core"),
        ]),
    );
    let royalty = api.linux_package(
        RIMWORLD_DLC[0],
        "en",
        V1,
        &gog_package(&[Item::File("Data/Royalty/about.txt", "royalty")]),
    );
    let ideology = api.linux_package(
        RIMWORLD_DLC[1],
        "en",
        V1,
        &gog_package(&[Item::File("Data/Ideology/about.txt", "ideology")]),
    );
    let details = product(
        RIMWORLD,
        "RimWorld",
        &[base],
        &[
            product(RIMWORLD_DLC[0], "Royalty", &[royalty], &[]),
            product(RIMWORLD_DLC[1], "Ideology", &[ideology], &[]),
        ],
    );
    api.with(|config| {
        config.products.insert(RIMWORLD, details);
        config.owned_ids = Some(vec![RIMWORLD, RIMWORLD_DLC[0]]);
    });
}

/// A logged-in wineo with nothing installed, and RimWorld and Disco Elysium
/// owned on GOG.
fn world() -> (Fixture, GogApi) {
    let f = Fixture::new();
    let api = GogApi::with_library(
        vec![
            owned_game(RIMWORLD, "RimWorld", true, true),
            owned_game(DISCO, "Disco Elysium", false, true),
        ],
        vec![],
    );
    rimworld(&api);
    disco(&api, "1", "");
    logged_in(&f, &api);
    f.inno_runner();
    (f, api)
}

/// Disco Elysium on GOG with only a Windows installer at `version`. Its
/// setup program runs `before` first, then installs the game and its info
/// file.
fn disco(api: &GogApi, version: &str, before: &str) {
    let tasks = serde_json::json!([{
        "isPrimary": true, "type": "FileTask", "path": "disco.exe", "category": "game",
    }]);
    disco_with(api, version, before, tasks);
}

/// `disco` with the info file's play tasks given.
fn disco_with(api: &GogApi, version: &str, before: &str, tasks: serde_json::Value) {
    let info = serde_json::json!({
        "gameId": DISCO.to_string(),
        "rootGameId": DISCO.to_string(),
        "name": "Disco Elysium",
        "language": "English",
        "playTasks": tasks,
        "version": 1,
    });
    let script = format!(
        "{before}\nmkdir -p \"$DIR\"\nprintf '{version}' > \"$DIR/disco.exe\"\ncat > \"$DIR/goggame-{DISCO}.info\" <<'INFO'\n{info}\nINFO\nexit 0\n"
    );
    let installer = api.windows_package(DISCO, "en", version, &script);
    api.with(|config| {
        config
            .products
            .insert(DISCO, product(DISCO, "Disco Elysium", &[installer], &[]));
    });
}

/// A setup script line that exits with `code` the first time it runs only.
fn fails_once(f: &Fixture, code: i32) -> String {
    let flag = f.inno_runs().join("failed-once");
    format!(
        "if [ ! -f '{flag}' ]; then : > '{flag}'; exit {code}; fi",
        flag = flag.display()
    )
}

fn import(f: &Fixture, api: &GogApi, args: &[&str]) -> std::process::Output {
    let mut all = vec!["import", "gog"];
    all.extend(args);
    gog_api::checked(
        f.gog_command(api)
            .args(&all)
            .env("DISPLAY", ":0")
            .output()
            .unwrap(),
    )
}

fn spawn(f: &Fixture, api: &GogApi, args: &[&str]) -> Pty {
    let mut command = f.gog_command(api);
    command.env("DISPLAY", ":0").args(args);
    Pty::spawn(command)
}

fn games(f: &Fixture) -> PathBuf {
    f.root.path().join("Games")
}

fn state(f: &Fixture, folder: &Path) -> FolderState {
    f.gog_state(&fs::canonicalize(folder).unwrap()).unwrap()
}

fn rimworld_id() -> String {
    RIMWORLD.to_string()
}

fn disco_id() -> String {
    DISCO.to_string()
}

#[test]
fn a_native_game_installs_with_its_owned_dlc() {
    let (f, api) = world();
    let out = import(&f, &api, &["--game", &rimworld_id(), "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with("Install RimWorld (1094900565)\n"),
        "{text}"
    );
    for row in [
        "  environment  rimworld (new native environment)\n",
        "  platform     Linux\n",
        "  language     en-US\n",
        &format!("  version      {V1}\n"),
        &format!("  dlc          Royalty (1233017772) {V1}\n"),
        "(an estimate)",
    ] {
        assert!(text.contains(row), "{row:?} in {text}");
    }
    // Ideology isn't owned, so it isn't listed.
    assert!(!text.contains("Ideology"), "{text}");
    assert!(text.contains("Installed RimWorld as rimworld.\n"), "{text}");
    assert!(
        stderr(&out).contains("Downloading RimWorld"),
        "{}",
        stderr(&out)
    );
    // New environments get desktop entries, as migrated ones do.
    assert!(
        stderr(&out).contains("Added a desktop entry for game in rimworld"),
        "{}",
        stderr(&out)
    );
    let folder = games(&f).join("RimWorld");
    assert_eq!(
        fs::read_to_string(folder.join("Data/core.txt")).unwrap(),
        "core"
    );
    assert_eq!(
        fs::read_to_string(folder.join("Data/Royalty/about.txt")).unwrap(),
        "royalty"
    );
    let installed = state(&f, &folder);
    assert!(installed.journal.is_none());
    let committed = installed.committed.unwrap();
    assert_eq!(committed.platform, Platform::Linux);
    assert_eq!(committed.products.len(), 2);
    let config = f.created_config("rimworld");
    assert!(
        config.contains(&format!("gog_id = \"{RIMWORLD}\"")),
        "{config}"
    );

    assert_eq!(committed.source, Some(Source::Install));

    // Listed as installed, not imported, in JSON and in the table.
    let list = import(&f, &api, &["--list", "--json"]);
    assert_success(&list);
    let json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let game = |id: u64| {
        json["games"]
            .as_array()
            .unwrap()
            .iter()
            .find(|game| game["id"].as_str() == Some(id.to_string().as_str()))
            .unwrap()
            .clone()
    };
    assert_eq!(game(RIMWORLD)["state"], "installed");
    assert_eq!(game(RIMWORLD)["environment"], "rimworld");
    assert_eq!(game(DISCO)["state"], "not_installed");
    let table = stdout(&import(&f, &api, &["--list"]));
    let row = table
        .lines()
        .find(|line| line.contains("RimWorld"))
        .unwrap();
    assert!(
        row.contains("installed") && !row.contains("imported"),
        "{table}"
    );

    // An update keeps the record of who installed it.
    let newer = api.linux_package(
        RIMWORLD,
        "en",
        "2",
        &gog_package(&[Item::Exe("start.sh", "#!/bin/sh\n")]),
    );
    let royalty = api.linux_package(
        RIMWORLD_DLC[0],
        "en",
        "2",
        &gog_package(&[Item::File("Data/Royalty/about.txt", "royalty 2")]),
    );
    api.with(|config| {
        config.products.insert(
            RIMWORLD,
            product(
                RIMWORLD,
                "RimWorld",
                &[newer],
                &[product(RIMWORLD_DLC[0], "Royalty", &[royalty], &[])],
            ),
        );
    });
    assert_success(&f.gog(&api, &["update", "rimworld", "--refresh", "--yes"]));
    let committed = state(&f, &folder).committed.unwrap();
    assert_eq!(committed.products[0].version.as_deref(), Some("2"));
    assert_eq!(committed.source, Some(Source::Install));

    // Installing it again says so and changes nothing.
    let again = import(&f, &api, &["--game", &rimworld_id(), "--yes"]);
    assert_success(&again);
    assert!(stdout(&again).starts_with("Already installed. rimworld owns "));
}

#[test]
fn a_windows_game_installs_from_its_installer() {
    let (f, api) = world();
    let out = import(
        &f,
        &api,
        &["--game", &disco_id(), "--runner", "inno", "--yes"],
    );
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with("Install Disco Elysium (1771589310)\n"),
        "{text}"
    );
    for row in [
        "  environment  disco-elysium (new Windows environment)\n",
        "  platform     Windows\n",
        "  runner       inno\n",
        "  arch         win64\n",
        "  version      1\n",
    ] {
        assert!(text.contains(row), "{row:?} in {text}");
    }
    assert!(
        text.contains("Installed Disco Elysium as disco-elysium.\n"),
        "{text}"
    );
    // The prefix is built before the installer runs, and says so first.
    let err = stderr(&out);
    let creating = err.find("Creating a wine prefix").expect(&err);
    let running = err.find("Running Disco Elysium's installer").expect(&err);
    assert!(creating < running, "{err}");
    assert!(
        err.contains("Added a desktop entry for game in disco-elysium"),
        "{err}"
    );
    let folder = games(&f).join("Disco Elysium");
    assert_eq!(fs::read_to_string(folder.join("disco.exe")).unwrap(), "1");
    let args = fs::read_to_string(f.inno_runs().join("setup_1771589310_1.exe.args")).unwrap();
    assert!(args.contains("/SILENT"), "{args}");
    let committed = state(&f, &folder).committed.unwrap();
    assert_eq!(committed.platform, Platform::Windows);
    assert_eq!(committed.source, Some(Source::Install));
    let config = f.created_config("disco-elysium");
    assert!(config.contains("disco.exe"), "{config}");
}

/// Every file under `root` with its contents and time, but not the login,
/// which a dry run may renew and save.
fn files_but_the_login(f: &Fixture) -> BTreeMap<PathBuf, (String, Vec<u8>)> {
    let paths = f.paths();
    let skip = [
        paths.gog_credentials_generation(),
        paths.gog_credentials_lock(),
    ];
    let mut out = BTreeMap::new();
    let mut stack = vec![f.root.path().to_owned()];
    while let Some(path) = stack.pop() {
        let meta = fs::symlink_metadata(&path).unwrap();
        if path.file_name().is_some_and(|name| name == "gog.key") || skip.contains(&path) {
            continue;
        }
        if meta.is_dir() {
            out.insert(path.clone(), (String::new(), Vec::new()));
            stack.extend(
                fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        } else if meta.is_file() {
            let when = format!("{:?}", meta.modified().unwrap());
            out.insert(path.clone(), (when, fs::read(&path).unwrap()));
        }
    }
    out
}

#[test]
fn a_dry_run_reviews_and_changes_nothing() {
    let (f, api) = world();
    let before = files_but_the_login(&f);
    let out = import(&f, &api, &["--game", &rimworld_id(), "--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with("Install RimWorld (1094900565)\n"),
        "{text}"
    );
    assert!(
        text.ends_with("Dry run, nothing was installed.\n"),
        "{text}"
    );
    assert_eq!(api.downloads(), 0);
    assert_eq!(before, files_but_the_login(&f));
    assert!(!f.paths().gog_product_cache(RIMWORLD).exists());
}

#[test]
fn installing_without_a_terminal_needs_yes() {
    let (f, api) = world();
    let out = import(&f, &api, &["--game", &rimworld_id()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Installing without a terminal needs --yes."),
        "{}",
        stderr(&out)
    );
}

#[test]
fn the_review_is_confirmed_and_declining_cancels() {
    let (f, api) = world();
    let mut pty = spawn(&f, &api, &["import", "gog", "--game", &rimworld_id()]);
    pty.expect("Esc or Ctrl+C to cancel.");
    pty.expect("Install it? [Y/n]: ");
    pty.send("n");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Install canceled."), "{output}");
    assert!(!games(&f).join("RimWorld").exists());

    let mut pty = spawn(&f, &api, &["import", "gog", "--game", &rimworld_id()]);
    pty.expect("Install it? [Y/n]: ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Installed RimWorld as rimworld."),
        "{output}"
    );
}

#[test]
fn install_options_are_refused_when_heroic_has_the_game() {
    let (f, api) = world();
    f.heroic_gog();
    f.heroic_rimworld();
    let before = Snapshot::capture(f.root.path()).unwrap();
    let out = import(
        &f,
        &api,
        &[
            "--game",
            &rimworld_id(),
            "--platform",
            "linux",
            "--dir",
            "/tmp/x",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains(
            "error: Heroic has RimWorld installed, so --game migrates it. Leave out --platform and --dir."
        ),
        "{}",
        stderr(&out)
    );
    before.assert_unchanged(f.root.path());
}

#[test]
fn runner_and_arch_are_refused_for_a_native_install() {
    let (f, api) = world();
    let out = import(
        &f,
        &api,
        &["--game", &rimworld_id(), "--runner", "inno", "--yes"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("error: RimWorld installs natively, so leave out --runner."),
        "{}",
        stderr(&out)
    );
    assert!(!games(&f).join("RimWorld").exists());
}

#[test]
fn the_language_comes_from_the_setting_and_the_platform_can_be_picked() {
    let (f, api) = world();
    f.global_config("[gog]\nlanguage = \"fr-FR\"\n");
    let out = import(&f, &api, &["--game", &rimworld_id(), "--dry-run"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("  language     fr-FR\n"),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out)
            .contains("RimWorld has no Linux installer in fr-FR, so it installs in English."),
        "{}",
        stderr(&out)
    );
    // RimWorld has no Windows installer on this GOG.
    let out = import(
        &f,
        &api,
        &[
            "--game",
            &rimworld_id(),
            "--platform",
            "windows",
            "--language",
            "de-DE",
            "--dry-run",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("RimWorld can't be installed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn an_interrupted_install_is_finished_by_update_or_by_installing_again() {
    for finish in ["update", "import"] {
        let (f, api) = world();
        let mut command = f.gog_command(&api);
        command.env("WINEO_TEST_UPDATE_PAUSE", "publish").args([
            "import",
            "gog",
            "--game",
            &rimworld_id(),
            "--yes",
        ]);
        let mut pty = Pty::spawn(command);
        pty.expect("Publishing");
        let status = Command::new("kill")
            .args(["-INT", &pty.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let (status, output) = pty.finish();
        assert_eq!(status.code(), Some(130), "{output}");
        assert!(output.contains("Install interrupted."), "{output}");
        assert!(
            output.contains("Finish the interrupted install with: wineo update rimworld"),
            "{output}"
        );
        let folder = games(&f).join("RimWorld");
        assert!(state(&f, &folder).journal.is_some());
        // It carries on as it began, so options that would change it are
        // refused.
        let refused = import(
            &f,
            &api,
            &["--game", &rimworld_id(), "--language", "de-DE", "--yes"],
        );
        assert_eq!(refused.status.code(), Some(2));
        assert!(
            stderr(&refused)
                .contains("install didn't finish, so --game finishes it. Leave out --language."),
            "{}",
            stderr(&refused)
        );

        let out = if finish == "update" {
            f.gog(&api, &["update", "rimworld", "--yes"])
        } else {
            import(&f, &api, &["--game", &rimworld_id(), "--yes"])
        };
        assert_success(&out);
        let text = stdout(&out);
        assert!(
            text.contains("Finish the interrupted install of"),
            "{finish}: {text}"
        );
        assert!(
            text.contains("Finished the interrupted install of rimworld.\n"),
            "{finish}: {text}"
        );
        assert!(state(&f, &folder).journal.is_none());
        assert_eq!(
            fs::read_to_string(folder.join("Data/core.txt")).unwrap(),
            "core"
        );
    }
}

#[test]
fn an_installer_that_never_starts_offers_its_own_setup_window() {
    let (f, api) = world();
    disco(&api, "1", &fails_once(&f, 3));
    let mut pty = spawn(
        &f,
        &api,
        &["import", "gog", "--game", &disco_id(), "--runner", "inno"],
    );
    pty.expect("Install it? [Y/n]: ");
    pty.send("y");
    pty.expect("Disco Elysium's installer didn't start. Run its own setup window? [Y/n]: ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Opening Disco Elysium's setup window"),
        "{output}"
    );
    assert!(
        output.contains("Installed Disco Elysium as disco-elysium."),
        "{output}"
    );
    let args = fs::read_to_string(f.inno_runs().join("setup_1771589310_1.exe.args")).unwrap();
    assert!(!args.contains("/SILENT"), "{args}");
}

#[test]
fn declining_the_setup_window_leaves_the_install_to_finish() {
    let (f, api) = world();
    disco(&api, "1", &fails_once(&f, 3));
    let mut pty = spawn(
        &f,
        &api,
        &["import", "gog", "--game", &disco_id(), "--runner", "inno"],
    );
    pty.expect("Install it? [Y/n]: ");
    pty.send("y");
    pty.expect("Run its own setup window? [Y/n]: ");
    pty.send("n");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(
        output.contains("Finish the interrupted install with: wineo update disco-elysium"),
        "{output}"
    );
}

#[test]
fn with_yes_an_installer_that_never_starts_is_reported_without_asking() {
    let (f, api) = world();
    disco(&api, "1", &fails_once(&f, 3));
    let out = import(
        &f,
        &api,
        &["--game", &disco_id(), "--runner", "inno", "--yes"],
    );
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains(
            "error: Disco Elysium's installer stopped before installing anything (exit code 3)."
        ),
        "{err}"
    );
    assert!(
        err.contains("Finish the interrupted install with: wineo update disco-elysium"),
        "{err}"
    );
    // Finishing it at a terminal offers the setup window too.
    let mut pty = spawn(&f, &api, &["update", "disco-elysium"]);
    pty.expect("Finish it? [Y/n]: ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Finished the interrupted install of disco-elysium."),
        "{output}"
    );
}

#[test]
fn a_windows_update_shows_its_installer_and_offers_the_setup_window() {
    let (f, api) = world();
    assert_success(&import(
        &f,
        &api,
        &["--game", &disco_id(), "--runner", "inno", "--yes"],
    ));
    disco(&api, "2", &fails_once(&f, 3));
    let mut pty = spawn(&f, &api, &["update", "disco-elysium", "--refresh"]);
    pty.expect("Update it? [Y/n]: ");
    pty.send("y");
    pty.expect("Disco Elysium's installer didn't start. Run its own setup window? [Y/n]: ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        output.contains("Running Disco Elysium's installer"),
        "{output}"
    );
    assert!(
        output.contains("Opening Disco Elysium's setup window"),
        "{output}"
    );
    assert!(output.contains("Updated disco-elysium."), "{output}");
    let folder = games(&f).join("Disco Elysium");
    assert_eq!(fs::read_to_string(folder.join("disco.exe")).unwrap(), "2");
}

#[test]
fn forget_drops_an_install_that_never_got_a_config() {
    let (f, api) = world();
    let folder = games(&f).join("RimWorld");
    fs::create_dir_all(&folder).unwrap();
    let folder = fs::canonicalize(folder).unwrap();
    let state = FolderState {
        journal: Some(Journal {
            operation: "op".to_owned(),
            kind: JournalKind::Install,
            target: Target {
                gog_id: RIMWORLD,
                platform: Platform::Linux,
                language: "en".to_owned(),
                track: Track::Installer,
                products: Vec::new(),
            },
            packages: Vec::new(),
            attempted: Vec::new(),
        }),
        ..FolderState::new(folder.clone(), Some("rimworld".to_owned()))
    };
    FolderLock::exclusive(&f.paths(), &folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
    let out = import(&f, &api, &["--forget", folder.to_str().unwrap()]);
    assert_success(&out);
    assert!(
        stdout(&out).starts_with("Forgot the GOG state of "),
        "{}",
        stdout(&out)
    );
    assert!(f.gog_state(&folder).is_none());
    assert!(folder.is_dir());
}

/// Ctrl+C while planning, held by `WINEO_TEST_UPDATE_PAUSE=plan`.
fn interrupted_while_planning(f: &Fixture, api: &GogApi) -> (std::process::ExitStatus, String) {
    let mut command = f.gog_command(api);
    command.env("WINEO_TEST_UPDATE_PAUSE", "plan").args([
        "import",
        "gog",
        "--game",
        &rimworld_id(),
        "--yes",
    ]);
    let mut pty = Pty::spawn(command);
    pty.expect("Planning");
    let status = Command::new("kill")
        .args(["-INT", &pty.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let (status, output) = pty.finish();
    (status, output.to_string())
}

#[test]
fn ctrl_c_while_planning_cancels_or_keeps_the_unfinished_install() {
    let (f, api) = world();
    let (status, output) = interrupted_while_planning(&f, &api);
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Install canceled."), "{output}");
    assert!(!games(&f).join("RimWorld").exists());

    // An install that stopped partway is left to finish.
    let mut command = f.gog_command(&api);
    command.env("WINEO_TEST_UPDATE_PAUSE", "publish").args([
        "import",
        "gog",
        "--game",
        &rimworld_id(),
        "--yes",
    ]);
    let mut pty = Pty::spawn(command);
    pty.expect("Publishing");
    Command::new("kill")
        .args(["-INT", &pty.id().to_string()])
        .status()
        .unwrap();
    assert_eq!(pty.finish().0.code(), Some(130));
    let (status, output) = interrupted_while_planning(&f, &api);
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Install interrupted."), "{output}");
    assert!(
        output.contains("Finish the interrupted install with: wineo update rimworld"),
        "{output}"
    );
    assert!(state(&f, &games(&f).join("RimWorld")).journal.is_some());
}

#[test]
fn an_installed_game_says_so_without_a_terminal_or_a_login() {
    let (f, api) = world();
    assert_success(&import(&f, &api, &["--game", &rimworld_id(), "--yes"]));
    assert_success(&import(&f, &api, &["--logout"]));
    let out = import(&f, &api, &["--game", &rimworld_id()]);
    assert_success(&out);
    assert!(
        stdout(&out).starts_with("Already installed. rimworld owns "),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_windows_game_that_names_no_program_says_how_to_pick_one() {
    let (f, api) = world();
    disco_with(&api, "1", "", serde_json::json!([]));
    let out = import(
        &f,
        &api,
        &["--game", &disco_id(), "--runner", "inno", "--yes"],
    );
    assert_success(&out);
    assert!(
        stdout(&out).contains(
            "Disco Elysium names no program to start. Pick one with: wineo setup disco-elysium\n"
        ),
        "{}",
        stdout(&out)
    );
}

#[test]
fn at_a_terminal_a_windows_game_that_names_no_program_offers_what_it_installed() {
    let (f, api) = world();
    let program = "mkdir -p \"$WINEPREFIX/drive_c/Program Files/Disco\" && : > \"$WINEPREFIX/drive_c/Program Files/Disco/disco.exe\"";
    disco_with(&api, "1", program, serde_json::json!([]));
    let mut pty = spawn(
        &f,
        &api,
        &["import", "gog", "--game", &disco_id(), "--runner", "inno"],
    );
    pty.expect("Install it? [Y/n]: ");
    pty.send("y");
    pty.expect("Programs in disco-elysium:");
    pty.send("1");
    pty.expect("Press Enter to add it");
    pty.send("");
    pty.expect("Save these changes?");
    pty.send("");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(!output.contains("names no program to start"), "{output}");
    let config = f.created_config("disco-elysium");
    assert!(
        config.contains(r"C:\Program Files\Disco\disco.exe"),
        "{config}"
    );
}

#[test]
fn a_setup_window_that_also_fails_is_reported_once() {
    let (f, api) = world();
    disco(&api, "1", "exit 3");
    let mut pty = spawn(
        &f,
        &api,
        &["import", "gog", "--game", &disco_id(), "--runner", "inno"],
    );
    pty.expect("Install it? [Y/n]: ");
    pty.send("y");
    pty.expect("Run its own setup window? [Y/n]: ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert_eq!(
        output.raw().matches("Run its own setup window?").count(),
        1,
        "{output}"
    );
    assert!(
        output.contains(
            "error: Disco Elysium's installer stopped before installing anything (exit code 3)."
        ),
        "{output}"
    );
    assert!(
        output.contains("Finish the interrupted install with: wineo update disco-elysium"),
        "{output}"
    );
}
