//! A fake `legendary` and `python3` for tests, answering from the probe's
//! fixtures, and the throwaway host they run in. Nothing here reaches the
//! network or the real Legendary.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use super::Env;
use super::legendary::{Found, Legendary, find_with};
use crate::cancel::Token;
use crate::paths::Paths;
use crate::test_script::write_script;

pub(crate) const CODE: &str = "0123456789abcdef0123456789abcdef";

pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/epic-fixtures/legendary-0.21.1")
        .join(name)
}

/// The first `name` on the test process's own `PATH`, as given, so a
/// multi-call binary still sees its own name.
fn tool(name: &str) -> String {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| crate::runner::is_executable(candidate))
        .unwrap_or_else(|| panic!("the tests need {name} on PATH"))
        .display()
        .to_string()
}

/// How the fake behaves. The defaults are Legendary 0.21.1 answering as
/// the probe saw it.
#[derive(Clone)]
pub(crate) struct Fake {
    pub version: String,
    /// The code `auth --code` accepts. Any other is refused, exit 0.
    pub code: String,
    /// When false, `auth --delete` exits 0 and keeps the login.
    pub delete_works: bool,
    /// A command that never finishes, bounded at 30 seconds.
    pub hang: Option<&'static str>,
    /// A command that prints a line Legendary never printed before its
    /// answer.
    pub noise: Option<&'static str>,
    /// A command that prints a megabyte on stderr before its answer.
    pub flood: Option<&'static str>,
    /// `auth --code` repeats the code in an error line on stderr.
    pub echo_code: bool,
    pub status_in: PathBuf,
    pub status_out: PathBuf,
    pub list: PathBuf,
    /// `list` prints this on stderr and exits 1 though logged in.
    pub list_fails: Option<PathBuf>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            version: fs::read_to_string(fixture("version.stdout")).unwrap(),
            code: CODE.to_owned(),
            delete_works: true,
            hang: None,
            noise: None,
            flood: None,
            echo_code: false,
            status_in: fixture("status-logged-in.json"),
            status_out: fixture("status-logged-out.json"),
            list: fixture("list.json"),
            list_fails: None,
        }
    }
}

impl Fake {
    pub fn version(version: &str) -> Self {
        Self {
            version: format!("legendary version \"{version}\", codename \"Test\"\n"),
            ..Self::default()
        }
    }
}

/// A host of its own: `HOME`, wineo's paths, a `PATH` holding only what a
/// test puts there, and a mark in the environment that every process the
/// test starts carries, so leftovers can be found.
pub(crate) struct Setup {
    pub root: tempfile::TempDir,
    pub paths: Paths,
    pub bin: PathBuf,
    mark: String,
}

