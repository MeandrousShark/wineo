//! Galaxy builds through the CLI against a fake GOG with a content system:
//! installs, checks, updates, repairs, and moves between tracks, with their
//! reviews, progress, reports, and flag rules.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use common::gog_api::{self, CODE, GogApi, owned_game, product, success_address};
use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout};
use serde_json::{Value, json};
use wineo_core::gog::folder::{FolderState, Platform, Track};
use wineo_gog::fake::content::{FakeBuild, FakeDependency, FakeDepot, FakeFile};

const GAME: u64 = 1207658924;

fn logged_in(f: &Fixture, api: &GogApi) {
    let mut command = f.gog_command(api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

/// The game's info file as a Galaxy build ships it.
fn info(build: &str) -> String {
    json!({
        "gameId": GAME.to_string(),
        "rootGameId": GAME.to_string(),
        "buildId": build,
        "name": "Papers, Please",
        "language": "English",
        "playTasks": [{
            "type": "FileTask", "isPrimary": true, "path": "Papers.exe", "category": "game",
        }],
        "version": 1,
    })
    .to_string()
}

/// A build of the game named `version`: its program and levels change from
/// build to build, its core data doesn't.
fn papers(build: &str, version: &str) -> FakeBuild {
    let mut build = FakeBuild::new(GAME, build).depot(
        FakeDepot::new(GAME)
            .file(FakeFile::new(
                "Papers.exe",
                format!("MZ {version}").as_bytes(),
            ))
            .file(FakeFile::new("data\\core.dat", b"core data that never changes").with_md5())
            .file(FakeFile::new(
                "data\\levels.dat",
                format!("levels of {version}").as_bytes(),
            ))
            .file(FakeFile::new(
                &format!("goggame-{GAME}.info"),
                info(build_id(version)).as_bytes(),
            )),
    );
    build.version_name = version.to_owned();
    build
}

/// The build id each version name used here goes with.
fn build_id(version: &str) -> &'static str {
    match version {
        "1.2.76" => "100",
        "1.4.11" => "200",
        "1.5.0" => "300",
        other => panic!("no build for {other}"),
    }
}

/// A logged-in wineo with Papers, Please owned on GOG, its Galaxy build
/// 1.2.76 listed, and a `galaxy` runner.
fn world() -> (Fixture, GogApi) {
    let f = Fixture::new();
    let api = GogApi::with_library(
        vec![owned_game(GAME, "Papers, Please", false, true)],
        vec![],
    );
    api.with(|config| {
        config
            .products
            .insert(GAME, product(GAME, "Papers, Please", &[], &[]));
        config.owned_ids = Some(vec![GAME]);
    });
    api.content
        .add_build(papers("100", "1.2.76").dependency("MSVC2017"));
    with_setup(&api, "0");
    logged_in(&f, &api);
    f.galaxy_runner();
    (f, api)
}

/// Lists build 1.4.11 ahead of 1.2.76.
fn newer(api: &GogApi) {
    api.newest_first
        .store(true, std::sync::atomic::Ordering::Relaxed);
    api.content
        .add_build(papers("200", "1.4.11").dependency("MSVC2017"));
}

fn gog(f: &Fixture, api: &GogApi, args: &[&str]) -> Output {
    gog_api::checked(
        f.gog_command(api)
            .args(args)
            .env("DISPLAY", ":0")
            .output()
            .unwrap(),
    )
}

fn install(f: &Fixture, api: &GogApi, extra: &[&str]) -> Output {
    let mut args = vec![
        "import",
        "gog",
        "--game",
        "1207658924",
        "--runner",
        "galaxy",
        "--yes",
    ];
    args.extend(extra);
    gog(f, api, &args)
}

fn folder(f: &Fixture) -> PathBuf {
    f.root.path().join("Games/Papers, Please")
}

fn state(f: &Fixture) -> FolderState {
    f.gog_state(&fs::canonicalize(folder(f)).unwrap()).unwrap()
}

fn read(f: &Fixture, path: &str) -> String {
    fs::read_to_string(folder(f).join(path)).unwrap()
}

/// The name the game's environment gets.
const NAME: &str = "papers-please";

#[test]
fn a_windows_game_installs_on_the_galaxy_track_by_default() {
    let (f, api) = world();
    let out = install(&f, &api, &[]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with("Install Papers, Please (1207658924)\n"),
        "{text}"
    );
    for row in [
        format!("  environment  {NAME} (new Windows environment)\n").as_str(),
        "  track        Galaxy\n",
        "  platform     Windows\n",
        "  runner       galaxy\n",
        "  version      1.2.76\n",
        "  download     ",
    ] {
        assert!(text.contains(row), "{row:?} in {text}");
    }
    // Galaxy installs know their size.
    assert!(!text.contains("an estimate"), "{text}");
    assert!(
        text.contains(&format!("Installed Papers, Please as {NAME}.\n")),
        "{text}"
    );
    let err = stderr(&out);
    assert!(err.contains("Downloading: "), "{err}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(read(&f, "data/levels.dat"), "levels of 1.2.76");
    let committed = state(&f).committed.unwrap();
    assert_eq!(committed.track, Track::Galaxy);
    assert_eq!(committed.platform, Platform::Windows);
    assert_eq!(committed.products[0].build.as_deref(), Some("100"));
}

/// GOG's script interpreter and one runtime, which exit with `code`.
fn with_setup(api: &GogApi, code: &str) {
    api.content.set_dependencies(
        "100",
        &[FakeDependency::new(
            "MSVC2017",
            "__redist\\MSVC2017\\vc.exe",
            "/quiet",
            FakeDepot::new(0)
                .file(FakeFile::new("__redist\\MSVC2017\\vc.exe", b"vc"))
                .file(FakeFile::new(
                    "__redist\\MSVC2017\\vc.exe.exit",
                    code.as_bytes(),
                )),
        )],
    );
}

/// Papers, Please from its Windows installer, whose setup program leaves
/// the program `content` says, and its details.
fn installer_package(api: &GogApi, version: &str, content: &str) {
    let info = json!({
        "gameId": GAME.to_string(), "rootGameId": GAME.to_string(), "name": "Papers, Please",
        "language": "English", "version": 1,
        "playTasks": [{"type": "FileTask", "isPrimary": true, "path": "Papers.exe", "category": "game"}],
    });
    let script = format!(
        "mkdir -p \"$DIR\"\nprintf '{content}' > \"$DIR/Papers.exe\"\ncat > \"$DIR/goggame-{GAME}.info\" <<'INFO'\n{info}\nINFO\nexit 0\n"
    );
    let entry = api.windows_package(GAME, "en", version, &script);
    api.with(|config| {
        config
            .products
            .insert(GAME, product(GAME, "Papers, Please", &[entry], &[]));
    });
}

/// Papers, Please installed on the Galaxy track at 1.2.76.
fn installed() -> (Fixture, GogApi) {
    let (f, api) = world();
    assert_success(&install(&f, &api, &[]));
    (f, api)
}

fn update(f: &Fixture, api: &GogApi, extra: &[&str]) -> Output {
    let mut args = vec!["update", NAME];
    args.extend(extra);
    gog(f, api, &args)
}

fn json_of(out: &Output) -> Value {
    serde_json::from_str(&stdout(out)).unwrap()
}

#[test]
fn the_installer_track_can_be_asked_for() {
    let (f, api) = world();
    f.inno_runner();
    installer_package(&api, "1.5", "installer");
    let out = install(&f, &api, &["--track", "installer"]);
    // The galaxy runner was asked for by `install`, the installer's wine is
    // the same one.
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("  track        installer\n"), "{text}");
    assert!(text.contains("  version      1.5\n"), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "installer");
    let committed = state(&f).committed.unwrap();
    assert_eq!(committed.track, Track::Installer);
    assert_eq!(committed.products[0].version.as_deref(), Some("1.5"));

    // Left alone, the same game would have gone on Galaxy.
    let (f, api) = world();
    installer_package(&api, "1.5", "installer");
    assert!(stdout(&install(&f, &api, &[])).contains("  track        Galaxy\n"));
}

#[test]
fn a_galaxy_install_shows_setup_and_runs_it() {
    let (f, api) = world();
    let out = install(&f, &api, &[]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("  setup        MSVC2017\n"), "{text}");
    let err = stderr(&out);
    assert!(err.contains("Running setup 1 of 1: MSVC2017"), "{err}");
    let log = fs::read_to_string(f.inno_runs().join("cmd.log")).unwrap();
    assert!(log.contains("vc.exe"), "{log}");
}

#[test]
fn check_shows_version_names_and_json_keeps_build_ids() {
    let (f, api) = installed();
    let out = gog(&f, &api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    let row = text.lines().nth(1).unwrap();
    assert_eq!(
        row.split_whitespace().collect::<Vec<_>>(),
        [NAME, "1.2.76", "1.2.76", "up", "to", "date"],
        "{text}"
    );

    newer(&api);
    let out = gog(&f, &api, &["update", "--check"]);
    let row = stdout(&out).lines().nth(1).unwrap().to_owned();
    assert_eq!(
        row.split_whitespace().collect::<Vec<_>>(),
        [NAME, "1.2.76", "1.4.11", "update", "available"]
    );
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    assert_eq!(
        json_of(&out)["environments"],
        json!([{
            "name": NAME, "id": GAME.to_string(), "track": "galaxy",
            "installed": "100", "available": "200",
            "update": "available", "applicable": true, "reason": null,
            "unfinished": false, "dlc": [],
        }])
    );
}

#[test]
fn an_update_reviews_then_reports_with_the_files_it_moved_aside() {
    let (f, api) = installed();
    newer(&api);
    // Edited by you, and the new build changes it.
    fs::write(folder(&f).join("data/levels.dat"), "my levels").unwrap();
    let out = update(&f, &api, &["--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with(&format!(
            "Update {NAME}\n  folder          ~/Games/Papers, Please\n  track           Galaxy\n  Papers, Please  1.2.76 -> 1.4.11\n  download        "
        )),
        "{text}"
    );
    assert!(
        text.ends_with(
            "Setup's own writes in the prefix aren't counted in the space check.\nDry run, nothing was updated.\n"
        ),
        "{text}"
    );
    // Nothing changed.
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert!(!text.contains("Nothing is deleted"), "{text}");

    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "Updated {NAME}.\n  Papers, Please  1.2.76 -> 1.4.11\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(
            "Moved your files aside where the new version has files:\n  data/levels.dat -> data/levels.dat.wineo-saved\n"
        ),
        "{text}"
    );
    assert!(stderr(&out).contains("Downloading: "), "{}", stderr(&out));
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    assert_eq!(read(&f, "data/levels.dat"), "levels of 1.4.11");
    assert_eq!(read(&f, "data/levels.dat.wineo-saved"), "my levels");
    assert_eq!(
        state(&f).committed.unwrap().products[0].build.as_deref(),
        Some("200")
    );
    let again = update(&f, &api, &[]);
    assert_eq!(stdout(&again), "Up to date.\n");
}

