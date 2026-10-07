//! Putting DXVK and vkd3d builds into a prefix and taking them back out.
//!
//! A config names the build a prefix should have, so wineo reconciles the two
//! rather than offering an install command. What it wrote is recorded in the
//! prefix, so removal restores exactly what was replaced.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::build_lock;
use crate::components::{Build, Builds, Component};
use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::paths::Paths;
use crate::runner::Runner;

/// What wineo put in a prefix, so it can take exactly that back out.
const MANIFEST: &str = ".wineo-components.toml";
/// What a replaced wine DLL is renamed to.
const BACKUP_SUFFIX: &str = ".wineo-orig";
/// Where wine keeps the overrides winecfg edits.
const OVERRIDES_KEY: &str = r"HKCU\Software\Wine\DllOverrides";

/// The build each component should have in a prefix, as a config asks for it.
/// The values may be `latest`, which is resolved at reconcile time.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
}

impl Wanted {
    /// Nothing for a native environment, which has no prefix to install into.
    pub fn from_config(config: &EnvironmentConfig) -> Self {
        match config.wine() {
            Some(wine) => Self {
                dxvk: wine.dxvk.clone(),
                vkd3d: wine.vkd3d.clone(),
            },
            None => Self::default(),
        }
    }

    pub fn get(&self, component: Component) -> Option<&str> {
        match component {
            Component::Dxvk => self.dxvk.as_deref(),
            Component::Vkd3d => self.vkd3d.as_deref(),
        }
    }

    fn is_empty(&self) -> bool {
        self.dxvk.is_none() && self.vkd3d.is_none()
    }
}

/// What reconciling did, for a caller that wants to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Installed {
        component: Component,
        build: String,
    },
    Replaced {
        component: Component,
        from: String,
        to: String,
    },
    Removed {
        component: Component,
        build: String,
    },
}

/// `<prefix>/.wineo-components.toml`, keyed by component name.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Manifest {
    #[serde(flatten)]
    components: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    /// The resolved build name. `latest` is never stored, since the point of
    /// the record is what is actually in the prefix.
    build: String,
    /// Everything wineo wrote, relative to the wine prefix.
    files: Vec<String>,
    /// The subset of `files` that had an original moved aside.
    #[serde(default)]
    backups: Vec<String>,
}

impl Manifest {
    fn read(wine_prefix: &Path) -> Result<Self> {
        let path = wine_prefix.join(MANIFEST);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        };
        toml::from_str(&text).map_err(|source| Error::StateSyntax {
            path,
            source: Box::new(source),
        })
    }

    fn write(&self, wine_prefix: &Path) -> Result<()> {
        let path = wine_prefix.join(MANIFEST);
        if self.components.is_empty() {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
            };
        }
        let text = toml::to_string(self).expect("the manifest always serializes");
        write_atomic(&path, &text)
    }

    fn get(&self, component: Component) -> Option<&Entry> {
        self.components.get(component.as_str())
    }
}

/// What a prefix currently has installed, for `status`.
pub fn installed(runner: &Runner, prefix: &Path, component: Component) -> Result<Option<String>> {
    Ok(Manifest::read(&runner.wine_prefix(prefix))?
        .get(component)
        .map(|entry| entry.build.clone()))
}

