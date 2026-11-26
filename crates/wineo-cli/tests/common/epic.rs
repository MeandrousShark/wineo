//! A fake `legendary`, `python3`, and `xdg-open` for `wineo import epic`,
//! answering from core's probe fixtures, with a `PATH` that holds only
//! them. Nothing here reaches the network, a browser, or the real
//! Legendary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use super::{Fixture, find_on_path, write_script};

#[path = "../../../wineo-core/tests/common/epic_install.rs"]
mod epic_install;

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
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                calls.display()
            ),
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

    /// `command` with the fixture's fake wine on `PATH` after the Epic
    /// fakes, so an install can create the game's environment.
    pub fn install_command(&self) -> Command {
        let mut command = self.command();
        let path =
            std::env::join_paths([self.bin.as_os_str(), self.f.sandbox.as_os_str()]).unwrap();
        command.env("PATH", path);
        command
    }

    pub fn install(&self, args: &[&str]) -> Output {
        self.install_command().args(args).output().unwrap()
    }

    /// How the fake's next install of `app` behaves; see `epic_install`.
    pub fn set_mode(&self, app: &str, mode: &str) {
        fs::write(self.file(&format!("mode-{app}")), mode).unwrap();
    }

    /// Waits for the fake to start installing `app`.
    pub fn wait_started(&self, app: &str) {
        let marker = self.file(&format!("installing-{app}"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the fake never started {app}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The fake's install runs, oldest first.
    pub fn install_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.starts_with("-y install"))
            .collect()
    }

    /// Where the games folder is by default.
    pub fn games(&self) -> PathBuf {
        fs::canonicalize(self.f.root.path()).unwrap().join("Games")
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

    /// A `python3` like `python("3.14.0")` that, on the second time it is
    /// asked its version, puts `stand_in` where wineo's own copy goes, as
    /// another wineo finishing the download first would. That is the check
    /// just before wineo takes the download's lock.
    pub fn python_that_fetches_first(&self, stand_in: &Path) {
        let count = self.file("python-versions");
        let own = self.own_copy();
        write_script(
            self.bin.join("python3"),
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = --version ]; then\n\
                   n=$({cat} '{count}' 2>/dev/null || echo 0); n=$((n + 1)); echo \"$n\" > '{count}'\n\
                   if [ \"$n\" -eq 2 ]; then {mkdir} -p '{dir}' && {cp} '{stand_in}' '{own}'; fi\n\
                   echo 'Python 3.14.0'; exit 0\n\
                 fi\n\
                 exec /bin/sh \"$@\"\n",
                cat = tool("cat"),
                mkdir = tool("mkdir"),
                cp = tool("cp"),
                count = count.display(),
                dir = own.parent().unwrap().display(),
                stand_in = stand_in.display(),
                own = own.display(),
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

    /// Every command line the fake ran, oldest first, with a login code
    /// written as `<code>`, so a search of the test's files for the code
    /// finds only what wineo left.
    pub fn calls(&self) -> Vec<String> {
        lines(&self.file("calls"))
    }

    /// How many times the owned list was fetched.
    pub fn list_calls(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.as_str() == "list --json -T")
            .count()
    }

    /// The file each run of the fake was started as, oldest first.
    pub fn programs(&self) -> Vec<String> {
        lines(&self.file("programs"))
    }

    /// The fake as wineo's own copy, as if downloaded earlier. It runs only
    /// through `python3`.
    pub fn own_copy_from(&self, fake: &Fake) -> PathBuf {
        let path = self.own_copy();
        self.legendary_at(&path, fake);
        path
    }

    /// The owned list with `edit` applied, as a file of its own for
    /// `Fake::list`.
    pub fn edited_list(&self, edit: impl FnOnce(&mut serde_json::Value)) -> PathBuf {
        let mut list: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(fixture("list.json")).unwrap()).unwrap();
        edit(&mut list);
        let path = self.file("edited-list.json");
        fs::write(&path, serde_json::to_string(&list).unwrap()).unwrap();
        path
    }

    /// Every file under the test's root that holds `needle`, leaving out
    /// the fakes themselves, which hold the code they accept.
    pub fn files_holding(&self, needle: &str) -> Vec<PathBuf> {
        let skip = [self.bin.clone(), self.file("stand-in"), self.own_copy()];
        let mut found = Vec::new();
        let mut dirs = vec![self.f.root.path().to_owned()];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if skip.iter().any(|skipped| path.starts_with(skipped)) {
                    continue;
                }
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    dirs.push(path);
                } else if kind.is_file()
                    && fs::read(&path).is_ok_and(|bytes| {
                        bytes
                            .windows(needle.len())
                            .any(|window| window == needle.as_bytes())
                    })
                {
                    found.push(path);
                }
            }
        }
        found
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
             printf '%s\\n' \"$0\" >> \"$root/programs\"\n\
             if [ \"$1 $2\" = 'auth --code' ]; then echo 'auth --code <code>' >> \"$root/calls\"\n\
             else printf '%s\\n' \"$*\" >> \"$root/calls\"; fi\n\
             user=\"$LEGENDARY_CONFIG_PATH/user.json\"\n\
             if [ \"$1\" = -y ]; then shift; fi\n\
             {functions}\
             case \"$1\" in\n\
             {install_arms}\
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
            functions = epic_install::functions(&tool),
            install_arms = epic_install::arms(&fixture(""), &tool),
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