#[test]
fn a_repair_finds_same_size_damage_and_saves_the_file_aside() {
    let (f, api) = installed();
    let core = "core data that never changes";
    let damaged = "x".repeat(core.len());
    fs::write(folder(&f).join("data/core.dat"), &damaged).unwrap();

    // A plain update trusts the file; the repair doesn't.
    let out = update(&f, &api, &[]);
    assert_eq!(stdout(&out), "Up to date.\n");
    assert_eq!(read(&f, "data/core.dat"), damaged);

    let out = update(&f, &api, &["--repair", "--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with(&format!(
            "Repair {NAME}\n  folder          ~/Games/Papers, Please\n  track           Galaxy\n  Papers, Please  1.2.76\n  check           every file, 4 files (264 B)\n  setup           MSVC2017, all run again\n  download        only what's missing or damaged\n"
        )),
        "{text}"
    );
    assert!(text.ends_with("Dry run, nothing was updated.\n"), "{text}");
    assert_eq!(read(&f, "data/core.dat"), damaged);
    assert!(!folder(&f).join("data/core.dat.wineo-saved").exists());

    let out = update(&f, &api, &["--repair", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains(&format!("Repaired {NAME}.\n  Papers, Please  1.2.76\n")),
        "{text}"
    );
    assert!(
        text.contains(
            "Saved these files aside, since they didn't match GOG's:\n  data/core.dat -> data/core.dat.wineo-saved\n"
        ),
        "{text}"
    );
    let err = stderr(&out);
    assert!(err.contains("Checking files: 4 / 4"), "{err}");
    assert!(err.contains("Running setup 1 of 1: MSVC2017"), "{err}");
    assert_eq!(read(&f, "data/core.dat"), core);
    assert_eq!(read(&f, "data/core.dat.wineo-saved"), damaged);
}