/// Makes the prefix match what the config asks for. A prefix that already has
/// the right builds is left untouched, so this is a read of one small file in
/// the case that matters.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    prefix: &Path,
    wanted: &Wanted,
) -> Result<Vec<Change>> {
    let _runner_guard = build_lock::runner(paths, runner)?;
    check(name, runner, wanted)?;
    // A prefix that already has what the config asks for is left alone, and
    // the build it came from need not still be extracted: the DLLs were
    // copied in, not linked.
    if is_current(runner, prefix, wanted)? {
        return Ok(Vec::new());
    }
    let wine_prefix = runner.wine_prefix(prefix);
    let mut manifest = Manifest::read(&wine_prefix)?;
    let mut changes = Vec::new();

    for component in Component::ALL {
        // Resolved first, so a name that is not installed fails before
        // anything in the prefix is touched.
        let (target, _build_guard) = match wanted.get(component) {
            Some(asked) => {
                let (build, guard) =
                    Builds::new(paths, global, component)?.resolve_locked(paths, asked)?;
                (Some(build), Some(guard))
            }
            None => (None, None),
        };
        let current = manifest.get(component).map(|entry| entry.build.clone());
        match (current, target) {
            (None, None) => {}
            (Some(build), None) => {
                remove(runner, &wine_prefix, &mut manifest, component)?;
                changes.push(Change::Removed { component, build });
            }
            (Some(from), Some(build)) if from == build.name() => {
                let _ = build;
            }
            (Some(from), Some(build)) => {
                // Old files go before new ones, so a swap never leaves a
                // prefix carrying halves of two builds.
                remove(runner, &wine_prefix, &mut manifest, component)?;
                add(runner, &wine_prefix, &mut manifest, &build)?;
                changes.push(Change::Replaced {
                    component,
                    from,
                    to: build.name().to_owned(),
                });
            }
            (None, Some(build)) => {
                add(runner, &wine_prefix, &mut manifest, &build)?;
                changes.push(Change::Installed {
                    component,
                    build: build.name().to_owned(),
                });
            }
        }
    }

    if !changes.is_empty() {
        manifest.write(&wine_prefix)?;
    }
    Ok(changes)
}

/// The refusals that do not depend on what is installed, so `set` reports them
/// before touching anything and a hand-edited config is caught at launch.
pub fn check(name: &str, runner: &Runner, wanted: &Wanted) -> Result<()> {
    if runner.is_proton()
        && let Some(component) = Component::ALL
            .into_iter()
            .find(|component| wanted.get(*component).is_some())
    {
        return Err(Error::ComponentOnProton {
            component: component.as_str(),
            environment: name.to_owned(),
            runner: runner.name().to_owned(),
        });
    }
    // vkd3d-proton ships no DXGI of its own; it shares DXVK's.
    if wanted.vkd3d.is_some() && wanted.dxvk.is_none() {
        return Err(Error::Vkd3dNeedsDxvk(name.to_owned()));
    }
    Ok(())
}

/// Resolves everything a config asks for without touching a prefix, so a
/// creation can refuse before it makes one.
pub fn check_available(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    wanted: &Wanted,
) -> Result<()> {
    check(name, runner, wanted)?;
    for component in Component::ALL {
        if let Some(asked) = wanted.get(component) {
            Builds::new(paths, global, component)?.resolve_locked(paths, asked)?;
        }
    }
    Ok(())
}

/// Resolves component names for a review without creating build locks.
pub fn resolve_available(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    wanted: &Wanted,
) -> Result<Wanted> {
    check(name, runner, wanted)?;
    let resolve = |component| {
        wanted
            .get(component)
            .map(|asked| {
                Builds::new(paths, global, component)?
                    .resolve(asked)
                    .map(|build| build.name().to_owned())
            })
            .transpose()
    };
    Ok(Wanted {
        dxvk: resolve(Component::Dxvk)?,
        vkd3d: resolve(Component::Vkd3d)?,
    })
}

/// Whether the prefix already matches, without resolving or writing anything.
/// `latest` still has to be resolved, so it is reported as a difference and
/// `reconcile` decides.
pub fn is_current(runner: &Runner, prefix: &Path, wanted: &Wanted) -> Result<bool> {
    if wanted.is_empty() {
        // Still a difference if the prefix has something the config dropped.
        return Ok(Manifest::read(&runner.wine_prefix(prefix))?
            .components
            .is_empty());
    }
    let manifest = Manifest::read(&runner.wine_prefix(prefix))?;
    Ok(Component::ALL.into_iter().all(|component| {
        let installed = manifest.get(component).map(|entry| entry.build.as_str());
        match (wanted.get(component), installed) {
            (None, None) => true,
            (Some(asked), Some(build)) => asked == build,
            _ => false,
        }
    }))
}