impl Setup {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            None,
        )
        .unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let mark = root.path().display().to_string();
        Self {
            root,
            paths,
            bin,
            mark,
        }
    }

    pub fn env(&self) -> Env {
        self.env_with(&[])
    }

    pub fn env_with(&self, extra: &[(&str, &str)]) -> Env {
        let bin = self.bin.display().to_string();
        let home = self.root.path().display().to_string();
        let mut pairs = vec![
            ("PATH", bin.as_str()),
            ("HOME", home.as_str()),
            ("WINEO_FAKE_MARK", self.mark.as_str()),
        ];
        pairs.extend_from_slice(extra);
        Env::from_pairs(&pairs)
    }

    /// A `python3` on `PATH` that says it is `version` and runs any file
    /// it is given with `sh`.
    pub fn python(&self, version: &str) {
        let root = self.root.path().display();
        write_script(
            self.bin.join("python3"),
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{root}/python-calls'\nif [ \"$1\" = --version ]; then echo 'Python {version}'; exit 0; fi\nexec /bin/sh \"$@\"\n"
            ),
        );
    }

    /// Writes the fake to `path`.
    pub fn legendary_at(&self, path: &Path, fake: &Fake) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_script(path, self.script(fake));
    }

    pub fn on_path(&self, fake: &Fake) -> PathBuf {
        let path = self.bin.join("legendary");
        self.legendary_at(&path, fake);
        path
    }

    /// The fake as wineo's own copy of the pinned version.
    pub fn own_copy(&self, fake: &Fake) -> PathBuf {
        let path = super::own_copy::file(&self.paths, super::own_copy::PINNED.version);
        self.legendary_at(&path, fake);
        path
    }

    /// The fake on `PATH`, found as wineo would, with short time limits.
    pub fn legendary(&self, fake: &Fake) -> Legendary {
        self.on_path(fake);
        self.find(self.env())
    }

    pub fn find(&self, env: Env) -> Legendary {
        match find_with(&self.paths, None, env, &Token::new()).unwrap() {
            Found::Usable(mut legendary) => {
                legendary.set_timeout(Duration::from_secs(10));
                *legendary
            }
            Found::Missing(missing) => panic!("no usable fake: {missing:?}"),
        }
    }

    /// A saved login, as Legendary leaves it.
    pub fn log_in(&self) {
        let state = super::state_dir(&self.paths);
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("user.json"), "{}\n").unwrap();
    }

    pub fn logged_in(&self) -> bool {
        super::state_dir(&self.paths).join("user.json").exists()
    }

    /// Every command line the fake ran, oldest first.
    pub fn calls(&self) -> Vec<String> {
        read_lines(&self.root.path().join("calls"))
    }

    pub fn python_calls(&self) -> Vec<String> {
        read_lines(&self.root.path().join("python-calls"))
    }

    /// What the fake saw for `command`, from files it wrote.
    pub fn seen(&self, what: &str, command: &str) -> String {
        fs::read_to_string(self.root.path().join(format!("{what}-{command}"))).unwrap()
    }

    /// Kills every live process that carries this test's mark, and
    /// returns their ids. A test that started any ends by asserting this
    /// is empty.
    pub fn sweep(&self) -> Vec<i32> {
        let needle = format!("WINEO_FAKE_MARK={}\0", self.mark);
        let mut found = Vec::new();
        for entry in fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<i32>().ok())
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
            let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let state = state
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
        let root = self.root.path().display().to_string();
        let (cat, sleep, rm, env, readlink) = (
            tool("cat"),
            tool("sleep"),
            tool("rm"),
            tool("env"),
            tool("readlink"),
        );
        if fake.flood.is_some() {
            fs::write(self.root.path().join("flood"), "x".repeat(1024 * 1024)).unwrap();
        }
        let when = |command: Option<&str>, then: String| match command {
            Some(command) => format!("[ \"$1\" = '{command}' ] && {{ {then}; }}\n"),
            None => String::new(),
        };
        let delete = if fake.delete_works {
            format!("{rm} -f \"$user\"")
        } else {
            ":".to_owned()
        };
        let echo_code = if fake.echo_code {
            "printf '[cli] ERROR: Login with code %s failed\\n' \"$3\" >&2\n".to_owned()
        } else {
            String::new()
        };
        let list_fails = match &fake.list_fails {
            Some(stderr) => format!("{cat} '{}' >&2; exit 1\n", stderr.display()),
            None => String::new(),
        };
        let version = fake.version.replace('\'', "'\\''");
        format!(
            "#!/bin/sh\n\
             root='{root}'\n\
             printf '%s\\n' \"$*\" >> \"$root/calls\"\n\
             {env} > \"$root/env-$1\"\n\
             {readlink} /proc/$$/fd/0 > \"$root/stdin-$1\"\n\
             {cat} /proc/$$/stat > \"$root/stat-$1\"\n\
             user=\"$LEGENDARY_CONFIG_PATH/user.json\"\n\
             {noise}{flood}{hang}\
             case \"$1\" in\n\
             --version) printf '%s' '{version}' ;;\n\
             status) if [ -f \"$user\" ]; then {cat} '{status_in}'; else {cat} '{status_out}'; fi ;;\n\
             auth)\n\
               case \"$2\" in\n\
               --delete) {delete} ;;\n\
               --code)\n\
                 {echo_code}\
                 if [ -f \"$user\" ]; then echo '[cli] INFO: Testing existing login data if present...' >&2\n\
                 elif [ \"$3\" = '{code}' ]; then printf '{{}}\\n' > \"$user\"\n\
                 else {cat} '{failed}' >&2; fi ;;\n\
               esac ;;\n\
             list)\n\
               if [ ! -f \"$user\" ]; then {cat} '{logged_out_list}' >&2; exit 1; fi\n\
               {list_fails}\
               {cat} '{list}' ;;\n\
             *) echo \"unknown command $1\" >&2; exit 2 ;;\n\
             esac\n\
             exit 0\n",
            noise = when(
                fake.noise,
                "echo 'Something Legendary never printed'".to_owned()
            ),
            flood = when(fake.flood, format!("{cat} \"$root/flood\" >&2")),
            hang = when(fake.hang, format!("exec {sleep} 30")),
            status_in = fake.status_in.display(),
            status_out = fake.status_out.display(),
            code = fake.code,
            failed = fixture("login-failed.stderr").display(),
            logged_out_list = fixture("logged-out-list.stderr").display(),
            list = fake.list.display(),
        )
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.sweep();
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}
