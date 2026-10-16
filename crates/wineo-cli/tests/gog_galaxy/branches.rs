//! Branches, pinned builds, rollback, and the builds listing, through the
//! CLI against the fake GOG's content system.

use super::*;
use wineo_core::gog::update::galaxy::record::Record;

/// A build of the game on `branch` (None for the default one), whose info
/// file names it.
pub(super) fn on(build: &str, version: &str, branch: Option<&str>) -> FakeBuild {
    let mut fake = FakeBuild::new(GAME, build)
        .depot(
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
                    info(build).as_bytes(),
                )),
        )
        .dependency("MSVC2017");
    fake.version_name = version.to_owned();
    fake.branch = branch.map(str::to_owned);
    fake
}

/// Lists a build on the beta branch.
pub(super) fn beta(api: &GogApi, build: &str, version: &str) {
    api.content.add_build(on(build, version, Some("beta")));
}

pub(super) fn record(f: &Fixture) -> Record {
    let path =
        wineo_core::gog::folder::sidecar_file(&f.paths(), &fs::canonicalize(folder(f)).unwrap());
    Record::parse(&fs::read(path).unwrap()).unwrap()
}

pub(super) fn installed_build(f: &Fixture) -> String {
    state(f).committed.unwrap().products[0]
        .build
        .clone()
        .unwrap()
}

/// What `--builds` printed, row by row, split on whitespace.
pub(super) fn rows(out: &Output) -> Vec<Vec<String>> {
    stdout(out)
        .lines()
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
        .collect()
}