/// Copies a build's DLLs in and points the overrides at them.
fn add(runner: &Runner, wine_prefix: &Path, manifest: &mut Manifest, build: &Build) -> Result<()> {
    let component = build.component();
    let (wide_dir, narrow_dir) = component.arch_dirs();
    let (wide_dest, narrow_dest) = destinations(wine_prefix);

    let mut entry = Entry {
        build: build.name().to_owned(),
        files: Vec::new(),
        backups: Vec::new(),
    };
    // A win32 prefix has no syswow64, so only the 32-bit DLLs belong in it.
    let mut sources = vec![(build.dir().join(narrow_dir), narrow_dest)];
    if let Some(wide_dest) = wide_dest {
        sources.insert(0, (build.dir().join(wide_dir), wide_dest));
    }
    for (source, dest) in sources {
        fs::create_dir_all(&dest).context(|| format!("failed to create {}", dest.display()))?;
        for dll in dlls(&source)? {
            let from = source.join(&dll);
            let to = dest.join(&dll);
            let relative = relative(wine_prefix, &to);
            if to.exists() {
                let backup = backup_path(&to);
                fs::rename(&to, &backup)
                    .context(|| format!("failed to move {} aside", to.display()))?;
                entry.backups.push(relative.clone());
            }
            fs::copy(&from, &to)
                .context(|| format!("failed to copy {} into the prefix", from.display()))?;
            entry.files.push(relative);
        }
    }

    for dll in component.overrides() {
        set_override(runner, wine_prefix, dll)?;
    }
    manifest
        .components
        .insert(component.as_str().to_owned(), entry);
    Ok(())
}

/// Puts back exactly what `add` replaced.
fn remove(
    runner: &Runner,
    wine_prefix: &Path,
    manifest: &mut Manifest,
    component: Component,
) -> Result<()> {
    let Some(entry) = manifest.components.remove(component.as_str()) else {
        return Ok(());
    };
    for file in &entry.files {
        let path = wine_prefix.join(file);
        if entry.backups.contains(file) {
            let backup = backup_path(&path);
            fs::rename(&backup, &path)
                .context(|| format!("failed to restore {}", path.display()))?;
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {}
                // Already gone is the state this is trying to reach.
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).context(|| format!("failed to remove {}", path.display()));
                }
            }
        }
    }
    for dll in component.overrides() {
        clear_override(runner, wine_prefix, dll);
    }
    Ok(())
}

/// Where each architecture's DLLs go. A win64 prefix has both directories; a
/// win32 prefix has only system32, and its 32-bit DLLs go there.
fn destinations(wine_prefix: &Path) -> (Option<PathBuf>, PathBuf) {
    let windows = wine_prefix.join("drive_c/windows");
    let syswow64 = windows.join("syswow64");
    if syswow64.is_dir() {
        (Some(windows.join("system32")), syswow64)
    } else {
        (None, windows.join("system32"))
    }
}

