//! Selecting a portable folder with `--portable` or `WINEO_PORTABLE_DIR`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::*;

const APP: &str = "[targets.app]\nexe = \"app\"\n";

/// Writes an environment into a portable folder, its prefix inside the folder.
fn portable_environment(dir: &Path, name: &str) -> PathBuf {
    let prefix = dir.join("prefixes").join(name);
    fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
    fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    let config = dir.join("config/environments").join(format!("{name}.toml"));
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n{APP}",
            prefix.display()
        ),
    )
    .unwrap();
    prefix
}

fn names(output: &std::process::Output) -> Vec<String> {
    assert_success(output);
    stdout(output)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn read_only_commands_do_not_create_the_folder() {
    let f = Fixture::new();
    let dir = f.root.path().join("portable");
    let dir_arg = dir.to_str().unwrap();
    for args in [
        vec!["list"],
        vec!["status"],
        vec!["config", "--global"],
        vec!["config", "hades"],
        vec!["templates"],
        vec!["runners"],
        vec!["dxvk"],
        vec!["vkd3d"],
        vec!["desktop"],
        vec!["logs", "hades"],
        vec!["programs", "hades"],
        vec!["snapshot", "list", "hades"],
    ] {
        let mut full = vec!["--portable", dir_arg];
        full.extend(&args);
        f.run(&full);
        assert!(!dir.exists(), "{args:?} created the folder");
        f.wineo()
            .env("WINEO_PORTABLE_DIR", &dir)
            .args(&args)
            .output()
            .unwrap();
        assert!(!dir.exists(), "{args:?} created the folder");
    }
}

#[test]
fn the_option_wins_over_the_variable_and_xdg_stays_put() {
    let f = Fixture::new();
    f.environment("xdg", APP);
    let one = f.root.path().join("one");
    let two = f.root.path().join("two");
    portable_environment(&one, "alpha");
    portable_environment(&two, "beta");

    assert_eq!(names(&f.run(&["list"])), ["xdg"]);
    let listed = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", &two)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["beta"]);
    let listed = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", &two)
        .args(["list", "--portable"])
        .arg(&one)
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["alpha"]);
    let listed = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", "")
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["xdg"]);
}

#[test]
fn a_relative_folder_resolves_from_where_wineo_starts() {
    let f = Fixture::new();
    let dir = f.root.path().join("portable");
    portable_environment(&dir, "alpha");
    let listed = f
        .wineo()
        .current_dir(f.root.path())
        .args(["--portable", "portable", "list"])
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["alpha"]);
}

/// Stops the portable run even when an assertion fails first.
struct StopOnDrop<'a>(&'a Fixture, PathBuf, &'a str);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .wineo()
            .arg("--portable")
            .arg(&self.1)
            .args(["stop", "--force", self.2])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
fn same_named_environments_keep_separate_runs() {
    let f = Fixture::new();
    f.environment("hades", APP);
    let dir = f.root.path().join("portable");
    let prefix = portable_environment(&dir, "hades");
    let _stop = StopOnDrop(&f, dir.clone(), "hades");
    let portable = |args: &[&str]| {
        f.wineo()
            .arg("--portable")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };

    assert_success(&portable(&["start", "hades"]));
    wait_for("the portable program", || prefix.join("fake-pids").exists());
    assert!(!f.fake_program_running("hades"));
    assert_eq!(f.state("hades"), "");
    let supervisor = format!("__supervise\0--portable\0{}\0", dir.display());
    let carried = fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        fs::read(entry.path().join("cmdline"))
            .is_ok_and(|cmdline| String::from_utf8_lossy(&cmdline).ends_with(&supervisor))
    });
    assert!(
        carried,
        "no supervisor was started with the portable folder"
    );
    assert!(stdout(&portable(&["status", "hades"])).contains("running"));
    assert!(!stdout(&f.run(&["status", "hades"])).contains("running"));

    // The XDG environment starts on its own while the portable one runs.
    assert_success(&f.run(&["start", "hades"]));
    assert!(f.state("hades").contains("status = \"running\""));
    assert_success(&f.run(&["stop", "hades"]));
    assert!(prefix.join("fake-pids").exists());

    assert_success(&portable(&["stop", "hades"]));
    assert!(!prefix.join("fake-pids").exists());
    assert!(!stdout(&portable(&["status", "hades"])).contains("running"));
}

