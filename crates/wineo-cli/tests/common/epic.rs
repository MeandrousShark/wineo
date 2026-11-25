//! A fake `legendary`, `python3`, and `xdg-open` for `wineo import epic`,
//! answering from core's probe fixtures, with a `PATH` that holds only
//! them. Nothing here reaches the network, a browser, or the real
//! Legendary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use super::{Fixture, find_on_path, write_script};

/// The code the fake accepts. It holds `SECRET`, so the pty's output hides
/// it when printed and the tests can look for it.
pub const CODE: &str = "SECRETepic0123456789abcdef";

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../wineo-core/tests/epic-fixtures/legendary-0.21.1")
        .join(name)
}

fn tool(name: &str) -> String {
    find_on_path(name)
        .unwrap_or_else(|| panic!("the tests need {name} on PATH"))
        .display()
        .to_string()
}

/// How the fake behaves. The defaults are Legendary 0.21.1 as the probe
/// saw it.
#[derive(Clone)]
pub struct Fake {
    pub version: String,
    /// `list` prints this on stderr and exits 1 though logged in.
    pub list_fails: bool,
    pub list: PathBuf,
}

impl Default for Fake {
    fn default() -> Self {
        Self::version("0.21.1")
    }
}

impl Fake {
    pub fn version(version: &str) -> Self {
        Self {
            version: format!("legendary version \"{version}\", codename \"Test\""),
            list_fails: false,
            list: fixture("list.json"),
        }
    }
}

pub struct Epic {
    pub f: Fixture,
    /// The only folder on wineo's `PATH`.
    pub bin: PathBuf,
    mark: String,
}

impl Epic {
    pub fn new() -> Self {
        let f = Fixture::new();
        let bin = f.root.path().join("epic-bin");
        fs::create_dir_all(&bin).unwrap();
        let mark = f.root.path().display().to_string();
        let epic = Self { f, bin, mark };
        let calls = epic.file("browser-calls");
        write_script(
            epic.bin.join("xdg-open"),
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", calls.display()),
        );
        epic
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.f.root.path().join(name)
    }

    /// wineo with its own `HOME`, no session bus, and only the fakes on
    /// `PATH`, so neither a real browser nor a real Legendary can be found.
    pub fn command(&self) -> Command {
        let mut command = self.f.wineo_at_home();
        command
            .env("PATH", &self.bin)
            .env("WINEO_FAKE_MARK", &self.mark)
            .env_remove("WINEO_TEST_LEGENDARY_FILE");
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// A `python3` that says it is `version` and runs any file with `sh`.
    pub fn python(&self, version: &str) {
        write_script(
            self.bin.join("python3"),
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'Python {version}'; exit 0; fi\nexec /bin/sh \"$@\"\n"
            ),
        );
    }

    pub fn legendary_at(&self, path: &Path, fake: &Fake) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_script(path, self.script(fake));
    }

    pub fn on_path(&self, fake: &Fake) -> PathBuf {
        let path = self.bin.join("legendary");
        self.legendary_at(&path, fake);
        path
    }

    /// The fake's text in a file of its own, for `WINEO_TEST_LEGENDARY_FILE`.
    pub fn stand_in(&self, fake: &Fake) -> PathBuf {
        let path = self.file("stand-in/legendary_linux_x64");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, self.script(fake)).unwrap();
        path
    }

    pub fn own_copy(&self) -> PathBuf {
        self.f
            .root
            .path()
            .join("data/wineo/epic/bin/legendary-0.21.1")
    }

    pub fn state(&self) -> PathBuf {
        self.f.root.path().join("data/wineo/epic/legendary")
    }

    pub fn cache(&self) -> PathBuf {
        self.f.root.path().join("cache/wineo/epic-owned.json")
    }

    /// A saved login, as Legendary leaves it.
    pub fn log_in(&self) {
        fs::create_dir_all(self.state()).unwrap();
        fs::write(self.state().join("user.json"), "{}\n").unwrap();
    }

    pub fn logged_in(&self) -> bool {
        self.state().join("user.json").exists()
    }

    /// Every command line the fake ran, oldest first.
    pub fn calls(&self) -> Vec<String> {
        lines(&self.file("calls"))
    }

    pub fn browser_calls(&self) -> Vec<String> {
        lines(&self.file("browser-calls"))
    }

    pub fn config(&self) -> PathBuf {
        self.f.root.path().join("config/wineo/config.toml")
    }

    /// Kills every live process that carries this test's mark, and returns
    /// their ids. A test that ran wineo ends by asserting this is empty.
    pub fn sweep(&self) -> Vec<i32> {
        let needle = format!("WINEO_FAKE_MARK={}\0", self.mark);
        let mut found = Vec::new();
        for entry in fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            let Some(handle) = Pid::from_raw(pid) else {
                continue;
            };
            // Pins the process, so the checks and the kill are about it.
            let Ok(pidfd) = pidfd_open(handle, PidfdFlags::empty()) else {
                continue;
            };
            let Ok(environ) = fs::read(format!("/proc/{pid}/environ")) else {
                continue;
            };
            if !environ
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
            {
                continue;
            }
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let state = stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .next();
            if matches!(state, Some("Z" | "X") | None) {
                continue;
            }
            let _ = pidfd_send_signal(&pidfd, Signal::KILL);
            found.push(pid);
        }
        found
    }

    fn script(&self, fake: &Fake) -> String {
        let root = self.f.root.path().display().to_string();
        let (cat, rm) = (tool("cat"), tool("rm"));
        let list_fails = if fake.list_fails {
            format!(
                "echo '[cli] ERROR: Fetching the library failed' >&2; {cat} '{}' >&2; exit 1\n",
                fixture("logged-out-list.stderr").display()
            )
        } else {
            String::new()
        };
        format!(
            "#!/bin/sh\n\
             root='{root}'\n\
             printf '%s\\n' \"$*\" >> \"$root/calls\"\n\
             user=\"$LEGENDARY_CONFIG_PATH/user.json\"\n\
             case \"$1\" in\n\
             --version) echo '{version}' ;;\n\
             status) if [ -f \"$user\" ]; then {cat} '{status_in}'; else {cat} '{status_out}'; fi ;;\n\
             auth)\n\
               case \"$2\" in\n\
               --delete) {rm} -f \"$user\" ;;\n\
               --code)\n\
                 if [ \"$3\" = '{CODE}' ]; then printf '{{}}\\n' > \"$user\"\n\
                 else {cat} '{failed}' >&2; fi ;;\n\
               esac ;;\n\
             list)\n\
               if [ ! -f \"$user\" ]; then {cat} '{logged_out_list}' >&2; exit 1; fi\n\
               {list_fails}\
               {cat} '{list}' ;;\n\
             *) echo \"unknown command $1\" >&2; exit 2 ;;\n\
             esac\n\
             exit 0\n",
            version = fake.version,
            status_in = fixture("status-logged-in.json").display(),
            status_out = fixture("status-logged-out.json").display(),
            failed = fixture("login-failed.stderr").display(),
            logged_out_list = fixture("logged-out-list.stderr").display(),
            list = fake.list.display(),
        )
    }
}

impl Drop for Epic {
    fn drop(&mut self) {
        self.sweep();
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}