/// The DLL file names in one of a build's architecture directories, sorted so
/// a manifest is stable. Whatever is there is copied, so a build that adds or
/// drops a DLL still installs correctly.
fn dlls(dir: &Path) -> Result<Vec<String>> {
    let entries = fs::read_dir(dir).context(|| format!("failed to read {}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", dir.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.to_ascii_lowercase().ends_with(".dll") && entry.path().is_file() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn backup_path(dll: &Path) -> PathBuf {
    let mut path = dll.as_os_str().to_owned();
    path.push(BACKUP_SUFFIX);
    PathBuf::from(path)
}

fn relative(wine_prefix: &Path, path: &Path) -> String {
    path.strip_prefix(wine_prefix)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn set_override(runner: &Runner, wine_prefix: &Path, dll: &str) -> Result<()> {
    let status = reg(runner, wine_prefix)?
        .arg("add")
        .args([
            OVERRIDES_KEY,
            "/v",
            dll,
            "/t",
            "REG_SZ",
            "/d",
            "native",
            "/f",
        ])
        .status()
        .context(|| "failed to run wine reg add".to_owned())?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::DllOverride(dll.to_owned()))
    }
}

/// Best effort: wine's `reg delete` also fails when the value was never there,
/// which is the state this wants, so the status says nothing useful.
fn clear_override(runner: &Runner, wine_prefix: &Path, dll: &str) {
    if let Ok(mut command) = reg(runner, wine_prefix) {
        let _ = command
            .arg("delete")
            .args([OVERRIDES_KEY, "/v", dll, "/f"])
            .status();
    }
}

fn reg(runner: &Runner, wine_prefix: &Path) -> Result<std::process::Command> {
    let mut command = runner.command(wine_prefix, &[])?;
    command
        .arg("reg")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_win64_prefix_takes_both_architectures() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path();
        fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
        let (wide, narrow) = destinations(prefix);
        assert_eq!(wide, Some(prefix.join("drive_c/windows/system32")));
        assert_eq!(narrow, prefix.join("drive_c/windows/syswow64"));
    }

    #[test]
    fn a_win32_prefix_puts_its_32_bit_dlls_in_system32() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path();
        fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        let (wide, narrow) = destinations(prefix);
        assert_eq!(wide, None);
        assert_eq!(narrow, prefix.join("drive_c/windows/system32"));
    }

    #[test]
    fn only_dll_files_are_copied() {
        let root = tempfile::tempdir().unwrap();
        for name in ["d3d11.dll", "DXGI.DLL", "README.md", "setup.sh"] {
            fs::write(root.path().join(name), "").unwrap();
        }
        fs::create_dir(root.path().join("nested.dll")).unwrap();
        assert_eq!(dlls(root.path()).unwrap(), ["DXGI.DLL", "d3d11.dll"]);
    }

    #[test]
    fn backups_sit_beside_what_they_replaced() {
        assert_eq!(
            backup_path(Path::new("/p/drive_c/windows/system32/d3d11.dll")),
            Path::new("/p/drive_c/windows/system32/d3d11.dll.wineo-orig")
        );
    }

    #[test]
    fn a_manifest_round_trips() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::read(root.path()).unwrap(), Manifest::default());

        let mut manifest = Manifest::default();
        manifest.components.insert(
            "dxvk".to_owned(),
            Entry {
                build: "2.7".to_owned(),
                files: vec!["drive_c/windows/system32/dxgi.dll".to_owned()],
                backups: vec!["drive_c/windows/system32/dxgi.dll".to_owned()],
            },
        );
        manifest.write(root.path()).unwrap();
        assert_eq!(Manifest::read(root.path()).unwrap(), manifest);
        assert_eq!(
            manifest
                .get(Component::Dxvk)
                .map(|entry| entry.build.as_str()),
            Some("2.7")
        );

        // Emptying it takes the file away rather than leaving `{}` behind.
        Manifest::default().write(root.path()).unwrap();
        assert!(!root.path().join(MANIFEST).exists());
    }

    #[test]
    fn vkd3d_needs_dxvk_beside_it() {
        let runner = Runner::fake("wine");
        let wanted = Wanted {
            dxvk: None,
            vkd3d: Some("2.14".to_owned()),
        };
        assert!(matches!(
            check("hades", &runner, &wanted),
            Err(Error::Vkd3dNeedsDxvk(_))
        ));

        let both = Wanted {
            dxvk: Some("2.7".to_owned()),
            ..wanted
        };
        assert!(check("hades", &runner, &both).is_ok());
    }

    #[test]
    fn a_proton_environment_has_its_own() {
        let runner = Runner::fake_proton("GE-Proton9-20");
        for wanted in [
            Wanted {
                dxvk: Some("2.7".to_owned()),
                vkd3d: None,
            },
            Wanted {
                dxvk: Some("2.7".to_owned()),
                vkd3d: Some("2.14".to_owned()),
            },
        ] {
            assert!(matches!(
                check("hades", &runner, &wanted),
                Err(Error::ComponentOnProton { .. })
            ));
        }
        assert!(check("hades", &runner, &Wanted::default()).is_ok());
    }

    #[test]
    fn is_current_compares_the_config_against_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let runner = Runner::fake("wine");
        let prefix = root.path();
        let dxvk = |name: &str| Wanted {
            dxvk: Some(name.to_owned()),
            vkd3d: None,
        };

        // Nothing asked for and nothing installed.
        assert!(is_current(&runner, prefix, &Wanted::default()).unwrap());
        assert!(!is_current(&runner, prefix, &dxvk("2.7")).unwrap());

        let mut manifest = Manifest::default();
        manifest.components.insert(
            "dxvk".to_owned(),
            Entry {
                build: "2.7".to_owned(),
                files: Vec::new(),
                backups: Vec::new(),
            },
        );
        manifest.write(prefix).unwrap();
        assert!(is_current(&runner, prefix, &dxvk("2.7")).unwrap());
        assert!(!is_current(&runner, prefix, &dxvk("2.8")).unwrap());
        // The config dropped it, so the prefix still has to change.
        assert!(!is_current(&runner, prefix, &Wanted::default()).unwrap());
    }
}
