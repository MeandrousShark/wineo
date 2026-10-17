//! The current wineo against files that real older releases wrote.
//!
//! Every set under `crates/wineo-core/tests/fixtures/` (see its README) holds
//! a portable root and a HOME-shaped root. Each test copies both into a fresh
//! directory under this crate, which is on the checkout's filesystem like the
//! snapshot tests, and runs the CLI over the copies. A set added later is
//! picked up without changing anything here.

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use serde_json::Value;
use wineo_core::paths::Paths;

use common::{Fixture, stderr, stdout};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../wineo-core/tests/fixtures")
}

/// Names of the sets, sorted: `legacy` and one directory per release.
fn sets() -> Vec<String> {
    let mut sets: Vec<String> = fs::read_dir(fixtures_dir())
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().join("portable").is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    sets.sort();
    assert!(
        sets.len() >= 8,
        "expected the legacy set and a set per release, found {sets:?}"
    );
    sets
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            symlink(fs::read_link(entry.path()).unwrap(), target).unwrap();
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The state a set must have, so deleting it from the fixtures fails a test
/// instead of quietly skipping the check. Snapshots exist in every generated
/// set, trash and playtime from v0.14.0, and the hand-written legacy set has
/// none (its README says so). A set added later is expected to have all.
struct Expected {
    playtime: bool,
    trash: bool,
    snapshots: bool,
}

fn expected(set: &str) -> Expected {
    let (playtime, trash, snapshots) = match set {
        "legacy" => (false, false, false),
        "v0.2.0" | "v0.7.0" => (false, false, true),
        _ => (true, true, true),
    };
    Expected {
        playtime,
        trash,
        snapshots,
    }
}

/// The entries of `dir` when the set should have it, and none when it
/// shouldn't. Either way the directory must agree with the table.
fn state_entries(name: &str, what: &str, dir: &Path, should_exist: bool) -> Vec<PathBuf> {
    assert_eq!(
        dir.is_dir(),
        should_exist,
        "{name}: {what} directory {} {}",
        dir.display(),
        if should_exist {
            "is missing"
        } else {
            "should not exist"
        }
    );
    if !should_exist {
        return Vec::new();
    }
    let entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(!entries.is_empty(), "{name}: {what} directory is empty");
    entries
}

const ROOTS: [&str; 2] = ["portable", "xdg"];

/// One set copied into place, with the CLI pointed at either root.
struct Set {
    name: String,
    f: Fixture,
}

impl Set {
    fn new(name: &str) -> Self {
        let f = Fixture::new_in(Path::new(env!("CARGO_MANIFEST_DIR")));
        // Later sets turn wayland on, which wineo accepts only for a wine that
        // has the driver, found two folders up from the wine binary.
        let driver = f.root.path().join("lib/wine/x86_64-unix");
        fs::create_dir_all(&driver).unwrap();
        fs::write(driver.join("winewayland.so"), "").unwrap();
        let source = fixtures_dir().join(name);
        copy_tree(&source.join("portable"), &f.root.path().join("portable"));
        copy_tree(&source.join("xdg"), &f.root.path().join("home"));
        let set = Self {
            name: name.to_owned(),
            f,
        };
        set.rebind_trash();
        set
    }

    fn portable(&self) -> PathBuf {
        self.f.root.path().join("portable")
    }

    fn home(&self) -> PathBuf {
        self.f.root.path().join("home")
    }

    /// A manifest records which directory it was written in, and a copy is a
    /// different one. Only those recorded identities are pointed at the copy;
    /// the check itself stays as strict as it is.
    fn rebind_trash(&self) {
        let portable = self.portable();
        let home = self.f.root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&portable),
        )
        .unwrap();
        wineo_core::trash::rebind_identities(&paths)
            .unwrap_or_else(|err| panic!("{}: rebinding trash identities: {err:#}", self.name));
    }

    fn portable_command(&self) -> Command {
        let mut command = self.f.wineo();
        command.arg("--portable").arg(self.portable());
        command
    }

    fn home_command(&self) -> Command {
        let home = self.home();
        let mut command = self.f.wineo();
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"));
        command
    }

    /// Runs `args` against the named root, expecting success.
    fn run(&self, root: &str, args: &[&str]) -> Output {
        let mut command = match root {
            "portable" => self.portable_command(),
            "xdg" => self.home_command(),
            other => panic!("no root {other}"),
        };
        let output = command.args(args).output().expect("wineo should start");
        assert!(
            output.status.success(),
            "{} {root}: wineo {}\n{}{}",
            self.name,
            args.join(" "),
            stdout(&output),
            stderr(&output)
        );
        output
    }

    fn json(&self, root: &str, args: &[&str]) -> Value {
        let output = self.run(root, args);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|err| panic!("{} {root} {args:?}: {err}", self.name))
    }

    fn config_dir(&self, root: &str) -> PathBuf {
        match root {
            "portable" => self.portable().join("config"),
            _ => self.home().join(".config/wineo"),
        }
    }

    fn tree(&self, root: &str) -> PathBuf {
        match root {
            "portable" => self.portable(),
            _ => self.home(),
        }
    }

    /// Environment names, from the config files themselves.
    fn environments(&self, root: &str) -> Vec<String> {
        stems(&self.config_dir(root).join("environments"))
    }
}