pub(super) fn words(row: &[&str]) -> Vec<String> {
    row.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn branch_flag_conflicts_are_usage_errors() {
    let (f, api) = installed();
    for args in [
        &["update", NAME, "--branch", "beta", "--latest"][..],
        &["update", NAME, "--branch", "beta", "--build", "100"],
        &["update", NAME, "--branch", "beta", "--default-branch"],
        &["update", NAME, "--default-branch", "--latest"],
        &["update", NAME, "--default-branch", "--build", "100"],
        &["update", NAME, "--build", "100", "--latest"],
        &["update", NAME, "--repair", "--branch", "beta"],
        &["update", NAME, "--repair", "--default-branch"],
        &["update", NAME, "--repair", "--build", "100"],
        &["update", NAME, "--repair", "--latest"],
        &["update", "--all", "--branch", "beta"],
        &["update", "--all", "--default-branch"],
        &["update", "--all", "--build", "100"],
        &["update", "--all", "--latest"],
        &["update", "--all", "--repair"],
        &["update", "--all", "--track", "galaxy"],
        &["update", "--all", "--builds"],
        &["update", "--check", "--branch", "beta"],
        &["update", "--check", "--default-branch"],
        &["update", "--check", "--build", "100"],
        &["update", "--check", "--latest"],
        &["update", NAME, "--check", "--builds"],
        &["update", NAME, "--builds", "--yes"],
        &["update", NAME, "--builds", "--dry-run"],
        &["update", NAME, "--builds", "--reinstall"],
        &["update", NAME, "--builds", "--repair"],
        &["update", NAME, "--builds", "--branch", "beta"],
        &["update", NAME, "--builds", "--latest"],
        &["update", "--builds"],
        &["update", NAME, "--json"],
        &[
            "update",
            NAME,
            "--reinstall",
            "--track",
            "installer",
            "--branch",
            "beta",
        ],
        &[
            "update",
            NAME,
            "--reinstall",
            "--track",
            "installer",
            "--latest",
        ],
        &["import", "gog", "--branch", "beta"],
        &["import", "gog", "--build", "100"],
        &[
            "import", "gog", "--game", "1", "--branch", "beta", "--build", "1",
        ],
        &[
            "import",
            "gog",
            "--game",
            "1",
            "--track",
            "installer",
            "--branch",
            "beta",
        ],
        &[
            "import",
            "gog",
            "--game",
            "1",
            "--track",
            "installer",
            "--build",
            "1",
        ],
    ] {
        let out = gog(&f, &api, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    // A move onto a branch is allowed.
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(installed_build(&f), "100");
}

#[test]
fn builds_lists_what_gog_offers_with_marks() {
    let (f, api) = installed();
    newer(&api);
    // The newest build of a branch is listed first.
    beta(&api, "400", "1.3.0-beta");
    beta(&api, "500", "1.3.1-beta");
    let out = update(&f, &api, &["--builds"]);
    assert_success(&out);
    assert_eq!(
        rows(&out),
        [
            words(&["BRANCH", "BUILD", "VERSION", "PUBLISHED"]),
            words(&["default", "200", "1.4.11", "2026-01-02", "latest"]),
            words(&["default", "100", "1.2.76", "2026-01-02", "installed"]),
            words(&["beta", "500", "1.3.1-beta", "2026-01-02", "latest"]),
            words(&["beta", "400", "1.3.0-beta", "2026-01-02"]),
        ],
        "{}",
        stdout(&out)
    );
    assert!(
        !stdout(&out).contains("private branches"),
        "{}",
        stdout(&out)
    );
    // A listing changes nothing.
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert!(state(&f).journal.is_none());

    let out = update(&f, &api, &["--builds", "--json"]);
    assert_success(&out);
    assert_eq!(
        json_of(&out),
        json!({
            "environment": NAME, "id": GAME.to_string(), "track": "galaxy",
            "branch": null, "pinned": null, "installed": "100",
            "private_branches": false,
            "builds": [
                {"branch": null, "build": "200", "version": "1.4.11",
                 "published": "2026-01-02T03:04:05+0000", "latest": true,
                 "private": false},
                {"branch": null, "build": "100", "version": "1.2.76",
                 "published": "2026-01-02T03:04:05+0000", "latest": false,
                 "private": false},
                {"branch": "beta", "build": "500", "version": "1.3.1-beta",
                 "published": "2026-01-02T03:04:05+0000", "latest": true,
                 "private": false},
                {"branch": "beta", "build": "400", "version": "1.3.0-beta",
                 "published": "2026-01-02T03:04:05+0000", "latest": false,
                 "private": false},
            ],
        })
    );
}

#[test]
fn builds_marks_the_pin_and_the_branch_followed_and_lists_only_generation_2() {
    let (f, api) = installed();
    newer(&api);
    beta(&api, "400", "1.3.0-beta");
    let mut ancient = on("50", "0.9", None);
    ancient.generation = 1;
    api.content.add_build(ancient);
    assert_success(&update(&f, &api, &["--branch", "beta", "--yes"]));
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    let out = update(&f, &api, &["--builds"]);
    assert_success(&out);
    // Pinned to 100, which is on the default branch, so the game follows it.
    assert_eq!(
        rows(&out),
        [
            words(&["BRANCH", "BUILD", "VERSION", "PUBLISHED"]),
            words(&["default", "200", "1.4.11", "2026-01-02", "latest"]),
            words(&[
                "default",
                "100",
                "1.2.76",
                "2026-01-02",
                "installed,",
                "pinned"
            ]),
            words(&["beta", "400", "1.3.0-beta", "2026-01-02", "latest"]),
        ],
        "{}",
        stdout(&out)
    );
    assert_success(&update(&f, &api, &["--branch", "beta", "--yes"]));
    let out = update(&f, &api, &["--builds"]);
    assert_eq!(
        rows(&out)[3],
        words(&[
            "beta",
            "400",
            "1.3.0-beta",
            "2026-01-02",
            "installed,",
            "latest,",
            "followed"
        ]),
        "{}",
        stdout(&out)
    );
}

#[test]
fn builds_says_when_gog_has_private_branches() {
    let (f, api) = installed();
    api.content.set_private_branches(true);
    let out = update(&f, &api, &["--builds"]);
    assert_success(&out);
    assert!(
        stdout(&out).ends_with("GOG also has private branches. Name one with --branch NAME.\n"),
        "{}",
        stdout(&out)
    );
    let out = update(&f, &api, &["--builds", "--json"]);
    assert_eq!(json_of(&out)["private_branches"], true);
}

#[test]
fn an_installer_game_has_no_branches() {
    let (f, api) = world();
    f.inno_runner();
    installer_package(&api, "1.5", "installer");
    assert_success(&install(&f, &api, &["--track", "installer"]));
    for flag in [
        &["--builds"][..],
        &["--branch", "beta", "--yes"],
        &["--default-branch", "--yes"],
        &["--build", "100", "--yes"],
        &["--latest", "--yes"],
    ] {
        let out = update(&f, &api, flag);
        assert_eq!(out.status.code(), Some(1), "{flag:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(
                "error: Only Galaxy games have branches. Move it with --reinstall --track galaxy."
            ),
            "{flag:?}: {}",
            stderr(&out)
        );
    }
}

/// Whether a review or table shows `label` with `value`, whatever the
/// column widths are.
fn has_row(text: &str, label: &str, value: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start()
            .strip_prefix(label)
            .is_some_and(|rest| rest.starts_with("  ") && rest.trim() == value)
    })
}

