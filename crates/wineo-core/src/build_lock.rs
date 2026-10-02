use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::components::{Builds, Component};
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::paths::{Paths, fnv1a, label};
use crate::runner::Runner;
use crate::runners::Runners;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Runner,
    Dxvk,
    Vkd3d,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Runner => "runner",
            Self::Dxvk => "dxvk",
            Self::Vkd3d => "vkd3d",
        }
    }
}

impl From<Component> for Kind {
    fn from(component: Component) -> Self {
        match component {
            Component::Dxvk => Self::Dxvk,
            Component::Vkd3d => Self::Vkd3d,
        }
    }
}

/// A supervisor reopens the same lock before its parent releases its guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lock {
    path: PathBuf,
    kind: Kind,
    name: String,
}

impl Lock {
    pub fn new(paths: &Paths, kind: Kind, name: &str) -> Self {
        let hash = fnv1a(name.as_bytes());
        Self {
            path: paths.build_locks_dir().join(format!(
                "build-{}-{}-{hash:016x}.lock",
                kind.as_str(),
                label(name),
            )),
            kind,
            name: name.to_owned(),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn open(&self) -> Result<File> {
        let dir = self.path.parent().expect("a lock has a parent");
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.path)
            .context(|| format!("failed to open {}", self.path.display()))
    }

    pub fn shared(&self) -> Result<Guard> {
        let file = self.open()?;
        flock(&file, FlockOperation::LockShared)
            .map_err(std::io::Error::from)
            .context(|| format!("failed to lock {}", self.path.display()))?;
        Ok(Guard {
            file,
            lock: self.clone(),
        })
    }

    /// None means a holder is using the build. Never waits for that holder.
    pub fn try_exclusive(&self) -> Result<Option<Guard>> {
        let file = self.open()?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(Guard {
                file,
                lock: self.clone(),
            })),
            Err(Errno::WOULDBLOCK) => Ok(None),
            Err(err) => Err(std::io::Error::from(err))
                .context(|| format!("failed to lock {}", self.path.display())),
        }
    }
}

/// The kernel releases the flock on exit. Lock files are never removed.
#[derive(Debug)]
pub struct Guard {
    file: File,
    lock: Lock,
}

impl Guard {
    pub fn lock(&self) -> &Lock {
        &self.lock
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

pub fn shared(paths: &Paths, kind: Kind, name: &str) -> Result<Guard> {
    Lock::new(paths, kind, name).shared()
}

pub fn try_exclusive(paths: &Paths, kind: Kind, name: &str) -> Result<Option<Guard>> {
    Lock::new(paths, kind, name).try_exclusive()
}

pub fn runner(paths: &Paths, runner: &Runner) -> Result<Option<Guard>> {
    if runner.name() == Runner::SYSTEM {
        return Ok(None);
    }
    let guard = shared(paths, Kind::Runner, runner.name())?;
    if !runner.directory().is_some_and(std::path::Path::is_dir) {
        return Err(Error::UnknownRunner {
            name: runner.name().to_owned(),
            installed: Vec::new(),
            root: runner
                .directory()
                .and_then(std::path::Path::parent)
                .unwrap_or_else(|| std::path::Path::new(""))
                .to_owned(),
        });
    }
    Ok(Some(guard))
}

/// Saving an absent selection is allowed by existing config writers. Once a
/// selection resolves, disappearance during locking must not go unnoticed.
pub fn config(
    paths: &Paths,
    global: &GlobalConfig,
    runner: Option<&str>,
    dxvk: Option<&str>,
    vkd3d: Option<&str>,
) -> Result<Vec<Guard>> {
    let mut guards = Vec::new();
    if let Some(name) = runner {
        let runners = Runners::new(paths, global)?;
        if let Ok(resolved) = runners.resolve(name) {
            if let Some(guard) = runners.lock_resolved(paths, name, resolved)?.1 {
                guards.push(guard);
            }
        } else if name != Runner::SYSTEM
            && !crate::runner_alias::is_alias(name)
            && crate::name::is_valid_runner(name)
        {
            guards.push(shared(paths, Kind::Runner, name)?);
        }
    }
    for (component, asked) in [(Component::Dxvk, dxvk), (Component::Vkd3d, vkd3d)] {
        if let Some(name) = asked {
            let builds = Builds::new(paths, global, component)?;
            if let Ok(resolved) = builds.resolve(name) {
                guards.push(builds.lock_resolved(paths, name, resolved)?.1);
            } else if name != crate::components::LATEST && crate::name::is_valid_runner(name) {
                guards.push(shared(paths, component.into(), name)?);
            }
        }
    }
    Ok(guards)
}

pub fn environment(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<Vec<Guard>> {
    let Some(wine) = environment.config.wine() else {
        return Ok(Vec::new());
    };
    config(
        paths,
        global,
        Some(&wine.runner),
        wine.dxvk.as_deref(),
        wine.vkd3d.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &std::path::Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
    }

    #[test]
    fn a_resolved_runner_uses_its_in_memory_root_with_broken_persisted_config() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let mut global = GlobalConfig::default();
        let custom = root.path().join("custom-runners");
        global.defaults.runner_root = Some(custom.display().to_string());
        fs::create_dir_all(custom.join("build/bin")).unwrap();
        for program in ["wine", "wineserver"] {
            std::os::unix::fs::symlink("/bin/true", custom.join("build/bin").join(program))
                .unwrap();
        }
        let resolved = Runners::new(&paths, &global)
            .unwrap()
            .resolve("build")
            .unwrap();
        let config = paths.global_config();
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "not = [valid TOML").unwrap();
        let held = runner(&paths, &resolved).unwrap().unwrap();
        assert!(
            try_exclusive(&paths, Kind::Runner, "build")
                .unwrap()
                .is_none()
        );
        drop(held);
        fs::remove_dir_all(custom.join("build")).unwrap();
        assert!(matches!(
            runner(&paths, &resolved),
            Err(Error::UnknownRunner { name, root, .. }) if name == "build" && root == custom
        ));
    }

    #[test]
    fn symlinked_executables_do_not_hide_a_deleted_named_build() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runners = Runners::new(&paths, &global).unwrap();
        let other = runners.root().join("other/bin");
        fs::create_dir_all(&other).unwrap();
        symlink("/bin/true", other.join("wine")).unwrap();
        symlink("/bin/true", other.join("wineserver")).unwrap();
        for target in [PathBuf::from("/bin/true"), other.join("wine")] {
            let dir = runners.root().join("linked");
            fs::create_dir_all(dir.join("bin")).unwrap();
            symlink(&target, dir.join("bin/wine")).unwrap();
            symlink("/bin/true", dir.join("bin/wineserver")).unwrap();
            let resolved = runners.resolve("linked").unwrap();
            let held = runner(&paths, &resolved).unwrap().unwrap();
            assert!(
                try_exclusive(&paths, Kind::Runner, "linked")
                    .unwrap()
                    .is_none()
            );
            drop(held);
            fs::remove_dir_all(dir).unwrap();
            assert!(
                matches!(runner(&paths, &resolved), Err(Error::UnknownRunner { name, .. }) if name == "linked")
            );
        }
    }