#[test]
fn a_move_onto_galaxy_keeps_what_matches_and_says_nothing_is_deleted() {
    let (f, api) = world();
    installer_package(&api, "1.5", "installer");
    let out = install(&f, &api, &["--track", "installer"]);
    assert_success(&out);
    fs::write(folder(&f).join("left-behind.txt"), "the installer's").unwrap();

    let out = update(&f, &api, &["--reinstall", "--track", "galaxy", "--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with(&format!(
            "Reinstall {NAME}\n  folder          ~/Games/Papers, Please\n  track           installer -> Galaxy\n  Papers, Please  1.5 -> 1.2.76\n  setup           MSVC2017, all run again\n  download        up to "
        )),
        "{text}"
    );
    assert!(
        text.contains(", files that already match are kept\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "Nothing is deleted: files the installer left that this build doesn't list stay.\n"
        ),
        "{text}"
    );
    assert_eq!(state(&f).committed.unwrap().track, Track::Installer);

    let out = update(&f, &api, &["--reinstall", "--track", "galaxy", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "Reinstalled {NAME}.\n  Papers, Please  1.5 -> 1.2.76\n"
        )),
        "{text}"
    );
    // The installer's program differs from the build's, so it is saved.
    assert!(
        text.contains("  Papers.exe -> Papers.exe.wineo-saved\n"),
        "{text}"
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(read(&f, "left-behind.txt"), "the installer's");
    let committed = state(&f).committed.unwrap();
    assert_eq!(committed.track, Track::Galaxy);
    assert_eq!(committed.products[0].build.as_deref(), Some("100"));
}

