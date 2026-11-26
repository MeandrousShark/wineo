//! Old spellings of flags and subcommands keep working and warn on stderr;
//! the new spellings and their aliases work quietly.

mod common;

use std::fs;

use common::pty::Pty;
use common::releases_api::{ReleasesApi, Repo, component_archive, proton_archive};
use common::*;

const DELETE_PREFIX: &str = "warning: --delete-prefix is deprecated; use --include-prefix\n";
const SNAPSHOT_DELETE: &str = "warning: snapshot delete is deprecated; use snapshot remove\n";

fn created(f: &Fixture, name: &str) {
    assert_success(&f.run(&["new", "-n", name]));
}

#[test]
fn rm_delete_prefix_still_works_and_warns_once() {
    let f = Fixture::new();
    created(&f, "old");
    created(&f, "new");
    let prefix = |name: &str| f.root.path().join("data/wineo/prefixes").join(name);
    assert!(prefix("old").exists());

    let old = f.run(&["rm", "old", "--delete-prefix"]);
    assert_success(&old);
    assert_eq!(stderr(&old), DELETE_PREFIX);
    assert!(!stdout(&old).contains("deprecated"), "{}", stdout(&old));
    assert!(!prefix("old").exists());

    let new = f.run(&["rm", "new", "--include-prefix"]);
    assert_success(&new);
    assert!(!stderr(&new).contains("deprecated"), "{}", stderr(&new));
    assert!(!prefix("new").exists());
    // Same text apart from the name and the trash id.
    assert!(
        stdout(&old).contains("and its prefix to trash"),
        "{}",
        stdout(&old)
    );
    assert!(
        stdout(&new).contains("and its prefix to trash"),
        "{}",
        stdout(&new)
    );
}

#[test]
fn rm_delete_prefix_keeps_its_refusals_and_exit_code() {
    let f = Fixture::new();
    let old = f.run(&["rm", "missing", "--delete-prefix"]);
    let new = f.run(&["rm", "missing", "--include-prefix"]);
    assert_eq!(old.status.code(), new.status.code());
    assert!(!old.status.success());
    assert!(stderr(&old).starts_with(DELETE_PREFIX), "{}", stderr(&old));
    assert_eq!(stderr(&old).replacen(DELETE_PREFIX, "", 1), stderr(&new));
}

#[test]
fn rm_help_shows_only_the_new_flag_and_the_remove_alias() {
    let f = Fixture::new();
    let help = stdout(&f.run(&["rm", "--help"]));
    assert!(help.contains("--include-prefix"), "{help}");
    assert!(!help.contains("delete-prefix"), "{help}");
    let top = stdout(&f.run(&["--help"]));
    let line = top
        .lines()
        .find(|l| l.contains("Remove an environment"))
        .unwrap();
    assert!(line.contains("[alias: remove]"), "{line}");
}

#[test]
fn remove_is_an_alias_of_rm_without_a_warning() {
    let f = Fixture::new();
    created(&f, "game");
    let removed = f.run(&["remove", "game", "--include-prefix"]);
    assert_success(&removed);
    assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
    assert!(!f.config_dir().join("environments/game.toml").exists());
}

#[test]
fn build_and_runner_removal_accept_rm_and_remove() {
    let f = Fixture::new();
    for component in ["dxvk", "vkd3d"] {
        let dirs: &[&str] = if component == "dxvk" {
            &["x64", "x32"]
        } else {
            &["x64", "x86"]
        };
        // The newest build stays, since a template asks for latest.
        f.install_build(component, "9.9", dirs);
        for (verb, name) in [("rm", "1.0"), ("remove", "2.0")] {
            f.install_build(component, name, dirs);
            let removed = f.run(&[component, verb, name, "--yes"]);
            assert_success(&removed);
            assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
            assert!(
                !f.root
                    .path()
                    .join("data/wineo")
                    .join(component)
                    .join(name)
                    .exists(),
                "{component} {verb}"
            );
        }
    }
    for (verb, name) in [("rm", "wine-a"), ("remove", "wine-b")] {
        f.install_runner(name);
        let removed = f.run(&["runners", verb, name, "--yes"]);
        assert_success(&removed);
        assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
        assert!(!f.root.path().join("data/wineo/runners").join(name).exists());
    }
}