    #[test]
    fn spawned_child_does_not_inherit_or_keep_the_build_lock() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};

        let root = tempfile::tempdir().unwrap();
        let lock = Lock::new(&paths(root.path()), Kind::Runner, "build");
        let guard = lock.shared().unwrap();
        let mut child = Command::new("sh")
            .args([
                "-c",
                "echo ready; read -r release; test \"$release\" = release",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        let inherited = fs::read_dir(format!("/proc/{}/fd", child.id()))
            .unwrap()
            .any(|entry| fs::read_link(entry.unwrap().path()).is_ok_and(|path| path == lock.path));
        drop(guard);
        let acquired = Command::new("flock")
            .arg("-n")
            .arg(&lock.path)
            .arg("true")
            .status()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"release\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(!inherited, "the lock descriptor must be closed on exec");
        assert!(
            acquired.success(),
            "a live child must not retain its parent's lock"
        );
    }

    #[test]
    fn shared_holders_coexist_and_exclusive_fails_fast() {
        let root = tempfile::tempdir().unwrap();
        let lock = Lock::new(&paths(root.path()), Kind::Runner, "build");
        let first = lock.shared().unwrap();
        let second = lock.shared().unwrap();
        let start = std::time::Instant::now();
        assert!(lock.try_exclusive().unwrap().is_none());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        drop(first);
        assert!(lock.try_exclusive().unwrap().is_none());
        drop(second);
        assert!(lock.try_exclusive().unwrap().is_some());
        assert!(lock.path.is_file());
    }

    #[test]
    fn system_wine_has_no_build_lock() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        assert!(
            runner(&paths, &Runner::fake(Runner::SYSTEM))
                .unwrap()
                .is_none()
        );
        assert!(!paths.build_locks_dir().exists());
    }

    #[test]
    fn components_have_independent_locks() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let _held = shared(&paths, Kind::Runner, "same").unwrap();
        assert!(try_exclusive(&paths, Kind::Dxvk, "same").unwrap().is_some());
        assert!(
            try_exclusive(&paths, Kind::Vkd3d, "same")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn portable_and_host_roots_share_build_locks() {
        let root = tempfile::tempdir().unwrap();
        let lookup = |key: &str| (key == "HOME").then(|| root.path().as_os_str().to_owned());
        let host = Paths::from_lookup(lookup).unwrap();
        let portable = Paths::select(lookup, Some(&root.path().join("portable"))).unwrap();
        let _held = shared(&host, Kind::Runner, "build").unwrap();
        assert!(
            try_exclusive(&portable, Kind::Runner, "build")
                .unwrap()
                .is_none()
        );
        assert!(portable.build_locks_dir().starts_with(root.path()));
    }
}