#[test]
fn a_move_to_the_installer_track_reviews_the_track_change() {
    let (f, api) = installed();
    f.inno_runner();
    installer_package(&api, "1.5", "installer");
    let out = update(
        &f,
        &api,
        &[
            "--reinstall",
            "--track",
            "installer",
            "--refresh",
            "--dry-run",
        ],
    );
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with(&format!("Reinstall {NAME}\n")), "{text}");
    assert!(
        text.contains("  track           Galaxy -> installer\n"),
        "{text}"
    );
    assert!(text.contains("  Papers, Please  1.5\n"), "{text}");
    assert_eq!(state(&f).committed.unwrap().track, Track::Galaxy);

    let out = update(
        &f,
        &api,
        &["--reinstall", "--track", "installer", "--refresh", "--yes"],
    );
    assert_success(&out);
    let committed = state(&f).committed.unwrap();
    assert_eq!(committed.track, Track::Installer);
    assert_eq!(committed.products[0].version.as_deref(), Some("1.5"));
    assert_eq!(read(&f, "Papers.exe"), "installer");
    // The record of Galaxy's files goes with the track.
    let paths = f.paths();
    assert!(
        !wineo_core::gog::folder::sidecar_file(&paths, &fs::canonicalize(folder(&f)).unwrap())
            .exists()
    );
}

#[test]
fn all_updates_a_galaxy_game_with_the_rest() {
    let (f, api) = installed();
    newer(&api);
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains(&format!("Update {NAME}\n")), "{text}");
    assert!(text.contains(&format!("Updating {NAME}\n")), "{text}");
    assert!(text.contains(&format!("Updated {NAME}.\n")), "{text}");
    assert!(text.contains("Updated 1 of 1 game.\n"), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_eq!(stdout(&out), "Nothing to update.\n");
}

