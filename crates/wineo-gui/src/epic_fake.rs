//! A fake `legendary` for the desktop app's Epic tests, answering from
//! core's probe fixtures with the install arms core's and the CLI's fakes
//! share. It is set as `[epic] legendary`, so no test changes `PATH`, which
//! every test thread shares, and neither a real Legendary nor a browser is
//! ever found.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use wineo_core::paths::Paths;

use crate::test_script::write_script;

#[path = "../../wineo-core/tests/common/epic_install.rs"]
mod epic_install;

/// The code the fake accepts.
pub(crate) const CODE: &str = "SECRETepic0123456789abcdef";

pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../wineo-core/tests/epic-fixtures/legendary-0.21.1")
        .join(name)
}

fn tool(name: &str) -> String {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("the tests need {name} on PATH"))
        .display()
        .to_string()
}

pub(crate) struct Epic {
    pub(crate) root: tempfile::TempDir,
    pub(crate) paths: Paths,
    /// The fake, which the config names.
    pub(crate) legendary: PathBuf,
}

impl Epic {
    pub(crate) fn new() -> Self {
        Self::version("0.21.1")
    }

    /// A host whose `[epic] legendary` is a fake saying it is `version`.
    pub(crate) fn version(version: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                .unwrap();
        let legendary = root.path().join("bin/legendary");
        let epic = Self {
            root,
            paths,
            legendary,
        };
        fs::create_dir_all(epic.legendary.parent().unwrap()).unwrap();
        write_script(&epic.legendary, epic.script(version));
        fs::copy(fixture("list.json"), epic.file("list.json")).unwrap();
        let config = epic.paths.global_config();
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            format!("[epic]\nlegendary = \"{}\"\n", epic.legendary.display()),
        )
        .unwrap();
        epic
    }

    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    pub(crate) fn state(&self) -> PathBuf {
        wineo_core::epic::state_dir(&self.paths)
    }

    /// A saved login, as Legendary leaves it.
    pub(crate) fn log_in(&self) {
        fs::create_dir_all(self.state()).unwrap();
        fs::write(self.state().join("user.json"), "{}\n").unwrap();
    }

    pub(crate) fn logged_in(&self) -> bool {
        self.state().join("user.json").exists()
    }

    /// Every command line the fake ran, oldest first, with a login code
    /// written as `<code>`.
    pub(crate) fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.file("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// How the fake's next install of `app` behaves; see `epic_install`.
    pub(crate) fn set_mode(&self, app: &str, mode: &str) {
        fs::write(self.file(&format!("mode-{app}")), mode).unwrap();
    }

    /// Waits for the fake to start installing `app`.
    pub(crate) fn wait_started(&self, app: &str) {
        let marker = self.file(&format!("installing-{app}"));
        let deadline = Instant::now() + Duration::from_secs(30);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "the fake never started {app}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Where the games folder is by default.
    pub(crate) fn games(&self) -> PathBuf {
        fs::canonicalize(self.root.path()).unwrap().join("Games")
    }

    /// The owned list the fake prints, with `edit` applied.
    pub(crate) fn edit_list(&self, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = self.file("list.json");
        let mut list: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        edit(&mut list);
        fs::write(&path, serde_json::to_string(&list).unwrap()).unwrap();
    }

    /// Kills every live process started from this test's root, and returns
    /// their ids. A test that ran the fake ends by asserting this is empty.
    pub(crate) fn sweep(&self) -> Vec<i32> {
        let needle = self.root.path().display().to_string();
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
            let Ok(cmdline) = fs::read(format!("/proc/{pid}/cmdline")) else {
                continue;
            };
            if !cmdline
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

    fn script(&self, version: &str) -> String {
        let root = self.root.path().display().to_string();
        let (cat, rm) = (tool("cat"), tool("rm"));
        format!(
            "#!/bin/sh\n\
             root='{root}'\n\
             if [ \"$1 $2\" = 'auth --code' ]; then echo 'auth --code <code>' >> \"$root/calls\"\n\
             else printf '%s\\n' \"$*\" >> \"$root/calls\"; fi\n\
             user=\"$LEGENDARY_CONFIG_PATH/user.json\"\n\
             if [ \"$1\" = -y ]; then shift; fi\n\
             {functions}\
             case \"$1\" in\n\
             {install_arms}\
             --version) echo 'legendary version \"{version}\", codename \"Test\"' ;;\n\
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
               {cat} '{list}' ;;\n\
             *) echo \"unknown command $1\" >&2; exit 2 ;;\n\
             esac\n\
             exit 0\n",
            status_in = fixture("status-logged-in.json").display(),
            status_out = fixture("status-logged-out.json").display(),
            failed = fixture("login-failed.stderr").display(),
            logged_out_list = fixture("logged-out-list.stderr").display(),
            list = self.file("list.json").display(),
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