#[test]
fn art_removal_accepts_rm_and_remove() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let png = f.root.path().join("grid.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
    for verb in ["rm", "remove"] {
        assert_success(&f.run(&["art", "set", "hades", "grid", png.to_str().unwrap()]));
        let removed = f.run(&["art", verb, "hades", "grid"]);
        assert_success(&removed);
        assert_eq!(stdout(&removed), "Removed grid from hades\n");
        assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
    }
}

#[test]
fn snapshot_delete_warns_once_and_remove_and_rm_do_not() {
    let f = Fixture::new();
    f.environment("game", "[targets.game]\nexe = \"game.exe\"\n");
    // No snapshot exists, so every spelling fails the same way; what differs
    // is only whether the deprecation line comes first.
    let old = f.run(&["snapshot", "delete", "game", "none"]);
    assert!(!old.status.success());
    assert!(
        stderr(&old).starts_with(SNAPSHOT_DELETE),
        "{}",
        stderr(&old)
    );
    for verb in ["remove", "rm"] {
        let new = f.run(&["snapshot", verb, "game", "none"]);
        assert_eq!(new.status.code(), old.status.code());
        assert_eq!(stderr(&new), stderr(&old).replacen(SNAPSHOT_DELETE, "", 1));
    }

    let help = stdout(&f.run(&["snapshot", "--help"]));
    let line = help
        .lines()
        .find(|l| l.contains("Remove a named snapshot"))
        .unwrap();
    assert!(
        line.contains("remove") && line.contains("[alias: rm]"),
        "{line}"
    );
    assert!(!help.contains("delete"), "{help}");
}

/// One thing `download` fetches, and how each spelling of the command
/// names it.
struct Kind {
    id: &'static str,
    repo: Repo,
    tag: &'static str,
    asset: &'static str,
    /// The folder a download makes under wineo's data folder.
    folder: &'static str,
    /// The new command, up to where its flags start.
    new: &'static [&'static str],
    /// What the old command's warning names in its place.
    says: &'static str,
}

const KINDS: [Kind; 4] = [
    Kind {
        id: "ge-proton",
        repo: Repo::GeProton,
        tag: "GE-Proton10-27",
        asset: "GE-Proton10-27.tar.gz",
        folder: "runners/GE-Proton10-27",
        new: &["runners", "download", "ge-proton"],
        says: "runners download",
    },
    Kind {
        id: "proton-cachyos",
        repo: Repo::ProtonCachyos,
        tag: "cachyos-10.0-20261101-slr",
        asset: "proton-cachyos-10.0-20261101-slr-x86_64.tar.gz",
        folder: "runners/proton-cachyos-10.0-20261101-slr-x86_64",
        new: &["runners", "download", "proton-cachyos"],
        says: "runners download",
    },
    Kind {
        id: "dxvk",
        repo: Repo::Dxvk,
        tag: "v2.7.1",
        asset: "dxvk-2.7.1.tar.gz",
        folder: "dxvk/dxvk-2.7.1",
        new: &["dxvk", "download"],
        says: "dxvk download",
    },
    Kind {
        id: "vkd3d",
        repo: Repo::Vkd3d,
        tag: "v2.14",
        asset: "vkd3d-proton-2.14.tar.gz",
        folder: "vkd3d/vkd3d-proton-2.14",
        new: &["vkd3d", "download"],
        says: "vkd3d download",
    },
];

impl Kind {
    fn archive(&self) -> Vec<u8> {
        self.archive_of(self.asset)
    }

    fn archive_of(&self, asset: &str) -> Vec<u8> {
        let root = asset.trim_end_matches(".tar.gz");
        match self.id {
            "dxvk" => component_archive(root, true),
            "vkd3d" => component_archive(root, false),
            _ => proton_archive(root),
        }
    }

    /// Lists the release as the newest and serves it by tag too.
    fn serve(&self, api: &ReleasesApi) {
        api.publish(self.repo, self.tag, self.asset, self.archive());
        api.publish_tag(self.repo, self.tag, self.asset, self.archive());
    }

    /// The `n`th release of a listing, as a tag and its asset.
    fn release(&self, n: u32) -> (String, String) {
        match self.id {
            "ge-proton" => (
                format!("GE-Proton10-{n}"),
                format!("GE-Proton10-{n}.tar.gz"),
            ),
            "proton-cachyos" => (
                format!("cachyos-10.0-202611{n:02}-slr"),
                format!("proton-cachyos-10.0-202611{n:02}-slr-x86_64.tar.gz"),
            ),
            "dxvk" => (format!("v2.{n}"), format!("dxvk-2.{n}.tar.gz")),
            _ => (format!("v2.{n}"), format!("vkd3d-proton-2.{n}.tar.gz")),
        }
    }