fn stems(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
                .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A file's top-level `key = value` lines, values as written.
fn top_level(text: &str) -> BTreeMap<String, String> {
    let mut keys = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('[') {
            break;
        }
        if let Some((key, value)) = line.split_once(" = ") {
            keys.insert(key.to_owned(), value.to_owned());
        }
    }
    keys
}

fn unquote(value: &str) -> &str {
    value.trim_matches(|c| c == '"' || c == '\'')
}

fn string_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(", ")
                .filter(|item| !item.is_empty())
                .map(|item| unquote(item).to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Every file's bytes and modification time, and every directory's and
/// link's too, keyed by path under `root`.
fn capture(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
        let meta = fs::symlink_metadata(path).unwrap();
        let key = path.strip_prefix(root).unwrap().to_owned();
        let bytes = if meta.file_type().is_symlink() {
            fs::read_link(path)
                .unwrap()
                .into_os_string()
                .into_encoded_bytes()
        } else if meta.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        out.insert(key, (bytes, meta.modified().unwrap()));
        if meta.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(root, &entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn problems(report: &Value) -> Vec<(String, String, Option<String>)> {
    report["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|problem| {
            (
                problem["level"].as_str().unwrap().to_owned(),
                problem["code"].as_str().unwrap().to_owned(),
                problem["environment"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[test]
fn every_config_loads_and_validate_finds_no_errors() {
    for name in sets() {
        let set = Set::new(&name);
        for root in ROOTS {
            let report = set.json(root, &["validate", "--json"]);
            let errors: Vec<_> = problems(&report)
                .into_iter()
                .filter(|(level, _, _)| level == "error")
                .collect();
            assert!(
                errors.is_empty() && report["errors"].as_array().unwrap().is_empty(),
                "{name} {root}: {report}"
            );

            let listed = set.json(root, &["list", "--json"]);
            assert!(
                listed["errors"].as_array().unwrap().is_empty(),
                "{name} {root}"
            );
        }
    }
}

#[test]
fn reading_changes_no_bytes_or_modification_times() {
    for name in sets() {
        let set = Set::new(&name);
        for root in ROOTS {
            let tree = set.tree(root);
            let before = capture(&tree);
            let mut commands: Vec<Vec<String>> = [
                &["status", "--json"][..],
                &["status"],
                &["list"],
                &["list", "--json"],
                &["validate"],
                &["validate", "--json"],
                &["templates"],
                &["trash"],
                &["stats"],
                &["config"],
            ]
            .iter()
            .map(|args| args.iter().map(|arg| (*arg).to_owned()).collect())
            .collect();
            for environment in set.environments(root) {
                for args in [
                    vec!["status", &environment, "--json"],
                    vec!["config", &environment],
                    vec!["stats", &environment],
                    vec!["snapshot", "list", &environment],
                ] {
                    commands.push(args.into_iter().map(str::to_owned).collect());
                }
            }
            for args in &commands {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                // Some of these have nothing to show in some sets, which is
                // still a read. Only a crash would be a problem.
                let output = match root {
                    "portable" => set.portable_command(),
                    _ => set.home_command(),
                }
                .args(&args)
                .output()
                .unwrap();
                assert!(
                    output.status.code().is_some(),
                    "{name} {root} {args:?}: killed"
                );
                assert!(
                    capture(&tree) == before,
                    "{name} {root}: `wineo {}` changed the files:\n{}",
                    args.join(" "),
                    changed(&before, &capture(&tree))
                );
            }
        }
    }
}

fn changed(
    before: &BTreeMap<PathBuf, (Vec<u8>, SystemTime)>,
    after: &BTreeMap<PathBuf, (Vec<u8>, SystemTime)>,
) -> String {
    let mut lines = Vec::new();
    for (path, entry) in after {
        match before.get(path) {
            None => lines.push(format!("added {}", path.display())),
            Some(old) if old != entry => lines.push(format!("changed {}", path.display())),
            Some(_) => {}
        }
    }
    for path in before.keys().filter(|path| !after.contains_key(*path)) {
        lines.push(format!("removed {}", path.display()));
    }
    lines.join("\n")
}

/// Every line of `before` is still in `after`, in order, except the ones that
/// set `key`, which an edit of `key` may replace.
fn keeps_every_line_but(before: &str, after: &str, key: &str) -> bool {
    let mut rest = after.lines();
    before
        .lines()
        .filter(|line| !line.starts_with(&format!("{key} = ")))
        .all(|line| rest.any(|next| next == line))
}

#[test]
fn a_real_edit_stamps_a_legacy_file_and_keeps_every_other_line() {
    for name in sets() {
        let set = Set::new(&name);
        for root in ROOTS {
            let config = set.config_dir(root);
            let mut edits: Vec<(PathBuf, Vec<String>, &str)> = vec![(
                config.join("config.toml"),
                ["set", "--global", "stop_timeout=21"]
                    .map(str::to_owned)
                    .to_vec(),
                "stop_timeout",
            )];
            for environment in set.environments(root) {
                edits.push((
                    config
                        .join("environments")
                        .join(format!("{environment}.toml")),
                    ["set", &environment, "tags=[\"upgrade\"]"]
                        .map(str::to_owned)
                        .to_vec(),
                    "tags",
                ));
            }
            for (file, args, key) in edits {
                if !file.exists() {
                    continue;
                }
                let before = fs::read_to_string(&file).unwrap();
                let legacy = !before
                    .lines()
                    .any(|line| line.starts_with("schema_version"));
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                set.run(root, &args);
                let after = fs::read_to_string(&file).unwrap();
                let label = format!("{name} {root} {}", file.display());

                // The edit landed, in the file and in what wineo reports.
                if key == "stop_timeout" {
                    assert!(
                        after.lines().any(|line| line == "stop_timeout = 21"),
                        "{label}:\n{after}"
                    );
                    let shown = stdout(&set.run(root, &["config"]));
                    assert!(
                        shown.lines().any(|line| line == "stop_timeout = 21"),
                        "{label}: config shows\n{shown}"
                    );
                } else {
                    assert!(
                        after.lines().any(|line| line == "tags = [\"upgrade\"]"),
                        "{label}:\n{after}"
                    );
                    let status = set.json(root, &["status", args[1], "--json"]);
                    assert_eq!(status["tags"], serde_json::json!(["upgrade"]), "{label}");
                }

                let stamps = after
                    .lines()
                    .filter(|line| *line == "schema_version = 1")
                    .count();
                assert_eq!(stamps, 1, "{label}:\n{after}");
                // Every old line survives in order, and the only new ones are
                // the edit itself and, for a legacy file, the stamp.
                assert!(
                    keeps_every_line_but(&before, &after, key),
                    "{label}: an old line changed or moved\n--- before\n{before}\n--- after\n{after}"
                );
                let old: Vec<&str> = before.lines().collect();
                for line in after.lines().filter(|line| !old.contains(line)) {
                    assert!(
                        line.starts_with(&format!("{key} = "))
                            || (legacy && line == "schema_version = 1"),
                        "{label}: unexpected new line {line:?}\n{after}"
                    );
                }
                assert_eq!(legacy, !before.contains("schema_version"), "{label}");
            }
            let report = set.json(root, &["validate", "--json"]);
            assert!(
                report["errors"].as_array().unwrap().is_empty()
                    && problems(&report)
                        .iter()
                        .all(|(level, _, _)| level != "error"),
                "{name} {root} after edits: {report}"
            );
        }
    }
}

#[test]
fn templates_are_listed_and_never_rewritten() {
    for name in sets() {
        let set = Set::new(&name);
        for root in ROOTS {
            let dir = set.config_dir(root).join("templates");
            if !dir.is_dir() {
                continue;
            }
            let before = capture(&dir);
            let listing = stdout(&set.run(root, &["templates"]));
            for template in stems(&dir) {
                assert!(
                    listing
                        .lines()
                        .any(|line| line.split_whitespace().next() == Some(template.as_str())),
                    "{name} {root}: `templates` hides {template}\n{listing}"
                );
            }
            assert!(capture(&dir) == before, "{name} {root}");
        }
    }
}

#[test]
fn playtime_reads_back_as_written() {
    for name in sets() {
        let set = Set::new(&name);
        let logs = set.portable().join("stats");
        let entries = state_entries(&name, "playtime", &logs, expected(&name).playtime);
        for path in entries {
            let environment = path.file_stem().unwrap().to_string_lossy().into_owned();
            let sessions: Vec<Value> = fs::read_to_string(&path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(!sessions.is_empty(), "{name} {environment}");
            let seconds: u64 = sessions
                .iter()
                .map(|session| session["seconds"].as_u64().unwrap())
                .sum();
            let status = set.json("portable", &["status", &environment, "--json"]);
            let played = &status["played"];
            assert_eq!(played["seconds"], seconds, "{name} {environment}");
            assert_eq!(played["sessions"], sessions.len(), "{name} {environment}");
            let last = sessions
                .iter()
                .map(|session| session["started_at"].as_str().unwrap())
                .max()
                .unwrap();
            assert_eq!(played["last_played"], last, "{name} {environment}");

            let overview = stdout(&set.run("portable", &["stats"]));
            let row = overview
                .lines()
                .find(|line| line.split_whitespace().next() == Some(environment.as_str()))
                .unwrap_or_else(|| panic!("{name}: no stats row for {environment}\n{overview}"));
            let played_text = if seconds < 60 {
                "< 1 m".to_owned()
            } else {
                format!("{} m", seconds / 60)
            };
            assert!(
                row.contains(&played_text),
                "{name} {environment}: {row:?} lacks {played_text:?}"
            );
            assert!(
                row.split_whitespace()
                    .any(|word| word == sessions.len().to_string()),
                "{name} {environment}: {row:?}"
            );
        }
    }
}

#[test]
fn trash_entries_read_back_as_written() {
    for name in sets() {
        let set = Set::new(&name);
        let trash = set.portable().join("data/trash");
        let entries = state_entries(&name, "trash", &trash, expected(&name).trash);
        if entries.is_empty() {
            continue;
        }
        let listing = stdout(&set.run("portable", &["trash"]));
        let mut seen = 0;
        for dir in entries {
            let id = dir.file_name().unwrap().to_string_lossy().into_owned();
            let manifest = top_level(&fs::read_to_string(dir.join("manifest.toml")).unwrap());
            let environment = unquote(&manifest["name"]).to_owned();
            let line = listing
                .lines()
                .find(|line| line.starts_with(&id))
                .unwrap_or_else(|| panic!("{name}: trash hides {id}\n{listing}"));
            assert!(
                line.split_whitespace().nth(1) == Some(environment.as_str()),
                "{name}: {line:?}"
            );
            // The prefix is either in the entry or left where it was.
            let taken = manifest["include_prefix"] == "true";
            assert_eq!(
                taken,
                set.portable()
                    .join("prefixes/.wineo-trash")
                    .join(&id)
                    .join("prefix")
                    .is_dir(),
                "{name} {id}"
            );
            assert_eq!(
                line.contains("prefix left in place"),
                !taken,
                "{name}: {line:?}"
            );
            // A removed environment is not listed as one.
            assert!(
                !set.environments("portable").contains(&environment),
                "{name}: {environment}"
            );
            seen += 1;
        }
        assert_eq!(listing.lines().count(), seen, "{name}:\n{listing}");
    }
}

#[test]
fn snapshots_read_back_as_written() {
    for name in sets() {
        let set = Set::new(&name);
        let root = set.portable().join("prefixes/.wineo-snapshots");
        let environments = state_entries(&name, "snapshot", &root, expected(&name).snapshots);
        for environment in environments {
            let owner = environment
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let listing = stdout(&set.run("portable", &["snapshot", "list", &owner]));
            let mut written = Vec::new();
            for label in fs::read_dir(&environment).unwrap() {
                let label = label.unwrap().path();
                let manifest: Value =
                    serde_json::from_slice(&fs::read(label.join("manifest.json")).unwrap())
                        .unwrap();
                written.push(format!(
                    "{}\t{}",
                    manifest["label"].as_str().unwrap(),
                    manifest["created_at"].as_str().unwrap()
                ));
            }
            written.sort();
            let mut listed: Vec<String> = listing.lines().map(str::to_owned).collect();
            listed.sort();
            assert_eq!(listed, written, "{name} {owner}");
        }
    }
}

#[test]
fn status_and_list_show_what_the_files_say() {
    for name in sets() {
        let set = Set::new(&name);
        for root in ROOTS {
            let listed = set.json(root, &["list", "--json"]);
            let listed = listed["environments"].as_array().unwrap();
            let names: Vec<&str> = listed
                .iter()
                .map(|environment| environment["name"].as_str().unwrap())
                .collect();
            let files = set.environments(root);
            assert_eq!(names, files, "{name} {root}");

            for environment in &files {
                let text = fs::read_to_string(
                    set.config_dir(root)
                        .join("environments")
                        .join(format!("{environment}.toml")),
                )
                .unwrap();
                let keys = top_level(&text);
                let kind = keys.get("kind").map_or("wine", |kind| unquote(kind));
                let tags = string_list(keys.get("tags"));
                let hooks = text.lines().any(|line| line == "[hooks]");

                let row = listed
                    .iter()
                    .find(|row| row["name"] == environment.as_str())
                    .unwrap();
                let status = set.json(root, &["status", environment, "--json"]);
                for shown in [row, &status] {
                    let label = format!("{name} {root} {environment}");
                    assert_eq!(shown["kind"], kind, "{label}");
                    assert_eq!(shown["tags"], serde_json::json!(tags), "{label}");
                    assert_eq!(shown["state"], "inactive", "{label}");
                    // Hooks arrived in v0.28.0; a set from before has none.
                    assert_eq!(!shown["hooks"].is_null(), hooks, "{label}");
                }
                match kind {
                    "wine" => {
                        assert_eq!(
                            status["runner"]["configured"],
                            unquote(&keys["runner"]),
                            "{name} {root} {environment}"
                        );
                        assert_eq!(
                            status["winetricks"],
                            serde_json::json!(string_list(keys.get("winetricks"))),
                            "{name} {root} {environment}"
                        );
                    }
                    "proxied" => {
                        assert_eq!(status["app_id"], unquote(&keys["app_id"]));
                        assert_eq!(status["launcher"], unquote(&keys["launcher"]));
                    }
                    _ => {}
                }
            }
        }
    }
}
