//! `wineo update` applying updates against a fake GOG that serves real
//! package files: reviews, confirmation, progress, recovery, and batches.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::gog_api::{CODE, GogApi, owned_game, product, success_address};
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

/// What GOG offers for RimWorld at `version`: the base game's package and
/// one for each DLC.
fn offer(api: &GogApi, version: &str) {
    let base = api.linux_package(
        RIMWORLD,
        "en",
        version,
        &gog_package(&[
            Item::Exe("start.sh", &format!("#!/bin/sh\necho {version}\n")),
            Item::File("Data/core.txt", &format!("core {version}")),
        ]),
    );
    let royalty = api.linux_package(
        RIMWORLD_DLC[0],
        "en",
        version,
        &gog_package(&[Item::File(
            "Data/Royalty/about.txt",
            &format!("royalty {version}"),
        )]),
    );
    let ideology = api.linux_package(
        RIMWORLD_DLC[1],
        "en",
        version,
        &gog_package(&[Item::File(
            "Data/Ideology/about.txt",
            &format!("ideology {version}"),
        )]),
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

/// RimWorld as a migration leaves it (no recorded files, one DLC without a
/// version) with an update on offer, and a logged-in wineo.
fn world() -> (Fixture, GogApi, PathBuf) {
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
    logged_in(&f, &api);
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