    fn installed(&self, f: &Fixture) -> bool {
        f.root.path().join("data/wineo").join(self.folder).is_dir()
    }
}

fn deprecated(says: &str) -> String {
    format!("warning: download is deprecated; use {says}\n")
}

fn fetch(f: &Fixture, api: &ReleasesApi, args: &[&str]) -> std::process::Output {
    f.releases_command(api)
        .args(args)
        .output()
        .expect("wineo should start")
}

#[test]
fn old_download_installs_each_provider_and_warns_naming_the_new_command() {
    for kind in &KINDS {
        let (old_f, new_f) = (Fixture::new(), Fixture::new());
        let api = ReleasesApi::start();
        kind.serve(&api);

        let old = fetch(&old_f, &api, &["download", kind.id, "--version", kind.tag]);
        assert_success(&old);
        let mut args = kind.new.to_vec();
        args.extend(["--version", kind.tag]);
        let new = fetch(&new_f, &api, &args);
        assert_success(&new);

        assert!(kind.installed(&old_f), "{}: {}", kind.id, stderr(&old));
        assert!(kind.installed(&new_f), "{}: {}", kind.id, stderr(&new));
        assert_eq!(stdout(&old), stdout(&new), "{}", kind.id);
        // Same text apart from the one warning in front.
        assert_eq!(
            stderr(&old),
            format!("{}{}", deprecated(kind.says), stderr(&new)),
            "{}",
            kind.id
        );
        assert_eq!(stderr(&old).matches("deprecated").count(), 1);
        assert!(!stderr(&new).contains("deprecated"), "{}", stderr(&new));
    }
}

#[test]
fn the_new_download_commands_without_a_version_fetch_the_newest() {
    for kind in &KINDS {
        let f = Fixture::new();
        let api = ReleasesApi::start();
        kind.serve(&api);
        let out = fetch(&f, &api, kind.new);
        assert_success(&out);
        assert!(kind.installed(&f), "{}: {}", kind.id, stderr(&out));
        assert!(!stderr(&out).contains("deprecated"), "{}", stderr(&out));
    }
}