fn reflink_supported(dir: &Path) -> bool {
    let source = dir.join("reflink-source");
    let destination = dir.join("reflink-destination");
    fs::write(&source, b"probe").unwrap();
    let input = fs::File::open(&source).unwrap();
    let output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .unwrap();
    let supported = rustix::fs::ioctl_ficlone(&output, &input).is_ok();
    fs::remove_file(source).unwrap();
    fs::remove_file(destination).unwrap();
    supported
}

fn config(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join("config/environments").join(format!("{name}.toml")))
        .unwrap_or_default()
}

#[test]
fn a_portable_environment_keeps_working_after_the_folder_moves() {
    let f = Fixture::new_in(Path::new(env!("CARGO_MANIFEST_DIR")));
    let snapshots = reflink_supported(f.root.path());
    let dir = f.root.path().join("usb");
    let elsewhere = f.root.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let wineo = |dir: &Path, args: &[&str]| {
        f.wineo()
            .current_dir(&elsewhere)
            .arg("--portable")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
    };

    assert_success(&wineo(&dir, &["new", "-n", "app"]));
    assert!(config(&dir, "app").contains("prefix = \"prefixes/app\""));
    let exe = dir.join("prefixes/app/drive_c/app.exe");
    fs::write(&exe, "").unwrap();
    assert_success(&wineo(
        &dir,
        &["set", "app", &format!("targets.app.exe={}", exe.display())],
    ));
    assert!(
        config(&dir, "app").contains("exe = \"prefixes/app/drive_c/app.exe\""),
        "{}",
        config(&dir, "app")
    );
    let outside = f.root.path().join("outside");
    let refused = wineo(
        &dir,
        &["set", "app", &format!("prefix={}", outside.display())],
    );
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("portable prefixes must be directly in"));
    let refused = wineo(
        &dir,
        &["new", "-n", "stray", "--dir", outside.to_str().unwrap()],
    );
    assert!(!refused.status.success());
    assert!(!outside.exists());
    assert_eq!(config(&dir, "stray"), "");

    assert_success(&wineo(&dir, &["clone", "app", "copy"]));
    let copied = config(&dir, "copy");
    assert!(copied.contains("prefix = \"prefixes/copy\""), "{copied}");
    assert!(
        copied.contains("exe = \"prefixes/copy/drive_c/app.exe\""),
        "{copied}"
    );
    if snapshots {
        assert_success(&wineo(&dir, &["snapshot", "create", "app", "clean"]));
    }

    let moved = f.root.path().join("moved");
    fs::rename(&dir, &moved).unwrap();
    let _stop = StopOnDrop(&f, moved.clone(), "app");
    assert_eq!(names(&wineo(&moved, &["list"])), ["app", "copy"]);

    let started = wineo(&moved, &["start", "app"]);
    assert_success(&started);
    let prefix = moved.join("prefixes/app");
    wait_for("the moved program", || prefix.join("fake-pids").exists());
    assert_success(&wineo(&moved, &["stop", "app"]));
    assert!(!prefix.join("fake-pids").exists());

    assert_success(&wineo(&moved, &["set", "app", "env.FAKE_VAR=1"]));
    assert!(config(&moved, "app").contains("FAKE_VAR"));

    if snapshots {
        fs::write(prefix.join("drive_c/save.dat"), "later").unwrap();
        let listed = stdout(&wineo(&moved, &["snapshot", "list", "app"]));
        assert!(listed.contains("clean"), "{listed}");
        assert_success(&wineo(
            &moved,
            &["snapshot", "restore", "app", "clean", "--yes"],
        ));
        assert!(!prefix.join("drive_c/save.dat").exists());
        assert!(!config(&moved, "app").contains("FAKE_VAR"));
    }

    assert_success(&wineo(&moved, &["rm", "app", "--delete-prefix"]));
    assert!(!prefix.exists());
    assert_eq!(names(&wineo(&moved, &["list"])), ["copy"]);
}