/// Starts `wineo` in a terminal, held at the first download until Ctrl+C.
fn paused(f: &Fixture, api: &GogApi, args: &[&str]) -> Pty {
    let mut command = f.gog_command(api);
    command
        .env("DISPLAY", ":0")
        .env("WINEO_TEST_UPDATE_PAUSE", "download")
        .args(args);
    Pty::spawn(command)
}

fn interrupt(pty: &Pty) {
    let status = std::process::Command::new("kill")
        .args(["-INT", &pty.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

/// An update to the newer build that Ctrl+C stopped after its journal.
fn interrupted_update(f: &Fixture, api: &GogApi) {
    newer(api);
    let mut pty = paused(f, api, &["update", NAME, "--yes"]);
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Update interrupted."), "{output}");
    assert!(
        output.contains(&format!(
            "Finish the interrupted update with: wineo update {NAME}"
        )),
        "{output}"
    );
    assert!(state(f).journal.is_some());
}

#[test]
fn all_finishes_a_galaxy_update_that_stopped() {
    let (f, api) = installed();
    interrupted_update(&f, &api);
    let check = gog(&f, &api, &["update", "--check"]);
    assert!(
        stdout(&check).contains(&format!(
            "Finish the interrupted update with: wineo update {NAME}"
        )),
        "{}",
        stdout(&check)
    );

    // The game won't launch half updated; `--all` finishes it.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains(&format!("Finished the interrupted update of {NAME}.\n")),
        "{text}"
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    assert!(state(&f).journal.is_none());
}

#[test]
fn all_finishes_a_journal_whose_check_failed_but_not_a_game_without_one() {
    let (f, api) = installed();
    interrupted_update(&f, &api);
    api.builds_status
        .store(503, std::sync::atomic::Ordering::Relaxed);
    // The check can't list builds, but the journal and its saved target are
    // enough to finish.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains(&format!("Finished the interrupted update of {NAME}.\n")),
        "{}",
        stdout(&out)
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    assert!(state(&f).journal.is_none());

    // With nothing to finish, the failed check is a failure.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("Failed {NAME}: ")),
        "{}",
        stderr(&out)
    );
}

#[test]
fn all_skips_a_galaxy_game_without_a_graphical_session() {
    let (f, api) = installed();
    newer(&api);
    let mut command = f.gog_command(&api);
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .args(["update", "--all", "--yes"]);
    let out = command.output().unwrap();
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "Skipped {NAME}: Updating a Windows game needs a graphical session. Run this from your desktop.\n"
        )),
        "{text}"
    );
    assert!(text.ends_with("Nothing to update.\n"), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
}

#[test]
fn a_build_gone_from_gog_retargets_when_you_say_yes() {
    let (f, api) = installed();
    interrupted_update(&f, &api);
    // Its saved target is lost, and GOG lists a newer build instead.
    fs::remove_file(wineo_core::gog::folder::sidecar_file(
        &f.paths(),
        &fs::canonicalize(folder(&f)).unwrap(),
    ))
    .unwrap();
    api.content
        .add_build(papers("300", "1.5.0").dependency("MSVC2017"));
    api.hidden_builds.lock().unwrap().insert("200".to_owned());

    let mut command = f.gog_command(&api);
    command.env("DISPLAY", ":0").args(["update", NAME]);
    let mut pty = Pty::spawn(command);
    pty.expect("Finish it?");
    pty.send("n");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(
        output.contains("GOG no longer lists build 200 of Papers, Please, so the update finishes with build 300 instead."),
        "{output}"
    );
    assert!(
        output.contains(&format!("Finish the interrupted update of {NAME}\n")),
        "{output}"
    );
    assert!(output.contains("1.5.0"), "{output}");
    // Declined: the journal waits, with nothing committed.
    let waiting = state(&f);
    assert!(waiting.journal.is_some());
    assert_eq!(
        waiting.committed.unwrap().products[0].build.as_deref(),
        Some("100")
    );

    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains(&format!("Finished the interrupted update of {NAME}.\n")),
        "{}",
        stdout(&out)
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.5.0");
    assert_eq!(
        state(&f).committed.unwrap().products[0].build.as_deref(),
        Some("300")
    );
}

