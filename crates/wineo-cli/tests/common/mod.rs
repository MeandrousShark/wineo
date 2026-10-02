//! The fake wine, wineserver, gamemoderun, and umu scripts, and a fixture
//! that points every XDG directory into a temporary root, shared by the test
//! files that drive wineo against them.

#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wineo_core::runtime::Runtime;

#[path = "../../../wineo-core/tests/common/runners.rs"]
mod runners;
pub use runners::{FAKE_GAMEMODERUN, FAKE_UMU, FAKE_WINE, FAKE_WINESERVER};

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub path_var: OsString,
    /// A PATH holding only the fakes and the handful of real tools they call.
    /// Nothing else is on it, so neither gamemoderun nor winetricks can be
    /// found however the machine running the tests is set up.
    pub sandbox: OsString,
}

impl Fixture {
    pub fn new() -> Self {
        Self::from_root(tempfile::tempdir().unwrap())
    }

    pub fn new_in(parent: &Path) -> Self {
        Self::from_root(tempfile::tempdir_in(parent).unwrap())
    }

    fn from_root(root: tempfile::TempDir) -> Self {
        let write_bin = |dir: &str, scripts: &[(&str, &str)]| {
            let bin = root.path().join(dir);
            fs::create_dir_all(&bin).unwrap();
            for (name, script) in scripts {
                let path = bin.join(name);
                fs::write(&path, script).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            bin
        };
        let wine = [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)];
        let bin = write_bin(
            "bin",
            &[
                wine[0],
                wine[1],
                ("gamemoderun", FAKE_GAMEMODERUN),
                ("umu-run", FAKE_UMU),
            ],
        );
        let sandbox = write_bin("bin-sandbox", &[wine[0], wine[1], ("umu-run", FAKE_UMU)]);
        // The machine running these tests may well have gamemode installed, so
        // the sandbox cannot inherit the real PATH. It gets links to the few
        // tools the fake scripts reach for instead.
        for tool in [
            "sh", "sleep", "rm", "kill", "printf", "seq", "touch", "head", "tr", "mkdir",
        ] {
            if let Some(real) = find_on_path(tool) {
                std::os::unix::fs::symlink(real, sandbox.join(tool)).unwrap();
            }
        }

        let inherited = std::env::var_os("PATH").unwrap_or_default();
        Self {
            path_var: std::env::join_paths(
                std::iter::once(bin).chain(std::env::split_paths(&inherited)),
            )
            .unwrap(),
            sandbox: sandbox.into_os_string(),
            root,
        }
    }

    /// Uses the wine installed on the system instead of the fakes.
    pub fn real() -> Self {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let root = tempfile::tempdir().unwrap();
        // umu's container bind-mounts the session bus path and refuses to start
        // when it's missing. A plain file mounts fine and still isn't a bus.
        fs::write(root.path().join("no-bus"), "").unwrap();
        Self {
            root,
            path_var: inherited.clone(),
            sandbox: inherited,
        }
    }