fn has_no_row(text: &str, label: &str) -> bool {
    !text.lines().any(|line| {
        line.trim_start()
            .strip_prefix(label)
            .is_some_and(|rest| rest.starts_with("  "))
    })
}

fn pinned_message(build: &str, version: &str) -> String {
    format!(
        "{NAME} is pinned to build {build} ({version}). Run wineo update {NAME} --latest to follow its branch again."
    )
}

fn pin_note(build: &str, version: &str) -> String {
    format!(
        "Pinned to build {build} ({version}). Run wineo update {NAME} --latest to follow its branch again."
    )
}

#[test]
fn a_game_installs_from_a_branch_and_follows_it() {
    let (f, api) = world();
    beta(&api, "400", "1.3.0-beta");
    let out = install(&f, &api, &["--branch", "beta"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("  track        Galaxy\n"), "{text}");
    assert!(text.contains("  branch       beta\n"), "{text}");
    assert!(text.contains("  version      1.3.0-beta\n"), "{text}");
    assert!(!text.contains("  pin "), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(installed_build(&f), "400");
    let follow = record(&f).follow;
    assert_eq!(follow.branch.as_deref(), Some("beta"));
    assert_eq!(follow.pinned, None);

    let out = gog(&f, &api, &["update", "--check", "--json"]);
    let env = &json_of(&out)["environments"][0];
    assert_eq!(env["branch"], "beta");
    assert_eq!(env["pinned"], Value::Null);
    assert_eq!(env["update"], "none");
    let out = gog(&f, &api, &["import", "gog", "--list", "--json"]);
    let games = json_of(&out);
    let game = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["environment"] == NAME)
        .unwrap();
    assert_eq!(game["branch"], "beta");
    assert_eq!(game["pinned"], Value::Null);
}

#[test]
fn a_game_installs_a_listed_build_and_stays_on_it() {
    let (f, api) = world();
    newer(&api);
    // By its version name, which only one build has.
    let out = install(&f, &api, &["--build", "1.2.76"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("  version      1.2.76\n"), "{text}");
    assert!(
        text.contains("  pin          build 100, until --latest\n"),
        "{text}"
    );
    assert!(!text.contains("  branch "), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    let follow = record(&f).follow;
    assert_eq!(follow.branch, None);
    assert_eq!(follow.pinned.as_deref(), Some("100"));

    let out = gog(&f, &api, &["update", "--check", "--json"]);
    let env = &json_of(&out)["environments"][0];
    assert_eq!(env["pinned"], "100");
    assert_eq!(env["reason"], "pinned");
}

#[test]
fn an_install_names_no_command_for_a_branch_or_build_gog_does_not_list() {
    let (f, api) = world();
    for (flag, value, message) in [
        (
            "--branch",
            "nope",
            "GOG lists no branch nope for Papers, Please.",
        ),
        (
            "--build",
            "999",
            "GOG doesn't list build 999 of Papers, Please.",
        ),
        (
            "--build",
            "100x",
            "GOG doesn't list build 100x of Papers, Please.",
        ),
    ] {
        let out = install(&f, &api, &[flag, value]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert!(
            stderr(&out).contains(&format!("error: {message}\n")),
            "{}",
            stderr(&out)
        );
        assert!(!folder(&f).exists());
    }
    // GOG's Linux-less content system has nothing for a native platform.
    let out = install(&f, &api, &["--platform", "linux", "--branch", "beta"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Galaxy builds are only for Windows games."),
        "{}",
        stderr(&out)
    );
}

#[test]
fn rolling_back_pins_the_game_until_latest() {
    let (f, api) = installed();
    newer(&api);
    assert_success(&update(&f, &api, &["--yes"]));
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");

    let out = update(&f, &api, &["--build", "100", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "pin", "build 100, until --latest"), "{text}");
    assert!(
        has_row(&text, "Papers, Please", "1.4.11 -> 1.2.76"),
        "{text}"
    );
    assert!(has_no_row(&text, "branch"), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));

    // A plain update changes nothing and says how to unpin.
    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert_eq!(
        stdout(&out),
        format!("{}\n", pinned_message("100", "1.2.76"))
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert!(state(&f).journal.is_none());

    // `--all` leaves it alone and says why.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_success(&out);
    assert_eq!(
        stdout(&out),
        format!(
            "Skipped {NAME}: {}\nNothing to update.\n",
            pin_note("100", "1.2.76")
        )
    );

    // A check shows the newer build, held back.
    let out = gog(&f, &api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    let mut lines = text.lines();
    lines.next();
    assert_eq!(
        lines.next().unwrap().split_whitespace().collect::<Vec<_>>(),
        [NAME, "1.2.76", "1.4.11", "pinned"],
        "{text}"
    );
    assert_eq!(
        lines.next(),
        Some(format!("  {}", pin_note("100", "1.2.76")).as_str())
    );
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    assert_eq!(
        json_of(&out)["environments"],
        json!([{
            "name": NAME, "id": GAME.to_string(), "track": "galaxy",
            "installed": "100", "available": "200", "branch": null, "pinned": "100",
            "update": "available", "applicable": false, "reason": "pinned",
            "unfinished": false, "dlc": [],
        }])
    );
    let out = gog(&f, &api, &["import", "gog", "--list", "--json"]);
    let games = json_of(&out);
    let game = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["environment"] == NAME)
        .unwrap();
    assert_eq!(game["pinned"], "100");
    assert_eq!(game["reason"], "pinned");
    assert_eq!(game["applicable"], false);
    let out = gog(&f, &api, &["import", "gog", "--list"]);
    let row = stdout(&out)
        .lines()
        .find(|line| line.contains(NAME))
        .unwrap()
        .to_owned();
    assert!(row.ends_with("pinned"), "{row}");

    // `--latest` follows the branch again.
    let out = update(&f, &api, &["--latest", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "pin", "removed"), "{text}");
    assert!(
        has_row(&text, "Papers, Please", "1.2.76 -> 1.4.11"),
        "{text}"
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    assert_eq!(record(&f).follow.pinned, None);
    let out = gog(&f, &api, &["update", "--check"]);
    assert!(stdout(&out).contains("up to date"), "{}", stdout(&out));
}

#[test]
fn a_build_can_be_named_by_a_version_that_one_build_has() {
    let (f, api) = installed();
    api.content.add_build(on("301", "same", None));
    api.content.add_build(on("302", "same", None));
    let out = update(&f, &api, &["--build", "same", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("error: same matches builds 301 and 302. Pick one with --build ID.\n"),
        "{}",
        stderr(&out)
    );
    let out = update(&f, &api, &["--build", "999", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(&format!(
            "error: GOG doesn't list build 999 of Papers, Please. See: wineo update {NAME} --builds\n"
        )),
        "{}",
        stderr(&out)
    );
    // An id wins, and a name that only one build has is enough.
    assert_success(&update(&f, &api, &["--build", "302", "--yes"]));
    assert_eq!(installed_build(&f), "302");
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("302"));
    assert_success(&update(&f, &api, &["--build", "1.2.76", "--yes"]));
    assert_eq!(installed_build(&f), "100");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));
}

#[test]
fn an_update_names_the_way_to_the_listing_for_a_branch_gog_does_not_list() {
    let (f, api) = installed();
    for branch in ["nope", "private-one"] {
        let out = update(&f, &api, &["--branch", branch, "--yes"]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert!(
            stderr(&out).contains(&format!(
                "error: GOG lists no branch {branch} for Papers, Please. See: wineo update {NAME} --builds\n"
            )),
            "{}",
            stderr(&out)
        );
    }
    assert_eq!(record(&f).follow, Default::default());
}

#[test]
fn switching_branch_reviews_the_change_and_going_back_returns_to_the_default() {
    let (f, api) = installed();
    newer(&api);
    beta(&api, "400", "1.3.0-beta");

    let out = update(&f, &api, &["--branch", "beta", "--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "branch", "default -> beta"), "{text}");
    assert!(text.ends_with("Dry run, nothing was updated.\n"), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(record(&f).follow, Default::default());

    let out = update(&f, &api, &["--branch", "beta", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "branch", "default -> beta"), "{text}");
    assert!(
        has_row(&text, "Papers, Please", "1.2.76 -> 1.3.0-beta"),
        "{text}"
    );
    assert!(has_no_row(&text, "pin"), "{text}");
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(installed_build(&f), "400");
    assert_eq!(record(&f).follow.branch.as_deref(), Some("beta"));

    // The branch is followed from now on: nothing is newer.
    let out = update(&f, &api, &["--yes"]);
    assert_eq!(stdout(&out), "Up to date.\n");
    let out = gog(&f, &api, &["update", "--check"]);
    assert!(stdout(&out).contains("up to date"), "{}", stdout(&out));

    // A newer build of the branch is what an update goes to.
    beta(&api, "500", "1.3.1-beta");
    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "branch", "beta"), "{text}");
    assert!(
        has_row(&text, "Papers, Please", "1.3.0-beta -> 1.3.1-beta"),
        "{text}"
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.1-beta");

    let out = update(&f, &api, &["--default-branch", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "branch", "beta -> default"), "{text}");
    assert!(
        has_row(&text, "Papers, Please", "1.3.1-beta -> 1.4.11"),
        "{text}"
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    assert_eq!(record(&f).follow, Default::default());
}

#[test]
fn a_change_that_touches_no_file_only_changes_what_the_game_follows() {
    let (f, api) = installed();
    // The same build is also listed on the beta branch.
    beta(&api, "100", "1.2.76");
    let before = tree(&folder(&f), &[]);

    let out = update(&f, &api, &["--build", "100", "--dry-run"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with(&format!("Pin {NAME}\n")), "{text}");
    assert!(has_row(&text, "pin", "build 100, until --latest"), "{text}");
    assert!(has_no_row(&text, "download"), "{text}");
    assert!(text.ends_with("Dry run, nothing was updated.\n"), "{text}");
    assert_eq!(record(&f).follow, Default::default());

    let out = update(&f, &api, &["--build", "100", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.ends_with(&format!("Pinned {NAME} to build 100.\n")),
        "{text}"
    );
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));
    assert!(state(&f).journal.is_none());

    // The pin to what is installed, again, is nothing.
    let out = update(&f, &api, &["--build", "100", "--yes"]);
    assert_eq!(stdout(&out), "Up to date.\n");

    let out = update(&f, &api, &["--latest", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with(&format!("Unpin {NAME}\n")), "{text}");
    assert!(has_row(&text, "pin", "removed"), "{text}");
    assert!(
        text.ends_with(&format!("{NAME} follows its branch again.\n")),
        "{text}"
    );
    assert_eq!(record(&f).follow, Default::default());
    let out = update(&f, &api, &["--latest", "--yes"]);
    assert_eq!(stdout(&out), "Up to date.\n");
    let out = update(&f, &api, &["--default-branch", "--yes"]);
    assert_eq!(stdout(&out), "Up to date.\n");

    let out = update(&f, &api, &["--branch", "beta", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with(&format!("Switch the branch of {NAME}\n")),
        "{text}"
    );
    assert!(has_row(&text, "branch", "default -> beta"), "{text}");
    assert!(
        text.ends_with(&format!("{NAME} now follows branch beta.\n")),
        "{text}"
    );
    assert_eq!(record(&f).follow.branch.as_deref(), Some("beta"));
    let out = update(&f, &api, &["--default-branch", "--yes"]);
    assert!(
        stdout(&out).ends_with(&format!("{NAME} now follows the default branch.\n")),
        "{}",
        stdout(&out)
    );

    // No file was touched, and nothing was left to finish.
    assert_eq!(tree(&folder(&f), &[]), before);
    assert!(state(&f).journal.is_none());
}

#[test]
fn a_branch_change_that_stopped_is_finished_with_its_branch() {
    let (f, api) = installed();
    newer(&api);
    beta(&api, "400", "1.3.0-beta");
    let mut pty = paused(&f, &api, &["update", NAME, "--branch", "beta", "--yes"]);
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(state(&f).journal.is_some());
    // The branch is only committed with the files.
    let waiting = record(&f);
    assert_eq!(waiting.follow, Default::default());
    assert_eq!(
        waiting
            .pending
            .as_ref()
            .and_then(|pending| pending.follow.as_ref())
            .and_then(|follow| follow.branch.as_deref()),
        Some("beta")
    );
    let check = gog(&f, &api, &["update", "--check", "--json"]);
    assert_eq!(json_of(&check)["environments"][0]["branch"], Value::Null);

    // Another branch can't be picked until it is finished.
    let out = update(&f, &api, &["--latest", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "{NAME} has an unfinished GOG operation, so it can't change its branch or build yet. Finish it with: wineo update {NAME}"
        )),
        "{}",
        stderr(&out)
    );

    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains(&format!("Finished the interrupted update of {NAME}.\n")),
        "{}",
        stdout(&out)
    );
    assert!(
        has_row(&stdout(&out), "branch", "default -> beta"),
        "{}",
        stdout(&out)
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(installed_build(&f), "400");
    let done = record(&f);
    assert_eq!(done.follow.branch.as_deref(), Some("beta"));
    assert_eq!(done.pending, None);
}

#[test]
fn a_pin_to_a_build_gog_stopped_listing_is_dropped_when_the_operation_retargets() {
    let (f, api) = installed();
    newer(&api);
    assert_success(&update(&f, &api, &["--yes"]));
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));

    // A reinstall rebuilds the pin; stop it, and lose what it saved of its
    // target while GOG stops listing the build.
    fs::write(folder(&f).join("Papers.exe"), "damaged").unwrap();
    let mut pty = paused(&f, &api, &["update", NAME, "--reinstall", "--yes"]);
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert_eq!(state(&f).journal.unwrap().kind, JournalKind::Reinstall);
    let path =
        wineo_core::gog::folder::sidecar_file(&f.paths(), &fs::canonicalize(folder(&f)).unwrap());
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(saved.as_object_mut().unwrap().remove("pending").is_some());
    fs::write(&path, saved.to_string()).unwrap();
    api.hidden_builds.lock().unwrap().insert("100".to_owned());

    let out = update(&f, &api, &["--yes"]);
    assert_success(&out);
    assert!(
        stderr(&out).contains(
            "GOG no longer lists build 100 of Papers, Please, so the update finishes with build 200 instead. The pin to build 100 is removed."
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.4.11");
    assert_eq!(installed_build(&f), "200");
    assert_eq!(record(&f).follow, Default::default());
}

#[test]
fn a_reinstall_of_a_pinned_game_whose_build_is_gone_says_how_to_unpin() {
    let (f, api) = installed();
    newer(&api);
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    api.hidden_builds.lock().unwrap().insert("100".to_owned());
    let out = update(&f, &api, &["--reinstall", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "error: GOG no longer lists build 100 of Papers, Please, which {NAME} is pinned to. Run wineo update {NAME} --latest to follow its branch again.\n"
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));
}

#[test]
fn a_reinstall_of_a_pinned_game_rebuilds_the_pinned_build() {
    let (f, api) = installed();
    newer(&api);
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    fs::write(folder(&f).join("Papers.exe"), "damaged").unwrap();
    let out = update(&f, &api, &["--reinstall", "--yes"]);
    assert_success(&out);
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(installed_build(&f), "100");
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));
    assert!(has_no_row(&stdout(&out), "pin"), "{}", stdout(&out));
}

#[test]
fn a_pinned_build_picks_the_branch_it_is_listed_on() {
    let (f, api) = installed();
    beta(&api, "400", "1.3.0-beta");
    // Only on beta: the game follows the branch it is listed on.
    assert_success(&update(&f, &api, &["--build", "400", "--yes"]));
    let follow = record(&f).follow;
    assert_eq!(follow.branch.as_deref(), Some("beta"));
    assert_eq!(follow.pinned.as_deref(), Some("400"));

    // Also on the branch followed: it stays there.
    beta(&api, "100", "1.2.76");
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    let follow = record(&f).follow;
    assert_eq!(follow.branch.as_deref(), Some("beta"));
    assert_eq!(follow.pinned.as_deref(), Some("100"));
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");

    // Only on the default branch: back it goes.
    api.content.add_build(on("150", "1.3.0", None));
    assert_success(&update(&f, &api, &["--build", "150", "--yes"]));
    let follow = record(&f).follow;
    assert_eq!(follow.branch, None);
    assert_eq!(follow.pinned.as_deref(), Some("150"));
}

#[test]
fn a_pinned_game_with_nothing_newer_still_shows_its_pin() {
    let (f, api) = installed();
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    let out = gog(&f, &api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    let mut lines = text.lines();
    lines.next();
    assert_eq!(
        lines.next().unwrap().split_whitespace().collect::<Vec<_>>(),
        [NAME, "1.2.76", "1.2.76", "up", "to", "date"]
    );
    assert_eq!(
        lines.next(),
        Some(format!("  {}", pin_note("100", "1.2.76")).as_str())
    );
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    let env = &json_of(&out)["environments"][0];
    assert_eq!(env["update"], "none");
    assert_eq!(env["applicable"], false);
    assert_eq!(env["reason"], Value::Null);
    assert_eq!(env["pinned"], "100");
    // Nothing to skip, so `--all` says nothing of it.
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_eq!(stdout(&out), "Nothing to update.\n");
}

#[test]
fn a_branch_without_a_generation_2_build_keeps_its_own_note() {
    let (f, api) = installed();
    beta(&api, "400", "1.3.0-beta");
    assert_success(&update(&f, &api, &["--branch", "beta", "--yes"]));
    api.hidden_builds.lock().unwrap().insert("400".to_owned());
    let note = format!(
        "GOG has no Galaxy build of this game on branch beta that wineo can update. Return to the default branch with: wineo update {NAME} --default-branch"
    );

    let out = gog(&f, &api, &["update", "--check"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("update on GOG"), "{text}");
    assert!(text.contains(&format!("  {note}\n")), "{text}");
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    let env = &json_of(&out)["environments"][0];
    assert_eq!(env["update"], "galaxy");
    assert_eq!(env["reason"], "galaxy");
    assert_eq!(env["branch"], "beta");

    let out = update(&f, &api, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("error: {note}")),
        "{}",
        stderr(&out)
    );
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert!(
        stdout(&out).contains(&format!("Skipped {NAME}: {note}\n")),
        "{}",
        stdout(&out)
    );

    // The way out works.
    assert_success(&update(&f, &api, &["--default-branch", "--yes"]));
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert_eq!(installed_build(&f), "100");
    assert_eq!(record(&f).follow, Default::default());
}

#[test]
fn a_move_onto_galaxy_can_land_on_a_branch() {
    let (f, api) = world();
    beta(&api, "400", "1.3.0-beta");
    installer_package(&api, "1.5", "installer");
    assert_success(&install(&f, &api, &["--track", "installer"]));
    let out = update(
        &f,
        &api,
        &[
            "--reinstall",
            "--track",
            "galaxy",
            "--branch",
            "beta",
            "--yes",
        ],
    );
    assert_success(&out);
    let text = stdout(&out);
    assert!(has_row(&text, "track", "installer -> Galaxy"), "{text}");
    assert!(has_row(&text, "branch", "default -> beta"), "{text}");
    assert_eq!(installed_build(&f), "400");
    assert_eq!(record(&f).follow.branch.as_deref(), Some("beta"));
}

#[test]
fn a_move_to_the_installer_track_drops_what_the_game_followed() {
    let (f, api) = installed();
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    f.inno_runner();
    installer_package(&api, "1.5", "installer");
    assert_success(&update(
        &f,
        &api,
        &["--reinstall", "--track", "installer", "--refresh", "--yes"],
    ));
    assert_eq!(state(&f).committed.unwrap().track, Track::Installer);
    let path =
        wineo_core::gog::folder::sidecar_file(&f.paths(), &fs::canonicalize(folder(&f)).unwrap());
    assert!(!path.exists());
    let out = gog(&f, &api, &["update", "--check", "--json"]);
    let env = &json_of(&out)["environments"][0];
    assert_eq!(env["track"], "installer");
    assert_eq!(env["branch"], Value::Null);
    assert_eq!(env["pinned"], Value::Null);
}

#[test]
fn a_change_of_pin_asks_before_it_writes() {
    let (f, api) = installed();
    for (answer, code) in [("n", Some(130)), ("y", Some(0))] {
        let mut command = f.gog_command(&api);
        command
            .env("DISPLAY", ":0")
            .args(["update", NAME, "--build", "100"]);
        let mut pty = Pty::spawn(command);
        pty.expect("Change it?");
        pty.send(answer);
        let (status, output) = pty.finish();
        assert_eq!(status.code(), code, "{output}");
        if answer == "n" {
            assert_eq!(record(&f).follow, Default::default());
        }
    }
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));
}

#[test]
fn an_install_on_a_branch_that_stopped_finishes_on_it() {
    let (f, api) = world();
    beta(&api, "400", "1.3.0-beta");
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
            "--branch",
            "beta",
            "--yes",
        ],
    );
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert_eq!(record(&f).follow, Default::default());

    // Finishing carries on as it began, so a branch can't be asked for again.
    let out = gog(
        &f,
        &api,
        &[
            "import",
            "gog",
            "--game",
            "1207658924",
            "--branch",
            "beta",
            "--yes",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Leave out --branch."),
        "{}",
        stderr(&out)
    );

    let out = gog(
        &f,
        &api,
        &["import", "gog", "--game", "1207658924", "--yes"],
    );
    assert_success(&out);
    assert!(has_row(&stdout(&out), "branch", "beta"), "{}", stdout(&out));
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.3.0-beta");
    assert_eq!(record(&f).follow.branch.as_deref(), Some("beta"));
}

#[test]
fn all_updates_a_pinned_game_that_has_dlc_to_install_and_skips_one_with_only_a_newer_build() {
    const DLC: u64 = 1207658925;
    let (f, api) = installed();
    // Build 100 is listed again with the DLC, behind a newer build 200.
    api.content.add_build(
        on("100", "1.2.76", None)
            .depot(FakeDepot::new(DLC).file(FakeFile::new("data\\dlc.dat", b"bought"))),
    );
    newer(&api);
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    let out = gog(&f, &api, &["update", "--all", "--yes"]);
    assert_eq!(
        stdout(&out),
        format!(
            "Skipped {NAME}: {}\nNothing to update.\n",
            pin_note("100", "1.2.76")
        )
    );

    // Bought since.
    api.with(|config| {
        config.products.insert(
            GAME,
            product(
                GAME,
                "Papers, Please",
                &[],
                &[json!({"id": DLC, "title": "Bonus"})],
            ),
        );
        config.owned_ids = Some(vec![GAME, DLC]);
    });
    let out = gog(&f, &api, &["update", "--check", "--refresh", "--json"]);
    let env = &json_of(&out)["environments"][0];
    assert_eq!(env["update"], "available");
    assert_eq!(env["applicable"], true);
    assert_eq!(env["reason"], Value::Null);
    assert_eq!(env["pinned"], "100");
    assert_eq!(env["dlc"][0]["state"], "not_installed");
    let out = gog(&f, &api, &["update", "--check", "--refresh"]);
    assert!(
        stdout(&out).contains("update available"),
        "{}",
        stdout(&out)
    );

    let out = gog(&f, &api, &["update", "--all", "--refresh", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains(&format!("Updating {NAME}\n")), "{text}");
    assert!(text.contains("Updated 1 of 1 game.\n"), "{text}");
    assert_eq!(read(&f, "data/dlc.dat"), "bought");
    assert_eq!(installed_build(&f), "100");
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));

    // Nothing left but the newer build.
    let out = gog(&f, &api, &["update", "--all", "--refresh", "--yes"]);
    assert!(
        stdout(&out).starts_with(&format!("Skipped {NAME}: Pinned to build 100")),
        "{}",
        stdout(&out)
    );
    let out = update(&f, &api, &["--refresh", "--yes"]);
    assert_success(&out);
    assert_eq!(
        stdout(&out),
        format!("{}\n", pinned_message("100", "1.2.76"))
    );
}

#[test]
fn a_move_off_galaxy_planned_before_a_pin_is_refused_and_keeps_the_pin() {
    let (f, api) = installed();
    f.inno_runner();
    installer_package(&api, "1.5", "installer");
    let mut command = f.gog_command(&api);
    command.env("DISPLAY", ":0").args([
        "update",
        NAME,
        "--reinstall",
        "--track",
        "installer",
        "--refresh",
    ]);
    let mut pty = Pty::spawn(command);
    pty.expect("Reinstall it?");
    // Meanwhile another command pins the build installed, which only
    // touches the record the move would delete.
    assert_success(&update(&f, &api, &["--build", "100", "--yes"]));
    pty.send("y");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(
        output.contains(&format!(
            "{NAME}'s GOG game changed while updating; run wineo update {NAME} again."
        )),
        "{output}"
    );
    assert_eq!(state(&f).committed.unwrap().track, Track::Galaxy);
    assert_eq!(record(&f).follow.pinned.as_deref(), Some("100"));
    assert_eq!(read(&f, "Papers.exe"), "MZ 1.2.76");
    assert!(state(&f).journal.is_none());
}

#[test]
fn an_unfinished_install_reports_the_follow_its_record_kept() {
    let (f, api) = world();
    beta(&api, "400", "1.3.0-beta");
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
            "--branch",
            "beta",
            "--yes",
        ],
    );
    pty.expect("Downloading: ");
    interrupt(&pty);
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(state(&f).committed.is_none());

    // The window after the record is written and before the state commits.
    let path =
        wineo_core::gog::folder::sidecar_file(&f.paths(), &fs::canonicalize(folder(&f)).unwrap());
    for (follow, branch, pinned) in [
        (json!({"branch": "beta"}), json!("beta"), Value::Null),
        (
            json!({"branch": "beta", "pinned": "400"}),
            json!("beta"),
            json!("400"),
        ),
    ] {
        let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        saved["follow"] = follow;
        fs::write(&path, saved.to_string()).unwrap();

        let out = gog(&f, &api, &["update", "--check", "--json"]);
        let env = &json_of(&out)["environments"][0];
        assert_eq!(env["update"], "cant_compare", "{env}");
        assert_eq!(env["unfinished"], true);
        assert_eq!(env["branch"], branch);
        assert_eq!(env["pinned"], pinned);
        let out = gog(&f, &api, &["import", "gog", "--list", "--json"]);
        let games = json_of(&out);
        let game = games["games"]
            .as_array()
            .unwrap()
            .iter()
            .find(|game| game["environment"] == NAME)
            .unwrap();
        assert_eq!(game["branch"], branch);
        assert_eq!(game["pinned"], pinned);
    }
}