#[test]
fn old_download_without_a_version_fetches_the_newest() {
    for kind in &KINDS {
        let f = Fixture::new();
        let api = ReleasesApi::start();
        kind.serve(&api);
        let out = fetch(&f, &api, &["download", kind.id]);
        assert_success(&out);
        assert!(kind.installed(&f), "{}: {}", kind.id, stderr(&out));
        assert!(
            stderr(&out).starts_with(&deprecated(kind.says)),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn old_and_new_listings_match_and_point_to_the_new_spelling() {
    for kind in &KINDS {
        let (old_f, new_f) = (Fixture::new(), Fixture::new());
        let api = ReleasesApi::start();
        // A full page, so the listing says there is another.
        let releases: Vec<_> = (1..=30).rev().map(|n| kind.release(n)).collect();
        api.announce_many(kind.repo, &releases, 2048);

        let old = fetch(&old_f, &api, &["download", kind.id, "--list"]);
        assert_success(&old);
        let mut args = kind.new.to_vec();
        args.push("--list");
        let new = fetch(&new_f, &api, &args);
        assert_success(&new);

        assert_eq!(stdout(&old), stdout(&new), "{}", kind.id);
        assert_eq!(stdout(&new).lines().count(), 30);
        assert_eq!(
            stderr(&old),
            format!("{}{}", deprecated(kind.says), stderr(&new)),
            "{}",
            kind.id
        );
        // The hint to the next page names the new command, even from the old one.
        let next = format!(
            "More releases: wineo {} --list --page 2\n",
            kind.new.join(" ")
        );
        assert!(stderr(&new).ends_with(&next), "{}", stderr(&new));
        assert!(stderr(&old).ends_with(&next), "{}", stderr(&old));

        // A later page keeps working in both, and the hint moves on.
        let old = fetch(
            &old_f,
            &api,
            &["download", kind.id, "--list", "--page", "2"],
        );
        let mut args = kind.new.to_vec();
        args.extend(["--list", "--page", "2"]);
        let new = fetch(&new_f, &api, &args);
        assert_success(&old);
        assert_success(&new);
        assert_eq!(stdout(&old), stdout(&new), "{}", kind.id);
        let page3 = "--list --page 3\n";
        assert!(stderr(&new).ends_with(page3), "{}", stderr(&new));
        assert!(stderr(&old).ends_with(page3), "{}", stderr(&old));
    }
}

#[test]
fn verbose_reaches_the_new_listings_and_adds_the_folder_name() {
    for kind in &KINDS {
        let f = Fixture::new();
        let api = ReleasesApi::start();
        kind.serve(&api);
        let folder = kind.folder.rsplit('/').next().unwrap();

        let mut quiet = kind.new.to_vec();
        quiet.push("--list");
        let plain = fetch(&f, &api, &quiet);
        assert_success(&plain);

        let mut verbose = quiet.clone();
        verbose.push("-v");
        let long = fetch(&f, &api, &verbose);
        assert_success(&long);
        // The folder name follows the tag and size, though for GE-Proton it
        // is the tag itself.
        assert_eq!(
            stdout(&long),
            format!("{}  {folder}\n", stdout(&plain).trim_end()),
            "{}",
            kind.id
        );
        // Ahead of the subcommand, where the flag is global, it counts too.
        let mut before = vec!["-v"];
        before.extend(&quiet);
        assert_eq!(stdout(&fetch(&f, &api, &before)), stdout(&long));

        // The old command always took the flag.
        let old = fetch(&f, &api, &["download", kind.id, "--list", "-v"]);
        assert_eq!(stdout(&old), stdout(&long), "{}", kind.id);
    }
}

#[test]
fn runners_download_refuses_the_graphics_components_and_names_their_commands() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    for (id, message) in [
        ("dxvk", "error: DXVK downloads with wineo dxvk download.\n"),
        (
            "vkd3d",
            "error: vkd3d-proton downloads with wineo vkd3d download.\n",
        ),
    ] {
        let out = fetch(&f, &api, &["runners", "download", id]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert_eq!(stderr(&out), message);
        assert_eq!(stdout(&out), "");
        // With a list or version too, and before anything is asked of GitHub.
        for flags in [&["--list"][..], &["--version", "v1"]] {
            let out = fetch(
                &f,
                &api,
                &[&["runners", "download", id][..], flags].concat(),
            );
            assert_eq!(stderr(&out), message, "{id} {flags:?}");
        }
    }
    assert!(api.requests().is_empty(), "{:?}", api.requests());

    // They aren't offered as values, and anything else is a usage error.
    let help = stdout(&f.run(&["runners", "download", "--help"]));
    assert!(
        help.contains("[possible values: ge-proton, proton-cachyos]"),
        "{help}"
    );
    let out = fetch(&f, &api, &["runners", "download", "wine"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn the_guided_download_needs_a_terminal_and_each_spelling_says_what_to_type() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    let new = fetch(&f, &api, &["runners", "download"]);
    assert_eq!(new.status.code(), Some(1), "{}", stderr(&new));
    assert_eq!(
        stderr(&new),
        "error: guided download needs a terminal; use \
         `wineo runners download <provider> [--version <tag>]`\n"
    );

    // The old command names all three, in the warning and in the error.
    let old = fetch(&f, &api, &["download"]);
    assert_eq!(old.status.code(), Some(1), "{}", stderr(&old));
    assert_eq!(
        stderr(&old),
        "warning: download is deprecated; use runners download, dxvk download, or vkd3d download\n\
         error: guided download needs a terminal; use `wineo runners download <provider>`, \
         `wineo dxvk download`, or `wineo vkd3d download`\n"
    );
    assert!(api.requests().is_empty(), "{:?}", api.requests());
}

/// Opens the guided picker and leaves it with Ctrl+C. What it showed.
fn picker(f: &Fixture, api: &ReleasesApi, args: &[&str]) -> (Option<i32>, String) {
    let mut command = f.releases_command(api);
    command.args(args);
    let mut pty = Pty::spawn(command);
    pty.expect("What would you like to download?");
    pty.expect("Tool");
    pty.ctrl_c();
    let (status, seen) = pty.finish();
    (status.code(), seen.raw().replace("\n\n", "\n"))
}

#[test]
fn the_guided_picker_offers_two_runners_and_the_old_command_still_offers_all_four() {
    let f = Fixture::new();
    let api = ReleasesApi::start();
    let (code, seen) = picker(&f, &api, &["runners", "download"]);
    assert_eq!(code, Some(130), "{seen}");
    assert_eq!(
        seen,
        "What would you like to download?\n  1) ge-proton\n  2) proton-cachyos\n\
         Esc to go back, Ctrl+C to cancel.\nTool [1]: \nDownload canceled\n"
    );

    let (code, seen) = picker(&f, &api, &["download"]);
    assert_eq!(code, Some(130), "{seen}");
    assert_eq!(
        seen,
        "warning: download is deprecated; use runners download, dxvk download, or vkd3d download\n\
         What would you like to download?\n  1) ge-proton\n  2) proton-cachyos\n  3) dxvk\n  4) vkd3d\n\
         Esc to go back, Ctrl+C to cancel.\nTool [1]: \nDownload canceled\n"
    );
    assert!(api.requests().is_empty(), "{:?}", api.requests());
}

#[test]
fn the_guided_download_installs_from_either_picker() {
    // The new runners picker's first choice, and the old picker's third.
    for (args, choice, kind) in [
        (&["runners", "download"][..], "1", &KINDS[0]),
        (&["download"][..], "3", &KINDS[2]),
    ] {
        let f = Fixture::new();
        let api = ReleasesApi::start();
        kind.serve(&api);
        let mut command = f.releases_command(&api);
        command.args(args);
        let mut pty = Pty::spawn(command);
        pty.expect("Tool [1]: ");
        pty.send(choice);
        pty.expect("Release [1]: ");
        pty.send("");
        pty.expect("Download this release?");
        pty.send("y");
        let (status, seen) = pty.finish();
        assert!(status.success(), "{seen}");
        assert!(seen.contains("Installed in "), "{seen}");
        assert!(kind.installed(&f), "{}", kind.id);
        assert_eq!(
            seen.raw().matches("deprecated").count(),
            usize::from(args[0] == "download"),
            "{seen}"
        );
    }
}

fn lists(help: &str, command: &str) -> bool {
    help.lines()
        .any(|line| line.split_whitespace().next() == Some(command))
}

#[test]
fn the_old_download_is_hidden_from_help_and_man_pages_but_the_new_commands_show() {
    let f = Fixture::new();
    let top = stdout(&f.run(&["--help"]));
    assert!(!lists(&top, "download"), "{top}");
    assert!(lists(&top, "runners"), "{top}");
    let help = stdout(&f.run(&["help"]));
    assert!(!lists(&help, "download"), "{help}");

    for parent in ["runners", "dxvk", "vkd3d"] {
        let help = stdout(&f.run(&[parent, "--help"]));
        assert!(lists(&help, "download"), "{parent}: {help}");
        assert!(lists(&help, "update"), "{parent}: {help}");
    }

    // Hidden is not gone: it still parses and says where to go.
    let old = stdout(&f.run(&["download", "--help"]));
    assert!(
        old.contains("Deprecated: use runners download, dxvk download, or vkd3d download"),
        "{old}"
    );

    let dir = f.root.path().join("man");
    assert_success(&f.run(&["manpages", dir.to_str().unwrap()]));
    for page in [
        "wineo-runners-download.1",
        "wineo-dxvk-download.1",
        "wineo-vkd3d-download.1",
        "wineo-dxvk-update.1",
        "wineo-vkd3d-update.1",
    ] {
        assert!(dir.join(page).is_file(), "missing {page}");
    }
    assert!(!dir.join("wineo-download.1").exists());
    let page = fs::read_to_string(dir.join("wineo.1")).unwrap();
    assert!(!page.contains(r"wineo\-download"), "{page}");
}

#[test]
fn every_spelling_installs_the_release_a_version_names_and_not_the_newest() {
    for kind in &KINDS {
        let (tag, asset) = kind.release(2);
        let folder = format!(
            "data/wineo/{}/{}",
            kind.folder.split('/').next().unwrap(),
            asset.trim_end_matches(".tar.gz")
        );
        let mut new = kind.new.to_vec();
        new.extend(["--version", &tag]);
        for args in [new, vec!["download", kind.id, "--version", &tag]] {
            let f = Fixture::new();
            let api = ReleasesApi::start();
            // The newest is listed, and the older one only by its tag.
            kind.serve(&api);
            api.publish_tag(kind.repo, &tag, &asset, kind.archive_of(&asset));
            let out = fetch(&f, &api, &args);
            assert_success(&out);
            assert!(f.root.path().join(&folder).is_dir(), "{args:?}");
            assert!(!kind.installed(&f), "{args:?} fetched the newest");
        }
    }
}