#[test]
fn imports_land_inside_the_folder_and_adopting_outside_is_refused() {
    let f = Fixture::new();
    let dir = f.root.path().join("usb");
    let source = f.root.path().join("source");
    fs::create_dir_all(source.join("drive_c")).unwrap();
    fs::write(
        source.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    let wineo = |args: &[&str]| {
        f.wineo()
            .arg("--portable")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };
    let source_arg = source.to_str().unwrap();

    let adopted = wineo(&[
        "import", "prefix", source_arg, "-n", "kept", "--runner", "wine", "--adopt",
    ]);
    assert!(!adopted.status.success());
    assert!(stderr(&adopted).contains("portable prefixes must be directly in"));
    assert_eq!(config(&dir, "kept"), "");

    assert_success(&wineo(&[
        "import", "prefix", source_arg, "-n", "copied", "--runner", "wine",
    ]));
    assert!(config(&dir, "copied").contains("prefix = \"prefixes/copied\""));
    assert!(dir.join("prefixes/copied/system.reg").exists());
}

#[test]
fn portable_environments_never_get_desktop_entries() {
    let f = Fixture::new();
    let applications = f.root.path().join("data/applications");
    fs::create_dir_all(&applications).unwrap();
    let host_entry = applications.join("wineo-hades-app.desktop");
    let entry = "[Desktop Entry]\nExec=wineo start hades --target app\nX-Wineo-Environment=hades\nX-Wineo-Target=app\n";
    fs::write(&host_entry, entry).unwrap();

    let dir = f.root.path().join("usb");
    let prefix = portable_environment(&dir, "hades");
    let config_path = dir.join("config/environments/hades.toml");
    fs::write(
        &config_path,
        "prefix = 'prefixes/hades'\nrunner = 'wine'\narch = 'win64'\n\n[targets.app]\nexe = 'app'\ndesktop = true\n",
    )
    .unwrap();
    let _stop = StopOnDrop(&f, dir.clone(), "hades");
    let wineo = |args: &[&str]| {
        f.wineo()
            .arg("--portable")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };
    const REFUSED: &str = "desktop entries aren't supported for portable environments";

    let started = wineo(&["start", "hades"]);
    assert_success(&started);
    let warning = stderr(&started);
    assert_eq!(warning.lines().count(), 1, "{warning}");
    assert!(warning.contains(REFUSED), "{warning}");
    wait_for("the portable program", || prefix.join("fake-pids").exists());
    assert_success(&wineo(&["stop", "hades"]));

    let desktop = wineo(&["desktop"]);
    assert!(!desktop.status.success());
    assert!(stderr(&desktop).contains(REFUSED));
    let set = wineo(&[
        "set",
        "hades",
        "targets.other.exe=app",
        "targets.other.desktop=true",
    ]);
    assert!(!set.status.success());
    assert!(stderr(&set).contains(REFUSED), "{}", stderr(&set));
    assert!(!fs::read_to_string(&config_path).unwrap().contains("other"));

    let status = stdout(&wineo(&["status", "hades"]));
    assert!(status.contains(prefix.to_str().unwrap()), "{status}");
    let json = stdout(&wineo(&["status", "hades", "--json"]));
    assert!(json.contains(prefix.to_str().unwrap()), "{json}");

    let root = wineo(&["set", "--global", "defaults.prefix_root=/elsewhere"]);
    assert!(!root.status.success());
    assert!(stderr(&root).contains("prefix_root"), "{}", stderr(&root));
    let nested = dir.join("prefixes/hades/nested");
    let moved = wineo(&["set", "hades", &format!("prefix={}", nested.display())]);
    assert!(!moved.status.success());

    let removed = wineo(&["rm", "hades"]);
    assert_success(&removed);
    assert!(stdout(&removed).contains(prefix.to_str().unwrap()));
    assert_eq!(fs::read_to_string(&host_entry).unwrap(), entry);
}