    pub fn wineo(&self) -> Command {
        let root = self.root.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_wineo"));
        command
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env_remove("WINEO_PORTABLE_DIR")
            .env_remove("WINEO_STEAMGRIDDB_KEY")
            // No session bus, so nothing a test runs can reach the real keyring.
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", root.join("no-bus").display()),
            )
            .env("PATH", &self.path_var)
            .stdin(Stdio::null());
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.wineo()
            .args(args)
            .output()
            .expect("wineo should start")
    }

    /// Installs the same fakes as a drop-in runner instead of putting them on
    /// PATH, so a test can prove resolution reaches the supervisor.
    pub fn install_runner(&self, name: &str) {
        let bin = self
            .root
            .path()
            .join("data/wineo/runners")
            .join(name)
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (file, script) in [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)] {
            let path = bin.join(file);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A DXVK or vkd3d build of stub DLLs, extracted into its drop-in root.
    /// The DLL bodies name where they came from, so a test can tell an
    /// installed file from what it replaced.
    pub fn install_build(&self, component: &str, name: &str, dirs: &[&str]) {
        let marker = if component == "dxvk" {
            "dxgi.dll"
        } else {
            "d3d12.dll"
        };
        let dir = self
            .root
            .path()
            .join("data/wineo")
            .join(component)
            .join(name);
        for arch in dirs {
            let arch = dir.join(arch);
            fs::create_dir_all(&arch).unwrap();
            for dll in [marker, "d3d11.dll"] {
                fs::write(arch.join(dll), format!("{name}/{arch:?}")).unwrap();
            }
        }
    }

    pub fn write_template(&self, name: &str, text: &str) {
        let dir = self.config_dir().join("templates");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
    }

    pub fn created_config(&self, name: &str) -> String {
        fs::read_to_string(self.config_dir().join(format!("environments/{name}.toml")))
            .unwrap_or_default()
    }

    pub fn system32(&self, name: &str) -> PathBuf {
        self.root
            .path()
            .join("prefixes")
            .join(name)
            .join("drive_c/windows/system32")
    }

    pub fn manifest(&self, name: &str) -> String {
        fs::read_to_string(
            self.root
                .path()
                .join("prefixes")
                .join(name)
                .join(".wineo-components.toml"),
        )
        .unwrap_or_default()
    }

    pub fn reg_log(&self, name: &str) -> String {
        fs::read_to_string(self.root.path().join("prefixes").join(name).join("reg.log"))
            .unwrap_or_default()
    }

    /// A Proton runner: umu only needs the `proton` script to be there, and
    /// wineo reads the name out of the `version` file beside it.
    pub fn install_proton(&self, name: &str) {
        let dir = self.root.path().join("data/wineo/runners").join(name);
        fs::create_dir_all(&dir).unwrap();
        let proton = dir.join("proton");
        fs::write(&proton, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&proton, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("version"), format!("1717451631 {name}\n")).unwrap();
        fs::create_dir_all(dir.join("files/bin")).unwrap();
        for exe in ["wine", "wineserver"] {
            let path = dir.join("files/bin").join(exe);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// wineo with the fakes off PATH entirely, so anything that still reaches
    /// for the system wine fails loudly instead of quietly working.
    pub fn wineo_without_fakes(&self) -> Command {
        let mut command = self.wineo();
        command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
        command
    }

    /// wineo with the fakes on PATH but nothing else, so a test can prove a
    /// command does not need gamemoderun or winetricks installed.
    pub fn run_sandboxed(&self, args: &[&str]) -> Output {
        self.wineo()
            .env("PATH", &self.sandbox)
            .args(args)
            .output()
            .expect("wineo should start")
    }

    pub fn run_without_gamemode(&self, args: &[&str]) -> Output {
        self.run_sandboxed(args)
    }

    pub fn run_without_fakes(&self, args: &[&str]) -> Output {
        self.wineo_without_fakes()
            .args(args)
            .output()
            .expect("wineo should start")
    }

    /// Writes an environment config with a fake prefix.
    pub fn environment(&self, name: &str, targets: &str) {
        self.environment_with_runner(name, "wine", targets);
    }

    pub fn environment_with_runner(&self, name: &str, runner: &str, targets: &str) {
        let prefix = self.root.path().join("prefixes").join(name);
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        // A win64 prefix, so both architectures have somewhere to go.
        fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n\n#arch=win64\n",
        )
        .unwrap();
        let config = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            format!(
                "prefix = \"{}\"\nrunner = \"{runner}\"\narch = \"win64\"\n\n{targets}",
                prefix.display()
            ),
        )
        .unwrap();
    }

    /// A Proton environment: its configured prefix is the compatdata directory
    /// and the wine prefix sits in `pfx/` inside it.
    pub fn proton_environment(&self, name: &str, runner: &str, targets: &str) {
        let prefix = self.root.path().join("prefixes").join(name);
        let pfx = prefix.join("pfx");
        fs::create_dir_all(pfx.join("drive_c")).unwrap();
        fs::create_dir_all(pfx.join("dosdevices")).unwrap();
        symlink("../drive_c", pfx.join("dosdevices/c:")).unwrap();
        fs::write(
            pfx.join("system.reg"),
            "WINE REGISTRY Version 2\n\n#arch=win64\n",
        )
        .unwrap();
        let config = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            format!(
                "prefix = \"{}\"\nrunner = \"{runner}\"\narch = \"win64\"\n\n{targets}",
                prefix.display()
            ),
        )
        .unwrap();
    }

    pub fn global_config(&self, text: &str) {
        let path = self.config_dir().join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.path().join("config/wineo")
    }

    pub fn runtime(&self, name: &str) -> Runtime {
        Runtime::at(self.root.path().join("state/wineo").join(name))
    }

    pub fn state(&self, name: &str) -> String {
        fs::read_to_string(self.runtime(name).dir().join("state.toml")).unwrap_or_default()
    }

    pub fn log(&self, name: &str) -> String {
        fs::read_to_string(self.runtime(name).log_path()).unwrap_or_default()
    }

    pub fn stats_log(&self, name: &str) -> PathBuf {
        self.root
            .path()
            .join("data/wineo/stats")
            .join(format!("{name}.jsonl"))
    }

    pub fn write_stats_log(&self, name: &str, text: &str) {
        let path = self.stats_log(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub fn read_stats_log(&self, name: &str) -> String {
        fs::read_to_string(self.stats_log(name)).unwrap_or_default()
    }

    pub fn fake_program_running(&self, name: &str) -> bool {
        self.root
            .path()
            .join("prefixes")
            .join(name)
            .join("fake-pids")
            .exists()
    }

    /// Whether the process group the last run recorded still exists. The only
    /// way to tell for a program killed outright, which leaves no pid file.
    pub fn run_group_alive(&self, name: &str) -> bool {
        self.state(name)
            .lines()
            .find_map(|line| line.strip_prefix("group = "))
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_some_and(|group| PathBuf::from(format!("/proc/{group}")).exists())
    }
}

impl Fixture {
    /// A state dir left behind by an environment that no longer exists.
    pub fn stray_state(&self, name: &str) -> PathBuf {
        let dir = self.root.path().join("state/wineo").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("output.log"), "old run\n").unwrap();
        dir
    }

    /// A desktop entry wineo wrote for an environment that no longer exists.
    pub fn stray_entry(&self, name: &str) -> PathBuf {
        let dir = self.root.path().join("data/applications");
        fs::create_dir_all(&dir).unwrap();
        let entry = dir.join(format!("wineo-{name}-{name}.desktop"));
        fs::write(
            &entry,
            format!(
                "[Desktop Entry]\nName={name}\nX-Wineo-Environment={name}\nX-Wineo-Target={name}\n"
            ),
        )
        .unwrap();
        entry
    }
}