/// Every file under `root` with its contents, except the files named in
/// `skip`.
fn tree(root: &std::path::Path, skip: &[&str]) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(
        path: &std::path::Path,
        skip: &[&str],
        out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if skip.contains(&name) {
            return;
        }
        let meta = fs::symlink_metadata(path).unwrap();
        if meta.is_dir() {
            out.insert(path.to_owned(), Vec::new());
            for entry in fs::read_dir(path).unwrap() {
                walk(&entry.unwrap().path(), skip, out);
            }
        } else if meta.is_file() {
            out.insert(path.to_owned(), fs::read(path).unwrap());
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, skip, &mut out);
    out
}

#[test]
fn a_dry_run_leaves_the_cache_and_another_tools_build_alone() {
    let (f, api) = installed();
    newer(&api);
    api.content
        .add_build(papers("300", "1.5.0").dependency("MSVC2017"));
    // Another tool updated the game to 1.4.11, which a real run would
    // record.
    let info_file = folder(&f).join(format!("goggame-{GAME}.info"));
    fs::write(&info_file, info("200")).unwrap();
    let paths = f.paths();
    fs::remove_file(paths.gog_product_cache(GAME)).unwrap();
    let generation = paths.gog_credentials_generation();
    let login = ["gog.key", generation.file_name().unwrap().to_str().unwrap()];
    let before = tree(f.root.path(), &login);
    for extra in [&["--dry-run"][..], &["--repair", "--dry-run"]] {
        let out = update(&f, &api, extra);
        assert_success(&out);
        assert!(
            stdout(&out).ends_with("Dry run, nothing was updated.\n"),
            "{}",
            stdout(&out)
        );
        assert!(!paths.gog_product_cache(GAME).exists());
        assert_eq!(tree(f.root.path(), &login), before);
        assert_eq!(
            state(&f).committed.unwrap().products[0].build.as_deref(),
            Some("100")
        );
    }
    // A real check does record it.
    assert_success(&gog(&f, &api, &["update", "--check"]));
    assert_eq!(
        state(&f).committed.unwrap().products[0].build.as_deref(),
        Some("200")
    );
}

