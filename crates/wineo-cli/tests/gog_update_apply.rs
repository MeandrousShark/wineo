//! `wineo update` applying updates against a fake GOG that serves real
//! package files: reviews, confirmation, progress, recovery, and batches.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::gog_api::{self, CODE, GogApi, owned_game, product, success_address};
use common::gog_world::{Item, RIMWORLD, RIMWORLD_DLC, gog_package};
use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout};

const V1: &str = "1.6.4871 rev588";
const V2: &str = "1.6.4900 rev600";

fn logged_in(f: &Fixture, api: &GogApi) {
    let mut command = f.gog_command(api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

/// What GOG offers for RimWorld at `version`: the base game's package, with
/// `extra` plain files, and one for each DLC.
fn offer_with(api: &GogApi, version: &str, extra: &[(&str, &str)]) {
    let start = format!("#!/bin/sh\necho {version}\n");
    let core = format!("core {version}");
    let mut items = vec![
        Item::Exe("start.sh", &start),
        Item::File("Data/core.txt", &core),
    ];
    items.extend(extra.iter().map(|(path, text)| Item::File(path, text)));
    let base = api.linux_package(RIMWORLD, "en", version, &gog_package(&items));
    let royalty_text = format!("royalty {version}");
    let royalty = api.linux_package(
        RIMWORLD_DLC[0],
        "en",
        version,
        &gog_package(&[Item::File("Data/Royalty/about.txt", &royalty_text)]),
    );
    let ideology_text = format!("ideology {version}");
    let ideology = api.linux_package(
        RIMWORLD_DLC[1],
        "en",
        version,
        &gog_package(&[Item::File("Data/Ideology/about.txt", &ideology_text)]),
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
    });
}

fn offer(api: &GogApi, version: &str) {
    offer_with(api, version, &[]);
}

/// RimWorld as a migration leaves it (no recorded files, one DLC without a
/// version) with an update on offer, and a logged-in wineo.
fn world() -> (Fixture, GogApi, PathBuf) {
    let (f, api, folder) = logged_out_world();
    logged_in(&f, &api);
    (f, api, folder)
}

fn logged_out_world() -> (Fixture, GogApi, PathBuf) {
    let f = Fixture::new();
    f.heroic_gog();
    let folder = f.heroic_rimworld();
    f.gog_environment(
        "rimworld",
        &folder,
        RIMWORLD,
        V1,
        &[(RIMWORLD_DLC[0], Some(V1)), (RIMWORLD_DLC[1], None)],
    );
    let api = GogApi::with_library(vec![owned_game(RIMWORLD, "RimWorld", true, false)], vec![]);
    offer(&api, V2);
    (f, api, folder)
}

fn read(folder: &Path, path: &str) -> String {
    fs::read_to_string(folder.join(path)).unwrap()
}

fn committed_version(f: &Fixture, folder: &Path) -> Option<String> {
    let state = f.gog_state(&fs::canonicalize(folder).unwrap()).unwrap();
    state.committed.unwrap().products[0].version.clone()
}

#[test]
fn updating_with_yes_downloads_unpacks_and_reports_what_changed() {
    let (f, api, folder) = world();
    let out = f.gog(&api, &["update", "rimworld", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with("Update rimworld\n"), "{text}");
    assert!(text.contains(&format!("RimWorld  {V1} -> {V2}")), "{text}");
    assert!(text.contains(&format!("Royalty   {V1} -> {V2}")), "{text}");
    assert!(
        text.contains(&format!("Ideology  unknown -> {V2}")),
        "{text}"
    );
    assert!(text.contains("Updated rimworld.\n"), "{text}");
    // A migrated game has no recorded files, so a file that differs is
    // overwritten and listed.
    assert!(
        text.contains("Overwrote these files that differed from the new version:\n  start.sh\n"),
        "{text}"
    );
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
    assert_eq!(
        read(&folder, "Data/Royalty/about.txt"),
        format!("royalty {V2}")
    );
    // Mods aren't shipped files, so they stay.
    assert!(folder.join("mods-1.5/MyMod").is_dir());
    assert_eq!(committed_version(&f, &folder).as_deref(), Some(V2));
    assert!(
        stderr(&out).contains("Downloading RimWorld"),
        "{}",
        stderr(&out)
    );
    assert_eq!(api.downloads(), 3);
}

fn journal_left(f: &Fixture, folder: &Path) -> bool {
    f.gog_state(&fs::canonicalize(folder).unwrap())
        .unwrap()
        .journal
        .is_some()
}

fn interrupt(pty: &Pty) {
    let status = Command::new("kill")
        .args(["-INT", &pty.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Starts `update` in a terminal, held at `pause` until Ctrl+C.
fn paused(f: &Fixture, api: &GogApi, pause: &str, args: &[&str]) -> Pty {
    let mut command = f.gog_command(api);
    command.env("WINEO_TEST_UPDATE_PAUSE", pause).arg("update");
    command.args(args);
    Pty::spawn(command)
}

#[test]
fn an_up_to_date_game_says_so_without_asking_or_downloading() {
    let (f, api, _) = world();
    assert_success(&f.gog(&api, &["update", "rimworld", "--yes"]));
    let downloads = api.downloads();
    let out = f.gog(&api, &["update", "rimworld"]);
    assert_success(&out);
    assert_eq!(stdout(&out), "Up to date.\n");
    assert_eq!(stderr(&out), "");
    assert_eq!(api.downloads(), downloads);
}

#[test]
fn an_update_without_a_terminal_needs_yes() {
    let (f, api, folder) = world();
    let out = f.gog(&api, &["update", "rimworld"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("error: Updating without a terminal needs --yes."),
        "{}",
        stderr(&out)
    );
    assert_eq!(stdout(&out), "");
    assert_eq!(api.downloads(), 0);
    assert!(!folder.join("Data/core.txt").exists());
}

#[test]
fn naming_nothing_is_a_usage_error() {
    let (f, api, _) = world();
    let out = f.gog(&api, &["update"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("error: Name a game to update, or use --all."),
        "{}",
        stderr(&out)
    );
    let both = f.gog(&api, &["update", "--all", "--reinstall"]);
    assert_eq!(both.status.code(), Some(2));
}

#[test]
fn a_dry_run_shows_the_review_and_changes_nothing() {
    let (f, api, folder) = world();
    let out = f.gog(&api, &["update", "rimworld", "--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with("Update rimworld\n"), "{text}");
    assert!(text.contains("  download  2.1 KiB\n"), "{text}");
    assert!(text.contains("(an estimate)"), "{text}");
    assert!(text.ends_with("Dry run, nothing was updated.\n"), "{text}");
    assert_eq!(api.downloads(), 0);
    assert!(!folder.join("Data/core.txt").exists());
    assert!(!journal_left(&f, &folder));
    assert_eq!(committed_version(&f, &folder).as_deref(), Some(V1));
}

#[test]
fn the_review_is_confirmed_at_a_terminal() {
    let (f, api, folder) = world();
    let mut command = f.gog_command(&api);
    command.args(["update", "rimworld"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Update rimworld");
    pty.expect("Press Enter to update it, Esc or Ctrl+C to cancel.");
    pty.expect("Update it?");
    assert_eq!(api.downloads(), 0);
    pty.send("");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(output.contains("Updated rimworld."), "{output}");
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
}

#[test]
fn declining_the_review_changes_nothing() {
    let (f, api, folder) = world();
    let mut command = f.gog_command(&api);
    command.args(["update", "rimworld"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Update it?");
    pty.send("n");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Update canceled."), "{output}");
    assert_eq!(api.downloads(), 0);
    assert!(!folder.join("Data/core.txt").exists());
}

#[test]
fn reinstalling_rebuilds_a_game_that_is_up_to_date() {
    let (f, api, folder) = world();
    assert_success(&f.gog(&api, &["update", "rimworld", "--yes"]));
    fs::remove_file(folder.join("Data/core.txt")).unwrap();
    fs::write(folder.join("start.sh"), "mine").unwrap();
    let out = f.gog(&api, &["update", "rimworld", "--reinstall", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with("Reinstall rimworld\n"), "{text}");
    assert!(
        text.contains(&format!("RimWorld  {V2} (rebuilt)")),
        "{text}"
    );
    assert!(text.contains("Reinstalled rimworld.\n"), "{text}");
    // A shipped file you changed is overwritten and listed.
    assert!(
        text.contains("Overwrote these files that differed from the new version:\n  start.sh\n"),
        "{text}"
    );
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
}

#[test]
fn the_result_lists_what_was_saved_kept_and_overwritten() {
    let (f, api, folder) = world();
    offer_with(&api, V2, &[("Data/old.txt", "old")]);
    assert_success(&f.gog(&api, &["update", "rimworld", "--yes"]));
    fs::write(folder.join("Data/core.txt"), "mine").unwrap();
    fs::write(folder.join("Data/old.txt"), "my old").unwrap();
    fs::write(folder.join("Data/new.txt"), "my new").unwrap();
    offer_with(&api, "1.6.5000 rev700", &[("Data/new.txt", "shipped")]);
    // GOG's details are kept for an hour.
    let out = f.gog(&api, &["update", "rimworld", "--yes", "--refresh"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains(
            "Moved your files aside where the new version has files:\n  Data/new.txt -> Data/new.txt.wineo-saved\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "Kept these old files you changed, which the new version no longer ships:\n  Data/old.txt\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "Overwrote these files that differed from the new version:\n  Data/core.txt\n"
        ),
        "{text}"
    );
    assert_eq!(read(&folder, "Data/new.txt.wineo-saved"), "my new");
    assert_eq!(read(&folder, "Data/new.txt"), "shipped");
    assert_eq!(read(&folder, "Data/old.txt"), "my old");
}

#[test]
fn ctrl_c_while_downloading_changes_nothing() {
    let (f, api, folder) = world();
    let mut pty = paused(&f, &api, "download", &["rimworld", "--yes"]);
    pty.expect("Downloading RimWorld");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Update canceled."), "{output}");
    assert!(
        !output.contains("Finish the interrupted update"),
        "{output}"
    );
    assert!(!folder.join("Data/core.txt").exists());
    assert!(!journal_left(&f, &folder));
    assert_eq!(committed_version(&f, &folder).as_deref(), Some(V1));
    assert_success(&f.gog(&api, &["update", "rimworld", "--yes"]));
}

/// An update stopped by Ctrl+C while publishing, which leaves a journal.
fn interrupted_while_publishing(f: &Fixture, api: &GogApi, folder: &Path) {
    let mut pty = paused(f, api, "publish", &["rimworld", "--yes"]);
    pty.expect("Publishing");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Update interrupted."), "{output}");
    assert!(
        output.contains("Finish the interrupted update with: wineo update rimworld"),
        "{output}"
    );
    assert!(journal_left(f, folder));
}

#[test]
fn ctrl_c_while_publishing_keeps_the_journal_and_the_next_run_finishes_it() {
    let (f, api, folder) = world();
    interrupted_while_publishing(&f, &api, &folder);
    let check = f.gog(&api, &["update", "--check", "rimworld"]);
    assert!(
        stdout(&check).contains("Finish the interrupted update with: wineo update rimworld"),
        "{}",
        stdout(&check)
    );
    let downloads = api.downloads();
    let out = f.gog(&api, &["update", "rimworld", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with("Finish the interrupted update of rimworld\n"),
        "{text}"
    );
    assert!(
        text.contains("none, uses the packages already downloaded"),
        "{text}"
    );
    assert!(
        text.contains("Finished the interrupted update of rimworld.\n"),
        "{text}"
    );
    assert!(!journal_left(&f, &folder));
    assert_eq!(committed_version(&f, &folder).as_deref(), Some(V2));
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
    assert_eq!(api.downloads(), downloads);
}

#[test]
fn an_interrupted_update_finishes_offline_when_logged_out() {
    let (f, api, folder) = world();
    interrupted_while_publishing(&f, &api, &folder);
    assert_success(&f.gog(&api, &["import", "gog", "--logout"]));
    let requests = api.requests().len();
    let out = f.gog(&api, &["update", "rimworld", "--yes"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("Finished the interrupted update of rimworld."),
        "{}",
        stdout(&out)
    );
    assert_eq!(api.requests().len(), requests, "it used the network");
    assert!(!journal_left(&f, &folder));
}

#[test]
fn logged_out_without_saved_packages_says_to_log_in() {
    let (f, api, folder) = logged_out_world();
    for args in [
        &["update", "rimworld", "--yes"][..],
        &["update", "--all", "--yes"],
    ] {
        let out = f.gog(&api, args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out).contains("error: Log in to GOG first: wineo import gog --login."),
            "{}",
            stderr(&out)
        );
    }
    assert!(!folder.join("Data/core.txt").exists());
}

#[test]
fn a_portable_folder_is_refused() {
    let (f, api, _) = world();
    let portable = f.root.path().join("portable");
    let out = gog_api::checked(
        f.gog_command(&api)
            .arg("--portable")
            .arg(&portable)
            .args(["update", "rimworld", "--yes"])
            .output()
            .unwrap(),
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("GOG games live in the host library. Run this without --portable."),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_windows_game_is_refused_by_name() {
    let (f, api, _) = world();
    f.galaxy_game_with_update(&api);
    let out = f.gog(&api, &["update", "disco", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("error: Updating Galaxy builds comes in a later release."),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_clone_running_from_the_folder_refuses_and_the_packages_stay_cached() {
    let (f, api, folder) = world();
    let exe = folder.join("RimWorld.exe");
    fs::write(&exe, "").unwrap();
    f.environment(
        "copy",
        &format!("[targets.game]\nexe = \"{}\"\n", exe.display()),
    );
    let mut running = f
        .wineo()
        .args(["exec", "copy", &exe.display().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pids = f.root.path().join("prefixes/copy/fake-pids");
    common::wait_for("the program to start", || pids.exists());

    let out = f.gog(&api, &["update", "rimworld", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "error: copy is running from this game folder. Stop it before updating rimworld."
        ),
        "{}",
        stderr(&out)
    );
    assert!(!folder.join("Data/core.txt").exists());
    assert!(!journal_left(&f, &folder));
    assert_eq!(api.downloads(), 3);

    running.kill().unwrap();
    running.wait().unwrap();
    // The cached packages are used, and a second run waits until the folder is free.
    common::wait_for("the exec to release the folder", || {
        let out = f.gog(&api, &["update", "rimworld", "--yes"]);
        out.status.success()
    });
    assert_eq!(api.downloads(), 3);
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
}

/// A second and third native game, and Disco Elysium from Galaxy. The
/// second can't be planned and the third fails when its download starts.
fn batch_world() -> (Fixture, GogApi, PathBuf) {
    let (f, api, folder) = world();
    f.galaxy_game_with_update(&api);
    f.native_game("beta", 111, "1");
    let mut two_files = gog_api::installer("linux", "en", "2");
    let more = two_files["files"][0].clone();
    two_files["files"].as_array_mut().unwrap().push(more);
    f.native_game("gamma", 222, "1");
    let gamma = api.linux_package(
        222,
        "en",
        "2",
        &gog_package(&[Item::File("Data/gamma.txt", "gamma")]),
    );
    api.with(|config| {
        config
            .products
            .insert(111, product(111, "Beta Game", &[two_files], &[]));
        config
            .products
            .insert(222, product(222, "Gamma Game", &[gamma], &[]));
        config
            .files
            .retain(|name, _| !name.starts_with("game_222_"));
    });
    (f, api, folder)
}

#[test]
fn all_updates_what_it_can_skips_windows_games_and_continues_past_failures() {
    let (f, api, folder) = batch_world();
    let out = f.gog(&api, &["update", "--all", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Skipped disco: Updating Galaxy builds comes in a later release.\n"),
        "{text}"
    );
    assert!(text.contains("Update rimworld\n"), "{text}");
    assert!(text.contains("Update gamma\n"), "{text}");
    assert!(text.contains("Updated rimworld.\n"), "{text}");
    assert!(text.contains("Updated 1 of 2 games.\n"), "{text}");
    assert!(text.contains("2 failed: beta, gamma\n"), "{text}");
    let errors = stderr(&out);
    assert!(
        errors.contains(
            "Failed beta: beta can't be updated: Beta Game's Linux installer comes in 2 files"
        ),
        "{errors}"
    );
    assert!(errors.contains("Failed gamma: "), "{errors}");
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
}

#[test]
fn all_dry_run_reviews_every_game_and_reports_the_ones_that_would_fail() {
    let (f, api, folder) = batch_world();
    let out = f.gog(&api, &["update", "--all", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Update rimworld\n"), "{text}");
    assert!(text.contains("Update gamma\n"), "{text}");
    assert!(text.ends_with("Dry run, nothing was updated.\n"), "{text}");
    assert!(
        stderr(&out).contains("beta: would fail: beta can't be updated:"),
        "{}",
        stderr(&out)
    );
    assert_eq!(api.downloads(), 0);
    assert!(!folder.join("Data/core.txt").exists());

    // Without a game that would fail, a dry run succeeds.
    let (f, api, _) = world();
    f.galaxy_game_with_update(&api);
    let out = f.gog(&api, &["update", "--all", "--dry-run"]);
    assert_success(&out);
}

#[test]
fn all_with_nothing_to_update_says_so() {
    let (f, api, _) = world();
    f.galaxy_game_with_update(&api);
    assert_success(&f.gog(&api, &["update", "rimworld", "--yes"]));
    let out = f.gog(&api, &["update", "--all", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("Skipped disco: "), "{text}");
    assert!(text.ends_with("Nothing to update.\n"), "{text}");
}

#[test]
fn all_asks_once_for_every_game() {
    let (f, api, folder) = world();
    f.native_game("gamma", 222, "1");
    let gamma = api.linux_package(
        222,
        "en",
        "2",
        &gog_package(&[Item::File("Data/gamma.txt", "gamma")]),
    );
    api.with(|config| {
        config
            .products
            .insert(222, product(222, "Gamma Game", &[gamma], &[]));
    });
    let mut command = f.gog_command(&api);
    command.args(["update", "--all"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Update 2 games?");
    assert_eq!(api.downloads(), 0);
    pty.send("");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(output.contains("Updated 2 of 2 games."), "{output}");
    assert_eq!(read(&folder, "Data/core.txt"), format!("core {V2}"));
}

#[test]
fn ctrl_c_stops_a_batch_and_keeps_what_finished() {
    let (f, api, folder) = world();
    f.native_game("gamma", 222, "1");
    let gamma = api.linux_package(
        222,
        "en",
        "2",
        &gog_package(&[Item::File("Data/gamma.txt", "gamma")]),
    );
    api.with(|config| {
        config
            .products
            .insert(222, product(222, "Gamma Game", &[gamma], &[]));
    });
    // Games go in name order, so gamma finishes and rimworld is held.
    let mut pty = paused(&f, &api, "publish:rimworld", &["--all", "--yes"]);
    pty.expect("Updated gamma.");
    pty.expect("Publishing");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(output.contains("Updated 1 of 2 games."), "{output}");
    assert!(output.contains("Update interrupted."), "{output}");
    assert!(
        output.contains("Finish the interrupted update with: wineo update rimworld"),
        "{output}"
    );
    assert!(f.root.path().join("Games/gamma/Data/gamma.txt").exists());
    assert!(journal_left(&f, &folder));
}