impl Drop for Fixture {
    /// A failed assertion must not leave supervisors or fake programs running.
    fn drop(&mut self) {
        let Ok(entries) = fs::read_dir(self.config_dir().join("environments")) else {
            return;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.path().file_stem().and_then(|stem| stem.to_str()) {
                let _ = self
                    .wineo()
                    .args(["stop", "--force", name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

pub fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

pub fn trash_id(output: &Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("Restore with: wineo restore "))
        .expect("removal should print a restore command")
        .to_owned()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[track_caller]
pub fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

pub fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn heroic_library(f: &Fixture, prefix: &std::path::Path) -> std::path::PathBuf {
    let config = f.root.path().join("heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    let exe = prefix.join("drive_c/Games/Game.exe");
    fs::write(&exe, "game executable").unwrap();
    fs::write(
        config.join("config.json"),
        format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        ),
    )
    .unwrap();
    let entries: Vec<_> = [("a", "Alpha"), ("b", "Beta")]
        .into_iter()
        .map(|(id, title)| {
            fs::write(config.join(format!("GamesConfig/{id}.json")), format!(r#"{{"{id}":{{}}}}"#)).unwrap();
            format!(r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}"#, exe.display())
        })
        .collect();
    fs::write(
        config.join("sideload_apps/library.json"),
        format!(r#"{{"games":[{}]}}"#, entries.join(",")),
    )
    .unwrap();
    config
}