#[test]
fn flag_conflicts_are_usage_errors() {
    let (f, api) = installed();
    for args in [
        &["update", NAME, "--repair", "--reinstall"][..],
        &["update", NAME, "--repair", "--track", "galaxy"],
        &["update", "--all", "--repair"],
        &["update", NAME, "--repair", "--check"],
        &["update", NAME, "--track", "galaxy"],
        &["update", "--all", "--reinstall", "--track", "galaxy"],
        &["update", NAME, "--reinstall", "--track", "nowhere"],
        &["import", "gog", "--track", "galaxy"],
        &["import", "gog", "--game", "1", "--track", "nowhere"],
    ] {
        let out = gog(&f, &api, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    // Nothing changed.
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
}

#[test]
fn a_repair_of_an_installer_game_says_how_to_move_it() {
    let (f, api) = world();
    f.inno_runner();
    installer_package(&api, "1.5", "installer");
    assert_success(&install(&f, &api, &["--track", "installer"]));
    let out = update(&f, &api, &["--repair", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(&format!(
            "error: Only Galaxy games can be repaired. Move it with: wineo update {NAME} --reinstall --track galaxy"
        )),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_setup_step_that_fails_keeps_the_journal_and_update_finishes_it() {
    let (f, api) = world();
    with_setup(&api, "1");
    let out = install(&f, &api, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("error: Setting up Papers, Please stopped: MSVC2017 failed (exit code 1).\n"),
        "{err}"
    );
    assert!(
        err.trim_end()
            .ends_with("Finish the interrupted install with: wineo import gog --game 1207658924"),
        "{err}"
    );
    // Updating finishes an update's setup the same way.
    let (f, api) = installed();
    newer(&api);
    api.content.set_dependencies(
        "200",
        &[FakeDependency::new(
            "MSVC2017",
            "__redist\\MSVC2017\\vc2.exe",
            "/quiet",
            FakeDepot::new(0)
                .file(FakeFile::new("__redist\\MSVC2017\\vc2.exe", b"vc2"))
                .file(FakeFile::new("__redist\\MSVC2017\\vc2.exe.exit", b"1")),
        )],
    );
    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("Setting up Papers, Please stopped: MSVC2017 failed (exit code 1)."),
        "{err}"
    );
    assert!(
        err.trim_end()
            .ends_with(&format!("Finish it with: wineo update {NAME}")),
        "{err}"
    );
    assert!(state(&f).journal.is_some());
    // The runtime is fixed where setup reads it from, and updating carries
    // on from that step without downloading again.
    let redist = f.root.path().join("data/wineo/gog/redist/MSVC2017");
    let code = fs::read_dir(&redist)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("__redist/MSVC2017/vc2.exe.exit"))
        .find(|path| path.exists())
        .unwrap();
    fs::write(code, "0").unwrap();
    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains(&format!("Finished the interrupted update of {NAME}.\n")),
        "{}",
        stdout(&out)
    );
    assert!(state(&f).journal.is_none());
}

#[test]
fn a_restart_a_redistributable_asks_for_is_noted() {
    let (f, api) = world();
    with_setup(&api, "3010");
    let out = install(&f, &api, &[]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("MSVC2017 asked for a restart, which Wine doesn't need.\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn an_interrupted_galaxy_install_is_finished_by_installing_again() {
    let (f, api) = world();
    let mut command = f.gog_command(&api);
    command
        .env("DISPLAY", ":0")
        .env("WINEO_TEST_UPDATE_PAUSE", "download")
        .args([
            "import",
            "gog",
            "--game",
            "1207658924",
            "--runner",
            "galaxy",
            "--yes",
        ]);
    let mut pty = Pty::spawn(command);
    pty.expect("Downloading: ");
    let status = std::process::Command::new("kill")
        .args(["-INT", &pty.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Install interrupted."), "{output}");
    assert!(
        output.contains("Finish the interrupted install with: wineo import gog --game 1207658924"),
        "{output}"
    );

    // Checking says the same, and updating can't carry on a first install.
    let check = gog(&f, &api, &["update", "--check"]);
    assert!(
        stdout(&check)
            .contains("Finish the interrupted install with: wineo import gog --game 1207658924"),
        "{}",
        stdout(&check)
    );
    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.trim_end().ends_with(
            "papers-please's GOG install didn't finish. Finish it with: wineo import gog --game 1207658924"
        ),
        "{err}"
    );
    assert!(!err.contains("wineo update"), "{err}");

    let out = gog(
        &f,
        &api,
        &["import", "gog", "--game", "1207658924", "--yes"],
    );
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with("Finish the interrupted install of Papers, Please (1207658924)\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("Finished the interrupted install of {NAME}.\n")),
        "{text}"
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert!(state(&f).journal.is_none());
}

#[test]
fn start_and_rm_name_the_install_command_for_an_interrupted_galaxy_install() {
    let (f, api) = world();
    let mut pty = paused(
        &f,
        &api,
        &[
            "import",
            "gog",
            "--game",
            "1207658924",
            "--runner",
            "galaxy",
            "--yes",
        ],
    );
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(state(&f).journal.is_some());

    let finish = "GOG install didn't finish. Finish it with wineo import gog --game 1207658924";
    let out = f.run(&["start", NAME]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains(finish), "{err}");
    assert!(!err.contains("wineo update"), "{err}");

    let out = f.run(&["rm", NAME]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains(finish), "{err}");
    assert!(!err.contains("wineo update"), "{err}");
    assert!(err.contains("or remove it anyway with --force."), "{err}");
}

/// `wineo` with its run state in a folder whose name setup's batch files
/// can't carry, so a Galaxy setup step fails without an exit code.
fn with_odd_state(f: &Fixture, api: &GogApi, args: &[&str]) -> Output {
    gog_api::checked(
        f.gog_command(api)
            .args(args)
            .env("DISPLAY", ":0")
            .env("XDG_STATE_HOME", f.root.path().join("státe"))
            .output()
            .unwrap(),
    )
}

#[test]
fn an_install_that_fails_after_its_journal_says_how_to_finish_it() {
    let (f, api) = world();
    let out = with_odd_state(
        &f,
        &api,
        &[
            "import",
            "gog",
            "--game",
            "1207658924",
            "--runner",
            "galaxy",
            "--yes",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("error: "), "{err}");
    assert!(
        err.trim_end()
            .ends_with("Finish the interrupted install with: wineo import gog --game 1207658924"),
        "{err}"
    );
    assert!(!err.contains("wineo update"), "{err}");
    assert!(state(&f).journal.is_some());

    let out = gog(
        &f,
        &api,
        &["import", "gog", "--game", "1207658924", "--yes"],
    );
    assert_success(&out);
    assert!(state(&f).journal.is_none());
}

#[test]
fn a_repair_that_fails_after_its_journal_says_how_to_finish_it() {
    let (f, api) = installed();
    let out = with_odd_state(&f, &api, &["update", NAME, "--repair", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.trim_end()
            .ends_with(&format!("Finish it with: wineo update {NAME}")),
        "{err}"
    );
    assert_eq!(
        err.matches("Finish it with").count(),
        1,
        "the hint comes once: {err}"
    );
    assert!(state(&f).journal.is_some());

    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains(&format!("Finished the interrupted repair of {NAME}.\n")),
        "{}",
        stdout(&out)
    );
}

/// The saved target of the folder's unfinished operation, lost.
fn lose_the_saved_target(f: &Fixture) {
    fs::remove_file(wineo_core::gog::folder::sidecar_file(
        &f.paths(),
        &fs::canonicalize(folder(f)).unwrap(),
    ))
    .unwrap();
}

#[test]
fn a_planning_failure_with_a_journal_says_how_to_finish_it() {
    let (f, api) = installed();
    interrupted_update(&f, &api);
    lose_the_saved_target(&f);
    api.builds_status
        .store(503, std::sync::atomic::Ordering::Relaxed);
    let command = format!("Finish it with: wineo update {NAME}");

    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.trim_end().ends_with(&command), "{err}");
    assert_eq!(err.matches("Finish it with").count(), 1, "{err}");

    // `--all` reports the game that couldn't be planned the same way.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    let failed = err
        .lines()
        .find(|line| line.starts_with(&format!("Failed {NAME}: ")))
        .unwrap_or_else(|| panic!("{err}"));
    assert!(failed.ends_with(&command), "{err}");
    assert_eq!(err.matches("Finish it with").count(), 1, "{err}");

    // Once GOG answers, it finishes.
    api.builds_status
        .store(0, std::sync::atomic::Ordering::Relaxed);
    assert_success(&update(&f, &api, &["--yes"]));
    assert!(state(&f).journal.is_none());
}

#[test]
fn a_resumed_galaxy_install_that_cant_be_planned_says_how_to_finish_it() {
    let (f, api) = world();
    let mut pty = paused(
        &f,
        &api,
        &[
            "import",
            "gog",
            "--game",
            "1207658924",
            "--runner",
            "galaxy",
            "--yes",
        ],
    );
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, _) = pty.finish();
    assert_eq!(status.code(), Some(130));
    lose_the_saved_target(&f);
    api.builds_status
        .store(503, std::sync::atomic::Ordering::Relaxed);
    let out = gog(
        &f,
        &api,
        &["import", "gog", "--game", "1207658924", "--yes"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.trim_end()
            .ends_with("Finish it with: wineo import gog --game 1207658924"),
        "{err}"
    );
    assert!(!err.contains("wineo update"), "{err}");
}
